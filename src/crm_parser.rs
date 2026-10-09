// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

//! `CrmMovie` — the CRM container view the SDK decode path consumes.
//!
//! The ISOBMFF box walk, sample tables and Canon-private box extraction live in
//! the SDK-free [`crm_container`] crate, which pure-Rust CRM decoders can share
//! without pulling in this crate. `CrmMovie` wraps a
//! [`crm_container::CrmContainer`] and adds what the Canon SDK needs on top: the
//! **full** Canon `uuid` box and the **full** `CRAW` sample entry as the SDK's
//! `kCrmHeaderUuid` / `kCrmHeaderCraw` headers, plus per-frame sample reads.

use std::io::{Read, Seek};

use crm_container::{CrmContainer, CrmContainerError};

use super::error::*;

/// A parsed CRM movie file — SDK-facing glue over [`crm_container::CrmContainer`].
#[derive(Debug)]
pub struct CrmMovie<R> {
    inner: CrmContainer<R>,
}

impl<R: Read + Seek> CrmMovie<R> {
    /// Load and parse a CRM movie from a reader.
    ///
    /// The SDK decode path requires a **per-video-frame** timed-metadata track:
    /// `CrmGetImage` is handed `timed_meta[frame_no]` for every video frame as
    /// that frame's colour-development metadata. So this validates — beyond the
    /// [`CrmContainer`] checks — that the Canon `uuid` box is present, the video
    /// track is non-empty, the timed-metadata track is non-empty, and its sample
    /// count matches the video frame count. [`CrmContainer`] itself does not
    /// require any of that, because a decoder that does its own colour science
    /// may only need frame 0's metadata, or none. The `CRAW` entry needs no gate
    /// here: [`CrmContainer::load`] already declines without it.
    ///
    /// # Errors
    ///
    /// **Every one of those gates raises [`CrmError::MissingData`]**, never
    /// [`CrmError::InvalidFormat`]. They say "this file does not meet the SDK
    /// path's requirements", which is a statement about the decoder, not
    /// evidence that the file is damaged. Container errors map the same way:
    /// `MissingData` is a decline (fall through to another decoder), while
    /// `InvalidFormat` is reserved for a box tree or sample table that does not
    /// resolve — genuine damage.
    pub fn load(reader: R) -> Result<Self> {
        let inner = CrmContainer::load(reader).map_err(map_err)?;
        // The SDK's `CrmGetImage` needs the Canon `uuid` box; the `CRAW` entry
        // is a container-load invariant.
        if inner.canon_uuid_box().is_none() {
            return Err(CrmError::MissingData("Canon metadata box"));
        }
        if inner.frame_count() == 0 {
            return Err(CrmError::MissingData("video index table"));
        }
        if inner.timed_meta_count() == 0 {
            return Err(CrmError::MissingData("timed metadata index table"));
        }
        if inner.frame_count() != inner.timed_meta_count() {
            // A decline, not damage: a timed-metadata track that is not
            // index-parallel with the video track breaks a requirement of the
            // SDK path (`timed_meta[frame_no]` per video frame), not of the
            // container format. Reporting it as `InvalidFormat` would tell a
            // caller that a perfectly readable clip is corrupt, and stop it from
            // trying a decoder that has no such requirement.
            //
            // The counts are not in the message: `MissingData` carries a
            // `&'static str`, and the caller can read them straight off
            // `frame_count()` / `timed_meta_count()`.
            return Err(CrmError::MissingData(
                "per-video-frame timed metadata (its sample count differs from the video frame count)",
            ));
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
    ///
    /// Infallible: [`CrmContainer::load`] cannot get past `CMP1` without the
    /// entry, so a loaded movie always carries it.
    pub fn craw_entry(&self) -> &[u8] {
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
///
/// # Which variant means damage
///
/// Callers that try several decoders in turn need to tell "this decoder cannot
/// serve this file" apart from "this file is broken", so the two travel on
/// different variants:
///
/// * [`CrmError::MissingData`] is a **decline** — the file may be perfectly
///   intact; something this decode path needs is not there. A caller should
///   fall through to its next backend.
/// * [`CrmError::InvalidFormat`] / [`CrmError::InvalidLength`] are **damage** —
///   a box tree or sample table that genuinely does not resolve.
///
/// So `InvalidFormat` is reserved for [`CrmContainerError::Parse`]. Nothing that
/// is merely "a shape this decoder does not serve" may travel on it;
/// [`CrmMovie::load`]'s own header gates all raise `MissingData` for exactly
/// that reason.
fn map_err(e: CrmContainerError) -> CrmError {
    match e {
        CrmContainerError::Io(io) => CrmError::Io(io),
        // "A well-formed container this parser does not decode" and "a required
        // box is absent" both land on `MissingData`: `CrmError` has no third
        // shape.
        //
        // For `Unsupported` that is exact.
        //
        // For `MissingBox` it is deliberately CONSERVATIVE: a missing standard
        // ISOBMFF box is damage as far as `crm-container` is concerned, and
        // collapsing it to a decline here loses that signal. That is the safe
        // direction — a caller that falls through to another CRM decoder will
        // hit the same missing box there and get the damage verdict from it,
        // whereas a false damage report from this one would condemn an intact
        // clip.
        CrmContainerError::Unsupported(w) => CrmError::MissingData(w),
        CrmContainerError::MissingBox(b) => CrmError::MissingData(b),
        CrmContainerError::Parse(m) => CrmError::InvalidFormat(m),
        CrmContainerError::FrameOutOfRange { index, count } => CrmError::FrameOutOfRange { index, count },
        CrmContainerError::BufferTooSmall { needed, got } => CrmError::BufferTooSmall { needed, got },
    }
}
