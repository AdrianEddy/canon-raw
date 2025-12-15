// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
#![allow(dead_code)]
#![doc = include_str!("../README.md")]

use core::ffi::{c_char, c_float, c_int, c_short, c_schar, c_uchar, c_uint, c_ulonglong, c_ushort, c_void};

pub mod crm_parser;
pub mod error;
pub use error::*;

#[cfg(target_os = "windows")]
use libloading::os::windows as dl;
#[cfg(not(target_os = "windows"))]
use libloading::os::unix as dl;

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


pub const MAX_FILE_SIZE: u32            = 28318048;
pub const RMF_4KDCI_FILE_SIZE: u32      = 1201840;
pub const RMF_QFHD_FILE_SIZE: u32       = 1127497;
pub const RMF_HALF_4KDCI_FILE_SIZE: u32 = 6047456;
pub const RMF_HALF_QFHD_FILE_SIZE: u32  = 5675404;
pub const SURFACE_NUM: u32              = 6;

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

#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FormatId {
    MovieType1 = 0x0001_0001,
    MovieType2 = 0x0001_0002,
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
    #[repr(C)]
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
#[derive(Copy, Clone)]
pub struct Umid32 {
    pub basic_umid: [CrmUInt8; 32],
}

#[repr(C)]
#[derive(Copy, Clone)]
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
#[derive(Copy, Clone, Debug, Default)]
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

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct GpuSystemInfo {
    pub device_num: CrmUInt32,
    pub driver_version: Version,
    pub runtime_version: Version,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct GpuInfo {
    pub device_name: [CrmASCII; 256],
    pub support: CrmBool,
    pub compute_capability: Version,
    pub cl_support_type: ClSupportType,
}
impl GpuInfo {
    pub fn device_name(&self) -> String {
        unsafe {
            let c_str = std::ffi::CStr::from_ptr(self.device_name.as_ptr());
            c_str.to_string_lossy().into_owned()
        }
    }
}
impl std::fmt::Debug for GpuInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuInfo")
            .field("device_name", &self.device_name())
            .field("support", &self.support)
            .field("compute_capability", &self.compute_capability)
            .field("cl_support_type", &self.cl_support_type)
            .finish()
    }
}

// handles
pub type ProcessRef = *mut CrmVoid;
pub type DeviceHandle = *mut CrmVoid;
pub type MemId = *mut CrmVoid;

#[repr(C)]
#[derive(Default, Copy, Clone, Debug)]
pub struct Header {
    pub header_id: HeaderId,
    pub data: *mut CrmVoid,
    pub size: CrmUInt32,
}

/* -------------------------------------------------------------------------------------------------
 * Callback
 * ------------------------------------------------------------------------------------------------- */

// No __stdcall specified in header typedef -> treat as C (cdecl) callback.
pub type GetImageCallback = Option<
    unsafe extern "system" fn(
        in_process_ref: ProcessRef,
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
        in_error: SdkError,
    ) -> SdkError,
>;

/* -------------------------------------------------------------------------------------------------
 * Exported function pointer types (libloading-friendly)
 * ------------------------------------------------------------------------------------------------- */

pub type CrmInitializeSDKFn = unsafe extern "system" fn() -> SdkError;
pub type CrmInitializeSDKWithOpenCLFn = unsafe extern "system" fn(cl_object: ClObjectRef) -> SdkError;
pub type CrmInitializeSDKWithDeviceFn = unsafe extern "system" fn(in_use_device: Device, in_gpu_param: *mut CrmVoid) -> SdkError;
pub type CrmTerminateSDKFn = unsafe extern "system" fn() -> SdkError;

pub type CrmGetFormatIdFn = unsafe extern "system" fn(
    in_buffer: *mut CrmVoid,
    in_data_size: CrmUInt32,
    out_format_id: *mut FormatId,
) -> SdkError;

pub type CrmCreateProcessRefFn = unsafe extern "system" fn(
    out_process_ref: *mut ProcessRef,
    in_format_id: FormatId,
    in_use_device: Device,
    in_use_gpu: Gpu,
    in_process_type: ProcessType,
    in_max_resolution: Resolution,
) -> SdkError;

