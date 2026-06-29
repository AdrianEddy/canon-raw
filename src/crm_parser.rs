// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

//! `CrmMovie` — the SDK-lane CRM container view.
//!
//! The ISOBMFF box-walk + sample-table + Canon-private-box extraction that used
//! to live here inline (the 767-LOC `CrmMovie<R>`) is now the shared, SDK-free
//! [`crm_container`] crate (oxivideo workspace, Stage-9 §9.9), so the Canon-SDK
//! lane and the pure-Rust oxiraw lane share ONE container foundation. This file
//! is now thin glue: `CrmMovie` wraps a [`crm_container::CrmContainer`] and
//! exposes the same public surface the SDK examples / `DecodeContext` consume
//! (the **full** Canon `uuid` box + the **full** `CRAW` sample entry as the
//! SDK's `kCrmHeaderUuid` / `kCrmHeaderCraw` headers, plus per-frame sample
//! reads).

use std::io::{Read, Seek};

use crm_container::{CrmContainer, CrmContainerError};

use super::error::*;

/// A parsed CRM movie file — SDK-lane glue over [`crm_container::CrmContainer`].
pub struct CrmMovie<R> {
    inner: CrmContainer<R>,
}

impl<R: Read + Seek> CrmMovie<R> {
    /// Load and parse a CRM movie from a reader.
    ///
    /// The SDK lane requires a **per-video-frame** timed-metadata track: its
    /// consumer (`oxivideo::decoder::crm`) reads `timed_meta[frame_no]` for every
    /// video frame and feeds it to `CrmGetImage` as the per-frame colour
    /// development metadata. So this validates — beyond the shared
    /// [`CrmContainer`] checks — that the Canon `uuid` box and `CRAW` entry are
    /// present, the video track is non-empty, the timed-metadata track is
    /// non-empty, and its sample count matches the video frame count. (The
    /// oxiraw lane uses [`CrmContainer`] directly and is best-effort about
    /// timed-metadata — it only reads frame 0's as-shot WB — so these stricter
    /// invariants live here in the SDK glue, not in the shared container.)
    pub fn load(reader: R) -> Result<Self> {
        let inner = CrmContainer::load(reader).map_err(map_err)?;
        // Validate the SDK lane's required headers are present (the SDK's
        // `CrmGetImage` needs both the Canon `uuid` box and the `CRAW` entry).
        if inner.canon_uuid_box().is_none() {
            return Err(CrmError::MissingData("Canon metadata box"));
        }
        if inner.craw_entry().is_none() {
            return Err(CrmError::MissingData("CRAW entry"));
        }
        if inner.frame_count() == 0 {
            return Err(CrmError::MissingData("video index table"));
        }
        if inner.timed_meta_count() == 0 {
            return Err(CrmError::MissingData("timed metadata index table"));
        }
        if inner.frame_count() != inner.timed_meta_count() {
            return Err(CrmError::InvalidFormat(format!(
                "video and timed meta frame counts don't match: {} vs {}",
                inner.frame_count(),
                inner.timed_meta_count(),
            )));
        }
        Ok(Self { inner })
    }

    /// Number of compressed video frames.
    pub fn frame_count(&self) -> u64 {
        self.inner.frame_count()
    }

    /// Timed-metadata sample count (may differ from [`frame_count`](Self::frame_count)).
    pub fn timed_meta_count(&self) -> u64 {
        self.inner.timed_meta_count()
    }

    /// Size of the compressed video frame at `frame_no`, in bytes.
    pub fn compressed_frame_size(&self, frame_no: u64) -> Result<u32> {
        self.inner.compressed_frame_size(frame_no).map_err(map_err)
    }

    /// Size of the timed-metadata sample at `frame_no`, in bytes.
    pub fn timed_meta_size(&self, frame_no: u64) -> Result<u32> {
        self.inner.timed_meta_size(frame_no).map_err(map_err)
    }

    /// Read a compressed video frame into a fresh `Vec<u8>`. For zero-allocation
    /// per-frame loops prefer [`read_compressed_frame_into`](Self::read_compressed_frame_into).
    pub fn read_compressed_frame(&mut self, frame_no: u64) -> Result<Vec<u8>> {
        self.inner.read_compressed_frame(frame_no).map_err(map_err)
    }

    /// Read timed metadata for a frame into a fresh `Vec<u8>`.
    pub fn read_timed_meta(&mut self, frame_no: u64) -> Result<Vec<u8>> {
        self.inner.read_timed_meta(frame_no).map_err(map_err)
    }

    /// Read the compressed video frame at `frame_no` into the caller-owned `dst`.
    /// Returns the bytes written.
    pub fn read_compressed_frame_into(&mut self, frame_no: u64, dst: &mut [u8]) -> Result<usize> {
        self.inner.read_compressed_frame_into(frame_no, dst).map_err(map_err)
    }

    /// Read the timed-metadata sample at `frame_no` into the caller-owned `dst`.
    pub fn read_timed_meta_into(&mut self, frame_no: u64, dst: &mut [u8]) -> Result<usize> {
        self.inner.read_timed_meta_into(frame_no, dst).map_err(map_err)
    }

    /// The **full** `CRAW` sample-entry bytes — the SDK's `kCrmHeaderCraw` header.
    pub fn craw_entry(&self) -> Option<&[u8]> {
        self.inner.craw_entry()
    }

    /// The **full** Canon `uuid` metadata box bytes — the SDK's `kCrmHeaderUuid`
    /// header.
    pub fn canon_metadata_box(&self) -> Option<&[u8]> {
        self.inner.canon_uuid_box()
    }

    /// Consume the movie and return the underlying reader.
    pub fn into_inner(self) -> R {
        self.inner.into_inner()
    }

    /// Shared reference to the underlying reader.
    pub fn get_ref(&self) -> &R {
        self.inner.get_ref()
    }

    /// Mutable reference to the underlying reader.
    pub fn get_mut(&mut self) -> &mut R {
        self.inner.get_mut()
    }
}

/// Map a [`crm_container::CrmContainerError`] into the crate's [`CrmError`].
fn map_err(e: CrmContainerError) -> CrmError {
    match e {
        CrmContainerError::Io(io) => CrmError::Io(io),
        CrmContainerError::MissingBox(b) => CrmError::MissingData(b),
        CrmContainerError::Parse(m) => CrmError::InvalidFormat(m),
        CrmContainerError::FrameOutOfRange { index, count } => {
            CrmError::FrameOutOfRange { index, count }
        }
        CrmContainerError::BufferTooSmall { needed, got } => {
            CrmError::BufferTooSmall { needed, got }
        }
    }
}
