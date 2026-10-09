// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pure-Rust, SDK-free Canon Cinema RAW (`.CRM`) ISOBMFF container parser.
//!
//! A `.CRM` movie is an ISO-BMFF file whose video samples are **CRX** bitstreams
//! and whose colour science lives in Canon-private boxes (`CMP1`/`CDI1`/`CRAW`/
//! the Canon `uuid` metadata box) plus a timed-metadata track. This crate is a
//! container foundation that serves both kinds of CRM decoder:
//!
//! - decoders built on Canon's own SDK, such as
//!   [`canon-raw`](https://crates.io/crates/canon-raw), which hand the SDK the
//!   raw `CRAW` entry and Canon `uuid` box verbatim, and
//! - pure-Rust CRX decoders, which need the parsed [`Cmp1`] geometry,
//!   [`ActiveCrop`] and the Canon metadata tree.
//!
//! It depends **only** on [`mp4_atom`] — no Canon SDK, no FFI — so it builds
//! anywhere and either kind of decoder can depend on it.
//!
//! ## What it parses, and how I/O is split
//!
//! [`CrmContainer::load`] reads the **`moov`** box into memory (small — tens of
//! KB) and parses it; the multi-hundred-MB **`mdat` is never read at load**. The
//! sample-table leaf boxes (`stts`/`stsc`/`stsz`/`stco`/`co64`) are decoded
//! through [`mp4_atom`]'s typed parsers; everything else is read by hand off the
//! raw `moov` bytes — the box tree itself and the two fields taken from it
//! (`hdlr`'s handler type, `mdhd`'s timescale) by a tolerant child-walk, and the
//! Canon-private `CRAW`/`CMP1`/`CDI1`/`IAD1` sample-entry payloads (which are
//! *not* standard child boxes — they sit inside a `VisualSampleEntry` whose
//! leading bytes are not box-shaped) plus the `uuid` metadata box by signature
//! scan. See [`CrmContainer::load`] for why the typed decode stops at the leaves.
//!
//! The caller owns per-sample `mdat` I/O: [`CrmContainer::read_compressed_frame_into`]
//! (and its `timed_meta` / `audio` siblings) seeks to the sample offset and
//! `read_exact`s exactly `size` bytes — one frame in memory, O(1) regardless of
//! file size.

use std::io::{Read, Seek, SeekFrom};

use mp4_atom::{Co64, Decode, Stco, Stsc, Stsz, StszSamples, Stts};

mod error;
pub use error::CrmContainerError;

// Compile the README's example as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

/// `crm-container`'s `Result` alias.
pub type Result<T> = std::result::Result<T, CrmContainerError>;

/// The 16-byte UUID identifying the Canon metadata `uuid` box (the box carrying
/// the `CMT1`…`CMT4` / `CNCV` EXIF + colour-science tree). Direct child of
/// `moov`.
pub const CANON_METADATA_BOX_UUID: [u8; 16] =
    [0x85, 0xC0, 0xB6, 0x87, 0x82, 0x0F, 0x11, 0xE0, 0x81, 0x11, 0xF4, 0xCE, 0x46, 0x2B, 0x6A, 0x48];

/// A `(absolute file offset, byte length)` pointer to one sample inside `mdat`.
/// For the video track this is one compressed CRX frame; for the timed-metadata
/// track it is one per-frame metadata blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// Absolute offset of the sample's first byte in the file.
    pub offset: u64,
    /// Sample size in bytes.
    pub size: u32,
}

/// The Canon `CMP1` compression header — the CRX geometry / codec parameters,
/// in plain integers (no dependency on any codec crate, so each decoder maps it
/// into its own config type). Field layout mirrors LibRaw's
/// `crxParseImageHeader` byte-for-byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cmp1 {
    /// CMP1 header version (`0x100` / `0x200`).
    pub version: u16,
    /// Full raw read-out width (`f_width`).
    pub f_width: u32,
    /// Full raw read-out height (`f_height`).
    pub f_height: u32,
    /// CRX tile width in pixels.
    pub tile_width: u32,
    /// CRX tile height in pixels.
    pub tile_height: u32,
    /// Bits per sample (`nBits`).
    pub n_bits: u8,
    /// Median bit depth (`medianBits`).
    pub median_bits: u8,
    /// Plane count (`nPlanes`): 1 (JPEG-class) or 4 (the CFA RAW path).
    pub n_planes: u8,
    /// CFA layout selector (0..=3).
    pub cfa_layout: u8,
    /// CRX encode type (`encType`).
    pub enc_type: u8,
    /// Wavelet image-level count (`imageLevels`).
    pub image_levels: u8,
    /// Size of the CRX `mdat` sub-header (`mdatHdrSize`).
    pub mdat_hdr_size: u32,
}

/// The sensor active-area crop, decoded from the `CRAW` sample entry's
/// `CDI1`/`IAD1` box. The CRX bitstream codes the FULL read-out
/// (`f_width × f_height`); the active area is the visible sub-rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveCrop {
    /// First visible column (origin x).
    pub left_margin: u32,
    /// First visible row (origin y).
    pub top_margin: u32,
    /// Visible width.
    pub active_width: u32,
    /// Visible height.
    pub active_height: u32,
}