pub type CrmResetProcessRefFn = unsafe extern "system" fn(in_process_ref: ProcessRef) -> SdkError;
pub type CrmReleaseProcessRefFn = unsafe extern "system" fn(in_process_ref: ProcessRef) -> SdkError;

pub type CrmAllocateFn = unsafe extern "system" fn(out_buffer: *mut *mut CrmVoid, in_buffer_size: CrmUInt32, in_use_device: Device) -> SdkError;
pub type CrmDeallocateFn = unsafe extern "system" fn(in_buffer: *mut CrmVoid, in_use_device: Device) -> SdkError;

pub type CrmDeviceAllocateFn = unsafe extern "system" fn(
    in_process_ref: ProcessRef,
    out_buffer: *mut *mut CrmVoid,
    in_buffer_size: CrmUInt32,
    in_buffer_type: BufferType,
    in_use_gpu: Gpu,
) -> SdkError;

pub type CrmDeviceDeallocateFn = unsafe extern "system" fn(in_process_ref: ProcessRef, in_buffer: *mut CrmVoid) -> SdkError;

pub type CrmGetImageFn = unsafe extern "system" fn(
    in_process_ref: ProcessRef,
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
) -> SdkError;

pub type CrmGetImageAsyncFn = unsafe extern "system" fn(
    in_process_ref: ProcessRef,
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
) -> SdkError;

pub type CrmEnqueueImageFn = unsafe extern "system" fn(
    in_process_ref: ProcessRef,
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
) -> SdkError;

pub type CrmDequeueImageFn = unsafe extern "system" fn(
    in_process_ref: ProcessRef,
    out_dst_buffer: *mut CrmVoid,
    in_dst_buffer_size: CrmUInt32,
    out_dst_data_size: *mut CrmUInt32,
    out_ref_con: *mut *mut CrmVoid,
) -> SdkError;

pub type CrmGetImageInfoFn = unsafe extern "system" fn(
    in_format_id: CrmUInt32,
    in_buffer: *mut CrmVoid,
    in_data_size: CrmUInt32,
    in_image_info_id: ImageInfoId,
    out_image_info: *mut CrmVoid,
) -> SdkError;

pub type CrmGetGpuSystemInfoFn = unsafe extern "system" fn(
    out_cuda_gpu_system_info: *mut GpuSystemInfo,
    out_cl_gpu_system_info: *mut GpuSystemInfo,
) -> SdkError;

pub type CrmGetGpuInfoListFn = unsafe extern "system" fn(out_cuda_gpu_info: *mut GpuInfo, out_cl_gpu_info: *mut GpuInfo) -> SdkError;

pub type CrmGetSdkInfoFn = unsafe extern "system" fn(
    out_sdk_version: *mut Version,
    out_pipeline_num: *mut CrmUInt32,
    out_usable_gpu_num: *mut CrmUInt32,
) -> SdkError;

pub type CrmGetWbGainFn = unsafe extern "system" fn(in_process_ref: ProcessRef, out_wb_gain: *mut WbGain) -> SdkError;

pub type CrmGetAcesIdtFn = unsafe extern "system" fn(
    in_process_ref: ProcessRef,
    out_aces_idt: *mut AcesIdt,
    out_aces_idt_str: *mut CrmASCII,
) -> SdkError;

pub type CrmGetSdkInfoWithDeviceFn = unsafe extern "system" fn(
    out_sdk_version: *mut Version,
    out_pipeline_num: *mut CrmUInt32,
    out_usable_gpu_num: *mut CrmUInt32,
    in_use_device: Device,
    in_process_type: ProcessType,
) -> SdkError;

pub type CrmGetGpuSystemInfoWithDeviceFn = unsafe extern "system" fn(out_gpu_system_info: *mut GpuSystemInfo, in_use_device: Device) -> SdkError;
pub type CrmGetGpuInfoListWithDeviceFn = unsafe extern "system" fn(out_gpu_info: *mut GpuInfo, in_use_device: Device) -> SdkError;

