// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

use thiserror::*;

/// Errors that can occur during CRM parsing
#[derive(Error, Debug)]
pub enum CrmError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid parameter: {0}")]
    InvalidParameter(&'static str),

    #[error("Invalid file format: {0}")]
    InvalidFormat(String),

    #[error("Invalid box length: expected {expected}, got {actual}")]
    InvalidLength { expected: u32, actual: u32 },

    #[error("Unsupported brand: {0:#010X}")]
    UnsupportedBrand(u32),

    #[error("Frame index out of range: {index} >= {count}")]
    FrameOutOfRange { index: u64, count: u64 },

    #[error("Missing required data: {0}")]
    MissingData(&'static str),

    #[error("SDK error: {0}")]
    SdkError(#[from] SdkError),

    #[error("Buffer too small: need {needed}, got {got}")]
    BufferTooSmall { needed: usize, got: usize },

    #[error("Libloading error: {0}")]
    Libloading(#[from] libloading::Error),

    #[error("Error: {0}")]
    Other(String),
}

/// The raw `CrmError` code every SDK entry point returns. `CRM_ERR_OK` is
/// [`CRM_ERR_OK`]; anything else converts with [`SdkError::from_code`].
///
/// Entry points are declared as returning this integer rather than an enum:
/// a C enum is just an `int`, and an SDK build that returns a code `CrmError.h`
/// does not list would otherwise be undefined behaviour on the Rust side.
pub type CrmErrorCode = u32;

/// `CRM_ERR_OK` — the one [`CrmErrorCode`] that is not an error.
pub const CRM_ERR_OK: CrmErrorCode = 0;

macro_rules! sdk_errors {
    ($($(#[$doc:meta])* $name:ident = $code:literal,)*) => {
        /// A failure the SDK reported, from `CrmError.h`.
        ///
        /// Codes the header (CRMSDK 2.10) does not define arrive as
        /// [`Unrecognized`](SdkError::Unrecognized) with the raw value, so a
        /// newer SDK can never produce a value this type cannot hold.
        #[non_exhaustive]
        #[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
        pub enum SdkError {
            $($(#[$doc])* $name,)*
            /// A code `CrmError.h` does not define.
            Unrecognized(CrmErrorCode),
        }

        impl SdkError {
            /// The error a raw [`CrmErrorCode`] stands for, or `None` for
            /// [`CRM_ERR_OK`].
            pub const fn from_code(code: CrmErrorCode) -> Option<Self> {
                match code {
                    CRM_ERR_OK => None,
                    $($code => Some(Self::$name),)*
                    other => Some(Self::Unrecognized(other)),
                }
            }

            /// The raw [`CrmErrorCode`].
            pub const fn code(self) -> CrmErrorCode {
                match self {
                    $(Self::$name => $code,)*
                    Self::Unrecognized(code) => code,
                }
            }
        }
    };
}

sdk_errors! {
    /// `CRM_ERR_UNIMPLEMENTED` — not implemented.
    Unimplemented = 0x8000_0001,
    /// `CRM_ERR_INTERNAL_ERROR` — internal error.
    InternalError = 0x8000_0002,
    /// `CRM_ERR_MEM_ALLOC_FAILED` — memory allocation error.
    MemAllocFailed = 0x8000_0003,
    /// `CRM_ERR_MEM_FREE_FAILED` — memory release error.
    MemFreeFailed = 0x8000_0004,
    /// `CRM_ERR_OPERATION_CANCELLED` — operation cancelled.
    OperationCancelled = 0x8000_0005,
    /// `CRM_ERR_INCOMPATIBLE_VERSION` — version error.
    IncompatibleVersion = 0x8000_0006,
    /// `CRM_ERR_NOT_SUPPORTED` — not supported.
    NotSupported = 0x8000_0007,

    /// `CRM_ERR_INVALID_PARAMETER` — invalid function parameter.
    InvalidParameter = 0x8000_0021,
    /// `CRM_ERR_INVALID_HANDLE` — handle error.
    InvalidHandle = 0x8000_0022,
    /// `CRM_ERR_INVALID_POINTER` — pointer error.
    InvalidPointer = 0x8000_0023,
    /// `CRM_ERR_INVALID_INDEX` — index error.
    InvalidIndex = 0x8000_0024,
    /// `CRM_ERR_INVALID_LENGTH` — length error.
    InvalidLength = 0x8000_0025,

    /// `CRM_ERR_DEVICE_NOT_FOUND` — device not found.
    DeviceNotFound = 0x8000_0031,
    /// `CRM_ERR_DEVICE_INVALID` — device error.
    DeviceInvalid = 0x8000_0032,
    /// `CRM_ERR_DEVICE_NOT_SUPPORTED` — device not supported.
    DeviceNotSupported = 0x8000_0033,

    /// `CRM_ERR_DEVELOP_NOT_COMPLETED` — development not completed.
    DevelopNotCompleted = 0x8000_0041,
    /// `CRM_ERR_DEVELOP_EMPTY_QUEUE` — empty queue.
    DevelopEmptyQueue = 0x8000_0042,
    /// `CRM_ERR_DECOMPRESS_NO_RESOURCE` — no resource for decompressing.
    DecompressNoResource = 0x8000_0043,

    /// `CRM_ERR_UNKNOWN` — the SDK's own "unknown error".
    Unknown = 0x8000_0051,
    /// `CRM_ERR_ENUM_NA` — enumeration terminated.
    EnumNa = 0x8000_0052,
    /// `CRM_ERR_INVALID_FN_CALL` — called in a mode where the function cannot be used.
    InvalidFnCall = 0x8000_0053,
    /// `CRM_ERR_INVALID_ID` — invalid ID.
    InvalidId = 0x8000_0054,
    /// `CRM_ERR_WAIT_TIMEOUT_ERROR` — timeout.
    WaitTimeoutError = 0x8000_0055,
}

impl std::fmt::Display for SdkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unrecognized(code) => write!(f, "unrecognized SDK error (0x{code:08X})"),
            known => write!(f, "{known:?} (0x{:08X})", known.code()),
        }
    }
}

impl std::error::Error for SdkError {}

pub type Result<T> = std::result::Result<T, CrmError>;