/// The uncompressed-LPCM audio track's format, decoded from the `soun` track's
/// `stsd` AudioSampleEntry. CRM audio is interleaved integer PCM; endianness is
/// carried by the codec fourcc (see [`AudioFormat::is_big_endian`]). Observed on
/// the Canon C80: `in24` (24-bit big-endian), mono, 48 kHz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    /// The sample-entry fourcc (`in24` / `in32` / `twos` / `sowt` / `ipcm` /
    /// `lpcm` / `fpcm`).
    pub codec_fourcc: [u8; 4],
    /// Interleaved channel count (`channelcount`).
    pub channels: u16,
    /// Bits per sample (`samplesize`, e.g. 16 or 24).
    pub bits_per_sample: u16,
    /// Sample rate in Hz (integer part of the 16.16 `samplerate`).
    pub sample_rate: u32,
}

impl AudioFormat {
    /// `true` when the PCM samples are big-endian. The big-endian QuickTime PCM
    /// fourccs are `twos` (16-bit), `in24` (24-bit) and `in32` (32-bit); the
    /// little-endian ones (`sowt` / `ipcm` / `lpcm` / `fpcm`, and anything else)
    /// return `false`.
    pub fn is_big_endian(&self) -> bool {
        matches!(&self.codec_fourcc, b"twos" | b"in24" | b"in32")
    }
}

/// A parsed CRM container over a seekable reader `R`.
///
/// Construct with [`CrmContainer::load`]; then read per-sample blobs
/// (seek-per-sample, `mdat` never bulk-loaded) from the video
/// ([`read_compressed_frame`](Self::read_compressed_frame)), timed-metadata
/// ([`read_timed_meta`](Self::read_timed_meta)) and LPCM audio
/// ([`read_audio_samples_into`](Self::read_audio_samples_into)) tracks, and
/// consult the container-level facts parsed at load ([`Cmp1`], [`ActiveCrop`],
/// fps, [`AudioFormat`], and the Canon `uuid` payload).
#[derive(Debug)]
pub struct CrmContainer<R> {
    reader: R,
    video_samples: Vec<Sample>,
    timed_meta_samples: Vec<Sample>,
    /// The `soun` track's per-sample table (empty if no audio track).
    audio_samples: Vec<Sample>,
    /// The parsed LPCM audio format (`None` if no audio track / unparseable).
    audio_format: Option<AudioFormat>,
    /// The audio media timescale (`= sample rate`); `0` if no audio track.
    audio_timescale: u32,
    cmp1: Cmp1,
    crop: ActiveCrop,
    fps: (u32, u32),
    /// The whole Canon `uuid` box bytes (including the 8-byte box header and the
    /// 16-byte UUID) — the Canon SDK consumes this verbatim as its
    /// `kCrmHeaderUuid` header.
    canon_uuid_box: Option<Vec<u8>>,
    /// The `CRAW` sample-entry bytes (the full entry: 4-byte size prefix +
    /// `'CRAW'` + body) — the Canon SDK consumes this verbatim as its
    /// `kCrmHeaderCraw` header. Not optional: [`CrmContainer::load`] cannot get
    /// past [`Cmp1`] without it.
    craw_entry: Vec<u8>,
}