#[cfg(target_os = "windows")]
pub type CrmSetDeviceHandleForIntelFn = unsafe extern "system" fn(
    in_process_ref: ProcessRef,
    in_device_handle: DeviceHandle,
    in_mem_type: MemType,
) -> SdkError;

#[cfg(target_os = "windows")]
pub type CrmSetImageParamForIntelFn = unsafe extern "system" fn(in_process_ref: ProcessRef, in_resolution: Size, in_crop: Rect) -> SdkError;

#[cfg(target_os = "windows")]
pub type CrmAllocateD3dSurfacesForIntelFn = unsafe extern "system" fn(in_process_ref: ProcessRef, out_mem_id_array: *mut MemId) -> SdkError;

#[cfg(target_os = "windows")]
pub type CrmDeallocateD3dSurfacesForIntelFn = unsafe extern "system" fn(in_process_ref: ProcessRef, in_mem_id_array: MemId) -> SdkError;

#[cfg(target_os = "windows")]
pub type CrmCreateDeviceHandleForIntelFn = unsafe extern "system" fn(
    in_process_ref: ProcessRef,
    out_device_handle: *mut DeviceHandle,
    in_mem_type: MemType,
) -> SdkError;

#[cfg(target_os = "windows")]
pub type CrmCleanupDeviceHandleForIntelFn = unsafe extern "system" fn(in_process_ref: ProcessRef) -> SdkError;

pub type CrmPreProcessForGetImageWithGpuMemoryFn = unsafe extern "system" fn(
    in_process_ref: ProcessRef,
    in_bits_per_sample: CrmUInt32,
    in_samples_per_pixel: CrmUInt32,
    in_bytes_per_line: CrmUInt32,
    in_sample_sequence: CrmUInt32,
    in_src_buffer: *mut CrmVoid,
    in_src_data_size: CrmUInt32,
    in_dev_param: *mut CrmVoid,
    in_res_param: *mut CrmVoid,
    out_required_buffer_size: *mut CrmUInt32,
) -> SdkError;

pub type CrmGetImageWithGpuMemoryFn = unsafe extern "system" fn(
    in_process_ref: ProcessRef,
    out_dst_buffer: *mut CrmVoid,
    in_dst_buffer_size: CrmUInt32,
    out_dst_data_size: *mut CrmUInt32,
    in_src_buffer: *mut CrmVoid,
    in_src_data_size: CrmUInt32,
) -> SdkError;

pub type CrmGetLastErrorFn = unsafe extern "system" fn(out_error_msg: *mut CrmChar, in_buffer_size: CrmUInt32) -> SdkError;

pub type CrmGetOutImageSizeFn = unsafe extern "system" fn(
    in_res_param: *mut CrmVoid,
    out_image_width: *mut CrmUInt32,
    out_image_height: *mut CrmUInt32,
    in_header_buffer: *mut CrmVoid,
    in_header_size: CrmUInt32,
) -> SdkError;

pub type CrmCreateHeaderPackFn = unsafe extern "system" fn(
    in_header_array: *mut Header,
    in_header_num: CrmUInt32,
    out_dst_buffer: *mut CrmVoid,
    in_dst_buffer_size: CrmUInt32,
    out_dst_data_size: *mut CrmUInt32,
) -> SdkError;

pub type CrmSetLutsDirectoryFn = unsafe extern "system" fn(in_directory_path: *const CrmChar, in_is_absolute_path: CrmBool) -> SdkError;

/* -------------------------------------------------------------------------------------------------
 * libloading wrapper
 * ------------------------------------------------------------------------------------------------- */

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
    pub CrmGetImageAsync: dl::Symbol<CrmGetImageAsyncFn>,
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

    // must be last
    lib: dl::Library,
}

