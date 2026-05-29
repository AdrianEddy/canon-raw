// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

use std::io::{self, Read, Seek, SeekFrom};
use super::error::*;

/// UUID for Canon Metadata Box
const CANON_METADATA_BOX_UUID: [u8; 16] = [
    0x85, 0xC0, 0xB6, 0x87, 0x82, 0x0F, 0x11, 0xE0,
    0x81, 0x11, 0xF4, 0xCE, 0x46, 0x2B, 0x6A, 0x48,
];

/// Media track types in a CRM file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaType {
    Video,
    TimedMeta,
}

impl MediaType {
    fn from_handler_type(handler: u32) -> Option<Self> {
        match handler {
            0x76696465 => Some(Self::Video),     // 'vide'
            0x6D657461 => Some(Self::TimedMeta), // 'meta'
            _ => None,
        }
    }
}

/// Index entry pointing to a sample in the file
#[derive(Debug, Clone, Copy)]
struct IndexEntry {
    offset: u64,
    size: u32,
}

/// Sample-to-chunk table entry
#[derive(Debug, Clone, Copy)]
struct StscEntry {
    first_chunk: u32,
    samples_per_chunk: u32,
}

/// FourCC box type constants
mod box_types {
    pub const FTYP: u32 = 0x66747970; // 'ftyp'
    pub const MOOV: u32 = 0x6D6F6F76; // 'moov'
    pub const MDAT: u32 = 0x6D646174; // 'mdat'
    pub const TRAK: u32 = 0x7472616B; // 'trak'
    pub const MDIA: u32 = 0x6D646961; // 'mdia'
    pub const MINF: u32 = 0x6D696E66; // 'minf'
    pub const STBL: u32 = 0x7374626C; // 'stbl'
    pub const HDLR: u32 = 0x68646C72; // 'hdlr'
    pub const STSD: u32 = 0x73747364; // 'stsd'
    pub const STSC: u32 = 0x73747363; // 'stsc'
    pub const STSZ: u32 = 0x7374737A; // 'stsz'
    pub const CO64: u32 = 0x636F3634; // 'co64'
    pub const UUID: u32 = 0x75756964; // 'uuid'
    pub const CRAW: u32 = 0x43524157; // 'CRAW'
    pub const CRX_BRAND: u32 = 0x63727820; // 'crx '
}

/// A parsed CRM movie file
pub struct CrmMovie<R> {
    reader: R,
    video_index: Vec<IndexEntry>,
    timed_meta_index: Vec<IndexEntry>,
    canon_metadata_box: Option<Vec<u8>>,
    craw_entry: Option<Vec<u8>>,
}

impl<R: Read + Seek> CrmMovie<R> {
    /// Load and parse a CRM movie from a reader
    pub fn load(reader: R) -> Result<Self> {
        let mut movie = CrmMovie {
            reader,
            video_index: Vec::new(),
            timed_meta_index: Vec::new(),
            canon_metadata_box: None,
            craw_entry: None,
        };

        movie.parse_top_level()?;
        movie.validate()?;

        Ok(movie)
    }