impl<R: Read + Seek> CrmContainer<R> {
    /// Parse a CRM container from `reader`. Reads **only** `moov` (the sample
    /// tables + Canon-private metadata) — the top-level box chain is walked by
    /// header, so `ftyp` is skipped over and `mdat` is never read.
    ///
    /// The `ftyp` brand is deliberately NOT a gate here: the load already rejects
    /// a foreign container structurally (no `moov` / no `vide` track / no Canon
    /// `CMP1`), and the brand check is the *caller's* identification step — it
    /// is what lets a caller tell "not a CRM at all" from "a real CRM that is
    /// damaged" when this load fails.
    ///
    /// **Why the typed decode stops at the sample-table leaves.** CRM's `stsd`
    /// carries Canon-private sample entries (the video `CRAW`, the timed-metadata
    /// `CTMD`) that [`mp4_atom`]'s strict typed `Moov` / `Stsd` decode rejects —
    /// so a typed whole-tree decode fails on every real `.CRM`. The tree is
    /// therefore walked tolerantly by hand ([`child_boxes`]) and only the standard
    /// leaf boxes are handed to `mp4-atom`'s verified parsers; `stsd` itself is
    /// never decoded (its Canon-private payloads are recovered by signature scan,
    /// and its LPCM AudioSampleEntry is read field-by-field).
    ///
    /// # Errors
    ///
    /// Two distinguishable classes, because consumers must treat them differently
    /// (see [`CrmContainerError`]):
    ///
    /// * [`CrmContainerError::Unsupported`] — the container reads, but is not a
    ///   Canon Cinema RAW **movie** of the shape this crate decodes: no `vide`
    ///   track, an empty video sample table, no `CRAW` sample entry, or no `CMP1`
    ///   inside it. Nothing here says the bytes are damaged, so a caller that
    ///   tries several decoders must keep falling through to the others.
    /// * [`CrmContainerError::MissingBox`] / [`CrmContainerError::Parse`] — a
    ///   standard ISOBMFF structure any movie must have is absent (`moov`, a
    ///   sample-table leaf) or present-but-garbage. On a positively identified
    ///   `.CRM` that IS a broken file.
    ///
    /// Plus [`CrmContainerError::Io`] when the reader fails — including
    /// `UnexpectedEof` on a file truncated inside `moov`.
    pub fn load(mut reader: R) -> Result<Self> {
        let flen = reader.seek(SeekFrom::End(0))?;

        // Locate the top-level `moov`, read its BODY into memory (small — the
        // multi-hundred-MB mdat is never read).
        let moov_span = top_level_box(&mut reader, flen, b"moov")?.ok_or(CrmContainerError::MissingBox("moov"))?;
        let mut moov_buf = vec![0u8; moov_span.body_len as usize];
        reader.seek(SeekFrom::Start(moov_span.body_off))?;
        reader.read_exact(&mut moov_buf)?;

        // CRM's `stsd` carries Canon-private sample entries (`CRAW`, the
        // timed-metadata `CTMD`) that `mp4-atom`'s STRICT typed `Moov`/`Stsd`
        // decode chokes on. So `crm-container` does a **tolerant raw child-walk**
        // of the moov tree (`child_boxes`) and hands `mp4-atom`'s verified leaf
        // parsers only the standard sample-table boxes — `stsd` is never DECODED.
        // (Its bytes are still read: `parse_audio_format` walks the LPCM
        // AudioSampleEntry by hand below, and the Canon-private CRAW entry is
        // recovered by signature scan — neither is a standard codec entry.)
        // A container with no `vide` track / no video samples is not damaged — it
        // is simply not a movie this parser can serve, so it declines
        // (`Unsupported`) rather than asserting the file is broken.
        let video_trak = find_track_body(&moov_buf, b"vide")
            .ok_or(CrmContainerError::Unsupported("no video track (handler 'vide')"))?;
        let video_samples = build_sample_table(video_trak.stbl)?;
        if video_samples.is_empty() {
            return Err(CrmContainerError::Unsupported("video track has zero samples"));
        }

        let timed_meta_track = find_track_body(&moov_buf, b"meta");
        let timed_meta_samples =
            timed_meta_track.as_ref().map(|t| build_sample_table(t.stbl)).transpose()?.unwrap_or_default();

        // Audio (`soun`) track — Canon's CRM SDK exposes zero audio, so the
        // uncompressed LPCM track is surfaced here from the container. Tolerant:
        // a malformed / unparseable audio track never fails the container load.
        let audio_track = find_track_body(&moov_buf, b"soun");
        let audio_samples =
            audio_track.as_ref().map(|t| build_sample_table(t.stbl).unwrap_or_default()).unwrap_or_default();
        let audio_format = audio_track.as_ref().and_then(|t| parse_audio_format(t.stbl));
        let audio_timescale = audio_track.as_ref().map(|t| t.timescale).unwrap_or(0);

        // fps = mdhd timescale / first stts sample_delta.
        let fps = {
            let timescale = video_trak.timescale;
            let delta = first_stts_delta(video_trak.stbl);
            if timescale != 0 && delta != 0 { (timescale, delta) } else { (24, 1) }
        };

        // Canon-private payloads — signature scans over the raw `moov` buffer.
        // The `CRAW` sample entry sits inside a `VisualSampleEntry` whose leading
        // bytes are not box-shaped, so these are not standard child boxes.
        //
        // `CRAW` is REQUIRED: it is the only source of `CMP1` (the CRX geometry
        // this container cannot be used without), so its absence is a load
        // failure and every consumer below can take it as present.
        //
        // Absent, these two mean the movie's video samples are NOT CRX — a
        // container variant this parser does not decode, not a damaged file. So
        // they decline (`Unsupported`) and a caller can still try another
        // decoder.
        let craw_entry =
            find_craw_entry(&moov_buf).ok_or(CrmContainerError::Unsupported("no CRAW video sample entry"))?;
        let cmp1 = parse_cmp1(&craw_entry)
            .ok_or(CrmContainerError::Unsupported("no CMP1 (CRX geometry) in the CRAW entry"))?;
        let crop = parse_active_crop(&craw_entry, &cmp1);
        let canon_uuid_box = find_canon_uuid_box(&moov_buf);

        Ok(Self {
            reader,
            video_samples,
            timed_meta_samples,
            audio_samples,
            audio_format,
            audio_timescale,
            cmp1,
            crop,
            fps,
            canon_uuid_box,
            craw_entry,
        })
    }

    /// The video track's per-sample `(offset, size)` table.
    pub fn video_samples(&self) -> &[Sample] {
        &self.video_samples
    }

    /// The timed-metadata track's per-sample table (empty if no `meta` track).
    pub fn timed_meta_samples(&self) -> &[Sample] {
        &self.timed_meta_samples
    }

    /// Number of compressed video frames.
    pub fn frame_count(&self) -> u64 {
        self.video_samples.len() as u64
    }

    /// Number of timed-metadata samples (may differ from [`Self::frame_count`]).
    pub fn timed_meta_count(&self) -> u64 {
        self.timed_meta_samples.len() as u64
    }

    /// The parsed `CMP1` CRX geometry / codec parameters.
    pub fn cmp1(&self) -> &Cmp1 {
        &self.cmp1
    }

    /// The sensor active-area crop (`CDI1`/`IAD1`, with a symmetric fallback).
    pub fn active_crop(&self) -> &ActiveCrop {
        &self.crop
    }

    /// Frame rate as `(numerator, denominator)` = `(timescale, sample_delta)`.
    pub fn fps(&self) -> (u32, u32) {
        self.fps
    }

    /// The Canon `uuid` metadata box payload **AFTER** the 16-byte UUID — the
    /// `CMT1`…`CMT4` / `CNCV` box tree a pure-Rust decoder parses for colour
    /// science (walk it with [`child_boxes`]). `None` on a non-Canon / absent
    /// box.
    pub fn canon_uuid_payload(&self) -> Option<&[u8]> {
        self.canon_uuid_box.as_deref().map(|b| &b[(8 + 16).min(b.len())..])
    }

    /// The **full** Canon `uuid` metadata box bytes (box header + UUID + tree) —
    /// the verbatim blob the Canon SDK consumes as its `kCrmHeaderUuid` header.
    pub fn canon_uuid_box(&self) -> Option<&[u8]> {
        self.canon_uuid_box.as_deref()
    }