impl CrmSdkLibrary {
    pub fn load<P: libloading::AsFilename>(path: P) -> std::result::Result<Self, libloading::Error> {
        unsafe {
            let lib = dl::Library::new(path)?;
            Ok(Self {
                CrmInitializeSDK: lib.get(b"CrmInitializeSDK\0").unwrap(),
                CrmInitializeSDKWithOpenCL: lib.get(b"CrmInitializeSDKWithOpenCL\0").unwrap(),
                CrmInitializeSDKWithDevice: lib.get(b"CrmInitializeSDKWithDevice\0").unwrap(),
                CrmTerminateSDK: lib.get(b"CrmTerminateSDK\0").unwrap(),

                CrmGetFormatId: lib.get(b"CrmGetFormatId\0").unwrap(),
                CrmCreateProcessRef: lib.get(b"CrmCreateProcessRef\0").unwrap(),
                CrmResetProcessRef: lib.get(b"CrmResetProcessRef\0").unwrap(),
                CrmReleaseProcessRef: lib.get(b"CrmReleaseProcessRef\0").unwrap(),

                CrmAllocate: lib.get(b"CrmAllocate\0").unwrap(),
                CrmDeallocate: lib.get(b"CrmDeallocate\0").unwrap(),
                CrmDeviceAllocate: lib.get(b"CrmDeviceAllocate\0").unwrap(),
                CrmDeviceDeallocate: lib.get(b"CrmDeviceDeallocate\0").unwrap(),

                CrmGetImage: lib.get(b"CrmGetImage\0").unwrap(),
                CrmGetImageAsync: lib.get(b"?CrmGetImageAsync@@YA?AW4CrmError@@PEAX0IPEAIIIII0I00PEAPEAX0IP6A?AW41@00IIIIII0I0000IW41@@Z@Z\0").unwrap(),
                CrmEnqueueImage: lib.get(b"CrmEnqueueImage\0").unwrap(),
                CrmDequeueImage: lib.get(b"CrmDequeueImage\0").unwrap(),

                CrmGetImageInfo: lib.get(b"CrmGetImageInfo\0").unwrap(),
                CrmGetGpuSystemInfo: lib.get(b"CrmGetGpuSystemInfo\0").unwrap(),
                CrmGetGpuInfoList: lib.get(b"CrmGetGpuInfoList\0").unwrap(),
                CrmGetSdkInfo: lib.get(b"CrmGetSdkInfo\0").unwrap(),
                CrmGetWBGain: lib.get(b"CrmGetWBGain\0").unwrap(),
                CrmGetAcesIDT: lib.get(b"CrmGetAcesIDT\0").unwrap(),
                CrmGetSdkInfoWithDevice: lib.get(b"CrmGetSdkInfoWithDevice\0").unwrap(),
                CrmGetGpuSystemInfoWithDevice: lib.get(b"CrmGetGpuSystemInfoWithDevice\0").unwrap(),
                CrmGetGpuInfoListWithDevice: lib.get(b"CrmGetGpuInfoListWithDevice\0").unwrap(),

                #[cfg(target_os = "windows")]
                CrmSetDeviceHandleForIntel: lib.get(b"CrmSetDeviceHandleForIntel\0").unwrap(),
                #[cfg(target_os = "windows")]
                CrmSetImageParamForIntel: lib.get(b"CrmSetImageParamForIntel\0").unwrap(),
                #[cfg(target_os = "windows")]
                CrmAllocateD3DSurfacesForIntel: lib.get(b"CrmAllocateD3DSurfacesForIntel\0").unwrap(),
                #[cfg(target_os = "windows")]
                CrmDeallocateD3DSurfacesForIntel: lib.get(b"CrmDeallocateD3DSurfacesForIntel\0").unwrap(),
                #[cfg(target_os = "windows")]
                CrmCreateDeviceHandleForIntel: lib.get(b"CrmCreateDeviceHandleForIntel\0").unwrap(),
                #[cfg(target_os = "windows")]
                CrmCleanupDeviceHandleForIntel: lib.get(b"CrmCleanupDeviceHandleForIntel\0").unwrap(),

                CrmPreProcessForGetImageWithGpuMemory: lib.get(b"CrmPreProcessForGetImageWithGpuMemory\0").unwrap(),
                CrmGetImageWithGpuMemory: lib.get(b"CrmGetImageWithGpuMemory\0").unwrap(),

                CrmGetLastError: lib.get(b"CrmGetLastError\0").unwrap(),
                CrmGetOutImageSize: lib.get(b"CrmGetOutImageSize\0").unwrap(),
                CrmCreateHeaderPack: lib.get(b"CrmCreateHeaderPack\0").unwrap(),
                CrmSetLUTsDirectory: lib.get(b"CrmSetLUTsDirectory\0").unwrap(),

                lib,
            })
        }
    }

