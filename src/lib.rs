// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]

use atomic_waker::AtomicWaker;
use core::ffi::{c_char, c_float, c_int, c_schar, c_short, c_uchar, c_uint, c_ulonglong, c_ushort, c_void};
use std::{
    ffi::CString,
    future::Future,
    marker::PhantomData,
    path::Path,
    pin::Pin,
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};

pub mod crm_parser;
pub mod cuda_exclusion;
pub mod decode_context;
pub mod error;
pub mod image_info;
pub use cuda_exclusion::{CudaExclusionGuard, cuda_exclusion};
pub use decode_context::{DecodeContext, OutputGeometry};
pub use error::*;
pub use image_info::ImageInfo;

// Crate-internal lifecycle test for the private `GetImageAsyncState`
// co-ownership model (needs access to its private fields, so it cannot live in
// `tests/`). Hardware-free — no SDK, no GPU.
#[cfg(test)]
mod get_image_async_tests;

// Crate-internal gates for the private `Path` -> `CrmChar*` encoder, kept in
// their own file rather than inline with the implementation. Hardware-free.
#[cfg(test)]
#[path = "sdk_path_tests.rs"]
mod sdk_path_tests;

// Crate-internal gates for how values the SDK writes back are read: return
// codes, format IDs, GPU info, text fields and image-info widths. Hardware-free.
#[cfg(test)]
mod ffi_value_tests;

#[cfg(not(target_os = "windows"))]
use libloading::os::unix as dl;
#[cfg(target_os = "windows")]
use libloading::os::windows as dl;

/* -------------------------------------------------------------------------------------------------
 * Basic typedefs
 * ------------------------------------------------------------------------------------------------- */

pub type CrmVoid = c_void;
pub type CrmBool = c_int;
pub type CrmChar = c_char;
pub type CrmASCII = c_char;

pub type CrmInt8 = c_schar;
pub type CrmUInt8 = c_uchar;
pub type CrmByte = c_uchar;
pub type CrmInt16 = c_short;
pub type CrmUInt16 = c_ushort;
pub type CrmInt32 = c_int;
pub type CrmUInt32 = c_uint;
pub type CrmInt64 = i64; // ABI: 64-bit on all supported targets
pub type CrmUInt64 = c_ulonglong;
pub type CrmFloat32 = c_float;

/* -------------------------------------------------------------------------------------------------
 * Enums
 * ------------------------------------------------------------------------------------------------- */