    /// The full `CRAW` sample-entry bytes (size prefix + `'CRAW'` + body) — the
    /// verbatim blob the Canon SDK consumes as its `kCrmHeaderCraw` header.
    ///
    /// Infallible on a loaded container — the entry is what [`Self::cmp1`] is
    /// parsed out of, so a `.CRM` without one never loads (unlike
    /// [`canon_uuid_box`](Self::canon_uuid_box), which genuinely CAN be absent
    /// and stays optional).
    pub fn craw_entry(&self) -> &[u8] {
        &self.craw_entry
    }

    /// The compressed-frame size in bytes at `frame_no`.
    pub fn compressed_frame_size(&self, frame_no: u64) -> Result<u32> {
        sample_size(&self.video_samples, frame_no)
    }

    /// The timed-metadata sample size in bytes at `frame_no`.
    pub fn timed_meta_size(&self, frame_no: u64) -> Result<u32> {
        sample_size(&self.timed_meta_samples, frame_no)
    }

    /// Read the compressed video frame at `frame_no` into a fresh buffer.
    pub fn read_compressed_frame(&mut self, frame_no: u64) -> Result<Vec<u8>> {
        let size = self.compressed_frame_size(frame_no)? as usize;
        let mut buf = vec![0u8; size];
        let n = read_sample_into(&mut self.reader, &self.video_samples, frame_no, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    /// Read the timed-metadata sample at `frame_no` into a fresh buffer.
    pub fn read_timed_meta(&mut self, frame_no: u64) -> Result<Vec<u8>> {
        let size = self.timed_meta_size(frame_no)? as usize;
        let mut buf = vec![0u8; size];
        let n = read_sample_into(&mut self.reader, &self.timed_meta_samples, frame_no, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    /// Read the compressed video frame at `frame_no` into the caller-owned `dst`.
    /// Returns the bytes written (always the sample size on success).
    pub fn read_compressed_frame_into(&mut self, frame_no: u64, dst: &mut [u8]) -> Result<usize> {
        read_sample_into(&mut self.reader, &self.video_samples, frame_no, dst)
    }

    /// Read the timed-metadata sample at `frame_no` into the caller-owned `dst`.
    pub fn read_timed_meta_into(&mut self, frame_no: u64, dst: &mut [u8]) -> Result<usize> {
        read_sample_into(&mut self.reader, &self.timed_meta_samples, frame_no, dst)
    }

    /// The uncompressed-LPCM audio format, or `None` when the container has no
    /// (parseable) `soun` track. Canon's CRM SDK exposes no audio, so this is the
    /// only source of the movie's audio.
    pub fn audio_format(&self) -> Option<AudioFormat> {
        self.audio_format
    }

    /// Number of audio samples (LPCM sample-table entries; `0` if no audio).
    pub fn audio_sample_count(&self) -> usize {
        self.audio_samples.len()
    }

    /// The audio media timescale (equals the LPCM sample rate); `0` if no audio.
    pub fn audio_timescale(&self) -> u32 {
        self.audio_timescale
    }

    /// Read the audio sample at `index` into the caller-owned `buf` (resized to
    /// the sample size). Returns the bytes written.
    pub fn read_audio_sample_into(&mut self, index: usize, buf: &mut Vec<u8>) -> Result<usize> {
        let size = sample_size(&self.audio_samples, index as u64)? as usize;
        buf.resize(size, 0);
        read_sample_into(&mut self.reader, &self.audio_samples, index as u64, buf)
    }

    /// Read a **contiguous run** of `count` audio samples starting at `start`
    /// into `buf` (cleared first), returning the total bytes written.
    ///
    /// LPCM sample tables store consecutive samples back-to-back within a `stco`
    /// chunk (`offset[i+1] == offset[i] + size[i]`), so this coalesces each
    /// maximal contiguous span into a **single** `seek` + `read_exact` — reading
    /// a whole chunk's worth of samples in one I/O instead of one per sample.
    /// The per-sample [`read_audio_sample_into`](Self::read_audio_sample_into)
    /// is O(count) syscalls; this is O(number of chunk spans) (typically 1–2 for
    /// a few hundred samples), which matters because LPCM samples are tiny
    /// (e.g. 3 bytes for mono 24-bit) and a playback buffer covers hundreds of
    /// them.
    ///
    /// The bytes are the raw interleaved container layout (endianness / width per
    /// [`audio_format`](Self::audio_format)); the caller normalises them.
    pub fn read_audio_samples_into(&mut self, start: usize, count: usize, buf: &mut Vec<u8>) -> Result<usize> {
        buf.clear();
        if count == 0 {
            return Ok(0);
        }
        let end = start
            .checked_add(count)
            .ok_or(CrmContainerError::FrameOutOfRange { index: u64::MAX, count: self.audio_samples.len() as u64 })?;
        if end > self.audio_samples.len() {
            return Err(CrmContainerError::FrameOutOfRange {
                index: end as u64 - 1,
                count: self.audio_samples.len() as u64,
            });
        }

        let mut total = 0usize;
        let mut i = start;
        while i < end {
            // Extend the run while the next sample sits immediately after the
            // current span in the file.
            let run_off = self.audio_samples[i].offset;
            let mut run_len = self.audio_samples[i].size as u64;
            let mut j = i + 1;
            while j < end && self.audio_samples[j].offset == run_off + run_len {
                run_len += self.audio_samples[j].size as u64;
                j += 1;
            }
            let span = run_len as usize;
            let at = buf.len();
            buf.resize(at + span, 0);
            self.reader.seek(SeekFrom::Start(run_off))?;
            self.reader.read_exact(&mut buf[at..at + span])?;
            total += span;
            i = j;
        }
        Ok(total)
    }

    /// Consume the container, returning the underlying reader.
    pub fn into_inner(self) -> R {
        self.reader
    }

    /// Shared reference to the underlying reader.
    pub fn get_ref(&self) -> &R {
        &self.reader
    }

    /// Mutable reference to the underlying reader.
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.reader
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Top-level box scan (seek-based, never reads mdat)
// ─────────────────────────────────────────────────────────────────────────────

/// A top-level box body's location.
struct BoxSpan {
    body_off: u64,
    body_len: u64,
}

/// Scan the top-level box list for the first box of `name`, returning its body
/// span. Reads only 8/16-byte headers, seeking past each box (never reads box
/// bodies — in particular never reads `mdat`).
fn top_level_box<R: Read + Seek>(reader: &mut R, flen: u64, name: &[u8; 4]) -> Result<Option<BoxSpan>> {
    let mut pos = 0u64;
    while pos.checked_add(8).is_some_and(|end| end <= flen) {
        reader.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 16];
        reader.read_exact(&mut hdr[..8])?;
        let size32 = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        let typ = [hdr[4], hdr[5], hdr[6], hdr[7]];
        let (body_off, total) = if size32 == 1 {
            reader.read_exact(&mut hdr[8..16])?;
            let size64 = u64::from_be_bytes([hdr[8], hdr[9], hdr[10], hdr[11], hdr[12], hdr[13], hdr[14], hdr[15]]);
            (pos + 16, size64)
        } else if size32 == 0 {
            (pos + 8, flen - pos)
        } else {
            (pos + 8, u64::from(size32))
        };
        let header_len = body_off - pos;
        // `total` is attacker-controlled (the box's 32/64-bit size field); guard
        // the `pos + total` advance against `u64` overflow.
        let Some(next_pos) = pos.checked_add(total) else {
            break; // malformed
        };
        if total < header_len {
            break; // malformed
        }
        if typ == *name {
            return Ok(Some(BoxSpan { body_off, body_len: total - header_len }));
        }
        pos = next_pos;
    }
    Ok(None)
}

// ─────────────────────────────────────────────────────────────────────────────
// Track / sample-table reconstruction
// ─────────────────────────────────────────────────────────────────────────────
//
// CRM's `stsd` carries Canon-private sample entries (`CRAW`, `CTMD`) that
// `mp4-atom`'s strict typed `Moov`/`Stsd` decode rejects. So we tolerate the
// tree with a raw child-walk (`child_boxes`) and decode only the STANDARD
// sample-table leaf boxes through `mp4-atom`'s verified leaf parsers — `stsd` is
// never decoded (its `CRAW` payload is recovered by signature scan).

/// A located track's media-box facts: the raw `stbl` body and the media
/// `timescale` (from `mdhd`).
struct TrackBody<'a> {
    stbl: &'a [u8],
    timescale: u32,
}

/// Find the first track whose `mdia/hdlr` handler equals `handler`, via a
/// tolerant raw child-walk: `moov` → `trak` → `mdia` → {`hdlr`, `mdhd`,
/// `minf`/`stbl`}. Returns the raw `stbl` body + media timescale.
fn find_track_body<'a>(moov_body: &'a [u8], handler: &[u8; 4]) -> Option<TrackBody<'a>> {
    for (t, trak) in child_boxes(moov_body) {
        if &t != b"trak" {
            continue;
        }
        let mdia = first_child(trak, b"mdia")?;
        let hdlr = first_child(mdia, b"hdlr")?;
        // hdlr body: version/flags(4) + pre_defined(4) + handler_type(4 @ +8).
        if hdlr.len() < 12 || &hdlr[8..12] != handler {
            continue;
        }
        let minf = first_child(mdia, b"minf")?;
        let stbl = first_child(minf, b"stbl")?;
        let timescale = first_child(mdia, b"mdhd").map(read_mdhd_timescale).unwrap_or(0);
        return Some(TrackBody { stbl, timescale });
    }
    None
}

/// Parse the LPCM [`AudioFormat`] out of a `soun` track's raw `stbl`, via a
/// tolerant manual walk of the `stsd` AudioSampleEntry (mirroring the video
/// `CRAW` recovery: `mp4-atom`'s strict `Stsd` decode is bypassed so private /
/// QuickTime-flavoured entries don't reject the whole tree). Reads the v0
/// AudioSampleEntry fields; endianness derives from the fourcc. Returns `None`
/// (skip audio) on any structural problem.
fn parse_audio_format(stbl: &[u8]) -> Option<AudioFormat> {
    // `stsd` body: version/flags(4) + entry_count(4), then the sample entries.
    // The first sample entry box begins at offset 8.
    let stsd = first_child(stbl, b"stsd")?;
    let entry = stsd.get(8..)?;
    // entry: size(4, big-endian) + fourcc(4) + AudioSampleEntry body.
    let codec_fourcc: [u8; 4] = entry.get(4..8)?.try_into().ok()?;
    // AudioSampleEntry body (after the entry's 8-byte size+type header):
    //   reserved[6] + data_reference_index[2]   → 0..8
    //   version[2] + revision[2] + vendor[4]    → 8..16
    //   channelcount[2]                          → 16
    //   samplesize[2]                            → 18
    //   pre_defined[2] + reserved[2]             → 20..24
    //   samplerate[4] as 16.16 fixed-point       → 24 (high 16 bits = integer Hz)
    let body = entry.get(8..)?;
    let rd_u16 = |o: usize| -> Option<u16> { body.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]])) };
    let channels = rd_u16(16)?;
    let bits_per_sample = rd_u16(18)?;
    let sample_rate = u32::from(rd_u16(24)?);
    if channels == 0 || sample_rate == 0 {
        return None;
    }
    Some(AudioFormat { codec_fourcc, channels, bits_per_sample, sample_rate })
}