    #[inline]
    pub fn initialize_sdk(&self) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmInitializeSDK)() })
    }
    #[inline]
    pub fn initialize_sdk_with_opencl(&self, cl_object: ClObjectRef) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmInitializeSDKWithOpenCL)(cl_object) })
    }

    #[inline]
    pub fn initialize_sdk_with_device(&self, in_use_device: Device, in_gpu_param: *mut CrmVoid) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmInitializeSDKWithDevice)(in_use_device, in_gpu_param) })
    }

    #[inline]
    pub fn terminate_sdk(&self) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmTerminateSDK)() })
    }

    #[inline]
    pub fn get_format_id(&self, in_buffer: &[u8]) -> Result<FormatId> {
        let mut out_format_id = FormatId::MovieType1;
        let in_data_size = in_buffer.len() as CrmUInt32;
        self.err_to_result(unsafe { (self.CrmGetFormatId)(in_buffer.as_ptr() as *mut _, in_data_size, &mut out_format_id) })?;
        Ok(out_format_id)
    }

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
        self.err_to_result(unsafe {
            (self.CrmCreateProcessRef)(
                &mut out_process_ref,
                in_format_id,
                in_use_device,
                in_use_gpu,
                in_process_type,
                in_max_resolution,
            )
        })?;
        Ok(out_process_ref)
    }

    #[inline]
    pub fn reset_process_ref(&self, in_process_ref: ProcessRef) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmResetProcessRef)(in_process_ref) })
    }

    #[inline]
    pub fn release_process_ref(&self, in_process_ref: ProcessRef) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmReleaseProcessRef)(in_process_ref) })
    }

    #[inline]
    pub fn allocate(&self, in_buffer_size: CrmUInt32, in_use_device: Device) -> Result<*mut CrmVoid> {
        let mut out_buffer: *mut CrmVoid = std::ptr::null_mut();
        self.err_to_result(unsafe { (self.CrmAllocate)(&mut out_buffer, in_buffer_size, in_use_device) })?;
        Ok(out_buffer)
    }

    #[inline]
    pub fn deallocate(&self, in_buffer: *mut CrmVoid, in_use_device: Device) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmDeallocate)(in_buffer, in_use_device) })
    }

    #[inline]
    pub fn device_allocate(
        &self,
        in_process_ref: ProcessRef,
        in_buffer_size: CrmUInt32,
        in_buffer_type: BufferType,
        in_use_gpu: Gpu,
    ) -> Result<*mut CrmVoid> {
        let mut out_buffer: *mut CrmVoid = std::ptr::null_mut();
        self.err_to_result(unsafe {
            (self.CrmDeviceAllocate)(in_process_ref, &mut out_buffer, in_buffer_size, in_buffer_type, in_use_gpu)
        })?;
        Ok(out_buffer)
    }

    #[inline]
    pub fn device_deallocate(&self, in_process_ref: ProcessRef, in_buffer: *mut CrmVoid) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmDeviceDeallocate)(in_process_ref, in_buffer) })
    }

    #[inline]
    pub fn get_image(
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
        self.err_to_result(unsafe {
            (self.CrmGetImage)(
                in_process_ref,
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
        })?;
        Ok(out_dst_data_size)
    }

    #[inline]
    pub fn get_image_async(
        &self,
        in_process_ref: ProcessRef,
        out_dst_buffer: *mut CrmVoid,
        in_dst_buffer_size: CrmUInt32,
        out_dst_data_size: &mut CrmUInt32,
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
    ) -> Result<()> {
        self.err_to_result(unsafe {
            (self.CrmGetImageAsync)(
                in_process_ref,
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
        })
    }

    #[inline]
    pub fn enqueue_image(
        &self,
        in_process_ref: ProcessRef,
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
    ) -> Result<()> {
        self.err_to_result(unsafe {
            (self.CrmEnqueueImage)(
                in_process_ref,
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
            )
        })
    }

    #[inline]
    pub fn dequeue_image(
        &self,
        in_process_ref: ProcessRef,
        out_dst_buffer: *mut CrmVoid,
        in_dst_buffer_size: CrmUInt32,
        out_dst_data_size: &mut CrmUInt32,
        out_ref_con: &mut *mut CrmVoid,
    ) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmDequeueImage)(in_process_ref, out_dst_buffer, in_dst_buffer_size, out_dst_data_size, out_ref_con) })
    }

    #[inline]
    pub fn get_image_info<T>(&self, format_id: FormatId, in_buffer: &[u8], image_info_id: ImageInfoId) -> Result<T> {
        let mut out_image_info = core::mem::MaybeUninit::<T>::uninit();
        self.err_to_result(unsafe { (self.CrmGetImageInfo)(format_id as CrmUInt32, in_buffer.as_ptr() as *mut _, in_buffer.len() as CrmUInt32, image_info_id, out_image_info.as_mut_ptr() as *mut _) })?;
        Ok(unsafe { out_image_info.assume_init() })
    }

    #[inline]
    pub fn get_gpu_system_info(&self, out_cuda_gpu_system_info: &mut GpuSystemInfo, out_cl_gpu_system_info: &mut GpuSystemInfo) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmGetGpuSystemInfo)(out_cuda_gpu_system_info, out_cl_gpu_system_info) })
    }

    #[inline]
    pub fn get_gpu_info_list(&self, out_cuda_gpu_info: &mut GpuInfo, out_cl_gpu_info: &mut GpuInfo) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmGetGpuInfoList)(out_cuda_gpu_info, out_cl_gpu_info) })
    }

    #[inline]
    pub fn get_sdk_info(&self, out_sdk_version: &mut Version, out_pipeline_num: &mut CrmUInt32, out_usable_gpu_num: &mut CrmUInt32) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmGetSdkInfo)(out_sdk_version, out_pipeline_num, out_usable_gpu_num) })
    }

    #[inline]
    pub fn get_wb_gain(&self, in_process_ref: ProcessRef, out_wb_gain: &mut WbGain) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmGetWBGain)(in_process_ref, out_wb_gain) })
    }

    #[inline]
    pub fn get_aces_idt(&self, in_process_ref: ProcessRef, out_aces_idt: &mut AcesIdt, out_aces_idt_str: *mut CrmASCII) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmGetAcesIDT)(in_process_ref, out_aces_idt, out_aces_idt_str) })
    }

    #[inline]
    pub fn get_sdk_info_with_device(&self, in_use_device: Device, in_process_type: ProcessType) -> Result<(Version, CrmUInt32, CrmUInt32)> {
        let mut out_sdk_version = core::mem::MaybeUninit::<Version>::uninit();
        let mut out_pipeline_num = core::mem::MaybeUninit::<CrmUInt32>::uninit();
        let mut out_usable_gpu_num = core::mem::MaybeUninit::<CrmUInt32>::uninit();

        self.err_to_result(unsafe {
            (self.CrmGetSdkInfoWithDevice)(
                out_sdk_version.as_mut_ptr(),
                out_pipeline_num.as_mut_ptr(),
                out_usable_gpu_num.as_mut_ptr(),
                in_use_device,
                in_process_type,
            )
        })?;

        Ok(unsafe {
            (
                out_sdk_version.assume_init(),
                out_pipeline_num.assume_init(),
                out_usable_gpu_num.assume_init(),
            )
        })
    }

    #[inline]
    pub fn get_gpu_system_info_with_device(&self, in_use_device: Device) -> Result<GpuSystemInfo> {
        let mut out = core::mem::MaybeUninit::<GpuSystemInfo>::uninit();
        self.err_to_result(unsafe { (self.CrmGetGpuSystemInfoWithDevice)(out.as_mut_ptr(), in_use_device) })?;
        Ok(unsafe { out.assume_init() })
    }

    #[inline]
    pub fn get_gpu_info_list_with_device(&self, in_use_device: Device) -> Result<Vec<GpuInfo>> {
        let info = self.get_gpu_system_info_with_device(in_use_device)?;
        let mut out = Vec::with_capacity(info.device_num as usize);
        unsafe { out.set_len(info.device_num as usize); }
        self.err_to_result(unsafe { (self.CrmGetGpuInfoListWithDevice)(out.as_mut_ptr(), in_use_device) })?;
        Ok(out)
    }

    #[cfg(target_os = "windows")]
    #[inline]
    pub fn set_device_handle_for_intel(&self, in_process_ref: ProcessRef, in_device_handle: DeviceHandle, in_mem_type: MemType) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmSetDeviceHandleForIntel)(in_process_ref, in_device_handle, in_mem_type) })
    }

    #[cfg(target_os = "windows")]
    #[inline]
    pub fn set_image_param_for_intel(&self, in_process_ref: ProcessRef, in_resolution: Size, in_crop: Rect) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmSetImageParamForIntel)(in_process_ref, in_resolution, in_crop) })
    }

    #[cfg(target_os = "windows")]
    #[inline]
    pub fn allocate_d3d_surfaces_for_intel(&self, in_process_ref: ProcessRef, out_mem_id_array: &mut MemId) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmAllocateD3DSurfacesForIntel)(in_process_ref, out_mem_id_array) })
    }

    #[cfg(target_os = "windows")]
    #[inline]
    pub fn deallocate_d3d_surfaces_for_intel(&self, in_process_ref: ProcessRef, in_mem_id_array: MemId) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmDeallocateD3DSurfacesForIntel)(in_process_ref, in_mem_id_array) })
    }

    #[cfg(target_os = "windows")]
    #[inline]
    pub fn create_device_handle_for_intel(
        &self,
        in_process_ref: ProcessRef,
        out_device_handle: &mut DeviceHandle,
        in_mem_type: MemType,
    ) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmCreateDeviceHandleForIntel)(in_process_ref, out_device_handle, in_mem_type) })
    }

    #[cfg(target_os = "windows")]
    #[inline]
    pub fn cleanup_device_handle_for_intel(&self, in_process_ref: ProcessRef) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmCleanupDeviceHandleForIntel)(in_process_ref) })
    }

    #[inline]
    pub fn pre_process_for_get_image_with_gpu_memory(
        &self,
        in_process_ref: ProcessRef,
        in_bits_per_sample: CrmUInt32,
        in_samples_per_pixel: CrmUInt32,
        in_bytes_per_line: CrmUInt32,
        in_sample_sequence: CrmUInt32,
        in_src_buffer: *mut CrmVoid,
        in_src_data_size: CrmUInt32,
        in_dev_param: *mut CrmVoid,
        in_res_param: *mut CrmVoid,
        out_required_buffer_size: &mut CrmUInt32,
    ) -> Result<()> {
        self.err_to_result(unsafe {
            (self.CrmPreProcessForGetImageWithGpuMemory)(
                in_process_ref,
                in_bits_per_sample,
                in_samples_per_pixel,
                in_bytes_per_line,
                in_sample_sequence,
                in_src_buffer,
                in_src_data_size,
                in_dev_param,
                in_res_param,
                out_required_buffer_size,
            )
        })
    }

    #[inline]
    pub fn get_image_with_gpu_memory(
        &self,
        in_process_ref: ProcessRef,
        out_dst_buffer: *mut CrmVoid,
        in_dst_buffer_size: CrmUInt32,
        out_dst_data_size: &mut CrmUInt32,
        in_src_buffer: *mut CrmVoid,
        in_src_data_size: CrmUInt32,
    ) -> Result<()> {
        self.err_to_result(unsafe {
            (self.CrmGetImageWithGpuMemory)(
                in_process_ref,
                out_dst_buffer,
                in_dst_buffer_size,
                out_dst_data_size,
                in_src_buffer,
                in_src_data_size,
            )
        })
    }

    #[inline]
    pub fn get_last_error(&self, out_error_msg: *mut CrmChar, in_buffer_size: CrmUInt32) -> Result<()> {
        self.err_to_result(unsafe { (self.CrmGetLastError)(out_error_msg, in_buffer_size) })
    }

    /// Convenience helper (still uses `CrmGetLastError` under the hood).
    pub fn get_last_error_string(&self) -> Result<String> {
        use std::ffi::CStr;

        let mut buf = vec![0u8; 8192];
        self.get_last_error(buf.as_mut_ptr() as *mut CrmChar, buf.len() as CrmUInt32)?;

        let cstr = unsafe { CStr::from_ptr(buf.as_ptr() as *const CrmChar) };
        Ok(cstr.to_string_lossy().into_owned())
    }

    #[inline]
    pub fn get_out_image_size(
        &self,
        in_res_param: &ResParamV13,
        header_bufer: &[u8],
    ) -> Result<(u32, u32)> {
        let mut out_image_width = 0;
        let mut out_image_height = 0;
        self.err_to_result(unsafe {
            (self.CrmGetOutImageSize)(in_res_param as *const _ as *mut _, &mut out_image_width, &mut out_image_height, header_bufer.as_ptr() as *mut _, header_bufer.len() as CrmUInt32)
        })?;
        Ok((out_image_width, out_image_height))
    }

    #[inline]
    pub fn create_header_pack(&self, header_array: &[Header]) -> Result<Vec<u8>> {
        let mut out_dst_data_size = 0;
        let mut out_dst_buffer = vec![0u8; MAX_FILE_SIZE as usize];
        self.err_to_result(unsafe {
            (self.CrmCreateHeaderPack)(header_array.as_ptr() as *mut _, header_array.len() as CrmUInt32, out_dst_buffer.as_mut_ptr() as *mut CrmVoid, out_dst_buffer.len() as CrmUInt32, &mut out_dst_data_size)
        })?;
        out_dst_buffer.resize(out_dst_data_size as usize, 0);
        Ok(out_dst_buffer)
    }

    #[inline]
    pub fn set_luts_directory(&self, path: &str, is_absolute_path: bool) -> Result<()> {
        let c_path = std::ffi::CString::new(path).unwrap();
        self.err_to_result(unsafe { (self.CrmSetLUTsDirectory)(c_path.as_ptr() as *const CrmChar, is_absolute_path as CrmBool) })
    }

    fn err_to_result(&self, err: SdkError) -> Result<()> {
        if err == SdkError::Ok {
            Ok(())
        } else {
            Err(CrmError::SdkError((err, self.get_last_error_string()?)))
        }

    }
}

impl Drop for CrmSdkLibrary {
    fn drop(&mut self) {
        let _ = unsafe { (self.CrmTerminateSDK)() };
    }
}


pub struct CrmMemory<'a> {
    ptr: *mut CrmVoid,
    device: Device,
    size: CrmUInt32,
    sdk: &'a CrmSdkLibrary,
}
impl<'a> CrmMemory<'a> {
    pub fn new(sdk: &'a CrmSdkLibrary, size: CrmUInt32, device: Device) -> Result<Self> {
        let ptr = sdk.allocate(size, device)?;
        Ok(Self { ptr, device, size, sdk })
    }

    pub fn as_ptr(&self) -> *mut CrmVoid {
        self.ptr
    }

    pub fn len(&self) -> usize {
        self.size as usize
    }
}
impl<'a> Drop for CrmMemory<'a> {
    fn drop(&mut self) {
        // ignore error
        let _ = self.sdk.deallocate(self.ptr, self.device);
    }
}