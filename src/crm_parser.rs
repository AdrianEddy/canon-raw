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
    /// [`CrmContainer`] checks — that the Canon `uuid` box is present, the video
    /// track is non-empty, the timed-metadata track is non-empty, and its sample
    /// count matches the video frame count. (The oxiraw lane uses
    /// [`CrmContainer`] directly and is best-effort about timed-metadata — it
    /// only reads frame 0's as-shot WB — so these stricter invariants live here
    /// in the SDK glue, not in the shared container.) The `CRAW` entry needs no
    /// gate here: [`CrmContainer::load`] already declines without it.
    ///
    /// **Every one of those gates raises [`CrmError::MissingData`]** — the
    /// crate's DECLINE shape (see this module's `map_err`), never
    /// [`CrmError::InvalidFormat`].
    /// They all say "this lane's requirements are not met by this file", which is
    /// a statement about the LANE; only the shared container parser gets to say
    /// the file is damaged.
    pub fn load(reader: R) -> Result<Self> {
        let inner = CrmContainer::load(reader).map_err(map_err)?;
        // Validate the SDK lane's required headers are present (the SDK's
        // `CrmGetImage` needs the Canon `uuid` box; the `CRAW` entry is a
        // container-load invariant).
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
            // DECLINE, not damage. A timed-metadata track that is not
            // index-parallel with the video track breaks an SDK-LANE requirement
            // (`timed_meta[frame_no]` per video frame) that the shared container
            // and the pure-Rust oxiraw lane do without — so the honest verdict is
            // "this lane cannot serve this file", exactly like the three gates
            // above it.
            //
            // This gate used to raise `InvalidFormat`, which
            // `oxivideo::decoder::crm::container_error` classifies
            // `CrmFault::Integrity` — on a `crx `-branded carrier a FATAL
            // damaged-container verdict. A perfectly readable clip whose
            // timed-metadata count was one off was therefore reported to the user
            // as a CORRUPT FILE, and the `Fatal` class also stopped the
            // dispatcher from offering it to the oxiraw lane that would have
            // played it (`decoder::dispatch::build_crm` falls through on
            // `Configuration` only).
            //
            // The counts are not in the message: `MissingData` carries a
            // `&'static str`, and widening it would ripple through every
            // exhaustive `CrmError` match in the workspace for a diagnostic that
            // is a DECLINE — normally swallowed by the fall-through — and that
            // the caller can read straight off `frame_count()` /
            // `timed_meta_count()`.
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
/// # The one line that matters: which variant means DAMAGE
///
/// `oxivideo::decoder::crm::container_error` reads this crate's error enum as a
/// two-class verdict — [`CrmError::MissingData`] is a **decline**
/// (`CrmFault::Unsupported` → `Configuration`, the class the dispatcher falls
/// through to the pure-Rust `crm-oxiraw` lane on), and
/// [`CrmError::InvalidFormat`] / [`CrmError::InvalidLength`] are **file damage**
/// (`CrmFault::Integrity`, which on a `crx `-branded carrier is a FATAL
/// "your clip is corrupt" report to the user).
///
/// So `InvalidFormat` is reserved for [`CrmContainerError::Parse`] — a box tree
/// or sample table that genuinely does not resolve. Nothing that is merely "a
/// shape this lane does not serve" may travel on it; [`CrmMovie::load`]'s own
/// header gates all raise `MissingData` for exactly that reason.
fn map_err(e: CrmContainerError) -> CrmError {
    match e {
        CrmContainerError::Io(io) => CrmError::Io(io),
        // "A well-formed container this parser does not decode" and "a required
        // box is absent" both land on `MissingData`: `CrmError` has no third
        // shape.
        //
        // For `Unsupported` that is exact — it is the same verdict
        // `crm_audio::classify_crm_container` reaches (`CrmFault::Unsupported`).
        //
        // For `MissingBox` it is deliberately CONSERVATIVE: the shared
        // classifier calls a missing standard ISOBMFF box `Integrity`, and
        // collapsing it here costs the SDK lane that damage signal. That is the
        // safe direction — the SDK lane declines, the dispatcher hands the
        // carrier to the oxiraw lane, and THAT lane's `CrmContainer::load` hits
        // the same missing box and raises the `Integrity`/damaged verdict
        // through the shared classifier. The alternative (a false `Fatal` from
        // the first lane) would libel an intact clip and terminate the open.
        CrmContainerError::Unsupported(w) => CrmError::MissingData(w),
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