/// First child box of `name` in an in-memory box body.
fn first_child<'a>(buf: &'a [u8], name: &[u8; 4]) -> Option<&'a [u8]> {
    child_boxes(buf).into_iter().find(|(t, _)| t == name).map(|(_, b)| b)
}

/// `mdhd` timescale (FullBox; version 0 → @12, version 1 → @20).
fn read_mdhd_timescale(mdhd: &[u8]) -> u32 {
    let version = mdhd.first().copied().unwrap_or(0);
    let o = if version == 1 { 20 } else { 12 };
    mdhd.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0)
}

/// First `stts` sample delta (for the fps denominator), via `mp4-atom`'s leaf
/// decoder. `0` when absent.
fn first_stts_delta(stbl: &[u8]) -> u32 {
    decode_leaf::<Stts>(stbl, b"stts")
        .ok()
        .flatten()
        .and_then(|s| s.entries.first().map(|e| e.sample_delta))
        .unwrap_or(0)
}

/// Decode one STANDARD sample-table leaf box (`name`) out of a raw `stbl` body
/// using `mp4-atom`'s typed parser. Returns `Ok(None)` when the box is absent.
/// The box bytes (header + body) are passed to `T::decode`, which consumes the
/// box header itself.
fn decode_leaf<T: Decode>(stbl: &[u8], name: &[u8; 4]) -> Result<Option<T>> {
    // Re-find the leaf with its full (header-inclusive) byte span so `T::decode`
    // sees the box header it expects.
    let mut p = 0usize;
    while p + 8 <= stbl.len() {
        let size32 = u32::from_be_bytes([stbl[p], stbl[p + 1], stbl[p + 2], stbl[p + 3]]) as usize;
        let typ = [stbl[p + 4], stbl[p + 5], stbl[p + 6], stbl[p + 7]];
        let total = if size32 == 1 {
            if p + 16 > stbl.len() {
                break;
            }
            u64::from_be_bytes([
                stbl[p + 8],
                stbl[p + 9],
                stbl[p + 10],
                stbl[p + 11],
                stbl[p + 12],
                stbl[p + 13],
                stbl[p + 14],
                stbl[p + 15],
            ]) as usize
        } else if size32 == 0 {
            stbl.len() - p
        } else {
            size32
        };
        // `total` is attacker-controlled; guard `p + total` against overflow.
        let Some(box_end) = p.checked_add(total) else { break };
        if total < 8 || box_end > stbl.len() {
            break;
        }
        if &typ == name {
            let mut b: &[u8] = &stbl[p..box_end];
            let atom =
                T::decode(&mut b).map_err(|e| CrmContainerError::Parse(format!("{} decode: {e}", fourcc_str(name))))?;
            return Ok(Some(atom));
        }
        p = box_end;
    }
    Ok(None)
}