pub const MAX_FILE_SIZE: u32 = 28318048;
pub const RMF_4KDCI_FILE_SIZE: u32 = 1201840;
pub const RMF_QFHD_FILE_SIZE: u32 = 1127497;
pub const RMF_HALF_4KDCI_FILE_SIZE: u32 = 6047456;
pub const RMF_HALF_QFHD_FILE_SIZE: u32 = 5675404;
pub const SURFACE_NUM: u32 = 6;

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum WbSetting {
    Record = 0x0000_0001,
    ColorTemperature = 0x0000_0002,
    ClickWb = 0x0000_0003,
    DayLight = 0x0000_0004,
    Tungsten = 0x0000_0005,
    Gain = 0x0000_0010,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SampleSequence {
    Rgb = 0x0000_0001,
    Bgr = 0x0000_0002,
    DpxRgb10Bit = 0x0000_0011,
    DpxRgb16Bit = 0x0000_0012,
    Argb16Bit = 0x0000_0013,
    AyCbCr16Bit = 0x0000_0014,
    DpxRgb10BitBe = 0x0000_0015,
    DpxRgb12Bit = 0x0000_0016,
    Rgba16BitLe = 0x0000_0017,
    OpenExrRgb16Bit = 0x0000_0021,
    Type1Rgb10Bit = 0x0000_0031,
    HalfRgb16Bit = 0x0000_0041,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Gamma {
    Linear = 0x0000_0000,
    CanonLog = 0x0000_0001,
    Rec709 = 0x0000_0002,
    WideDr = 0x0000_0003,
    Dci = 0x0000_0004,
    CanonLog2 = 0x0000_0005,
    CanonLog3 = 0x0000_0006,
    Cmt709 = 0x0000_0041,
    Canon709 = 0x0000_0042,
    CmtPq = 0x0000_0052,
    CmtHlg = 0x0000_0053,
    CmtDci = 0x0000_0054,
    WideDr3d = 0x0001_0003,
    Dci3d = 0x0001_0004,
    Bt2100Pq = 0x0002_0002,
    Bt2100Hlg = 0x0002_0003,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DebayerSetting {
    HighQuality = 0x0000_0001,
    HighSpeed = 0x0000_0002,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Downscale {
    X1 = 0x0000_0001,
    X2 = 0x0000_0002,
    X4 = 0x0000_0003,
    Hd = 0x0000_0010,
    Sd = 0x0000_0011,
    Qfhd = 0x0000_0012,
    K4Dci = 0x0000_0020,
    K2Dci = 0x0000_0021,
    Bayer2 = 0x0000_0102,
    Bayer4 = 0x0000_0103,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Device {
    Cpu = 0x0000_0001,
    Cuda = 0x0000_0002,
    OpenCl = 0x0000_0004,
    Intel = 0x0000_0008,
    Metal = 0x0000_0010,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Gpu {
    NotUse = 0x0000_0000,
    Gpu1 = 0x0000_0001,
    Gpu2 = 0x0000_0002,
    Gpu3 = 0x0000_0004,
    Gpu4 = 0x0000_0008,
    Gpu5 = 0x0000_0010,
    Gpu6 = 0x0000_0020,
    Gpu7 = 0x0000_0040,
    Gpu8 = 0x0000_0080,
}

/// `CrmFormatId` — the format of a clip, as [`CrmSdkLibrary::get_format_id`]
/// reports it.
///
/// A newtype over the raw value rather than an enum: the SDK hands this out,
/// and it goes straight back in — to [`CrmSdkLibrary::create_process_ref`] and
/// inside [`DevParamV13`] / [`ResParamV13`] — so every value the SDK can
/// produce must be representable, including ones a newer SDK adds.
#[repr(transparent)]
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct FormatId(CrmUInt32);

impl FormatId {
    /// `kCrmFormatId_MovieType1`.
    pub const MOVIE_TYPE_1: Self = Self(0x0001_0001);
    /// `kCrmFormatId_MovieType2` — what the CRM clips of current cameras report.
    pub const MOVIE_TYPE_2: Self = Self(0x0001_0002);

    /// Wrap a raw `CrmFormatId` value.
    pub const fn from_raw(raw: CrmUInt32) -> Self {
        Self(raw)
    }

    /// The raw `CrmFormatId` value.
    pub const fn raw(self) -> CrmUInt32 {
        self.0
    }
}

impl std::fmt::Debug for FormatId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::MOVIE_TYPE_1 => f.write_str("FormatId::MOVIE_TYPE_1"),
            Self::MOVIE_TYPE_2 => f.write_str("FormatId::MOVIE_TYPE_2"),
            Self(raw) => write!(f, "FormatId({raw:#010x})"),
        }
    }
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum StructId {
    DevParamV10 = 0x0001_0001,
    DevParamV11 = 0x0002_0001,
    DevParamV12 = 0x0003_0001,
    DevParamV13 = 0x0004_0001,
    ResParamV10 = 0x0001_0002,
    ResParamV11 = 0x0002_0002,
    ResParamV12 = 0x0003_0002,
    ResParamV13 = 0x0004_0002,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ImageInfoId {
    CaptureWidth = 0x0000_0001,
    CaptureHeight = 0x0000_0002,
    BitDepth = 0x0000_0003,
    TotalImageSize = 0x0000_0004,
    OffsetToImageData = 0x0000_0005,
    RawFrameWidth = 0x0000_0006,
    RawFrameHeight = 0x0000_0007,

    Position = 0x0001_0001,
    Timecode = 0x0001_0002,
    PlaybackFrameRate = 0x0001_0003,
    PlaybackSystemFlag = 0x0001_0004,
    CaptureFrameRate = 0x0001_0005,
    CaptureSystemFlag = 0x0001_0006,
    PixelAspectRatio = 0x0001_0007,

    CreationDateTime = 0x0002_0001,
    ShutterAngle = 0x0002_0003,
    UserBits = 0x0002_0004,

    DeviceModelName = 0x0003_0001,
    DeviceFirmwareVersion = 0x0003_0002,
    DeviceSerialNumber = 0x0003_0003,
    LensName = 0x0003_0004,
    LensSqueezeFactor = 0x0003_0005,
    LensSerialNumber = 0x0003_0006,

    RelatedClipName = 0x0004_0001,
    RelatedClipId = 0x0004_0002,
    RelatedFrameId = 0x0004_0003,
    ShootingGamutInformation = 0x0004_0004,
    ShootingGammaType = 0x0004_0005,
    ShootingColorMatrix = 0x0004_0006,

    FocalLength = 0x0005_0001,
    FocusPosition = 0x0005_0002,
    FNumber = 0x0005_0003,
    NdFilter = 0x0005_0004,
    TNumber = 0x0005_0005,

    ShutterSpeedMode = 0x0006_0001,
    ShutterSpeed = 0x0006_0002,
    WbMode = 0x0006_0003,
    WbPreset = 0x0006_0004,
    ColorTemperature = 0x0006_0005,
    ColorTemperatureStatus = 0x0006_0006,
    IsoSpeed = 0x0006_0007,
    ImageStabilizer = 0x0006_0008,
    PeripheralIlluminationCorrection = 0x0006_0009,
    GamutInformation = 0x0006_0010,
    GammaType = 0x0006_0011,
    Gain = 0x0006_0012,
    ColorCompensation = 0x0006_0013,
    WbGainR = 0x0006_0014,
    WbGainB = 0x0006_0015,
    WbGainDenominator = 0x0006_0016,
    ColorMatrix = 0x0006_0017,
    ChromaticAberrationCorrection = 0x0006_0018,
    DistortionCorrection = 0x0006_0019,
    FocusBreathingCorrection = 0x0006_0020,

    VfxTilt = 0x0007_0001,
    VfxRoll = 0x0007_0002,

    IsoAdjustAvailable = 0x0008_0001,
    ActualIsoSpeed = 0x0008_0002,

    RawType = 0x0009_0001,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AcesIdt {
    Auto = 0x0000_0000,
    D55 = 0x0000_0001,
    Tng = 0x0000_0002,
}

/// What [`CrmSdkLibrary::get_aces_idt`] reports: the IDT the last develop
/// applied.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AcesIdtInfo {
    /// The raw `CrmAcesIDT` value; [`idt`](Self::idt) decodes it.
    pub raw: CrmUInt32,
    /// The SDK's name for it, e.g. `"D55 Ver. 2.0"`.
    pub name: String,
}

impl AcesIdtInfo {
    /// The IDT, or `None` for a value `CrmSdkTypes.h` does not define.
    pub fn idt(&self) -> Option<AcesIdt> {
        AcesIdt::try_from(self.raw).ok()
    }
}

impl TryFrom<CrmUInt32> for AcesIdt {
    /// The raw value, when it is not one `CrmSdkTypes.h` defines.
    type Error = CrmUInt32;

    fn try_from(raw: CrmUInt32) -> std::result::Result<Self, CrmUInt32> {
        match raw {
            0 => Ok(Self::Auto),
            1 => Ok(Self::D55),
            2 => Ok(Self::Tng),
            other => Err(other),
        }
    }
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ColorSpace {
    Bt709 = 0x0000_0000,
    DciP3Plus = 0x0000_0001,
    CinemaGamut = 0x0000_0002,
    Bt2020 = 0x0000_0003,
    Aces = 0x0000_0011,
    DciP3 = 0x0001_0001,
    Original = 0x9999_9999,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum IsoSetting {
    Record = 0x0000_0001,
    Custom = 0x0000_0002,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MemType {
    SystemMemory = 0x0000_0000,
    D3d9Memory = 0x0000_0001,
    D3d11Memory = 0x0000_0002,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FrameRendering {
    Alternate = 0x0000_0001,
    Split = 0x0000_0002,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DataType {
    Rmf = 0x0000_0001,
    CrmCompressed = 0x0000_0010,
    CrmDecompressed = 0x0000_0011,
    RgbDebayered = 0x0001_0001,
    RgbLutApplied = 0x0002_0001,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BufferType {
    Cpu = 0x0000_0001,
    Cuda = 0x0000_0002,
    OpenCl = 0x0000_0004,
    OpenClImage = 0x0000_0005,
    Metal = 0x0000_0010,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum LensSqueeze {
    Off = 0x0000_0000,
    X13 = 0x0000_0001,
    X20 = 0x0000_0002,
}

bitflags::bitflags! {
    #[repr(transparent)]
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    pub struct ProcessType: u32 {
        const DEVELOP                = 0x0000_0001;
        const DECOMPRESS             = 0x0000_0002;
        const DEVELOP_EXT_GPU_MEM_USE = 0x0000_0004;
    }
}

#[repr(u32)]
#[derive(Default, Copy, Clone, Debug, Eq, PartialEq)]
pub enum HeaderId {
    #[default]
    Uuid = 0x0000_0001,
    Craw = 0x0000_0002,
    TimedMetadata = 0x0001_0001,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DecodeLevel {
    Full = 0x0000_0001,
    Half = 0x0000_0002,
    Quarter = 0x0000_0004,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[allow(non_camel_case_types)] // `K4_5` / `K5_9` mirror the SDK's 4.5K / 5.9K names
pub enum Resolution {
    K4 = 0x0000_0001,
    K4_5 = 0x0000_0002,
    K5_9 = 0x0000_0003,
    K8 = 0x0000_0004,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ClSupportType {
    Full = 0x0000_0001,
    Limited = 0x0000_0002,
}

impl TryFrom<CrmUInt32> for ClSupportType {
    /// The raw value, when it is not one `CrmSdkTypes.h` defines.
    type Error = CrmUInt32;

    fn try_from(raw: CrmUInt32) -> std::result::Result<Self, CrmUInt32> {
        match raw {
            1 => Ok(Self::Full),
            2 => Ok(Self::Limited),
            other => Err(other),
        }
    }
}

/* -------------------------------------------------------------------------------------------------
 * Structs
 * ------------------------------------------------------------------------------------------------- */

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Version {
    pub major: CrmUInt8,
    pub minor: CrmUInt8,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Rational16 {
    pub numerator: CrmUInt16,
    pub denominator: CrmUInt16,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct Umid32 {
    pub basic_umid: [CrmUInt8; 32],
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct Umid64 {
    pub extended_umid: [CrmUInt8; 64],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Point {
    pub x: CrmUInt32,
    pub y: CrmUInt32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Size {
    pub width: CrmUInt32,
    pub height: CrmUInt32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: CrmUInt32,
    pub y: CrmUInt32,
    pub width: CrmUInt32,
    pub height: CrmUInt32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct WbGain {
    pub r: CrmInt32,  // C: int
    pub g1: CrmInt32, // C: int
    pub g2: CrmInt32, // C: int
    pub b: CrmInt32,  // C: int
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct DevParamV10 {
    pub size: CrmUInt32,
    pub struct_id: StructId,
    pub format_id: FormatId,
    pub wb_setting: WbSetting,
    pub click_wb_point: Point,
    pub tint: CrmFloat32,
    pub exposure: CrmFloat32,
    pub cct: CrmUInt32,
    pub sharpness: CrmUInt32,
    pub noise: CrmUInt32,
    pub gamma: Gamma,
    pub wb_gain: WbGain,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct DevParamV11 {
    pub size: CrmUInt32,
    pub struct_id: StructId,
    pub format_id: FormatId,
    pub wb_setting: WbSetting,
    pub click_wb_point: Point,
    pub tint: CrmFloat32,
    pub exposure: CrmFloat32,
    pub cct: CrmUInt32,
    pub sharpness: CrmUInt32,
    pub noise: CrmUInt32,
    pub gamma: Gamma,
    pub wb_gain: WbGain,
    pub aces_idt: AcesIdt,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct DevParamV12 {
    pub size: CrmUInt32,
    pub struct_id: StructId,
    pub format_id: FormatId,
    pub wb_setting: WbSetting,
    pub click_wb_point: Point,
    pub tint: CrmFloat32,
    pub exposure: CrmFloat32,
    pub cct: CrmUInt32,
    pub sharpness: CrmUInt32,
    pub noise: CrmUInt32,
    pub gamma: Gamma,
    pub wb_gain: WbGain,
    pub aces_idt: AcesIdt,
    pub color_space: ColorSpace,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct DevParamV13 {
    pub size: CrmUInt32,
    pub struct_id: StructId,
    pub format_id: FormatId,
    pub wb_setting: WbSetting,
    pub click_wb_point: Point,
    pub tint: CrmFloat32,
    pub exposure: CrmFloat32,
    pub cct: CrmUInt32,
    pub sharpness: CrmUInt32,
    pub noise: CrmUInt32,
    pub gamma: Gamma,
    pub wb_gain: WbGain,
    pub aces_idt: AcesIdt,
    pub color_space: ColorSpace,
    pub iso_setting: IsoSetting,
    pub iso_speed: CrmUInt32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct ResParamV10 {
    pub size: CrmUInt32,
    pub struct_id: StructId,
    pub format_id: FormatId,
    pub debayer_setting: DebayerSetting,
    pub downscale: Downscale,
    pub crop: Rect,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct ResParamV11 {
    pub size: CrmUInt32,
    pub struct_id: StructId,
    pub format_id: FormatId,
    pub debayer_setting: DebayerSetting,
    pub downscale: Downscale,
    pub crop: Rect,
    pub frame_rendering: FrameRendering,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct ResParamV12 {
    pub size: CrmUInt32,
    pub struct_id: StructId,
    pub format_id: FormatId,
    pub debayer_setting: DebayerSetting,
    pub downscale: Downscale,
    pub crop: Rect,
    pub frame_rendering: FrameRendering,
    pub lens_squeeze: LensSqueeze,
    pub in_data_type: DataType,
    pub in_buffer_type: BufferType,
    pub out_data_type: DataType,
    pub out_buffer_type: BufferType,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct ResParamV13 {
    pub size: CrmUInt32,
    pub struct_id: StructId,
    pub format_id: FormatId,
    pub debayer_setting: DebayerSetting,
    pub downscale: Downscale,
    pub crop: Rect,
    pub frame_rendering: FrameRendering,
    pub lens_squeeze: LensSqueeze,
    pub in_data_type: DataType,
    pub in_buffer_type: BufferType,
    pub out_data_type: DataType,
    pub out_buffer_type: BufferType,
    pub decode_level: DecodeLevel,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct ClObject {
    pub platform_id: *mut CrmVoid,
    pub device_ids: *mut *mut CrmVoid,
    pub device_num: CrmUInt32,
    pub context: *mut CrmVoid,
    pub command_queue: *mut *mut CrmVoid,
}
pub type ClObjectRef = *mut ClObject;

#[cfg(not(target_vendor = "apple"))]
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CudaObject {
    pub device_num: CrmUInt32,
    pub devices: *mut CrmUInt32,
    pub compute_stream: *mut *mut CrmVoid,
}
#[cfg(not(target_vendor = "apple"))]
pub type CudaObjectRef = *mut CudaObject;

#[cfg(target_vendor = "apple")]
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct MtlObject {
    pub device_ids: *mut CrmVoid,
    pub device_num: CrmUInt32,
    pub command_queue: *mut CrmVoid,
}
#[cfg(target_vendor = "apple")]
pub type MtlObjectRef = *mut MtlObject;

// ─────────────────────────────────────────────────────────────────────
// Owned init wrappers — safe surface over the raw `*Object` structs.
//
// The raw `CudaObject` / `ClObject` / `MtlObject` types are layout-
// compatible with the SDK's C structs; constructing one requires the
// caller to provide raw pointer arrays whose lifetime extends past the
// SDK init call. Manual array-on-stack juggling at the call site is a
// footgun (the arrays MUST outlive the object that points at them, in
// the right move order); the owned wrappers below bind the backing
// `Box<[…]>` to the object so the relationship is type-system-
// enforceable.
// ─────────────────────────────────────────────────────────────────────

/// Owned CUDA init descriptor — handed to
/// [`CrmSdkLibrary::initialize_with_cuda`].
///
/// Holds the backing device-id / stream arrays so the raw pointers
/// inside the underlying `CudaObject` remain valid for the life of
/// this struct.
#[cfg(not(target_vendor = "apple"))]
pub struct CrmCudaInit {
    _devices: Box<[CrmUInt32]>,
    _streams: Box<[*mut CrmVoid]>,
    raw: CudaObject,
}

#[cfg(not(target_vendor = "apple"))]
unsafe impl Send for CrmCudaInit {}

#[cfg(not(target_vendor = "apple"))]
impl CrmCudaInit {
    /// Single-GPU shape — most common.
    pub fn new(device_id: u32, compute_stream: *mut core::ffi::c_void) -> Self {
        Self::with_devices(&[device_id], &[compute_stream])
    }

    /// Multi-GPU shape — pairs each `devices[i]` with `streams[i]`.
    ///
    /// Panics on length mismatch; the FFI layout requires the two
    /// arrays to have the same `device_num` count.
    pub fn with_devices(devices: &[u32], streams: &[*mut core::ffi::c_void]) -> Self {
        assert_eq!(
            devices.len(),
            streams.len(),
            "CrmCudaInit::with_devices: devices and streams must have equal length"
        );
        let devices_box: Box<[CrmUInt32]> = Box::from(devices);
        let streams_box: Box<[*mut CrmVoid]> =
            streams.iter().map(|s| *s as *mut CrmVoid).collect::<Vec<_>>().into_boxed_slice();
        let raw = CudaObject {
            device_num: devices_box.len() as CrmUInt32,
            devices: devices_box.as_ptr() as *mut CrmUInt32,
            compute_stream: streams_box.as_ptr() as *mut *mut CrmVoid,
        };
        Self { _devices: devices_box, _streams: streams_box, raw }
    }

    /// Mutable access to the raw FFI struct. The SDK never mutates the
    /// pointer arrays, only the scalar `device_num` field — but the FFI
    /// signature requires `&mut`.
    pub fn as_raw_mut(&mut self) -> &mut CudaObject {
        &mut self.raw
    }
}

#[cfg(not(target_vendor = "apple"))]
impl std::fmt::Debug for CrmCudaInit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrmCudaInit").field("device_num", &self._devices.len()).finish_non_exhaustive()
    }
}

/// Owned OpenCL init descriptor — handed to
/// [`CrmSdkLibrary::initialize_with_opencl`].
pub struct CrmOpenClInit {
    _devices: Box<[*mut CrmVoid]>,
    _queues: Box<[*mut CrmVoid]>,
    raw: ClObject,
}

unsafe impl Send for CrmOpenClInit {}

impl CrmOpenClInit {
    /// Single-device shape.
    pub fn new(
        platform_id: *mut core::ffi::c_void,
        device_id: *mut core::ffi::c_void,
        context: *mut core::ffi::c_void,
        command_queue: *mut core::ffi::c_void,
    ) -> Self {
        Self::with_devices(platform_id, context, &[device_id], &[command_queue])
    }

    /// Multi-device shape.
    pub fn with_devices(
        platform_id: *mut core::ffi::c_void,
        context: *mut core::ffi::c_void,
        device_ids: &[*mut core::ffi::c_void],
        command_queues: &[*mut core::ffi::c_void],
    ) -> Self {
        assert_eq!(
            device_ids.len(),
            command_queues.len(),
            "CrmOpenClInit::with_devices: device_ids and command_queues must have equal length"
        );
        let devices_box: Box<[*mut CrmVoid]> =
            device_ids.iter().map(|d| *d as *mut CrmVoid).collect::<Vec<_>>().into_boxed_slice();
        let queues_box: Box<[*mut CrmVoid]> =
            command_queues.iter().map(|q| *q as *mut CrmVoid).collect::<Vec<_>>().into_boxed_slice();
        let raw = ClObject {
            platform_id: platform_id as *mut CrmVoid,
            device_ids: devices_box.as_ptr() as *mut *mut CrmVoid,
            device_num: devices_box.len() as CrmUInt32,
            context: context as *mut CrmVoid,
            command_queue: queues_box.as_ptr() as *mut *mut CrmVoid,
        };
        Self { _devices: devices_box, _queues: queues_box, raw }
    }

    pub fn as_raw_mut(&mut self) -> &mut ClObject {
        &mut self.raw
    }
}

impl std::fmt::Debug for CrmOpenClInit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrmOpenClInit").field("device_num", &self._devices.len()).finish_non_exhaustive()
    }
}

/// Owned Metal init descriptor — handed to
/// [`CrmSdkLibrary::initialize_with_metal`]. macOS only.
#[cfg(target_vendor = "apple")]
pub struct CrmMetalInit {
    raw: MtlObject,
}

#[cfg(target_vendor = "apple")]
unsafe impl Send for CrmMetalInit {}

#[cfg(target_vendor = "apple")]
impl CrmMetalInit {
    pub fn new(device: *mut core::ffi::c_void, command_queue: *mut core::ffi::c_void) -> Self {
        // `MtlObject::device_ids` is a single pointer (not a pointer
        // array) per the SDK's `CrmSdkTypes.h`. Same for
        // `command_queue` — both are scalar Objective-C handles.
        Self {
            raw: MtlObject {
                device_ids: device as *mut CrmVoid,
                device_num: 1,
                command_queue: command_queue as *mut CrmVoid,
            },
        }
    }

    pub fn as_raw_mut(&mut self) -> &mut MtlObject {
        &mut self.raw
    }
}

#[cfg(target_vendor = "apple")]
impl std::fmt::Debug for CrmMetalInit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrmMetalInit").finish_non_exhaustive()
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct GpuSystemInfo {
    pub device_num: CrmUInt32,
    pub driver_version: Version,
    pub runtime_version: Version,
}

/// One GPU, as [`CrmSdkLibrary::get_gpu_info_list_with_device`] reports it.
///
/// Layout-compatible with the SDK's `CrmGpuInfo`, with every field held as the
/// plain integer the SDK writes: `clSupportType` is a C enum the SDK leaves at
/// `0` — a value [`ClSupportType`] does not have — for every CUDA device.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct GpuInfo {
    device_name: [CrmASCII; 256],
    support: CrmBool,
    compute_capability: Version,
    cl_support_type: CrmUInt32,
}

impl GpuInfo {
    /// All-zero, which is a valid value of every field.
    const ZEROED: Self = Self {
        device_name: [0; 256],
        support: 0,
        compute_capability: Version { major: 0, minor: 0 },
        cl_support_type: 0,
    };

    /// The device name, e.g. `"NVIDIA GeForce RTX 5090"`.
    pub fn device_name(&self) -> String {
        fixed_ascii(self.device_name.map(|c| c as u8).as_slice())
    }

    /// Whether the SDK can develop on this device.
    pub fn is_supported(&self) -> bool {
        self.support != 0
    }

    /// CUDA compute capability, or the OpenCL version, as the SDK reports it.
    pub fn compute_capability(&self) -> Version {
        self.compute_capability
    }

    /// How far OpenCL processing is supported. `None` for a device the field
    /// does not apply to (the SDK reports `0` for CUDA devices) or a value
    /// this crate does not know — see [`cl_support_type_raw`](Self::cl_support_type_raw).
    pub fn cl_support_type(&self) -> Option<ClSupportType> {
        ClSupportType::try_from(self.cl_support_type).ok()
    }

    /// The raw `clSupportType` value.
    pub fn cl_support_type_raw(&self) -> CrmUInt32 {
        self.cl_support_type
    }
}

impl std::fmt::Debug for GpuInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuInfo")
            .field("device_name", &self.device_name())
            .field("supported", &self.is_supported())
            .field("compute_capability", &self.compute_capability)
            .field("cl_support_type", &self.cl_support_type)
            .finish()
    }
}

/// Decode a fixed-width SDK text field: up to the first NUL, or the whole field
/// when the SDK fills it completely (it does — `CreationDateTime` is 24
/// characters in a 24-byte field). Never reads past `bytes`.
pub(crate) fn fixed_ascii(bytes: &[u8]) -> String {
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..len]).into_owned()
}

/// The raw `CrmProcessRef` (`void*`) the C entry points take.
pub type RawProcessRef = *mut CrmVoid;

/// A process ref from [`CrmSdkLibrary::create_process_ref`]: the SDK's handle
/// plus the device it was created for.
///
/// The device is what decides whether a call on the ref is CUDA processing,
/// and so whether it runs under the [`cuda_exclusion()`] of spec § 9.2.1. A
/// plain copyable handle — it does not release itself; [`DecodeContext`] is
/// the owning wrapper.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ProcessRef {
    raw: RawProcessRef,
    device: Device,
}

// A handle value. Every use of it is an `unsafe` call whose contract covers
// which thread may make it, so moving or sharing the value itself is sound.
unsafe impl Send for ProcessRef {}
unsafe impl Sync for ProcessRef {}

impl ProcessRef {
    /// Wrap a raw handle.
    ///
    /// # Safety
    ///
    /// `raw` must be a process ref the SDK created for `device`; a wrong
    /// `device` makes calls skip, or take, the CUDA exclusion wrongly.
    pub unsafe fn from_raw(raw: RawProcessRef, device: Device) -> Self {
        Self { raw, device }
    }

    /// The raw `CrmProcessRef`.
    pub fn as_raw(self) -> RawProcessRef {
        self.raw
    }

    /// The device the ref was created for.
    pub fn device(self) -> Device {
        self.device
    }
}

pub type DeviceHandle = *mut CrmVoid;
pub type MemId = *mut CrmVoid;

/// One chunk of a header pack, for [`CrmSdkLibrary::create_header_pack`]: a
/// borrowed byte slice tagged with what it is.
///
/// Layout-compatible with the SDK's `CrmHeader`. It can only be built from a
/// slice, so the pointer the SDK reads through always covers `size` live bytes
/// for as long as the `Header` exists.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct Header<'a> {
    header_id: HeaderId,
    data: *const CrmVoid,
    size: CrmUInt32,
    _data: PhantomData<&'a [u8]>,
}

impl<'a> Header<'a> {
    /// A chunk of kind `header_id` over `data`.
    ///
    /// # Panics
    ///
    /// If `data` is longer than `u32::MAX` bytes, which no CRM header is.
    pub fn new(header_id: HeaderId, data: &'a [u8]) -> Self {
        let size = CrmUInt32::try_from(data.len()).expect("a CRM header chunk is far smaller than 4 GiB");
        Self { header_id, data: data.as_ptr().cast(), size, _data: PhantomData }
    }

    /// The Canon `uuid` metadata box ([`CrmMovie::canon_metadata_box`](crm_parser::CrmMovie::canon_metadata_box)).
    pub fn uuid(data: &'a [u8]) -> Self {
        Self::new(HeaderId::Uuid, data)
    }

    /// The `CRAW` sample entry ([`CrmMovie::craw_entry`](crm_parser::CrmMovie::craw_entry)).
    pub fn craw(data: &'a [u8]) -> Self {
        Self::new(HeaderId::Craw, data)
    }

    /// One frame's timed metadata ([`CrmMovie::read_timed_meta`](crm_parser::CrmMovie::read_timed_meta)).
    pub fn timed_metadata(data: &'a [u8]) -> Self {
        Self::new(HeaderId::TimedMetadata, data)
    }

    /// What this chunk is.
    pub fn header_id(&self) -> HeaderId {
        self.header_id
    }

    /// The chunk's bytes.
    pub fn data(&self) -> &'a [u8] {
        // SAFETY: built from a `&'a [u8]` of exactly `size` bytes in `new`.
        unsafe { std::slice::from_raw_parts(self.data.cast(), self.size as usize) }
    }
}

/* -------------------------------------------------------------------------------------------------
 * Callback
 * ------------------------------------------------------------------------------------------------- */

// The header typedef carries no calling-convention annotation. `extern "system"`
// is the C convention on every target the SDK ships for (x86_64 Windows,
// Linux, macOS).
/// `CRM_GetImageCallback`. `in_error` and the return value are raw
/// [`CrmErrorCode`]s, for the same reason every entry point returns one.
pub type GetImageCallback = Option<
    unsafe extern "system" fn(
        in_process_ref: RawProcessRef,
        in_dst_buffer: *mut CrmVoid,
        in_dst_buffer_size: CrmUInt32,
        in_dst_data_size: CrmUInt32,
        in_bits_per_sample: CrmUInt32,
        in_samples_per_pixel: CrmUInt32,
        in_bytes_per_line: CrmUInt32,
        in_sample_sequence: CrmUInt32,
        in_src_buffer: *mut CrmVoid,
        in_src_data_size: CrmUInt32,
        in_dev_param: *mut CrmVoid,
        in_res_param: *mut CrmVoid,
        in_ref_con: *mut CrmVoid,
        in_header_buffer: *mut CrmVoid,
        in_header_data_size: CrmUInt32,
        in_error: CrmErrorCode,
    ) -> CrmErrorCode,
>;

/// The `outDstDataSize` cell of a [`CrmSdkLibrary::get_image_async_raw`] job.
/// The SDK may write it until the job's callback fires, so keep the token
/// alive until then, and read it only afterwards.
#[derive(Debug)]
pub struct GetImageAsyncToken {
    dst_data_size: Box<CrmUInt32>,
}
impl GetImageAsyncToken {
    pub fn dst_data_size(&self) -> CrmUInt32 {
        *self.dst_data_size
    }
}

#[derive(Debug, Copy, Clone)]
pub struct GetImageAsyncOutput {
    pub dst_buffer: *mut CrmVoid,
    pub dst_data_size: CrmUInt32,
}

/// Completion state of one `CrmGetImageAsync` job, shared by its future and
/// the SDK: the future holds one `Arc`, and the SDK holds another, handed over
/// through `inRefCon` with `Arc::into_raw` and reclaimed in the callback.
#[derive(Debug)]
struct GetImageAsyncState {
    waker: AtomicWaker,
    done: AtomicBool,
    result: Mutex<Option<GetImageAsyncCallbackResult>>,
    /// Signalled, with `result` locked, once the callback has run.
    completed: Condvar,
}

#[derive(Debug, Copy, Clone)]
struct GetImageAsyncCallbackResult {
    dst_buffer: *mut CrmVoid,
    dst_data_size: CrmUInt32,
    error: CrmErrorCode,
}

// The callback fires on an SDK worker thread and hands this result to the
// polling thread through `GetImageAsyncState`. `dst_buffer` is the caller's own
// destination pointer echoed back by the SDK; this crate never dereferences it,
// so moving the value between threads is sound. This is what makes
// `GetImageAsyncState` `Send + Sync`, which the cross-thread `Arc` requires.
unsafe impl Send for GetImageAsyncCallbackResult {}

impl GetImageAsyncState {
    fn new() -> Self {
        Self {
            waker: AtomicWaker::new(),
            done: AtomicBool::new(false),
            result: Mutex::new(None),
            completed: Condvar::new(),
        }
    }

    fn lock_result(&self) -> MutexGuard<'_, Option<GetImageAsyncCallbackResult>> {
        // Nothing that holds this lock can panic, but a poisoned slot is
        // still a valid one.
        self.result.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn complete(&self, result: GetImageAsyncCallbackResult) {
        let mut slot = self.lock_result();
        *slot = Some(result);
        self.done.store(true, Ordering::Release);
        self.completed.notify_all();
        drop(slot);
        self.waker.wake();
    }

    /// Block until the SDK's completion callback has run. The SDK calls it
    /// once it has finished with the job, so afterwards nothing the job was
    /// given — inputs, destination, `outDstDataSize` — is touched again.
    fn wait(&self) {
        let mut slot = self.lock_result();
        while !self.done.load(Ordering::Acquire) {
            slot = self.completed.wait(slot).unwrap_or_else(|e| e.into_inner());
        }
    }
}

unsafe extern "system" fn get_image_async_callback(
    _in_process_ref: RawProcessRef,
    in_dst_buffer: *mut CrmVoid,
    _in_dst_buffer_size: CrmUInt32,
    in_dst_data_size: CrmUInt32,
    _in_bits_per_sample: CrmUInt32,
    _in_samples_per_pixel: CrmUInt32,
    _in_bytes_per_line: CrmUInt32,
    _in_sample_sequence: CrmUInt32,
    _in_src_buffer: *mut CrmVoid,
    _in_src_data_size: CrmUInt32,
    _in_dev_param: *mut CrmVoid,
    _in_res_param: *mut CrmVoid,
    in_ref_con: *mut CrmVoid,
    _in_header_buffer: *mut CrmVoid,
    _in_header_data_size: CrmUInt32,
    in_error: CrmErrorCode,
) -> CrmErrorCode {
    if in_ref_con.is_null() {
        return in_error;
    }

    // SAFETY: `in_ref_con` is the `Arc::into_raw` that `submit_get_image_async`
    // gave this one job, and the SDK calls back exactly once per job.
    let state = unsafe { Arc::from_raw(in_ref_con as *const GetImageAsyncState) };
    state.complete(GetImageAsyncCallbackResult {
        dst_buffer: in_dst_buffer,
        dst_data_size: in_dst_data_size,
        error: in_error,
    });

    CRM_ERR_OK
}

/// Shared `poll` body for [`GetImageFuture`] / [`OwnedGetImageFuture`]:
/// register the waker, and on completion translate the SDK result into
/// `Poll::Ready`.
fn poll_get_image(state: &GetImageAsyncState, cx: &mut Context<'_>) -> Poll<Result<GetImageAsyncOutput>> {
    state.waker.register(cx.waker());
    if !state.done.load(Ordering::Acquire) {
        return Poll::Pending;
    }
    match state.lock_result().take() {
        Some(res) => Poll::Ready(
            sdk_result(res.error)
                .map(|()| GetImageAsyncOutput { dst_buffer: res.dst_buffer, dst_data_size: res.dst_data_size }),
        ),
        None => Poll::Pending,
    }
}

/// One `CrmGetImageAsync` job, borrowing its inputs
/// ([`CrmSdkLibrary::get_image_future`]). Resolves when the SDK's completion
/// callback fires.
///
/// Dropping it while the job is still running **blocks** until the SDK
/// finishes: the job reads the borrowed inputs, and writes the destination,
/// until then.
pub struct GetImageFuture<'a> {
    state: Arc<GetImageAsyncState>,
    _out_dst_data_size: Box<CrmUInt32>,
    _inputs: PhantomData<(&'a CrmSdkLibrary, &'a [u8], &'a DevParamV13, &'a ResParamV13)>,
}

impl std::fmt::Debug for GetImageFuture<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GetImageFuture").field("done", &self.state.done.load(Ordering::Acquire)).finish_non_exhaustive()
    }
}

impl Future for GetImageFuture<'_> {
    type Output = Result<GetImageAsyncOutput>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        poll_get_image(&self.state, cx)
    }
}

impl Drop for GetImageFuture<'_> {
    fn drop(&mut self) {
        self.state.wait();
    }
}

/// `'static` owned-source variant of [`GetImageFuture`]
/// ([`CrmSdkLibrary::get_image_future_owned`]).
///
/// The borrowed [`GetImageFuture`] pins `&'a` references to the source /
/// header / dev / res inputs, so it cannot be parked in a `Vec` alongside
/// the very buffers it borrows (the self-referential-struct problem) — which
/// is exactly what a pipelined scheduler keeping `depth` jobs in flight needs.
/// This variant instead **owns** the dev/res param structs (which the SDK
/// reads via raw pointer until the callback fires) plus opaque `Send`
/// keepalives backing the source + header pointers. The output buffer is the
/// caller's. `Unpin`.
///
/// Dropping it while the job is still running **blocks** until the SDK
/// finishes, so the keepalives — and anything the caller drops after the
/// future, such as the destination — outlive the job.
pub struct OwnedGetImageFuture {
    state: Arc<GetImageAsyncState>,
    _sdk: Arc<CrmSdkLibrary>,
    _out_dst_data_size: Box<CrmUInt32>,
    _dev_param: Box<DevParamV13>,
    _res_param: Box<ResParamV13>,
    _src_keep: Box<dyn core::any::Any + Send>,
    _hdr_keep: Box<dyn core::any::Any + Send>,
}

impl std::fmt::Debug for OwnedGetImageFuture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedGetImageFuture")
            .field("done", &self.state.done.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl Future for OwnedGetImageFuture {
    type Output = Result<GetImageAsyncOutput>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        poll_get_image(&self.state, cx)
    }
}

impl Drop for OwnedGetImageFuture {
    fn drop(&mut self) {
        self.state.wait();
    }
}

/* -------------------------------------------------------------------------------------------------
 * Exported function pointer types (libloading-friendly)
 * ------------------------------------------------------------------------------------------------- */

pub type CrmInitializeSDKFn = unsafe extern "system" fn() -> CrmErrorCode;
pub type CrmInitializeSDKWithOpenCLFn = unsafe extern "system" fn(cl_object: ClObjectRef) -> CrmErrorCode;
/// `in_use_device` is the C enum `CrmDevice` — the SDK accepts bitwise-
/// OR'd values like `kCrmDeviceCpu | kCrmDeviceCuda` (Canon's sample code,
/// `TestCrmSdk.cpp`, builds every mask that way), so the Rust binding takes a
/// raw `CrmUInt32` to allow the OR-mask shape. The typed `initialize_with_*`
/// wrappers build the right mask.
pub type CrmInitializeSDKWithDeviceFn =
    unsafe extern "system" fn(in_use_device: CrmUInt32, in_gpu_param: *mut CrmVoid) -> CrmErrorCode;
pub type CrmTerminateSDKFn = unsafe extern "system" fn() -> CrmErrorCode;

pub type CrmGetFormatIdFn = unsafe extern "system" fn(
    in_buffer: *mut CrmVoid,
    in_data_size: CrmUInt32,
    out_format_id: *mut CrmUInt32,
) -> CrmErrorCode;

pub type CrmCreateProcessRefFn = unsafe extern "system" fn(
    out_process_ref: *mut RawProcessRef,
    in_format_id: FormatId,
    in_use_device: Device,
    in_use_gpu: Gpu,
    in_process_type: ProcessType,
    in_max_resolution: Resolution,
) -> CrmErrorCode;

pub type CrmResetProcessRefFn = unsafe extern "system" fn(in_process_ref: RawProcessRef) -> CrmErrorCode;
pub type CrmReleaseProcessRefFn = unsafe extern "system" fn(in_process_ref: RawProcessRef) -> CrmErrorCode;

pub type CrmAllocateFn = unsafe extern "system" fn(
    out_buffer: *mut *mut CrmVoid,
    in_buffer_size: CrmUInt32,
    in_use_device: Device,
) -> CrmErrorCode;
pub type CrmDeallocateFn = unsafe extern "system" fn(in_buffer: *mut CrmVoid, in_use_device: Device) -> CrmErrorCode;

pub type CrmDeviceAllocateFn = unsafe extern "system" fn(
    in_process_ref: RawProcessRef,
    out_buffer: *mut *mut CrmVoid,
    in_buffer_size: CrmUInt32,
    in_buffer_type: BufferType,
    in_use_gpu: Gpu,
) -> CrmErrorCode;

pub type CrmDeviceDeallocateFn =
    unsafe extern "system" fn(in_process_ref: RawProcessRef, in_buffer: *mut CrmVoid) -> CrmErrorCode;

pub type CrmGetImageFn = unsafe extern "system" fn(
    in_process_ref: RawProcessRef,
    out_dst_buffer: *mut CrmVoid,
    in_dst_buffer_size: CrmUInt32,
    out_dst_data_size: *mut CrmUInt32,
    in_bits_per_sample: CrmUInt32,
    in_samples_per_pixel: CrmUInt32,
    in_bytes_per_line: CrmUInt32,
    in_sample_sequence: CrmUInt32,
    in_src_buffer: *mut CrmVoid,
    in_src_data_size: CrmUInt32,
    in_dev_param: *mut CrmVoid,
    in_res_param: *mut CrmVoid,
    in_header_buffer: *mut CrmVoid,
    in_header_data_size: CrmUInt32,
) -> CrmErrorCode;

pub type CrmGetImageAsyncFn = unsafe extern "system" fn(
    in_process_ref: RawProcessRef,
    out_dst_buffer: *mut CrmVoid,
    in_dst_buffer_size: CrmUInt32,
    out_dst_data_size: *mut CrmUInt32,
    in_bits_per_sample: CrmUInt32,
    in_samples_per_pixel: CrmUInt32,
    in_bytes_per_line: CrmUInt32,
    in_sample_sequence: CrmUInt32,
    in_src_buffer: *mut CrmVoid,
    in_src_data_size: CrmUInt32,
    in_dev_param: *mut CrmVoid,
    in_res_param: *mut CrmVoid,
    in_ref_con: *mut CrmVoid,
    in_header_buffer: *mut CrmVoid,
    in_header_data_size: CrmUInt32,
    in_func: GetImageCallback,
) -> CrmErrorCode;

pub type CrmEnqueueImageFn = unsafe extern "system" fn(
    in_process_ref: RawProcessRef,
    in_bits_per_sample: CrmUInt32,
    in_samples_per_pixel: CrmUInt32,
    in_bytes_per_line: CrmUInt32,
    in_sample_sequence: CrmUInt32,
    in_src_buffer: *mut CrmVoid,
    in_src_data_size: CrmUInt32,
    in_dev_param: *mut CrmVoid,
    in_res_param: *mut CrmVoid,
    in_ref_con: *mut CrmVoid,
    in_header_buffer: *mut CrmVoid,
    in_header_data_size: CrmUInt32,
) -> CrmErrorCode;

pub type CrmDequeueImageFn = unsafe extern "system" fn(
    in_process_ref: RawProcessRef,
    out_dst_buffer: *mut CrmVoid,
    in_dst_buffer_size: CrmUInt32,
    out_dst_data_size: *mut CrmUInt32,
    out_ref_con: *mut *mut CrmVoid,
) -> CrmErrorCode;

pub type CrmGetImageInfoFn = unsafe extern "system" fn(
    in_format_id: CrmUInt32,
    in_buffer: *mut CrmVoid,
    in_data_size: CrmUInt32,
    in_image_info_id: CrmUInt32,
    out_image_info: *mut CrmVoid,
) -> CrmErrorCode;

pub type CrmGetGpuSystemInfoFn = unsafe extern "system" fn(
    out_cuda_gpu_system_info: *mut GpuSystemInfo,
    out_cl_gpu_system_info: *mut GpuSystemInfo,
) -> CrmErrorCode;

pub type CrmGetGpuInfoListFn =
    unsafe extern "system" fn(out_cuda_gpu_info: *mut GpuInfo, out_cl_gpu_info: *mut GpuInfo) -> CrmErrorCode;

pub type CrmGetSdkInfoFn = unsafe extern "system" fn(
    out_sdk_version: *mut Version,
    out_pipeline_num: *mut CrmUInt32,
    out_usable_gpu_num: *mut CrmUInt32,
) -> CrmErrorCode;

pub type CrmGetWbGainFn =
    unsafe extern "system" fn(in_process_ref: RawProcessRef, out_wb_gain: *mut WbGain) -> CrmErrorCode;

pub type CrmGetAcesIdtFn = unsafe extern "system" fn(
    in_process_ref: RawProcessRef,
    out_aces_idt: *mut CrmUInt32,
    out_aces_idt_str: *mut CrmASCII,
) -> CrmErrorCode;

pub type CrmGetSdkInfoWithDeviceFn = unsafe extern "system" fn(
    out_sdk_version: *mut Version,
    out_pipeline_num: *mut CrmUInt32,
    out_usable_gpu_num: *mut CrmUInt32,
    in_use_device: Device,
    in_process_type: ProcessType,
) -> CrmErrorCode;

pub type CrmGetGpuSystemInfoWithDeviceFn =
    unsafe extern "system" fn(out_gpu_system_info: *mut GpuSystemInfo, in_use_device: Device) -> CrmErrorCode;
pub type CrmGetGpuInfoListWithDeviceFn =
    unsafe extern "system" fn(out_gpu_info: *mut GpuInfo, in_use_device: Device) -> CrmErrorCode;

#[cfg(target_os = "windows")]
pub type CrmSetDeviceHandleForIntelFn = unsafe extern "system" fn(
    in_process_ref: RawProcessRef,
    in_device_handle: DeviceHandle,
    in_mem_type: MemType,
) -> CrmErrorCode;

#[cfg(target_os = "windows")]
pub type CrmSetImageParamForIntelFn =
    unsafe extern "system" fn(in_process_ref: RawProcessRef, in_resolution: Size, in_crop: Rect) -> CrmErrorCode;

#[cfg(target_os = "windows")]
pub type CrmAllocateD3dSurfacesForIntelFn =
    unsafe extern "system" fn(in_process_ref: RawProcessRef, out_mem_id_array: *mut MemId) -> CrmErrorCode;

#[cfg(target_os = "windows")]
pub type CrmDeallocateD3dSurfacesForIntelFn =
    unsafe extern "system" fn(in_process_ref: RawProcessRef, in_mem_id_array: MemId) -> CrmErrorCode;

#[cfg(target_os = "windows")]
pub type CrmCreateDeviceHandleForIntelFn = unsafe extern "system" fn(
    in_process_ref: RawProcessRef,
    out_device_handle: *mut DeviceHandle,
    in_mem_type: MemType,
) -> CrmErrorCode;

#[cfg(target_os = "windows")]
pub type CrmCleanupDeviceHandleForIntelFn = unsafe extern "system" fn(in_process_ref: RawProcessRef) -> CrmErrorCode;

pub type CrmPreProcessForGetImageWithGpuMemoryFn = unsafe extern "system" fn(
    in_process_ref: RawProcessRef,
    in_bits_per_sample: CrmUInt32,
    in_samples_per_pixel: CrmUInt32,
    in_bytes_per_line: CrmUInt32,
    in_sample_sequence: CrmUInt32,
    in_src_buffer: *mut CrmVoid,
    in_src_data_size: CrmUInt32,
    in_dev_param: *mut CrmVoid,
    in_res_param: *mut CrmVoid,
    out_required_buffer_size: *mut CrmUInt32,
) -> CrmErrorCode;

pub type CrmGetImageWithGpuMemoryFn = unsafe extern "system" fn(
    in_process_ref: RawProcessRef,
    out_dst_buffer: *mut CrmVoid,
    in_dst_buffer_size: CrmUInt32,
    out_dst_data_size: *mut CrmUInt32,
    in_src_buffer: *mut CrmVoid,
    in_src_data_size: CrmUInt32,
) -> CrmErrorCode;

pub type CrmGetLastErrorFn =
    unsafe extern "system" fn(out_error_msg: *mut CrmChar, in_buffer_size: CrmUInt32) -> CrmErrorCode;

pub type CrmGetOutImageSizeFn = unsafe extern "system" fn(
    in_res_param: *mut CrmVoid,
    out_image_width: *mut CrmUInt32,
    out_image_height: *mut CrmUInt32,
    in_header_buffer: *mut CrmVoid,
    in_header_size: CrmUInt32,
) -> CrmErrorCode;

pub type CrmCreateHeaderPackFn = unsafe extern "system" fn(
    in_header_array: *const Header<'_>,
    in_header_num: CrmUInt32,
    out_dst_buffer: *mut CrmVoid,
    in_dst_buffer_size: CrmUInt32,
    out_dst_data_size: *mut CrmUInt32,
) -> CrmErrorCode;

pub type CrmSetLutsDirectoryFn =
    unsafe extern "system" fn(in_directory_path: *const CrmChar, in_is_absolute_path: CrmBool) -> CrmErrorCode;

/* -------------------------------------------------------------------------------------------------
 * libloading wrapper
 * ------------------------------------------------------------------------------------------------- */

/// A loaded `CrmSdk` library with every entry point resolved.
///
/// Construct with [`load`](Self::load). The `pub` fields are the raw resolved
/// symbols, for anything the methods below do not cover.
///
/// # What is `unsafe`, and why
///
/// The methods are thin wrappers over the C entry points, and the SDK checks
/// none of what it is handed. So every method that passes the SDK a raw
/// pointer, a [`ProcessRef`], or GPU handles it will use later is an
/// `unsafe fn`, with a `# Safety` section stating what the SDK needs; the
/// methods that take only slices and values are safe. [`DecodeContext`] and
/// [`CrmMemory`] are the safe owning layer over the unsafe surface.
///
/// In the other direction, every value the SDK writes back is read as the
/// plain integer it is — return codes, format IDs, `clSupportType`, ACES
/// IDTs, image-info properties — and only then converted, so no SDK build can
/// hand Rust an invalid enum.
///
/// # Threads
///
/// Spec § 9.2.1 requires that, with CUDA processing, `CrmCreateProcessRef`,
/// `CrmGetImage`, `CrmEnqueueImage`, `CrmDequeueImage` and
/// `CrmReleaseProcessRef` never run at the same time as one another or as the
/// application's own CUDA API calls. Those five methods take the process-wide
/// [`cuda_exclusion()`] whenever their [`ProcessRef`] is a
/// [`Device::Cuda`] one, so concurrent decoders stay within the contract by
/// construction; [`cuda_exclusion()`] is also how the application brings its own
/// CUDA calls under it.
///
/// Everything else runs concurrently: CPU, OpenCL and Metal refs (§ 9.2.2 and
/// § 9.2.3 allow multi-threaded use), and `CrmGetImageAsync`, which is not on
/// the § 9.2.1 list. A pipeline that decompresses asynchronously on a CPU ref
/// and develops on a CUDA one keeps its decompress parallel.
#[allow(non_snake_case)]
pub struct CrmSdkLibrary {
    pub CrmInitializeSDK: dl::Symbol<CrmInitializeSDKFn>,
    pub CrmInitializeSDKWithOpenCL: dl::Symbol<CrmInitializeSDKWithOpenCLFn>,
    pub CrmInitializeSDKWithDevice: dl::Symbol<CrmInitializeSDKWithDeviceFn>,
    pub CrmTerminateSDK: dl::Symbol<CrmTerminateSDKFn>,

    pub CrmGetFormatId: dl::Symbol<CrmGetFormatIdFn>,
    pub CrmCreateProcessRef: dl::Symbol<CrmCreateProcessRefFn>,
    pub CrmResetProcessRef: dl::Symbol<CrmResetProcessRefFn>,
    pub CrmReleaseProcessRef: dl::Symbol<CrmReleaseProcessRefFn>,

    pub CrmAllocate: dl::Symbol<CrmAllocateFn>,
    pub CrmDeallocate: dl::Symbol<CrmDeallocateFn>,
    pub CrmDeviceAllocate: dl::Symbol<CrmDeviceAllocateFn>,
    pub CrmDeviceDeallocate: dl::Symbol<CrmDeviceDeallocateFn>,

    pub CrmGetImage: dl::Symbol<CrmGetImageFn>,
    // Exported with C++ linkage on some SDK builds even though `CrmSdk.h`
    // declares it inside `extern "C"`, so the name to look up is
    // toolchain-mangled and platform-specific — and differs per platform for
    // the *same* SDK version. Verified against CRMSDK 2.10: `CrmSdk.dll`
    // (Win x64) exports `?CrmGetImageAsync@@YA?AW4CrmError@@…`,
    // `libCrmSdk.so.2.10` exports the Itanium form
    // `_Z16CrmGetImageAsyncPvS_jPjjjjj…`, and the macOS framework exports it
    // unmangled. It is not the only mangled entry either: the Windows DLL's
    // 41-entry export table carries four (`CrmGetColorSpace`,
    // `CrmGetImageAsync`, `CrmReadProcessInfo`, `CrmWriteProcessInfo`) — this
    // is simply the only one the crate binds, the other three being absent
    // from the public header.
    //
    // Hence `Option`: an entry whose exported name is a build artefact may
    // not be there at all. `None` when the current SDK build/platform
    // exports no name we recognise — the async path is then unavailable, but
    // the synchronous `get_image` path still works.
    pub CrmGetImageAsync: Option<dl::Symbol<CrmGetImageAsyncFn>>,
    pub CrmEnqueueImage: dl::Symbol<CrmEnqueueImageFn>,
    pub CrmDequeueImage: dl::Symbol<CrmDequeueImageFn>,

    pub CrmGetImageInfo: dl::Symbol<CrmGetImageInfoFn>,
    pub CrmGetGpuSystemInfo: dl::Symbol<CrmGetGpuSystemInfoFn>,
    pub CrmGetGpuInfoList: dl::Symbol<CrmGetGpuInfoListFn>,
    pub CrmGetSdkInfo: dl::Symbol<CrmGetSdkInfoFn>,
    pub CrmGetWBGain: dl::Symbol<CrmGetWbGainFn>,
    pub CrmGetAcesIDT: dl::Symbol<CrmGetAcesIdtFn>,
    pub CrmGetSdkInfoWithDevice: dl::Symbol<CrmGetSdkInfoWithDeviceFn>,
    pub CrmGetGpuSystemInfoWithDevice: dl::Symbol<CrmGetGpuSystemInfoWithDeviceFn>,
    pub CrmGetGpuInfoListWithDevice: dl::Symbol<CrmGetGpuInfoListWithDeviceFn>,

    #[cfg(target_os = "windows")]
    pub CrmSetDeviceHandleForIntel: dl::Symbol<CrmSetDeviceHandleForIntelFn>,
    #[cfg(target_os = "windows")]
    pub CrmSetImageParamForIntel: dl::Symbol<CrmSetImageParamForIntelFn>,
    #[cfg(target_os = "windows")]
    pub CrmAllocateD3DSurfacesForIntel: dl::Symbol<CrmAllocateD3dSurfacesForIntelFn>,
    #[cfg(target_os = "windows")]
    pub CrmDeallocateD3DSurfacesForIntel: dl::Symbol<CrmDeallocateD3dSurfacesForIntelFn>,
    #[cfg(target_os = "windows")]
    pub CrmCreateDeviceHandleForIntel: dl::Symbol<CrmCreateDeviceHandleForIntelFn>,
    #[cfg(target_os = "windows")]
    pub CrmCleanupDeviceHandleForIntel: dl::Symbol<CrmCleanupDeviceHandleForIntelFn>,

    pub CrmPreProcessForGetImageWithGpuMemory: dl::Symbol<CrmPreProcessForGetImageWithGpuMemoryFn>,
    pub CrmGetImageWithGpuMemory: dl::Symbol<CrmGetImageWithGpuMemoryFn>,

    pub CrmGetLastError: dl::Symbol<CrmGetLastErrorFn>,
    pub CrmGetOutImageSize: dl::Symbol<CrmGetOutImageSizeFn>,
    pub CrmCreateHeaderPack: dl::Symbol<CrmCreateHeaderPackFn>,
    pub CrmSetLUTsDirectory: dl::Symbol<CrmSetLutsDirectoryFn>,

    // Must be last: fields drop in declaration order, so the library stays
    // mapped until every resolved symbol above is gone.
    _lib: dl::Library,
}

// CrmSdkLibrary — Send + Sync. Holds a refcounted `libloading::Library`
// handle plus resolved (stateless) function pointers; no apartment-model
// thread affinity. The SDK's one threading restriction (spec § 9.2.1, CUDA)
// is enforced by the methods themselves; see "Threads" above.
unsafe impl Send for CrmSdkLibrary {}
unsafe impl Sync for CrmSdkLibrary {}

impl std::fmt::Debug for CrmSdkLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrmSdkLibrary")
            .field("has_get_image_async", &self.CrmGetImageAsync.is_some())
            .finish_non_exhaustive()
    }
}

/// Encode a filesystem path the way `CrmSdk` decodes it.
///
/// The SDK's path arguments are `CrmChar*` — plain `char*`. On Windows it
/// widens them with `MultiByteToWideChar(CP_THREAD_ACP, …)` before the
/// `_wfopen`, so the bytes it wants are the thread's ANSI code page, **not**
/// UTF-8; handing it `str::as_bytes()` mis-decodes every non-ASCII path on a
/// non-UTF-8 ACP. On the POSIX builds `char*` IS the OS byte string, so the
/// `OsStr` bytes go through unchanged.
///
/// Errors instead of panicking on caller data the SDK cannot express: an
/// interior NUL, or (Windows) a character with no ACP representation.
fn sdk_path_cstring(path: &Path) -> Result<CString> {
    #[cfg(target_os = "windows")]
    let bytes = {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Globalization::{CP_THREAD_ACP, WC_NO_BEST_FIT_CHARS, WideCharToMultiByte};

        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.is_empty() {
            Vec::new()
        } else {
            // SAFETY: `wide` is a valid `[u16]` of `wide.len()`; a null
            // destination with size 0 is the documented length query.
            let len = unsafe {
                WideCharToMultiByte(
                    CP_THREAD_ACP,
                    WC_NO_BEST_FIT_CHARS,
                    wide.as_ptr(),
                    wide.len() as i32,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                )
            };
            if len <= 0 {
                return Err(CrmError::InvalidParameter(
                    "path is not representable in the ANSI code page the SDK decodes with",
                ));
            }
            let mut buf = vec![0u8; len as usize];
            let mut used_default: CrmBool = 0;
            // SAFETY: `buf` has exactly the `len` bytes the query asked for.
            let written = unsafe {
                WideCharToMultiByte(
                    CP_THREAD_ACP,
                    WC_NO_BEST_FIT_CHARS,
                    wide.as_ptr(),
                    wide.len() as i32,
                    buf.as_mut_ptr(),
                    len,
                    std::ptr::null(),
                    &mut used_default,
                )
            };
            if written != len || used_default != 0 {
                return Err(CrmError::InvalidParameter(
                    "path is not representable in the ANSI code page the SDK decodes with",
                ));
            }
            buf
        }
    };
    #[cfg(not(target_os = "windows"))]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };

    CString::new(bytes).map_err(|_| CrmError::InvalidParameter("path contains an interior NUL byte"))
}

/// Turn a raw [`CrmErrorCode`] into a `Result`.
///
/// Error strings are not fetched here: `get_last_error_string` allocates 8 KiB
/// per call, and `DevelopEmptyQueue` / `WaitTimeoutError` are ordinary flow
/// control on the queue path. Callers that want the SDK's detail string call
/// [`CrmSdkLibrary::get_last_error_string`] themselves.
fn sdk_result(code: CrmErrorCode) -> Result<()> {
    match SdkError::from_code(code) {
        None => Ok(()),
        Some(e) => Err(CrmError::SdkError(e)),
    }
}

/// The scratch buffer every [`CrmSdkLibrary::get_image_info`] call hands the
/// SDK. `CrmGetImageInfo` takes no output size, and writes as many bytes as
/// the property is wide — 80 at most in CRMSDK 2.10 (`LensName`), measured
/// against the real library. 512 zeroed bytes leave room for a wider property
/// from a newer SDK, and the zeroing gives a defined value when the SDK
/// returns `CRM_ERR_OK` without writing anything (it does, for an ID it does
/// not know).
const IMAGE_INFO_SCRATCH: usize = 512;

#[repr(C, align(8))]
struct ImageInfoScratch([u8; IMAGE_INFO_SCRATCH]);

impl CrmSdkLibrary {
    /// Load `CrmSdk` from `path` and resolve every entry point. This does not
    /// initialize the SDK; call one of the `initialize_*` methods next.
    ///
    /// # Safety
    ///
    /// Loading a library runs its initialization code, and every entry point
    /// is then called through the signature `CrmSdk.h` (CRMSDK 2.10)
    /// declares. `path` must name a genuine Canon `CrmSdk` build with that
    /// ABI — `CrmSdk.dll`, `libCrmSdk.so`, or the Mach-O inside
    /// `CrmSdk.framework`.
    pub unsafe fn load<P: libloading::AsFilename>(path: P) -> std::result::Result<Self, libloading::Error> {
        unsafe {
            let lib = dl::Library::new(path)?;
            Ok(Self {
                CrmInitializeSDK: lib.get(b"CrmInitializeSDK\0")?,
                CrmInitializeSDKWithOpenCL: lib.get(b"CrmInitializeSDKWithOpenCL\0")?,
                CrmInitializeSDKWithDevice: lib.get(b"CrmInitializeSDKWithDevice\0")?,
                CrmTerminateSDK: lib.get(b"CrmTerminateSDK\0")?,

                CrmGetFormatId: lib.get(b"CrmGetFormatId\0")?,
                CrmCreateProcessRef: lib.get(b"CrmCreateProcessRef\0")?,
                CrmResetProcessRef: lib.get(b"CrmResetProcessRef\0")?,
                CrmReleaseProcessRef: lib.get(b"CrmReleaseProcessRef\0")?,

                CrmAllocate: lib.get(b"CrmAllocate\0")?,
                CrmDeallocate: lib.get(b"CrmDeallocate\0")?,
                CrmDeviceAllocate: lib.get(b"CrmDeviceAllocate\0")?,
                CrmDeviceDeallocate: lib.get(b"CrmDeviceDeallocate\0")?,

                CrmGetImage: lib.get(b"CrmGetImage\0")?,
                // Best-effort, non-fatal: try every name CRMSDK 2.10 is known
                // to export it under — MSVC mangling (Windows), Itanium
                // mangling (Linux), then the plain name (macOS, and any build
                // that gets the `extern "C"` right). If none resolves,
                // `load()` still succeeds and only the async path is
                // unavailable.
                CrmGetImageAsync: lib
                    .get(b"?CrmGetImageAsync@@YA?AW4CrmError@@PEAX0IPEAIIIII0I00PEAPEAX0IP6A?AW41@00IIIIII0I0000IW41@@Z@Z\0")
                    .or_else(|_| lib.get(b"_Z16CrmGetImageAsyncPvS_jPjjjjjS_jS_S_PS_S_jPF8CrmErrorS_S_jjjjjjS_jS_S_S_S_jS2_E\0"))
                    .or_else(|_| lib.get(b"CrmGetImageAsync\0"))
                    .ok(),
                CrmEnqueueImage: lib.get(b"CrmEnqueueImage\0")?,
                CrmDequeueImage: lib.get(b"CrmDequeueImage\0")?,

                CrmGetImageInfo: lib.get(b"CrmGetImageInfo\0")?,
                CrmGetGpuSystemInfo: lib.get(b"CrmGetGpuSystemInfo\0")?,
                CrmGetGpuInfoList: lib.get(b"CrmGetGpuInfoList\0")?,
                CrmGetSdkInfo: lib.get(b"CrmGetSdkInfo\0")?,
                CrmGetWBGain: lib.get(b"CrmGetWBGain\0")?,
                CrmGetAcesIDT: lib.get(b"CrmGetAcesIDT\0")?,
                CrmGetSdkInfoWithDevice: lib.get(b"CrmGetSdkInfoWithDevice\0")?,
                CrmGetGpuSystemInfoWithDevice: lib.get(b"CrmGetGpuSystemInfoWithDevice\0")?,
                CrmGetGpuInfoListWithDevice: lib.get(b"CrmGetGpuInfoListWithDevice\0")?,

                #[cfg(target_os = "windows")]
                CrmSetDeviceHandleForIntel: lib.get(b"CrmSetDeviceHandleForIntel\0")?,
                #[cfg(target_os = "windows")]
                CrmSetImageParamForIntel: lib.get(b"CrmSetImageParamForIntel\0")?,
                #[cfg(target_os = "windows")]
                CrmAllocateD3DSurfacesForIntel: lib.get(b"CrmAllocateD3DSurfacesForIntel\0")?,
                #[cfg(target_os = "windows")]
                CrmDeallocateD3DSurfacesForIntel: lib.get(b"CrmDeallocateD3DSurfacesForIntel\0")?,
                #[cfg(target_os = "windows")]
                CrmCreateDeviceHandleForIntel: lib.get(b"CrmCreateDeviceHandleForIntel\0")?,
                #[cfg(target_os = "windows")]
                CrmCleanupDeviceHandleForIntel: lib.get(b"CrmCleanupDeviceHandleForIntel\0")?,

                CrmPreProcessForGetImageWithGpuMemory: lib.get(b"CrmPreProcessForGetImageWithGpuMemory\0")?,
                CrmGetImageWithGpuMemory: lib.get(b"CrmGetImageWithGpuMemory\0")?,

                CrmGetLastError: lib.get(b"CrmGetLastError\0")?,
                CrmGetOutImageSize: lib.get(b"CrmGetOutImageSize\0")?,
                CrmCreateHeaderPack: lib.get(b"CrmCreateHeaderPack\0")?,
                CrmSetLUTsDirectory: lib.get(b"CrmSetLUTsDirectory\0")?,

                _lib: lib,
            })
        }
    }

    /* ── Initialization ───────────────────────────────────────────────────────────────────────── */

    #[inline]
    pub fn initialize_sdk(&self) -> Result<()> {
        sdk_result(unsafe { (self.CrmInitializeSDK)() })
    }

    /// CPU-only init (no GPU device handle required).
    #[inline]
    pub fn initialize_with_cpu(&self) -> Result<()> {
        self.initialize_sdk()
    }

    /// Initialize the SDK against an OpenCL device.
    ///
    /// The SDK's `CrmInitializeSDKWithDevice` accepts a bitmask of
    /// `kCrmDeviceCpu | kCrmDevice<Gpu>` — spec § 7.2.15 spells it
    /// `(CrmDevice)( kCrmDeviceCpu | kCrmDeviceOpenCl )`. Passing the
    /// GPU bit alone returns `InvalidParameter`.
    ///
    /// # Safety
    ///
    /// Every handle in `cl` must be a live OpenCL object of the kind its field
    /// names — a `cl_platform_id`, `device_num` `cl_device_id`s and as many
    /// `cl_command_queue`s, and a `cl_context` — and they must stay alive
    /// until the SDK is terminated: it develops on them.
    #[inline]
    pub unsafe fn initialize_with_opencl(&self, cl: &mut ClObject) -> Result<()> {
        let mask = Device::Cpu as CrmUInt32 | Device::OpenCl as CrmUInt32;
        sdk_result(unsafe { (self.CrmInitializeSDKWithDevice)(mask, cl as *mut _ as *mut CrmVoid) })
    }

    /// Initialize the SDK against a CUDA device. Combines `kCrmDeviceCpu |
    /// kCrmDeviceCuda`, as Canon's sample code (`TestCrmSdk.cpp`) does.
    ///
    /// # Safety
    ///
    /// `cuda` must name `device_num` CUDA device ordinals and as many live
    /// `CUstream`s, and the streams must stay alive until the SDK is
    /// terminated: it develops on them.
    #[cfg(not(target_vendor = "apple"))]
    #[inline]
    pub unsafe fn initialize_with_cuda(&self, cuda: &mut CudaObject) -> Result<()> {
        let mask = Device::Cpu as CrmUInt32 | Device::Cuda as CrmUInt32;
        sdk_result(unsafe { (self.CrmInitializeSDKWithDevice)(mask, cuda as *mut _ as *mut CrmVoid) })
    }

    /// Initialize the SDK against a Metal device. macOS only. Combines
    /// `kCrmDeviceCpu | kCrmDeviceMetal`, as Canon's sample code
    /// (`TestCrmSdk.cpp`) does.
    ///
    /// # Safety
    ///
    /// `mtl` must hold a live `id<MTLDevice>` and `id<MTLCommandQueue>`, and
    /// both must stay alive until the SDK is terminated: it develops on them.
    #[cfg(target_vendor = "apple")]
    #[inline]
    pub unsafe fn initialize_with_metal(&self, mtl: &mut MtlObject) -> Result<()> {
        let mask = Device::Cpu as CrmUInt32 | Device::Metal as CrmUInt32;
        sdk_result(unsafe { (self.CrmInitializeSDKWithDevice)(mask, mtl as *mut _ as *mut CrmVoid) })
    }

    /// Escape hatch for callers that already hold a `*mut CrmVoid` of the
    /// right shape (FFI bridges, test harnesses). Prefer the typed
    /// constructors ([`initialize_with_cuda`](Self::initialize_with_cuda),
    /// [`initialize_with_opencl`](Self::initialize_with_opencl),
    /// [`initialize_with_metal`](Self::initialize_with_metal)) for new code.
    ///
    /// # Safety
    /// `in_gpu_param` must point to a struct of the shape the SDK expects
    /// for `in_use_device` — `CudaObject` for the `Cuda` bit, `ClObject`
    /// for the `OpenCl` bit, `MtlObject` for the `Metal` bit, ignored
    /// for `Cpu`-only — whose handles meet the contract of the matching
    /// typed constructor. The SDK accepts ORed device bits —
    /// `kCrmDeviceCpu | kCrmDeviceCuda` etc. — so this surface takes a
    /// `CrmUInt32` rather than the typed `Device` enum.
    #[doc(hidden)]
    #[inline]
    pub unsafe fn initialize_sdk_with_device(
        &self,
        in_use_device: CrmUInt32,
        in_gpu_param: *mut CrmVoid,
    ) -> Result<()> {
        sdk_result(unsafe { (self.CrmInitializeSDKWithDevice)(in_use_device, in_gpu_param) })
    }

    /// Terminate the SDK, releasing everything it allocated.
    ///
    /// Dropping the library does this too, and that is the usual way to end:
    /// every [`DecodeContext`], [`CrmMemory`] and future holds the library
    /// through an `Arc`, so the drop cannot come too early.
    ///
    /// # Safety
    ///
    /// Termination frees every process ref and allocation the SDK made, so
    /// nothing made through this library may be used — or dropped — after
    /// it: no [`DecodeContext`], [`CrmMemory`], [`ProcessRef`] or pending
    /// async job may outlive this call.
    #[inline]
    pub unsafe fn terminate_sdk(&self) -> Result<()> {
        sdk_result(unsafe { (self.CrmTerminateSDK)() })
    }

    /* ── Clip identification ─────────────────────────────────────────────────────────────────── */

    #[inline]
    pub fn get_format_id(&self, in_buffer: &[u8]) -> Result<FormatId> {
        let mut out_format_id: CrmUInt32 = 0;
        sdk_result(unsafe {
            (self.CrmGetFormatId)(in_buffer.as_ptr() as *mut _, in_buffer.len() as CrmUInt32, &mut out_format_id)
        })?;
        Ok(FormatId::from_raw(out_format_id))
    }

    /// Read one property of a clip, typed by `info`:
    ///
    /// ```no_run
    /// # fn f(sdk: &canon_raw::CrmSdkLibrary, format_id: canon_raw::FormatId, header_pack: &[u8]) -> canon_raw::Result<()> {
    /// use canon_raw::image_info;
    ///
    /// let width: u16 = sdk.get_image_info(format_id, header_pack, image_info::CaptureWidth)?;
    /// let model: String = sdk.get_image_info(format_id, header_pack, image_info::DeviceModelName)?;
    /// # Ok(()) }
    /// ```
    ///
    /// `CrmGetImageInfo` takes no output size: the SDK writes as many bytes as
    /// the property is wide (spec § 7.4.2), so the output type has to follow
    /// from the ID. Each key in [`image_info`] carries that type — from spec
    /// § 8.2, checked against what the SDK actually writes — and the value is
    /// read out of a zeroed scratch buffer larger than any property.
    ///
    /// A property the clip does not carry comes back as
    /// [`SdkError::InvalidParameter`] (`TotalImageSize`, `Position` and
    /// `Timecode` on `.CRM` data, for instance).
    pub fn get_image_info<I: ImageInfo>(&self, format_id: FormatId, header: &[u8], info: I) -> Result<I::Value> {
        const { assert!(size_of::<I::Raw>() <= IMAGE_INFO_SCRATCH) };
        let _ = info;
        let mut scratch = ImageInfoScratch([0; IMAGE_INFO_SCRATCH]);
        // SAFETY: `scratch` is larger than anything `CrmGetImageInfo` writes
        // (see `IMAGE_INFO_SCRATCH`).
        unsafe { self.get_image_info_raw(format_id, header, I::ID as CrmUInt32, scratch.0.as_mut_ptr().cast()) }?;
        // SAFETY: `I::Raw` is plain data — every bit pattern is a value — and
        // no larger than the fully initialized scratch buffer.
        let raw = unsafe { scratch.0.as_ptr().cast::<I::Raw>().read_unaligned() };
        Ok(I::from_raw(raw))
    }

    /// `CrmGetImageInfo` with a caller-supplied output, for a property ID
    /// [`image_info`] does not cover (one added by a newer SDK, say).
    ///
    /// # Safety
    ///
    /// `out_image_info` must be valid for writes of however many bytes the SDK
    /// writes for `image_info_id`. The SDK takes no size, so that width is
    /// the caller's to know; nothing here can check it.
    pub unsafe fn get_image_info_raw(
        &self,
        format_id: FormatId,
        header: &[u8],
        image_info_id: CrmUInt32,
        out_image_info: *mut CrmVoid,
    ) -> Result<()> {
        sdk_result(unsafe {
            (self.CrmGetImageInfo)(
                format_id.raw(),
                header.as_ptr() as *mut _,
                header.len() as CrmUInt32,
                image_info_id,
                out_image_info,
            )
        })
    }

    /* ── Process refs and memory ─────────────────────────────────────────────────────────────── */

    /// Mint a decode context inside the SDK.
    ///
    /// # Hold it. Do not mint one per decode.
    ///
    /// A process ref is cheap on its own — creating and releasing one with no
    /// decode in between moves neither the thread count nor the working set.
    /// The first [`get_image`](Self::get_image) on it is not: the SDK lazily
    /// builds a **worker pool on the ref** — measured at 76 extra OS threads and
    /// ~2 GB of committed arena for a 6000 × 3164 clip on CRMSDK 2.10 — and
    /// [`release_process_ref`](Self::release_process_ref) tears the whole thing
    /// down again.
    ///
    /// That teardown races. It returns before every worker it spawned has
    /// exited, and a straggler then enters a `CRITICAL_SECTION` the release has
    /// already deleted: an `ACCESS_VIOLATION` writing
    /// `RTL_CRITICAL_SECTION_DEBUG::ContentionCount` (offset `0x24`) through a
    /// NULLed `DebugInfo`, on a thread whose stack is entirely
    /// `RtlUserThreadStart` → `CrmSdk.dll` → `RtlEnterCriticalSection`. It needs
    /// contention to lose, so it presents as a rare, load-dependent crash with
    /// no relation to the parameters of the call that preceded it — easy to
    /// misattribute to whichever SDK call happened to run last.
    ///
    /// So every release after a decode is a dice roll. One ref held for the life
    /// of a decoder is one roll; one per frame, or per test case, is one each.
    /// [`reset_process_ref`](Self::reset_process_ref) is the between-decodes
    /// reset that keeps the pool up and is the right call after a refused
    /// decode. [`DecodeContext`] owns exactly one ref for its whole life for
    /// this reason.
    ///
    /// The returned handle records `in_use_device`, which is what makes calls
    /// on a CUDA ref take the [`cuda_exclusion()`] (spec § 9.2.1) — this one
    /// included. Every call that takes the handle is `unsafe`, and nothing
    /// releases it for you; [`DecodeContext`] is the owning wrapper.
    #[inline]
    pub fn create_process_ref(
        &self,
        in_format_id: FormatId,
        in_use_device: Device,
        in_use_gpu: Gpu,
        in_process_type: ProcessType,
        in_max_resolution: Resolution,
    ) -> Result<ProcessRef> {
        let mut out_process_ref = std::ptr::null_mut();
        cuda_exclusion::exclusive_if_cuda(in_use_device, || {
            sdk_result(unsafe {
                (self.CrmCreateProcessRef)(
                    &mut out_process_ref,
                    in_format_id,
                    in_use_device,
                    in_use_gpu,
                    in_process_type,
                    in_max_resolution,
                )
            })
        })?;
        Ok(ProcessRef { raw: out_process_ref, device: in_use_device })
    }

    /// Reset a process ref between discontinuous decodes (spec § 7.2.4).
    ///
    /// # Safety
    ///
    /// `in_process_ref` must come from [`create_process_ref`](Self::create_process_ref)
    /// on this library, not yet released, with no async or queued job still
    /// pending on it.
    #[inline]
    pub unsafe fn reset_process_ref(&self, in_process_ref: ProcessRef) -> Result<()> {
        sdk_result(unsafe { (self.CrmResetProcessRef)(in_process_ref.as_raw()) })
    }

    /// Release a process ref and everything the SDK allocated for it. See
    /// [`create_process_ref`](Self::create_process_ref) for why this should
    /// happen once per decoder, not once per decode.
    ///
    /// # Safety
    ///
    /// `in_process_ref` must come from [`create_process_ref`](Self::create_process_ref)
    /// on this library and not have been released already, no async or
    /// queued job may still be pending on it, and it must not be used again.
    #[inline]
    pub unsafe fn release_process_ref(&self, in_process_ref: ProcessRef) -> Result<()> {
        cuda_exclusion::exclusive_if_cuda(in_process_ref.device(), || {
            sdk_result(unsafe { (self.CrmReleaseProcessRef)(in_process_ref.as_raw()) })
        })
    }

    /// `CrmAllocate`: a buffer for clip data or output. For
    /// [`Device::Cuda`] this is page-locked host memory (`cudaHostAlloc`,
    /// spec § 7.2.6), not VRAM. [`CrmMemory::new`] is the owning wrapper.
    #[inline]
    pub fn allocate(&self, in_buffer_size: CrmUInt32, in_use_device: Device) -> Result<*mut CrmVoid> {
        let mut out_buffer: *mut CrmVoid = std::ptr::null_mut();
        sdk_result(unsafe { (self.CrmAllocate)(&mut out_buffer, in_buffer_size, in_use_device) })?;
        Ok(out_buffer)
    }

    /// # Safety
    ///
    /// `in_buffer` must come from [`allocate`](Self::allocate) on this library
    /// with the same `in_use_device`, not be freed already, and not be in use
    /// by a pending job.
    #[inline]
    pub unsafe fn deallocate(&self, in_buffer: *mut CrmVoid, in_use_device: Device) -> Result<()> {
        sdk_result(unsafe { (self.CrmDeallocate)(in_buffer, in_use_device) })
    }

    /// `CrmDeviceAllocate`: a buffer in VRAM (spec § 7.2.16).
    ///
    /// # Safety
    ///
    /// `in_process_ref` must be a live ref from [`create_process_ref`](Self::create_process_ref)
    /// on this library.
    #[inline]
    pub unsafe fn device_allocate(
        &self,
        in_process_ref: ProcessRef,
        in_buffer_size: CrmUInt32,
        in_buffer_type: BufferType,
        in_use_gpu: Gpu,
    ) -> Result<*mut CrmVoid> {
        let mut out_buffer: *mut CrmVoid = std::ptr::null_mut();
        sdk_result(unsafe {
            (self.CrmDeviceAllocate)(
                in_process_ref.as_raw(),
                &mut out_buffer,
                in_buffer_size,
                in_buffer_type,
                in_use_gpu,
            )
        })?;
        Ok(out_buffer)
    }

    /// # Safety
    ///
    /// `in_process_ref` must be the live ref `in_buffer` was allocated
    /// against with [`device_allocate`](Self::device_allocate); `in_buffer`
    /// must not be freed already, and not be in use by pending work.
    #[inline]
    pub unsafe fn device_deallocate(&self, in_process_ref: ProcessRef, in_buffer: *mut CrmVoid) -> Result<()> {
        sdk_result(unsafe { (self.CrmDeviceDeallocate)(in_process_ref.as_raw(), in_buffer) })
    }

    /* ── Decoding ─────────────────────────────────────────────────────────────────────────────── */

    /// `CrmGetImage`: decompress and/or develop one frame. Returns the number
    /// of bytes the SDK reports it produced.
    ///
    /// [`DecodeContext::decode_frame_cpu_into`] is the safe form for a CPU
    /// destination.
    ///
    /// # Safety
    ///
    /// * `in_process_ref` is a live ref from [`create_process_ref`](Self::create_process_ref)
    ///   on this library, with no other decode running on it.
    /// * `out_dst_buffer` lives in the memory domain `in_res_param.out_buffer_type`
    ///   names — a host pointer for `Cpu`, a `CUdeviceptr` for `Cuda`, a
    ///   `cl_mem` for `OpenCl`, an `id<MTLBuffer>` for `Metal` — and is valid
    ///   for writes of `in_dst_buffer_size` bytes.
    /// * For a GPU domain, `in_dst_buffer_size` must also be at least the
    ///   size the recipe produces. Measured on CRMSDK 2.10: the CPU path
    ///   honours `in_dst_buffer_size` (and writes nothing at all, while still
    ///   returning `CRM_ERR_OK`, when it is too small); a CUDA destination is
    ///   written to its full output size whatever this argument says.
    /// * For a GPU domain, the SDK's device work can still be writing the
    ///   destination when this returns: the buffer must stay allocated until
    ///   the stream or queue the SDK was initialized with has been
    ///   synchronized.
    ///
    /// On a CUDA ref this runs under the [`cuda_exclusion()`] (spec § 9.2.1).
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn get_image(
        &self,
        in_process_ref: ProcessRef,
        out_dst_buffer: *mut CrmVoid,
        in_dst_buffer_size: CrmUInt32,
        in_bits_per_sample: CrmUInt32,
        in_samples_per_pixel: CrmUInt32,
        in_bytes_per_line: CrmUInt32,
        in_sample_sequence: SampleSequence,
        in_src_buffer: &[u8],
        in_dev_param: &DevParamV13,
        in_res_param: &ResParamV13,
        in_header_buffer: &[u8],
    ) -> Result<u32> {
        let mut out_dst_data_size = 0;
        cuda_exclusion::exclusive_if_cuda(in_process_ref.device(), || {
            sdk_result(unsafe {
                (self.CrmGetImage)(
                    in_process_ref.as_raw(),
                    out_dst_buffer,
                    in_dst_buffer_size,
                    &mut out_dst_data_size,
                    in_bits_per_sample,
                    in_samples_per_pixel,
                    in_bytes_per_line,
                    in_sample_sequence as _,
                    in_src_buffer.as_ptr() as *mut CrmVoid,
                    in_src_buffer.len() as CrmUInt32,
                    in_dev_param as *const _ as *mut CrmVoid,
                    in_res_param as *const _ as *mut CrmVoid,
                    in_header_buffer.as_ptr() as *mut CrmVoid,
                    in_header_buffer.len() as CrmUInt32,
                )
            })
        })?;
        Ok(out_dst_data_size)
    }

    /// `CrmGetImageAsync`, verbatim: the caller supplies the callback and
    /// `in_ref_con`. [`get_image_future`](Self::get_image_future) and
    /// [`get_image_future_owned`](Self::get_image_future_owned) are the
    /// future-based forms.
    ///
    /// At most three jobs may be in flight per process ref; a fourth fails
    /// with [`SdkError::DecompressNoResource`]. Measured on CRMSDK 2.10, the
    /// SDK accepts this call only on a DECOMPRESS-only ref: a combined
    /// `DEVELOP | DECOMPRESS` ref is refused with *"CrmGetImageAsync can
    /// execute either decompress or develop processing asynchronously."* and
    /// a DEVELOP-only one with *"CrmGetImageAsync cannot execute develop
    /// processing asynchronously."*.
    ///
    /// # Safety
    ///
    /// Everything [`get_image`](Self::get_image) requires, for as long as the
    /// job runs — until `in_func` has been called — rather than for the call:
    /// the source, parameter and header pointers must stay valid and
    /// unmodified, the destination valid, and the returned token alive. Each
    /// pointer must cover the size passed with it. `in_func`, if any, must be
    /// sound to call once from an SDK worker thread with `in_ref_con`; with
    /// `None` the call runs synchronously.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn get_image_async_raw(
        &self,
        in_process_ref: ProcessRef,
        out_dst_buffer: *mut CrmVoid,
        in_dst_buffer_size: CrmUInt32,
        in_bits_per_sample: CrmUInt32,
        in_samples_per_pixel: CrmUInt32,
        in_bytes_per_line: CrmUInt32,
        in_sample_sequence: CrmUInt32,
        in_src_buffer: *mut CrmVoid,
        in_src_data_size: CrmUInt32,
        in_dev_param: *mut CrmVoid,
        in_res_param: *mut CrmVoid,
        in_ref_con: *mut CrmVoid,
        in_header_buffer: *mut CrmVoid,
        in_header_data_size: CrmUInt32,
        in_func: GetImageCallback,
    ) -> Result<GetImageAsyncToken> {
        let mut token = GetImageAsyncToken { dst_data_size: Box::new(0) };
        let out_dst_data_size = &mut *token.dst_data_size as *mut CrmUInt32;

        let async_fn = self
            .CrmGetImageAsync
            .as_ref()
            .ok_or(CrmError::MissingData("CrmGetImageAsync is not available in this Canon SDK build"))?;
        sdk_result(unsafe {
            (async_fn)(
                in_process_ref.as_raw(),
                out_dst_buffer,
                in_dst_buffer_size,
                out_dst_data_size,
                in_bits_per_sample,
                in_samples_per_pixel,
                in_bytes_per_line,
                in_sample_sequence,
                in_src_buffer,
                in_src_data_size,
                in_dev_param,
                in_res_param,
                in_ref_con,
                in_header_buffer,
                in_header_data_size,
                in_func,
            )
        })?;

        Ok(token)
    }

    /// Core `CrmGetImageAsync` submission shared by the borrowed
    /// ([`get_image_future`](Self::get_image_future)) and owned
    /// ([`get_image_future_owned`](Self::get_image_future_owned))
    /// builders: performs the `Arc::into_raw` co-ownership handoff to the
    /// SDK and reclaims the refcount if submission fails, returning the
    /// live [`GetImageAsyncState`] on success.
    ///
    /// # Safety
    /// Every pointer must stay valid until the returned state's callback
    /// fires; the caller owns keeping the backing allocations alive (via
    /// the future, whose `Drop` waits for the callback).
    #[allow(clippy::too_many_arguments)]
    unsafe fn submit_get_image_async(
        &self,
        in_process_ref: ProcessRef,
        out_dst_buffer: *mut CrmVoid,
        in_dst_buffer_size: CrmUInt32,
        out_dst_data_size: *mut CrmUInt32,
        in_bits_per_sample: CrmUInt32,
        in_samples_per_pixel: CrmUInt32,
        in_bytes_per_line: CrmUInt32,
        in_sample_sequence: CrmUInt32,
        src_ptr: *mut CrmVoid,
        src_size: CrmUInt32,
        dev_param: *mut CrmVoid,
        res_param: *mut CrmVoid,
        header_ptr: *mut CrmVoid,
        header_size: CrmUInt32,
    ) -> Result<Arc<GetImageAsyncState>> {
        let async_fn = self
            .CrmGetImageAsync
            .as_ref()
            .ok_or(CrmError::MissingData("CrmGetImageAsync is not available in this Canon SDK build"))?;
        let state = Arc::new(GetImageAsyncState::new());
        let ref_con = Arc::into_raw(state.clone()) as *mut CrmVoid;
        let submit_res = sdk_result(unsafe {
            (async_fn)(
                in_process_ref.as_raw(),
                out_dst_buffer,
                in_dst_buffer_size,
                out_dst_data_size,
                in_bits_per_sample,
                in_samples_per_pixel,
                in_bytes_per_line,
                in_sample_sequence,
                src_ptr,
                src_size,
                dev_param,
                res_param,
                ref_con,
                header_ptr,
                header_size,
                Some(get_image_async_callback),
            )
        });
        if let Err(e) = submit_res {
            // The SDK refused the job, so it never calls back: take its
            // refcount back.
            unsafe { drop(Arc::from_raw(ref_con as *const GetImageAsyncState)) };
            return Err(e);
        }
        Ok(state)
    }

    /// Submit `CrmGetImageAsync` and return a future for the job, borrowing
    /// its inputs. See [`get_image_async_raw`](Self::get_image_async_raw) for
    /// which process refs the SDK accepts this on.
    ///
    /// # Safety
    ///
    /// * `in_process_ref` is a live ref from [`create_process_ref`](Self::create_process_ref)
    ///   on this library.
    /// * `out_dst_buffer` meets [`get_image`](Self::get_image)'s destination
    ///   contract, and stays valid until the future has completed or been
    ///   dropped — dropping it waits for the job.
    /// * The future must not be leaked (`mem::forget`): its `Drop` is what
    ///   keeps the borrowed inputs alive until the SDK has finished with them.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn get_image_future<'a>(
        &'a self,
        in_process_ref: ProcessRef,
        out_dst_buffer: *mut CrmVoid,
        in_dst_buffer_size: CrmUInt32,
        in_bits_per_sample: CrmUInt32,
        in_samples_per_pixel: CrmUInt32,
        in_bytes_per_line: CrmUInt32,
        in_sample_sequence: SampleSequence,
        in_src_buffer: &'a [u8],
        in_dev_param: &'a DevParamV13,
        in_res_param: &'a ResParamV13,
        in_header_buffer: &'a [u8],
    ) -> Result<GetImageFuture<'a>> {
        let mut out_dst_data_size = Box::new(0u32);
        // SAFETY: the inputs are borrowed for `'a`, which the future carries,
        // and its `Drop` waits for the job; the destination and the ref are
        // the caller's contract.
        let state = unsafe {
            self.submit_get_image_async(
                in_process_ref,
                out_dst_buffer,
                in_dst_buffer_size,
                &mut *out_dst_data_size as *mut CrmUInt32,
                in_bits_per_sample,
                in_samples_per_pixel,
                in_bytes_per_line,
                in_sample_sequence as CrmUInt32,
                in_src_buffer.as_ptr() as *mut CrmVoid,
                in_src_buffer.len() as CrmUInt32,
                in_dev_param as *const _ as *mut CrmVoid,
                in_res_param as *const _ as *mut CrmVoid,
                in_header_buffer.as_ptr() as *mut CrmVoid,
                in_header_buffer.len() as CrmUInt32,
            )?
        };

        Ok(GetImageFuture { state, _out_dst_data_size: out_dst_data_size, _inputs: PhantomData })
    }

    /// `'static` owned-source counterpart to [`get_image_future`](Self::get_image_future).
    ///
    /// Submits `CrmGetImageAsync` and returns an [`OwnedGetImageFuture`] that
    /// borrows nothing — the dev/res param structs are moved in (boxed; the
    /// SDK reads them via raw pointer until the callback fires), and the
    /// source + header buffers are kept alive by the opaque `Send` keepalives
    /// (`src_keep` / `hdr_keep`). The output buffer is the caller's. This is
    /// what lets a pipelined scheduler park `depth` jobs in a pool.
    ///
    /// `in_sample_sequence` is the raw SDK code (`0` for a CRMDecompressed
    /// Bayer output; `SampleSequence::Rgba16BitLe as u32` for a developed
    /// RGBA16 output), matching `CrmGetImageAsync`'s untyped parameter.
    ///
    /// # Safety
    /// - `in_process_ref` is a live ref from [`create_process_ref`](Self::create_process_ref)
    ///   on this library.
    /// - `src_ptr` (`src_size` bytes) and `header_ptr` (`header_size` bytes)
    ///   must point into the live allocations owned by `src_keep` / `hdr_keep`.
    /// - `out_dst_buffer` (`in_dst_buffer_size` bytes) meets
    ///   [`get_image`](Self::get_image)'s destination contract and stays valid
    ///   until the future has completed or been dropped — dropping it waits
    ///   for the job. The caller owns it (e.g. a pipeline output-slot
    ///   keepalive dropped after the future).
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn get_image_future_owned(
        self: &Arc<Self>,
        in_process_ref: ProcessRef,
        out_dst_buffer: *mut CrmVoid,
        in_dst_buffer_size: CrmUInt32,
        in_bits_per_sample: CrmUInt32,
        in_samples_per_pixel: CrmUInt32,
        in_bytes_per_line: CrmUInt32,
        in_sample_sequence: CrmUInt32,
        src_ptr: *const u8,
        src_size: CrmUInt32,
        in_dev_param: Box<DevParamV13>,
        in_res_param: Box<ResParamV13>,
        header_ptr: *const u8,
        header_size: CrmUInt32,
        src_keep: Box<dyn core::any::Any + Send>,
        hdr_keep: Box<dyn core::any::Any + Send>,
    ) -> Result<OwnedGetImageFuture> {
        let mut out_dst_data_size = Box::new(0u32);
        let state = unsafe {
            self.submit_get_image_async(
                in_process_ref,
                out_dst_buffer,
                in_dst_buffer_size,
                &mut *out_dst_data_size as *mut CrmUInt32,
                in_bits_per_sample,
                in_samples_per_pixel,
                in_bytes_per_line,
                in_sample_sequence,
                src_ptr as *mut CrmVoid,
                src_size,
                &*in_dev_param as *const _ as *mut CrmVoid,
                &*in_res_param as *const _ as *mut CrmVoid,
                header_ptr as *mut CrmVoid,
                header_size,
            )?
        };

        Ok(OwnedGetImageFuture {
            state,
            _sdk: self.clone(),
            _out_dst_data_size: out_dst_data_size,
            _dev_param: in_dev_param,
            _res_param: in_res_param,
            _src_keep: src_keep,
            _hdr_keep: hdr_keep,
        })
    }

    /// [`get_image_future`](Self::get_image_future), awaited.
    ///
    /// # Safety
    ///
    /// As for [`get_image_future`](Self::get_image_future): the returned
    /// future must not be leaked.
    #[allow(clippy::too_many_arguments)]
    pub async unsafe fn get_image_async(
        &self,
        in_process_ref: ProcessRef,
        out_dst_buffer: *mut CrmVoid,
        in_dst_buffer_size: CrmUInt32,
        in_bits_per_sample: CrmUInt32,
        in_samples_per_pixel: CrmUInt32,
        in_bytes_per_line: CrmUInt32,
        in_sample_sequence: SampleSequence,
        in_src_buffer: &[u8],
        in_dev_param: &DevParamV13,
        in_res_param: &ResParamV13,
        in_header_buffer: &[u8],
    ) -> Result<GetImageAsyncOutput> {
        unsafe {
            self.get_image_future(
                in_process_ref,
                out_dst_buffer,
                in_dst_buffer_size,
                in_bits_per_sample,
                in_samples_per_pixel,
                in_bytes_per_line,
                in_sample_sequence,
                in_src_buffer,
                in_dev_param,
                in_res_param,
                in_header_buffer,
            )
        }?
        .await
    }

    /// `CrmEnqueueImage`: submit a frame to the SDK's development queue
    /// (spec § 7.3.2). The queue is CPU-output only.
    ///
    /// # Safety
    ///
    /// * `in_process_ref` is a live ref from [`create_process_ref`](Self::create_process_ref)
    ///   on this library.
    /// * `in_src_buffer` points to `in_src_data_size` bytes allocated with
    ///   [`allocate`](Self::allocate) — a plain Rust allocation faults the
    ///   SDK's worker — and stays valid and unmodified until
    ///   [`dequeue_image`](Self::dequeue_image) has handed back this
    ///   submission's `in_ref_con`: the SDK keeps the pointer, not a copy.
    /// * `in_header_buffer` points to `in_header_data_size` readable bytes for
    ///   the duration of the call.
    /// * A null `in_src_buffer` with size `0` is the end-of-stream marker.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn enqueue_image(
        &self,
        in_process_ref: ProcessRef,
        in_bits_per_sample: CrmUInt32,
        in_samples_per_pixel: CrmUInt32,
        in_bytes_per_line: CrmUInt32,
        in_sample_sequence: CrmUInt32,
        in_src_buffer: *mut CrmVoid,
        in_src_data_size: CrmUInt32,
        in_dev_param: &DevParamV13,
        in_res_param: &ResParamV13,
        in_ref_con: *mut CrmVoid,
        in_header_buffer: *mut CrmVoid,
        in_header_data_size: CrmUInt32,
    ) -> Result<()> {
        cuda_exclusion::exclusive_if_cuda(in_process_ref.device(), || {
            sdk_result(unsafe {
                (self.CrmEnqueueImage)(
                    in_process_ref.as_raw(),
                    in_bits_per_sample,
                    in_samples_per_pixel,
                    in_bytes_per_line,
                    in_sample_sequence,
                    in_src_buffer,
                    in_src_data_size,
                    in_dev_param as *const _ as *mut CrmVoid,
                    in_res_param as *const _ as *mut CrmVoid,
                    in_ref_con,
                    in_header_buffer,
                    in_header_data_size,
                )
            })
        })
    }

    /// `CrmDequeueImage`: collect a developed frame from the queue. Returns
    /// the bytes written and the submission's `in_ref_con`.
    ///
    /// # Safety
    ///
    /// `in_process_ref` is a live ref from [`create_process_ref`](Self::create_process_ref)
    /// on this library, and `out_dst_buffer` is a CPU buffer valid for writes
    /// of `in_dst_buffer_size` bytes, at least the size one developed frame
    /// takes.
    #[inline]
    pub unsafe fn dequeue_image(
        &self,
        in_process_ref: ProcessRef,
        out_dst_buffer: *mut CrmVoid,
        in_dst_buffer_size: CrmUInt32,
    ) -> Result<(CrmUInt32, *mut CrmVoid)> {
        let mut out_dst_data_size = 0;
        let mut out_ref_con: *mut CrmVoid = core::ptr::null_mut();
        cuda_exclusion::exclusive_if_cuda(in_process_ref.device(), || {
            sdk_result(unsafe {
                (self.CrmDequeueImage)(
                    in_process_ref.as_raw(),
                    out_dst_buffer,
                    in_dst_buffer_size,
                    &mut out_dst_data_size,
                    &mut out_ref_con,
                )
            })
        })?;
        Ok((out_dst_data_size, out_ref_con))
    }

    /* ── System and process information ──────────────────────────────────────────────────────── */

    #[inline]
    pub fn get_gpu_system_info(&self) -> Result<(GpuSystemInfo, GpuSystemInfo)> {
        let mut out_cuda_gpu_system_info = GpuSystemInfo::default();
        let mut out_cl_gpu_system_info = GpuSystemInfo::default();
        sdk_result(unsafe { (self.CrmGetGpuSystemInfo)(&mut out_cuda_gpu_system_info, &mut out_cl_gpu_system_info) })?;
        Ok((out_cuda_gpu_system_info, out_cl_gpu_system_info))
    }

    /// The CUDA and OpenCL device lists (the deprecated `CrmGetGpuInfoList`;
    /// prefer [`get_gpu_info_list_with_device`](Self::get_gpu_info_list_with_device)).
    #[inline]
    pub fn get_gpu_info_list(&self) -> Result<(Vec<GpuInfo>, Vec<GpuInfo>)> {
        let (cuda_sys, cl_sys) = self.get_gpu_system_info()?;
        // The SDK writes one entry per device it reported, with no size
        // argument, so each list is sized from that count.
        let mut cuda = vec![GpuInfo::ZEROED; (cuda_sys.device_num as usize).max(1)];
        let mut cl = vec![GpuInfo::ZEROED; (cl_sys.device_num as usize).max(1)];
        sdk_result(unsafe { (self.CrmGetGpuInfoList)(cuda.as_mut_ptr(), cl.as_mut_ptr()) })?;
        cuda.truncate(cuda_sys.device_num as usize);
        cl.truncate(cl_sys.device_num as usize);
        Ok((cuda, cl))
    }

    #[inline]
    pub fn get_sdk_info(&self) -> Result<(Version, CrmUInt32, CrmUInt32)> {
        let mut out_sdk_version = Version::default();
        let mut out_pipeline_num = 0;
        let mut out_usable_gpu_num = 0;
        sdk_result(unsafe {
            (self.CrmGetSdkInfo)(&mut out_sdk_version, &mut out_pipeline_num, &mut out_usable_gpu_num)
        })?;
        Ok((out_sdk_version, out_pipeline_num, out_usable_gpu_num))
    }

    /// The white-balance gain the last click-WB develop applied (spec § 7.4.6).
    ///
    /// # Safety
    ///
    /// `in_process_ref` is a live ref from [`create_process_ref`](Self::create_process_ref)
    /// on this library.
    #[inline]
    pub unsafe fn get_wb_gain(&self, in_process_ref: ProcessRef) -> Result<WbGain> {
        let mut out_wb_gain = WbGain::default();
        sdk_result(unsafe { (self.CrmGetWBGain)(in_process_ref.as_raw(), &mut out_wb_gain) })?;
        Ok(out_wb_gain)
    }

    /// The ACES IDT the last develop applied (spec § 7.4.7).
    ///
    /// # Safety
    ///
    /// `in_process_ref` is a live ref from [`create_process_ref`](Self::create_process_ref)
    /// on this library.
    #[inline]
    pub unsafe fn get_aces_idt(&self, in_process_ref: ProcessRef) -> Result<AcesIdtInfo> {
        // `outAcesIDTStr` takes no size. CRMSDK 2.10 writes a short
        // NUL-terminated name ("D55 Ver. 2.0" is 13 bytes); the buffer leaves
        // generous room, and the zeroing bounds the read either way.
        let mut raw: CrmUInt32 = 0;
        let mut name = [0u8; 1024];
        sdk_result(unsafe { (self.CrmGetAcesIDT)(in_process_ref.as_raw(), &mut raw, name.as_mut_ptr().cast()) })?;
        Ok(AcesIdtInfo { raw, name: fixed_ascii(&name) })
    }

    #[inline]
    pub fn get_sdk_info_with_device(
        &self,
        in_use_device: Device,
        in_process_type: ProcessType,
    ) -> Result<(Version, CrmUInt32, CrmUInt32)> {
        let mut out_sdk_version = Version::default();
        let mut out_pipeline_num = 0;
        let mut out_usable_gpu_num = 0;
        sdk_result(unsafe {
            (self.CrmGetSdkInfoWithDevice)(
                &mut out_sdk_version,
                &mut out_pipeline_num,
                &mut out_usable_gpu_num,
                in_use_device,
                in_process_type,
            )
        })?;
        Ok((out_sdk_version, out_pipeline_num, out_usable_gpu_num))
    }

    #[inline]
    pub fn get_gpu_system_info_with_device(&self, in_use_device: Device) -> Result<GpuSystemInfo> {
        let mut out = GpuSystemInfo::default();
        sdk_result(unsafe { (self.CrmGetGpuSystemInfoWithDevice)(&mut out, in_use_device) })?;
        Ok(out)
    }

    #[inline]
    pub fn get_gpu_info_list_with_device(&self, in_use_device: Device) -> Result<Vec<GpuInfo>> {
        let count = self.get_gpu_system_info_with_device(in_use_device)?.device_num as usize;
        // One entry per device the SDK just reported — it writes exactly that
        // many and takes no size argument.
        let mut out = vec![GpuInfo::ZEROED; count.max(1)];
        sdk_result(unsafe { (self.CrmGetGpuInfoListWithDevice)(out.as_mut_ptr(), in_use_device) })?;
        out.truncate(count);
        Ok(out)
    }

    /* ── Intel GPU development (spec § 7.2.8 – 7.2.13: not supported by CRMSDK 2.10) ───────────── */

    /// # Safety
    ///
    /// `in_process_ref` is a live ref from [`create_process_ref`](Self::create_process_ref)
    /// on this library, and `in_device_handle` a live device manager of the
    /// kind `in_mem_type` names.
    #[cfg(target_os = "windows")]
    #[inline]
    pub unsafe fn set_device_handle_for_intel(
        &self,
        in_process_ref: ProcessRef,
        in_device_handle: DeviceHandle,
        in_mem_type: MemType,
    ) -> Result<()> {
        sdk_result(unsafe { (self.CrmSetDeviceHandleForIntel)(in_process_ref.as_raw(), in_device_handle, in_mem_type) })
    }

    /// # Safety
    ///
    /// `in_process_ref` is a live ref from [`create_process_ref`](Self::create_process_ref)
    /// on this library.
    #[cfg(target_os = "windows")]
    #[inline]
    pub unsafe fn set_image_param_for_intel(
        &self,
        in_process_ref: ProcessRef,
        in_resolution: Size,
        in_crop: Rect,
    ) -> Result<()> {
        sdk_result(unsafe { (self.CrmSetImageParamForIntel)(in_process_ref.as_raw(), in_resolution, in_crop) })
    }

    /// # Safety
    ///
    /// `in_process_ref` is a live ref from [`create_process_ref`](Self::create_process_ref)
    /// on this library.
    #[cfg(target_os = "windows")]
    #[inline]
    pub unsafe fn allocate_d3d_surfaces_for_intel(
        &self,
        in_process_ref: ProcessRef,
    ) -> Result<[MemId; SURFACE_NUM as usize]> {
        let mut out_mem_id_array = [core::ptr::null_mut(); SURFACE_NUM as usize];
        sdk_result(unsafe {
            (self.CrmAllocateD3DSurfacesForIntel)(in_process_ref.as_raw(), out_mem_id_array.as_mut_ptr())
        })?;
        Ok(out_mem_id_array)
    }

    /// # Safety
    ///
    /// `in_process_ref` is the live ref `in_mem_id_array` was allocated
    /// against with [`allocate_d3d_surfaces_for_intel`](Self::allocate_d3d_surfaces_for_intel),
    /// and the surfaces are not freed already.
    #[cfg(target_os = "windows")]
    #[inline]
    pub unsafe fn deallocate_d3d_surfaces_for_intel(
        &self,
        in_process_ref: ProcessRef,
        in_mem_id_array: &[MemId; SURFACE_NUM as usize],
    ) -> Result<()> {
        sdk_result(unsafe {
            (self.CrmDeallocateD3DSurfacesForIntel)(in_process_ref.as_raw(), in_mem_id_array.as_ptr() as MemId)
        })
    }

    /// # Safety
    ///
    /// `in_process_ref` is a live ref from [`create_process_ref`](Self::create_process_ref)
    /// on this library.
    #[cfg(target_os = "windows")]
    #[inline]
    pub unsafe fn create_device_handle_for_intel(
        &self,
        in_process_ref: ProcessRef,
        in_mem_type: MemType,
    ) -> Result<DeviceHandle> {
        let mut out_device_handle: DeviceHandle = core::ptr::null_mut();
        sdk_result(unsafe {
            (self.CrmCreateDeviceHandleForIntel)(in_process_ref.as_raw(), &mut out_device_handle, in_mem_type)
        })?;
        Ok(out_device_handle)
    }

    /// # Safety
    ///
    /// `in_process_ref` is a live ref from [`create_process_ref`](Self::create_process_ref)
    /// on this library.
    #[cfg(target_os = "windows")]
    #[inline]
    pub unsafe fn cleanup_device_handle_for_intel(&self, in_process_ref: ProcessRef) -> Result<()> {
        sdk_result(unsafe { (self.CrmCleanupDeviceHandleForIntel)(in_process_ref.as_raw()) })
    }

    /* ── Deprecated GPU-memory path (spec § 7.3.4 – 7.3.5) ─────────────────────────────────────── */

    /// Pre-process step before [`get_image_with_gpu_memory`](Self::get_image_with_gpu_memory)
    /// (deprecated in the spec, OpenCL only). Returns the size in bytes of the
    /// GPU output buffer the caller must allocate.
    ///
    /// # Safety
    ///
    /// `in_process_ref` is a live ref from [`create_process_ref`](Self::create_process_ref)
    /// on this library.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn pre_process_for_get_image_with_gpu_memory(
        &self,
        in_process_ref: ProcessRef,
        in_bits_per_sample: CrmUInt32,
        in_samples_per_pixel: CrmUInt32,
        in_bytes_per_line: CrmUInt32,
        in_sample_sequence: SampleSequence,
        in_src_buffer: &[u8],
        in_dev_param: &DevParamV13,
        in_res_param: &ResParamV13,
    ) -> Result<CrmUInt32> {
        let mut out_required_buffer_size = 0;
        sdk_result(unsafe {
            (self.CrmPreProcessForGetImageWithGpuMemory)(
                in_process_ref.as_raw(),
                in_bits_per_sample,
                in_samples_per_pixel,
                in_bytes_per_line,
                in_sample_sequence as CrmUInt32,
                in_src_buffer.as_ptr() as *mut CrmVoid,
                in_src_buffer.len() as CrmUInt32,
                in_dev_param as *const _ as *mut CrmVoid,
                in_res_param as *const _ as *mut CrmVoid,
                &mut out_required_buffer_size,
            )
        })?;
        Ok(out_required_buffer_size)
    }

    /// # Safety
    ///
    /// `in_process_ref` is a live ref prepared by
    /// [`pre_process_for_get_image_with_gpu_memory`](Self::pre_process_for_get_image_with_gpu_memory);
    /// `in_src_buffer` and `out_dst_buffer` are OpenCL buffers of at least
    /// `in_src_data_size` and the pre-processed size respectively, valid until
    /// the SDK's queue has been finished.
    #[inline]
    pub unsafe fn get_image_with_gpu_memory(
        &self,
        in_process_ref: ProcessRef,
        out_dst_buffer: *mut CrmVoid,
        in_dst_buffer_size: CrmUInt32,
        in_src_buffer: *mut CrmVoid,
        in_src_data_size: CrmUInt32,
    ) -> Result<CrmUInt32> {
        let mut out_dst_data_size = 0;
        sdk_result(unsafe {
            (self.CrmGetImageWithGpuMemory)(
                in_process_ref.as_raw(),
                out_dst_buffer,
                in_dst_buffer_size,
                &mut out_dst_data_size,
                in_src_buffer,
                in_src_data_size,
            )
        })?;
        Ok(out_dst_data_size)
    }

    /* ── Utilities ────────────────────────────────────────────────────────────────────────────── */

    /// The SDK's detail message for the last error (`CrmGetLastError`).
    pub fn get_last_error_string(&self) -> Result<String> {
        let mut buf = vec![0u8; 8192];
        sdk_result(unsafe { (self.CrmGetLastError)(buf.as_mut_ptr().cast(), buf.len() as CrmUInt32) })?;
        Ok(fixed_ascii(&buf))
    }

    #[inline]
    pub fn get_out_image_size(&self, in_res_param: &ResParamV13, header_buffer: &[u8]) -> Result<(u32, u32)> {
        let mut out_image_width = 0;
        let mut out_image_height = 0;
        sdk_result(unsafe {
            (self.CrmGetOutImageSize)(
                in_res_param as *const _ as *mut _,
                &mut out_image_width,
                &mut out_image_height,
                header_buffer.as_ptr() as *mut _,
                header_buffer.len() as CrmUInt32,
            )
        })?;
        Ok((out_image_width, out_image_height))
    }

    /// Convenience wrapper around [`create_header_pack_into`](Self::create_header_pack_into);
    /// allocates a fresh `Vec<u8>`. For decode loops that rebuild the
    /// header pack per frame, prefer `_into` against an owned scratch
    /// buffer — repeated calls steady into one growth then stay.
    #[inline]
    pub fn create_header_pack(&self, header_array: &[Header<'_>]) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(64 * 1024);
        self.create_header_pack_into(header_array, &mut buf)?;
        Ok(buf)
    }

    /// Build the SDK header pack for `header_array` into `dst`.
    ///
    /// Starts with a 64 KB working buffer (real header packs are a few
    /// kilobytes, far below the SDK's `MAX_FILE_SIZE` of 28 MB) and
    /// retries with the SDK-reported size if the SDK reports the initial
    /// buffer is too small. On success `dst` is truncated to the exact
    /// output size.
    pub fn create_header_pack_into(&self, header_array: &[Header<'_>], dst: &mut Vec<u8>) -> Result<()> {
        if dst.capacity() == 0 {
            dst.reserve(64 * 1024);
        }
        dst.clear();
        dst.resize(dst.capacity(), 0);

        let mut out_size: CrmUInt32 = 0;
        let first = unsafe {
            (self.CrmCreateHeaderPack)(
                header_array.as_ptr(),
                header_array.len() as CrmUInt32,
                dst.as_mut_ptr() as *mut CrmVoid,
                dst.len() as CrmUInt32,
                &mut out_size,
            )
        };

        if first != CRM_ERR_OK && (out_size as usize) > dst.len() {
            // SDK reported a larger required size — grow and retry once.
            dst.clear();
            dst.resize(out_size as usize, 0);
            sdk_result(unsafe {
                (self.CrmCreateHeaderPack)(
                    header_array.as_ptr(),
                    header_array.len() as CrmUInt32,
                    dst.as_mut_ptr() as *mut CrmVoid,
                    dst.len() as CrmUInt32,
                    &mut out_size,
                )
            })?;
        } else {
            sdk_result(first)?;
        }
        dst.truncate(out_size as usize);
        Ok(())
    }

    /// Point the SDK at the directory that **contains** the `CanonLUTs`
    /// folder — not at `CanonLUTs` itself.
    ///
    /// `CrmSetLUTsDirectory` appends the folder name for you: inside
    /// `CrmSdk.dll` the argument is only ever consumed as
    /// `sprintf(buf, "%s\CanonLUTs", path)`, and each LUT is then opened as
    /// `<buf>/<name>.bin`. That is what the spec's wording for
    /// `inDirectoryPath` means (§ 7.2.18: *"a path of the directory where the
    /// CanonLUTs folder is stored"*), so:
    ///
    /// | call | where the SDK looks |
    /// |---|---|
    /// | never called | `<dir of CrmSdk.dll>/CanonLUTs`, loaded during `CrmInitializeSDK*` |
    /// | `is_absolute_path == false` | `<dir of CrmSdk.dll>/<path>/CanonLUTs` |
    /// | `is_absolute_path == true` | `<path>/CanonLUTs` |
    ///
    /// # A wrong directory is a SILENT no-op
    ///
    /// The SDK does not validate `path`. Pointing it at a directory with no
    /// `CanonLUTs` in it — or at a path that does not exist at all — still
    /// returns `CRM_ERR_OK`, and leaves whatever LUTs were loaded before
    /// (normally the `CrmInitializeSDK*` default) in force. An `Ok` here is
    /// therefore NOT evidence that the LUTs were found; a caller that needs
    /// them must check `path.join("CanonLUTs")` itself. The mistake surfaces
    /// only later, on the first develop whose recipe needs a 3-D LUT, as
    /// [`SdkError::InvalidParameter`] plus `"The necessary LUT is not
    /// available."` from [`get_last_error_string`](Self::get_last_error_string).
    ///
    /// Ordering: after SDK initialization, before the first
    /// [`create_process_ref`](Self::create_process_ref) (spec § 7.2.18).
    #[inline]
    pub fn set_luts_directory(&self, path: impl AsRef<Path>, is_absolute_path: bool) -> Result<()> {
        let c_path = sdk_path_cstring(path.as_ref())?;
        sdk_result(unsafe {
            (self.CrmSetLUTsDirectory)(c_path.as_ptr() as *const CrmChar, is_absolute_path as CrmBool)
        })
    }
}

impl Drop for CrmSdkLibrary {
    fn drop(&mut self) {
        let _ = unsafe { (self.CrmTerminateSDK)() };
    }
}

/// Owned device/CPU memory allocation from the CRM SDK. The `Arc<CrmSdkLibrary>`
/// keeps the SDK deallocator symbol alive for the life of the allocation —
/// no lifetime parameter, so the value moves cleanly into long-lived carriers
/// such as a frame handle that outlives the decode call.
pub struct CrmMemory {
    ptr: *mut CrmVoid,
    device: Device,
    size: CrmUInt32,
    sdk: Arc<CrmSdkLibrary>,
}
// CrmMemory — Send only. Plain device (or aligned CPU) memory; the
// `Arc<CrmSdkLibrary>` keeps the deallocator symbol alive for the life of
// the allocation. Single-owner GPU/CPU buffer → not `Sync`.
unsafe impl Send for CrmMemory {}

impl std::fmt::Debug for CrmMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrmMemory")
            .field("ptr", &self.ptr)
            .field("device", &self.device)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl CrmMemory {
    pub fn new(sdk: Arc<CrmSdkLibrary>, size: CrmUInt32, device: Device) -> Result<Self> {
        let ptr = sdk.allocate(size, device)?;
        Ok(Self { ptr, device, size, sdk })
    }

    pub fn as_ptr(&self) -> *mut CrmVoid {
        self.ptr
    }

    pub fn as_mut_ptr(&mut self) -> *mut CrmVoid {
        self.ptr
    }

    pub fn len(&self) -> usize {
        self.size as usize
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    pub fn device(&self) -> Device {
        self.device
    }
}
impl Drop for CrmMemory {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from `allocate` with `device` on this library and
        // is freed only here. A failure has nowhere to go from `Drop`.
        let _ = unsafe { self.sdk.deallocate(self.ptr, self.device) };
    }
}