    /// Parse top-level boxes until mdat
    fn parse_top_level(&mut self) -> Result<()> {
        loop {
            let header = match self.read_box_header() {
                Ok(h) => h,
                Err(CrmError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            };

            match header.box_type {
                box_types::FTYP => {
                    self.parse_ftyp(&header)?;
                }
                box_types::MOOV => {
                    let data = self.read_box_data(&header)?;
                    self.parse_moov(&data)?;
                }
                box_types::MDAT => {
                    // Stop parsing at mdat - actual data follows
                    break;
                }
                _ => {
                    // Skip unknown boxes
                    self.skip_box_data(&header)?;
                }
            }
        }

        Ok(())
    }

    /// Validate that all required data was parsed
    fn validate(&self) -> Result<()> {
        if self.craw_entry.is_none() {
            return Err(CrmError::MissingData("CRAW entry"));
        }
        if self.canon_metadata_box.is_none() {
            return Err(CrmError::MissingData("Canon metadata box"));
        }
        if self.video_index.is_empty() {
            return Err(CrmError::MissingData("video index table"));
        }
        if self.timed_meta_index.is_empty() {
            return Err(CrmError::MissingData("timed metadata index table"));
        }
        if self.video_index.len() != self.timed_meta_index.len() {
            return Err(CrmError::InvalidFormat(format!(
                "video and timed meta frame counts don't match: {} vs {}",
                self.video_index.len(),
                self.timed_meta_index.len()
            )));
        }
        Ok(())
    }

    /// Parse the ftyp box and verify the brand
    fn parse_ftyp(&mut self, header: &BoxHeader) -> Result<()> {
        // ftyp requires: major_brand (4) + minor_version (4) after header
        if header.size < header.header_len + 8 {
            return Err(CrmError::InvalidFormat("ftyp box too small".into()));
        }

        let major_brand = self.read_u32()?;
        let _minor_version = self.read_u32()?;

        if major_brand != box_types::CRX_BRAND {
            return Err(CrmError::UnsupportedBrand(major_brand));
        }

        // Skip the rest of ftyp payload (if any)
        let remaining = header.size - header.header_len - 8;
        self.skip_bytes(remaining)?;
        Ok(())
    }

    /// Parse the moov box containing movie metadata
    fn parse_moov(&mut self, data: &[u8]) -> Result<()> {
        let mut offset = box_header_len_from_data(data);

        while offset + 8 <= data.len() {
            let (size, box_type, _hlen) = read_box_from_slice(data, offset)?;

            let box_data = &data[offset..offset + size];

            match box_type {
                box_types::TRAK => {
                    let mut current_media_type = None;
                    self.parse_container_box(box_data, &mut current_media_type)?;
                }
                box_types::UUID => {
                    self.parse_uuid_box(box_data)?;
                }
                _ => {}
            }

            offset += size;
        }

        Ok(())
    }

    /// Parse a container box recursively (trak, mdia, minf)
    fn parse_container_box(&mut self, data: &[u8], current_media_type: &mut Option<MediaType>) -> Result<()> {
        if data.len() < 8 {
            return Ok(());
        }

        let container_type = read_u32_be(&data[4..]);
        let mut offset = box_header_len_from_data(data);

        while offset + 8 <= data.len() {
            let (size, box_type, _hlen) = read_box_from_slice(data, offset)?;
            let box_data = &data[offset..offset + size];

            match (container_type, box_type) {
                (box_types::TRAK, box_types::MDIA)
                | (box_types::MDIA, box_types::MINF) => {
                    self.parse_container_box(box_data, current_media_type)?;
                }
                (box_types::MDIA, box_types::HDLR) => {
                    *current_media_type = self.parse_hdlr_box(box_data);
                }
                (box_types::MINF, box_types::STBL) => {
                    if let Some(media_type) = *current_media_type {
                        self.parse_stbl_box(box_data, media_type)?;
                    }
                }
                _ => {}
            }

            offset += size;
        }

        Ok(())
    }

    /// Parse the hdlr (handler reference) box to determine media type
    fn parse_hdlr_box(&self, data: &[u8]) -> Option<MediaType> {
        let h = box_header_len_from_data(data);
        // Need: version/flags (4) + pre_defined (4) + handler_type (4)
        if data.len() < h + 12 {
            return None;
        }
        let handler_type = read_u32_be(&data[h + 8..]);
        MediaType::from_handler_type(handler_type)
    }

    /// Parse the stbl (sample table) box
    fn parse_stbl_box(&mut self, data: &[u8], media_type: MediaType) -> Result<()> {
        if data.len() < 8 {
            return Err(CrmError::InvalidFormat("stbl too small".into()));
        }

        let mut stsc_entries: Vec<StscEntry> = Vec::new();
        let mut size_entries: Vec<u32> = Vec::new();
        let mut uniform_size: Option<u32> = None;
        let mut sample_count: u32 = 0;
        let mut chunk_offsets: Vec<u64> = Vec::new();

        let mut offset = box_header_len_from_data(data);

        while offset + 8 <= data.len() {
            let (box_size, box_type, _hlen) = read_box_from_slice(data, offset)?;
            let box_data = &data[offset..offset + box_size];

            match box_type {
                box_types::STSD => {
                    if media_type == MediaType::Video {
                        self.parse_stsd_box(box_data)?;
                    }
                }
                box_types::STSC => {
                    stsc_entries = self.parse_stsc_box(box_data)?;
                }
                box_types::STSZ => {
                    let (uniform, count, sizes) = self.parse_stsz_box(box_data)?;
                    uniform_size = uniform;
                    sample_count = count;
                    size_entries = sizes;
                }
                box_types::CO64 => {
                    chunk_offsets = self.parse_co64_box(box_data)?;
                }
                _ => {}
            }

            offset += box_size;
        }

        // Build the index table
        let index = self.build_index_table(
            &stsc_entries,
            uniform_size,
            &size_entries,
            sample_count,
            &chunk_offsets,
        )?;

        match media_type {
            MediaType::Video => self.video_index = index,
            MediaType::TimedMeta => self.timed_meta_index = index,
        }

        Ok(())
    }

    /// Parse the stsd (sample description) box
    fn parse_stsd_box(&mut self, data: &[u8]) -> Result<()> {
        let h = box_header_len_from_data(data);
        if data.len() < h + 8 {
            return Ok(());
        }

        let entry_count = read_u32_be(&data[h + 4..]);
        if entry_count == 0 {
            return Ok(());
        }

        let mut offset = h + 8;

        while offset + 8 <= data.len() {
            let entry_size = read_u32_be(&data[offset..]) as usize;
            let format_type = read_u32_be(&data[offset + 4..]);

            if entry_size > data.len() - offset || entry_size < 8 {
                break;
            }

            if format_type == box_types::CRAW {
                self.craw_entry = Some(data[offset..offset + entry_size].to_vec());
                // If you expect only one, you could break here.
            }

            offset += entry_size;
        }

        Ok(())
    }

    /// Parse the stsc (sample-to-chunk) box
    fn parse_stsc_box(&self, data: &[u8]) -> Result<Vec<StscEntry>> {
        let h = box_header_len_from_data(data);
        if data.len() < h + 8 {
            return Err(CrmError::InvalidFormat("stsc too small".into()));
        }

        let entry_count = read_u32_be(&data[h + 4..]) as usize;
        let required_size = h + 8 + entry_count * 12;

        if data.len() < required_size {
            return Err(CrmError::InvalidFormat("stsc entries truncated".into()));
        }

        let mut entries = Vec::with_capacity(entry_count);
        let mut offset = h + 8;

        for _ in 0..entry_count {
            entries.push(StscEntry {
                first_chunk: read_u32_be(&data[offset..]),
                samples_per_chunk: read_u32_be(&data[offset + 4..]),
                // sample_description_index at offset+8 is not needed
            });
            offset += 12;
        }

        Ok(entries)
    }

    /// Parse the stsz (sample size) box
    fn parse_stsz_box(&self, data: &[u8]) -> Result<(Option<u32>, u32, Vec<u32>)> {
        let h = box_header_len_from_data(data);
        // Need: version/flags (4) + sample_size (4) + sample_count (4)
        if data.len() < h + 12 {
            return Err(CrmError::InvalidFormat("stsz too small".into()));
        }

        let sample_size = read_u32_be(&data[h + 4..]);
        let sample_count = read_u32_be(&data[h + 8..]);

        if sample_size != 0 {
            // Uniform size for all samples
            Ok((Some(sample_size), sample_count, Vec::new()))
        } else {
            // Variable sizes
            let required_size = h + 12 + (sample_count as usize) * 4;
            if data.len() < required_size {
                return Err(CrmError::InvalidFormat("stsz entries truncated".into()));
            }

            let mut sizes = Vec::with_capacity(sample_count as usize);
            let mut offset = h + 12;

            for _ in 0..sample_count {
                sizes.push(read_u32_be(&data[offset..]));
                offset += 4;
            }

            Ok((None, sample_count, sizes))
        }
    }

    /// Parse the co64 (chunk offset 64-bit) box
    fn parse_co64_box(&self, data: &[u8]) -> Result<Vec<u64>> {
        let h = box_header_len_from_data(data);
        if data.len() < h + 8 {
            return Err(CrmError::InvalidFormat("co64 too small".into()));
        }

        let entry_count = read_u32_be(&data[h + 4..]) as usize;
        let required_size = h + 8 + entry_count * 8;

        if data.len() < required_size {
            return Err(CrmError::InvalidFormat("co64 entries truncated".into()));
        }

        let mut offsets = Vec::with_capacity(entry_count);
        let mut offset = h + 8;

        for _ in 0..entry_count {
            offsets.push(read_u64_be(&data[offset..]));
            offset += 8;
        }

        Ok(offsets)
    }

    /// Parse the uuid box for Canon metadata
    fn parse_uuid_box(&mut self, data: &[u8]) -> Result<()> {
        let h = box_header_len_from_data(data);
        if data.len() < h + 16 {
            return Ok(());
        }

        let uuid = &data[h..h + 16];

        if uuid == CANON_METADATA_BOX_UUID.as_slice() {
            self.canon_metadata_box = Some(data.to_vec());
        }

        Ok(())
    }

    /// Build the sample index table from parsed stbl data
    fn build_index_table(
        &self,
        stsc_entries: &[StscEntry],
        uniform_size: Option<u32>,
        size_entries: &[u32],
        sample_count: u32,
        chunk_offsets: &[u64],
    ) -> Result<Vec<IndexEntry>> {
        if stsc_entries.is_empty() || chunk_offsets.is_empty() || sample_count == 0 {
            return Ok(Vec::new());
        }

        if uniform_size.is_none() && size_entries.len() < sample_count as usize {
            return Err(CrmError::InvalidFormat(
                "stsz indicates variable sample sizes but entries are missing".into(),
            ));
        }

        let mut index = Vec::with_capacity(sample_count as usize);
        let mut sample_idx = 0u32;
        let mut stsc_idx = 0usize;

        for (chunk_idx, &chunk_offset) in chunk_offsets.iter().enumerate() {
            let chunk_num = (chunk_idx + 1) as u32;

            // Advance stsc_idx if needed
            if stsc_idx + 1 < stsc_entries.len()
                && stsc_entries[stsc_idx + 1].first_chunk <= chunk_num
            {
                stsc_idx += 1;
            }

            let samples_in_chunk = stsc_entries[stsc_idx].samples_per_chunk;
            let mut current_offset = chunk_offset;

            for _ in 0..samples_in_chunk {
                if sample_idx >= sample_count {
                    return Err(CrmError::InvalidFormat(
                        "stsc specifies more samples than stsz".into(),
                    ));
                }

                let size = match uniform_size {
                    Some(s) => s,
                    None => size_entries[sample_idx as usize],
                };

                index.push(IndexEntry {
                    offset: current_offset,
                    size,
                });

                current_offset += size as u64;
                sample_idx += 1;
            }
        }

        Ok(index)
    }

    /// Read a box header (size + type), supporting extended sizes.
    fn read_box_header(&mut self) -> Result<BoxHeader> {
        let start_pos = self.reader.seek(SeekFrom::Current(0))?;

        let raw_size = self.read_u32()?;
        let box_type = self.read_u32()?;

        let mut header_len = 8u64;
        let mut ext_size: Option<u64> = None;

        let size = if raw_size == 1 {
            // 64-bit extended size
            let s = self.read_u64()?;
            header_len = 16;
            ext_size = Some(s);
            s
        } else if raw_size == 0 {
            // extends to end of file
            let cur = self.reader.seek(SeekFrom::Current(0))?;
            let end = self.reader.seek(SeekFrom::End(0))?;
            self.reader.seek(SeekFrom::Start(cur))?;
            end.saturating_sub(start_pos)
        } else {
            raw_size as u64
        };

        if size < header_len {
            return Err(CrmError::InvalidFormat("box size too small".into()));
        }

        Ok(BoxHeader {
            size,
            box_type,
            header_len,
            raw_size,
            ext_size,
        })
    }

    /// Read box bytes including its header (reconstructed) without seeking backwards.
    fn read_box_data(&mut self, header: &BoxHeader) -> Result<Vec<u8>> {
        if header.size > (usize::MAX as u64) {
            return Err(CrmError::InvalidFormat("box too large to buffer".into()));
        }

        let mut data = vec![0u8; header.size as usize];

        // Reconstruct header
        data[0..4].copy_from_slice(&header.raw_size.to_be_bytes());
        data[4..8].copy_from_slice(&header.box_type.to_be_bytes());
        if header.raw_size == 1 {
            let ext = header.ext_size.ok_or_else(|| {
                CrmError::InvalidFormat("missing extended size for box".into())
            })?;
            data[8..16].copy_from_slice(&ext.to_be_bytes());
        }

        // Read remaining payload
        let payload_start = header.header_len as usize;
        let payload_len = (header.size - header.header_len) as usize;
        self.reader.read_exact(&mut data[payload_start..payload_start + payload_len])?;

        Ok(data)
    }

    /// Skip box data (excluding already-read header)
    fn skip_box_data(&mut self, header: &BoxHeader) -> Result<()> {
        let remaining = header.size - header.header_len;
        self.skip_bytes(remaining)
    }

    /// Skip n bytes
    fn skip_bytes(&mut self, mut n: u64) -> Result<()> {
        // SeekFrom::Current takes i64; guard huge skips.
        while n > 0 {
            let step = n.min(i64::MAX as u64);
            self.reader.seek(SeekFrom::Current(step as i64))?;
            n -= step;
        }
        Ok(())
    }

    /// Read a big-endian u32
    fn read_u32(&mut self) -> Result<u32> {
        let mut buf = [0u8; 4];
        self.reader.read_exact(&mut buf)?;
        Ok(u32::from_be_bytes(buf))
    }

    /// Read a big-endian u64
    fn read_u64(&mut self) -> Result<u64> {
        let mut buf = [0u8; 8];
        self.reader.read_exact(&mut buf)?;
        Ok(u64::from_be_bytes(buf))
    }

    /// Get the number of frames in the movie
    pub fn frame_count(&self) -> u64 {
        self.video_index.len() as u64
    }

    /// Get the timed-metadata sample count (may differ from `frame_count()`
    /// if the file's video and metadata tracks have different sample counts).
    pub fn timed_meta_count(&self) -> u64 {
        self.timed_meta_index.len() as u64
    }

    /// Size of the compressed video frame at `frame_no`, in bytes.
    pub fn compressed_frame_size(&self, frame_no: u64) -> Result<u32> {
        Self::sample_size(&self.video_index, frame_no)
    }

    /// Size of the timed-metadata sample at `frame_no`, in bytes.
    pub fn timed_meta_size(&self, frame_no: u64) -> Result<u32> {
        Self::sample_size(&self.timed_meta_index, frame_no)
    }

    /// Read a compressed video frame. Convenience wrapper around
    /// [`read_compressed_frame_into`](Self::read_compressed_frame_into);
    /// allocates a fresh `Vec<u8>` per call. For zero-allocation
    /// per-frame decode loops prefer the `_into` variant + an owned
    /// scratch buffer.
    pub fn read_compressed_frame(&mut self, frame_no: u64) -> Result<Vec<u8>> {
        let size = self.compressed_frame_size(frame_no)? as usize;
        let mut buf = Vec::with_capacity(size);
        unsafe { buf.set_len(size); }
        let n = self.read_compressed_frame_into(frame_no, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    /// Read timed metadata for a frame. Convenience wrapper; see
    /// [`read_compressed_frame`](Self::read_compressed_frame) for the
    /// allocation note.
    pub fn read_timed_meta(&mut self, frame_no: u64) -> Result<Vec<u8>> {
        let size = self.timed_meta_size(frame_no)? as usize;
        let mut buf = Vec::with_capacity(size);
        unsafe { buf.set_len(size); }
        let n = self.read_timed_meta_into(frame_no, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    /// Read the compressed video frame at `frame_no` into the caller-owned
    /// buffer `dst`. Returns the number of bytes written (always equal to
    /// `compressed_frame_size(frame_no)` on success). Returns
    /// [`CrmError::BufferTooSmall`] if `dst` is shorter than the sample size.
    pub fn read_compressed_frame_into(&mut self, frame_no: u64, dst: &mut [u8]) -> Result<usize> {
        Self::read_sample_into(&mut self.reader, &self.video_index, frame_no, dst)
    }

    /// Read the timed-metadata sample at `frame_no` into the caller-owned
    /// buffer `dst`. Semantics match [`read_compressed_frame_into`](Self::read_compressed_frame_into).
    pub fn read_timed_meta_into(&mut self, frame_no: u64, dst: &mut [u8]) -> Result<usize> {
        Self::read_sample_into(&mut self.reader, &self.timed_meta_index, frame_no, dst)
    }

    fn sample_size(index: &[IndexEntry], frame_no: u64) -> Result<u32> {
        let count = index.len() as u64;
        if frame_no >= count {
            return Err(CrmError::FrameOutOfRange { index: frame_no, count });
        }
        Ok(index[frame_no as usize].size)
    }

    /// Internal sample reader that writes into a caller-owned buffer.
    fn read_sample_into(reader: &mut R, index: &[IndexEntry], frame_no: u64, dst: &mut [u8]) -> Result<usize> {
        let count = index.len() as u64;
        if frame_no >= count {
            return Err(CrmError::FrameOutOfRange { index: frame_no, count });
        }

        let entry = &index[frame_no as usize];
        let size = entry.size as usize;

        if dst.len() < size {
            return Err(CrmError::BufferTooSmall { needed: size, got: dst.len() });
        }

        reader.seek(SeekFrom::Start(entry.offset))?;
        reader.read_exact(&mut dst[..size])?;

        Ok(size)
    }

    /// Get a reference to the CRAW entry data
    pub fn craw_entry(&self) -> Option<&[u8]> {
        self.craw_entry.as_deref()
    }

    /// Get a reference to the Canon metadata box data
    pub fn canon_metadata_box(&self) -> Option<&[u8]> {
        self.canon_metadata_box.as_deref()
    }

    /// Consume the movie and return the underlying reader
    pub fn into_inner(self) -> R {
        self.reader
    }

    /// Get a reference to the underlying reader
    pub fn get_ref(&self) -> &R {
        &self.reader
    }

    /// Get a mutable reference to the underlying reader
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.reader
    }
}

struct BoxHeader {
    size: u64,           // actual size
    box_type: u32,
    header_len: u64,     // 8 or 16
    raw_size: u32,       // original 32-bit size field (0, 1, or actual size)
    ext_size: Option<u64>,
}

/// Determine the header length of a box buffer (8 or 16)
#[inline]
fn box_header_len_from_data(data: &[u8]) -> usize {
    if data.len() >= 16 && read_u32_be(data) == 1 {
        16
    } else {
        8
    }
}

/// Read (size, type, header_len) for a child box at `offset` inside `data`.
fn read_box_from_slice(data: &[u8], offset: usize) -> Result<(usize, u32, usize)> {
    if offset + 8 > data.len() {
        return Err(CrmError::InvalidFormat("truncated box header".into()));
    }

    let size32 = read_u32_be(&data[offset..]);
    let box_type = read_u32_be(&data[offset + 4..]);

    let (size, header_len) = if size32 == 1 {
        if offset + 16 > data.len() {
            return Err(CrmError::InvalidFormat("truncated extended box header".into()));
        }
        let size64 = read_u64_be(&data[offset + 8..]);
        let size = usize::try_from(size64)
            .map_err(|_| CrmError::InvalidFormat("box too large".into()))?;
        (size, 16usize)
    } else if size32 == 0 {
        // extends to end of parent buffer
        (data.len() - offset, 8usize)
    } else {
        (size32 as usize, 8usize)
    };

    if size < header_len {
        return Err(CrmError::InvalidFormat("invalid box size".into()));
    }
    if offset + size > data.len() {
        return Err(CrmError::InvalidFormat("box size exceeds parent".into()));
    }

    Ok((size, box_type, header_len))
}

/// Read a big-endian u32 from a byte slice
#[inline]
fn read_u32_be(data: &[u8]) -> u32 {
    u32::from_be_bytes([data[0], data[1], data[2], data[3]])
}

/// Read a big-endian u64 from a byte slice
#[inline]
fn read_u64_be(data: &[u8]) -> u64 {
    u64::from_be_bytes([
        data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
    ])
}