fn fourcc_str(name: &[u8; 4]) -> String {
    String::from_utf8_lossy(name).into_owned()
}

/// Build the per-sample `(offset, size)` table from the standard sample-table
/// leaf boxes (`stsz` / `stsc` / `stco`|`co64`), each decoded via `mp4-atom`.
/// CRM is intra-only with one sample-per-chunk in practice, but the full `stsc`
/// run-length walk is honoured.
fn build_sample_table(stbl: &[u8]) -> Result<Vec<Sample>> {
    let stsz = decode_leaf::<Stsz>(stbl, b"stsz")?.ok_or(CrmContainerError::MissingBox("stsz (sample sizes)"))?;
    let stsc = decode_leaf::<Stsc>(stbl, b"stsc")?.ok_or(CrmContainerError::MissingBox("stsc (sample-to-chunk)"))?;
    let chunk_offsets = read_chunk_offsets(stbl)?;

    let sample_sizes: Vec<u32> = match &stsz.samples {
        StszSamples::Identical { count, size } => vec![*size; *count as usize],
        StszSamples::Different { sizes } => sizes.clone(),
    };

    let mut samples = Vec::with_capacity(sample_sizes.len());
    let mut si = 0usize;
    for (ci, &chunk_off) in chunk_offsets.iter().enumerate() {
        let spc = samples_per_chunk((ci + 1) as u32, &stsc);
        let mut off = chunk_off;
        for _ in 0..spc {
            let Some(&size) = sample_sizes.get(si) else { break };
            samples.push(Sample { offset: off, size });
            off += u64::from(size);
            si += 1;
        }
        if si >= sample_sizes.len() {
            break;
        }
    }
    Ok(samples)
}

/// samples-per-chunk for a 1-based chunk index, per the run-length `stsc` table.
fn samples_per_chunk(chunk_1based: u32, stsc: &Stsc) -> u32 {
    let mut spc = 1;
    for e in &stsc.entries {
        if e.first_chunk <= chunk_1based {
            spc = e.samples_per_chunk;
        } else {
            break;
        }
    }
    spc
}

/// Chunk offsets from `co64` (preferred when present) or `stco`, decoded via
/// `mp4-atom`'s leaf parsers.
fn read_chunk_offsets(stbl: &[u8]) -> Result<Vec<u64>> {
    if let Some(co64) = decode_leaf::<Co64>(stbl, b"co64")? {
        Ok(co64.entries)
    } else if let Some(stco) = decode_leaf::<Stco>(stbl, b"stco")? {
        Ok(stco.entries.into_iter().map(u64::from).collect())
    } else {
        Err(CrmContainerError::MissingBox("stco/co64 (chunk offsets)"))
    }
}

