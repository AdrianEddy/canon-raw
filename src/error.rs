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

    #[error("SDK error: {0:?}")]
    SdkError((SdkError, String)),

    #[error("Buffer too small: need {needed}, got {got}")]
    BufferTooSmall { needed: usize, got: usize },

    #[error("Libloading error: {0}")]
    Libloading(#[from] libloading::Error),

    #[error("Error: {0}")]
    Other(String),
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SdkError {
    Ok = 0x0000_0000,

    Unimplemented = 0x8000_0001,
    InternalError = 0x8000_0002,
    MemAllocFailed = 0x8000_0003,
    MemFreeFailed = 0x8000_0004,
    OperationCancelled = 0x8000_0005,
    IncompatibleVersion = 0x8000_0006,
    NotSupported = 0x8000_0007,

    InvalidParameter = 0x8000_0021,
    InvalidHandle = 0x8000_0022,
    InvalidPointer = 0x8000_0023,
    InvalidIndex = 0x8000_0024,
    InvalidLength = 0x8000_0025,

    DeviceNotFound = 0x8000_0031,
    DeviceInvalid = 0x8000_0032,
    DeviceNotSupported = 0x8000_0033,

    DevelopNotCompleted = 0x8000_0041,
    DevelopEmptyQueue = 0x8000_0042,
    DecompressNoResource = 0x8000_0043,

    Unknown = 0x8000_0051,
    EnumNa = 0x8000_0052,
    InvalidFnCall = 0x8000_0053,
    InvalidId = 0x8000_0054,
    WaitTimeoutError = 0x8000_0055,
}

pub type Result<T> = std::result::Result<T, CrmError>;
