// SPDX-License-Identifier: MIT OR Apache-2.0

//! `crm-container` error type. Hand-rolled (no `thiserror`) so the crate keeps
//! `mp4-atom` as its only dependency.

use std::fmt;

/// Errors from parsing a CRM container or reading its samples.
///
/// Three of the variants answer three DIFFERENT questions, and a consumer that
/// tries several decoders in turn classifies on that difference — "not mine,
/// try the next backend" versus "this file is broken":
///
/// * [`Unsupported`](Self::Unsupported) — *"this is a well-formed container this
///   parser does not decode"*. The box tree read fine; it simply is not a Canon
///   Cinema RAW **movie** of the shape this crate handles (no `CRAW` sample
///   entry, no `CMP1`, no video track, an empty video sample table). NOT damage:
///   a consumer must keep falling through to other backends.
/// * [`MissingBox`](Self::MissingBox) — a **standard ISOBMFF** structure that any
///   movie must carry is absent (`moov`, `stsz`, `stsc`, `stco`/`co64`). On a
///   positively identified `.CRM` that is a broken file.
/// * [`Parse`](Self::Parse) — a box that IS present decoded to garbage. Also a
///   broken file.
#[derive(Debug)]
pub enum CrmContainerError {
    /// An I/O error from the underlying reader.
    Io(std::io::Error),
    /// The container parsed, but is not a CRM **variant** this crate decodes —
    /// see the type-level note. Never evidence that the file is damaged.
    Unsupported(&'static str),
    /// A required **standard** box / structure was absent.
    MissingBox(&'static str),
    /// A box decoded but was structurally invalid.
    Parse(String),
    /// A requested frame index was past the end of the sample table.
    FrameOutOfRange { index: u64, count: u64 },
    /// A caller-owned destination buffer was too small for the sample.
    BufferTooSmall { needed: usize, got: usize },
}

impl fmt::Display for CrmContainerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CrmContainerError::Io(e) => write!(f, "I/O error: {e}"),
            CrmContainerError::Unsupported(w) => write!(f, "not a supported Canon Cinema RAW movie: {w}"),
            CrmContainerError::MissingBox(b) => write!(f, "missing required box: {b}"),
            CrmContainerError::Parse(m) => write!(f, "invalid CRM container: {m}"),
            CrmContainerError::FrameOutOfRange { index, count } => {
                write!(f, "frame index out of range: {index} >= {count}")
            }
            CrmContainerError::BufferTooSmall { needed, got } => {
                write!(f, "buffer too small: need {needed}, got {got}")
            }
        }
    }
}

impl std::error::Error for CrmContainerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CrmContainerError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for CrmContainerError {
    fn from(e: std::io::Error) -> Self {
        CrmContainerError::Io(e)
    }
}