fn sample_size(index: &[Sample], frame_no: u64) -> Result<u32> {
    index
        .get(frame_no as usize)
        .map(|s| s.size)
        .ok_or(CrmContainerError::FrameOutOfRange { index: frame_no, count: index.len() as u64 })
}

fn read_sample_into<R: Read + Seek>(reader: &mut R, index: &[Sample], frame_no: u64, dst: &mut [u8]) -> Result<usize> {
    let s = *index
        .get(frame_no as usize)
        .ok_or(CrmContainerError::FrameOutOfRange { index: frame_no, count: index.len() as u64 })?;
    let size = s.size as usize;
    if dst.len() < size {
        return Err(CrmContainerError::BufferTooSmall { needed: size, got: dst.len() });
    }
    reader.seek(SeekFrom::Start(s.offset))?;
    reader.read_exact(&mut dst[..size])?;
    Ok(size)
}

// ─────────────────────────────────────────────────────────────────────────────
// Canon-private payloads (signature scans over the raw moov buffer)
// ─────────────────────────────────────────────────────────────────────────────

/// Generic child-box iterator over an in-memory box body — yields `(type, body)`
/// for each child. Tolerant of truncation (stops at the first malformed header).
/// Used by this crate's own moov walk, and exported for colour-science parsers
/// that walk the Canon `uuid` payload's `CMT1`…`CMT4` tree. A decoder built on
/// the Canon SDK does not need it — the SDK is handed the `uuid` box verbatim.
pub fn child_boxes(buf: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut p = 0usize;
    while p + 8 <= buf.len() {
        let size32 = u32::from_be_bytes([buf[p], buf[p + 1], buf[p + 2], buf[p + 3]]) as usize;
        let typ = [buf[p + 4], buf[p + 5], buf[p + 6], buf[p + 7]];
        let (body_start, total) = if size32 == 1 {
            if p + 16 > buf.len() {
                break;
            }
            let s = u64::from_be_bytes([
                buf[p + 8],
                buf[p + 9],
                buf[p + 10],
                buf[p + 11],
                buf[p + 12],
                buf[p + 13],
                buf[p + 14],
                buf[p + 15],
            ]) as usize;
            (p + 16, s)
        } else if size32 == 0 {
            (p + 8, buf.len() - p)
        } else {
            (p + 8, size32)
        };
        // `total` is attacker-controlled (the box's 32/64-bit size field); guard
        // the `p + total` end offset against `usize` overflow before bounds-checks.
        let Some(box_end) = p.checked_add(total) else { break };
        if total < (body_start - p) || box_end > buf.len() {
            break;
        }
        out.push((typ, &buf[body_start..box_end]));
        p = box_end;
    }
    out
}

/// First occurrence of `needle` in `hay`.
fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// Locate and copy the `CRAW` sample entry (size prefix + `'CRAW'` + body) out of
/// the raw `moov` buffer. The 4-byte big-endian size precedes the `'CRAW'` type.
fn find_craw_entry(moov: &[u8]) -> Option<Vec<u8>> {
    let craw = find_sub(moov, b"CRAW")?;
    let box_start = craw.checked_sub(4)?;
    let box_size =
        u32::from_be_bytes([moov[box_start], moov[box_start + 1], moov[box_start + 2], moov[box_start + 3]]) as usize;
    let end = box_start.checked_add(box_size)?;
    if box_size < 8 || end > moov.len() {
        return None;
    }
    Some(moov[box_start..end].to_vec())
}

/// Parse the `CMP1` compression header out of the `CRAW` sample-entry bytes.
/// Mirrors LibRaw's `crxParseImageHeader` field layout (big-endian).
fn parse_cmp1(craw_entry: &[u8]) -> Option<Cmp1> {
    let cpos = find_sub(craw_entry, b"CMP1")?;
    let box_size = u32::from_be_bytes([
        *craw_entry.get(cpos.checked_sub(4)?)?,
        *craw_entry.get(cpos - 3)?,
        *craw_entry.get(cpos - 2)?,
        *craw_entry.get(cpos - 1)?,
    ]) as usize;
    let payload_start = cpos + 4; // skip the 4-byte 'CMP1' type
    // `box_size` is attacker-controlled (the 4 bytes preceding 'CMP1'); guard the
    // box-start − 4 + box_size sum against `usize` overflow before bounds-checking.
    let payload_end = cpos.checked_sub(4)?.checked_add(box_size)?;
    if box_size < 8 + 32 || payload_end > craw_entry.len() || payload_end <= payload_start {
        return None;
    }
    let p = &craw_entry[payload_start..payload_end];
    let rd_u16 = |o: usize| u16::from_be_bytes([p[o], p[o + 1]]);
    let rd_u32 = |o: usize| u32::from_be_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]]);

    let n_bits = p[24];
    let plane_byte = p[25];
    let enc_byte = p[26];
    let n_planes = plane_byte >> 4;

    // medianBits: default nBits, overridden by the CMP1 extended header.
    let mut median_bits = n_bits;
    let ext_header = p.len() > 32 && (p[32] >> 7) != 0;
    if ext_header && p.len() > 56 && n_planes == 4 {
        let use_median_bits = (p[56] >> 6) & 1 != 0;
        if use_median_bits && p.len() > 84 {
            median_bits = p[84];
        }
    }

    Some(Cmp1 {
        version: rd_u16(4),
        f_width: rd_u32(8),
        f_height: rd_u32(12),
        tile_width: rd_u32(16),
        tile_height: rd_u32(20),
        n_bits,
        median_bits,
        n_planes,
        cfa_layout: plane_byte & 0x0F,
        enc_type: enc_byte >> 4,
        image_levels: enc_byte & 0x0F,
        mdat_hdr_size: rd_u32(28),
    })
}

/// Parse the active-area crop from the `CRAW` sample-entry's `CDI1`→`IAD1` box.
/// The `IAD1` payload encodes the visible sub-rectangle with **inclusive**
/// bounds. Cross-checks the decoded size against the `CRAW` display dimensions,
/// falling back to a symmetric `(coded − display)/2` margin on any mismatch.
fn parse_active_crop(craw_entry: &[u8], cmp1: &Cmp1) -> ActiveCrop {
    let (coded_w, coded_h) = (cmp1.f_width, cmp1.f_height);
    let (disp_w, disp_h) = craw_display_dims(craw_entry).unwrap_or((coded_w, coded_h));

    let symmetric = || {
        let aw = disp_w.min(coded_w);
        let ah = disp_h.min(coded_h);
        ActiveCrop {
            left_margin: (coded_w - aw) / 2,
            top_margin: (coded_h - ah) / 2,
            active_width: aw,
            active_height: ah,
        }
    };

    let Some(ipos) = find_sub(craw_entry, b"IAD1") else {
        return symmetric();
    };
    let p = ipos + 4; // skip the 4-byte 'IAD1' type
    let Some(payload) = craw_entry.get(p..p + 24) else {
        return symmetric();
    };
    let rd_u16 = |o: usize| u16::from_be_bytes([payload[o], payload[o + 1]]) as u32;
    let left = rd_u16(16);
    let top = rd_u16(18);
    let right = rd_u16(20); // inclusive
    let bottom = rd_u16(22); // inclusive

    if right < left || bottom < top {
        return symmetric();
    }
    let active_width = right - left + 1;
    let active_height = bottom - top + 1;
    let in_bounds = left + active_width <= coded_w && top + active_height <= coded_h;
    let matches_display = active_width == disp_w && active_height == disp_h;
    if !in_bounds || !matches_display {
        return symmetric();
    }
    ActiveCrop { left_margin: left, top_margin: top, active_width, active_height }
}

/// The `CRAW` VisualSampleEntry's display `(width, height)` — big-endian u16 at
/// box-offsets +0x20 / +0x22. (Within the entry bytes the box header starts at
/// offset 0; the `'CRAW'` type is at +4.)
fn craw_display_dims(craw_entry: &[u8]) -> Option<(u32, u32)> {
    let w = u16::from_be_bytes([*craw_entry.get(0x20)?, *craw_entry.get(0x21)?]);
    let h = u16::from_be_bytes([*craw_entry.get(0x22)?, *craw_entry.get(0x23)?]);
    if w == 0 || h == 0 {
        return None;
    }
    Some((w as u32, h as u32))
}

/// Scan `moov` for the Canon `uuid` metadata box and return the **whole** box
/// bytes (box header + 16-byte UUID + tree). `None` if no box with the Canon
/// UUID is present.
fn find_canon_uuid_box(moov: &[u8]) -> Option<Vec<u8>> {
    let mut p = 0usize;
    while p + 8 <= moov.len() {
        let size32 = u32::from_be_bytes([moov[p], moov[p + 1], moov[p + 2], moov[p + 3]]) as usize;
        let typ = [moov[p + 4], moov[p + 5], moov[p + 6], moov[p + 7]];
        let (body_start, total) = if size32 == 1 {
            if p + 16 > moov.len() {
                break;
            }
            let s = u64::from_be_bytes([
                moov[p + 8],
                moov[p + 9],
                moov[p + 10],
                moov[p + 11],
                moov[p + 12],
                moov[p + 13],
                moov[p + 14],
                moov[p + 15],
            ]) as usize;
            (p + 16, s)
        } else if size32 == 0 {
            (p + 8, moov.len() - p)
        } else {
            (p + 8, size32)
        };
        // `total` is attacker-controlled; guard `p + total` against overflow.
        let Some(box_end) = p.checked_add(total) else { break };
        if total < (body_start - p) || box_end > moov.len() {
            break;
        }
        if &typ == b"uuid"
            && moov[body_start..box_end].len() >= 16
            && moov[body_start..body_start + 16] == CANON_METADATA_BOX_UUID
        {
            return Some(moov[p..box_end].to_vec());
        }
        p = box_end;
    }
    None
}

impl Cmp1 {
    /// `true` when this is the 4-plane CFA RAW path (the CRM video case), as
    /// opposed to the single-plane JPEG-class header.
    pub fn is_cfa(&self) -> bool {
        self.n_planes == 4
    }
}

/// Parse the [`Cmp1`] CRX geometry from a raw `CRAW` sample-entry byte slice
/// (the full entry: size prefix + `'CRAW'` + body, as
/// [`CrmContainer::craw_entry`] returns it). `None` if no `CMP1` box is present.
pub fn parse_craw_cmp1(craw_entry: &[u8]) -> Option<Cmp1> {
    parse_cmp1(craw_entry)
}

/// Parse the [`ActiveCrop`] active-area crop (`CDI1`/`IAD1`) from a raw `CRAW`
/// sample-entry byte slice, given its already-parsed [`Cmp1`] (for the coded
/// read-out dimensions). Falls back to the symmetric `(coded − display)/2`
/// margin when `IAD1` is absent / inconsistent.
pub fn parse_craw_active_crop(craw_entry: &[u8], cmp1: &Cmp1) -> ActiveCrop {
    parse_active_crop(craw_entry, cmp1)
}
