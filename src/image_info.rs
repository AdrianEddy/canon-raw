// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

//! Typed keys for [`CrmSdkLibrary::get_image_info`](crate::CrmSdkLibrary::get_image_info).
//!
//! `CrmGetImageInfo` writes into an untyped pointer as many bytes as the
//! requested property is wide, and takes no buffer size (spec § 7.4.2). So the
//! output type cannot be the caller's choice: a type narrower than the property
//! is overrun, and one with invalid bit patterns (`bool`, an enum) is undefined
//! behaviour on the first unexpected value. Each key here fixes the type for its
//! property instead:
//!
//! ```no_run
//! # fn f(sdk: &canon_raw::CrmSdkLibrary, format_id: canon_raw::FormatId, header_pack: &[u8]) -> canon_raw::Result<()> {
//! use canon_raw::image_info;
//!
//! let (w, h) = (
//!     sdk.get_image_info(format_id, header_pack, image_info::CaptureWidth)?,  // u16
//!     sdk.get_image_info(format_id, header_pack, image_info::CaptureHeight)?, // u16
//! );
//! let shot_at = sdk.get_image_info(format_id, header_pack, image_info::CreationDateTime)?; // String
//! # Ok(()) }
//! ```
//!
//! The types follow spec § 8.2 and were checked against what CRMSDK 2.10
//! actually writes. Text properties are fixed-width fields — NUL-padded when
//! shorter, but filled to the last byte when not (`CreationDateTime` is 24
//! characters in 24 bytes) — and come back as `String`. Coded properties
//! (`WbMode`, `GammaType`, …) come back as the raw code, whose meanings the
//! spec tabulates per camera group.

use crate::{CrmBool, ImageInfoId, Rational16, Umid32, Umid64, fixed_ascii};

/// A property [`CrmSdkLibrary::get_image_info`](crate::CrmSdkLibrary::get_image_info)
/// can read, and the type it reads as. Implemented by the keys in this module
/// only.
pub trait ImageInfo: sealed::Sealed + Copy {
    /// What the property reads as.
    type Value;
    #[doc(hidden)]
    const ID: ImageInfoId;
    #[doc(hidden)]
    type Raw: sealed::Pod;
    #[doc(hidden)]
    fn from_raw(raw: Self::Raw) -> Self::Value;
}

mod sealed {
    pub trait Sealed {}

    /// Plain data: every bit pattern is a valid value, so it can be read out of
    /// whatever bytes the SDK left in the scratch buffer.
    ///
    /// # Safety
    ///
    /// Implement only for types with no invalid bit patterns and no padding.
    pub unsafe trait Pod: Copy + 'static {}

    unsafe impl Pod for u8 {}
    unsafe impl Pod for i8 {}
    unsafe impl Pod for u16 {}
    unsafe impl Pod for u32 {}
    unsafe impl Pod for i32 {}
    unsafe impl Pod for f32 {}
    unsafe impl<T: Pod, const N: usize> Pod for [T; N] {}
    unsafe impl Pod for crate::Rational16 {}
    unsafe impl Pod for crate::Umid32 {}
    unsafe impl Pod for crate::Umid64 {}
}

fn same<T>(raw: T) -> T {
    raw
}

fn flag(raw: CrmBool) -> bool {
    raw != 0
}

fn text<const N: usize>(raw: [u8; N]) -> String {
    fixed_ascii(&raw)
}

macro_rules! image_info {
    ($($section:literal $name:ident: $raw:ty => $value:ty = $from_raw:path, $doc:literal;)*) => {$(
        #[doc = $doc]
        #[doc = ""]
        #[doc = concat!("`kCrmImageInfoId_", stringify!($name), "`, spec § ", $section, ".")]
        #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
        pub struct $name;

        impl sealed::Sealed for $name {}

        impl ImageInfo for $name {
            type Value = $value;
            const ID: ImageInfoId = ImageInfoId::$name;
            type Raw = $raw;
            fn from_raw(raw: $raw) -> $value {
                $from_raw(raw)
            }
        }
    )*};
}

image_info! {
    "8.2.1"  CaptureWidth: u16 => u16 = same, "Width of the effective image.";
    "8.2.2"  CaptureHeight: u16 => u16 = same, "Height of the effective image.";
    "8.2.3"  BitDepth: u8 => u8 = same, "Bit depth of each pixel.";
    "8.2.4"  TotalImageSize: u32 => u32 = same, "Total image file size in bytes. Not available for `.CRM` compressed data.";
    "8.2.5"  OffsetToImageData: u32 => u32 = same, "Offset to the image data in bytes. Not available for `.CRM` compressed data.";
    "8.2.41" RawFrameWidth: u16 => u16 = same, "Width of the RAW frame, peripheral pixel areas included.";
    "8.2.42" RawFrameHeight: u16 => u16 = same, "Height of the RAW frame, peripheral pixel areas included.";

    "8.2.6"  Position: u32 => u32 = same, "Frame position in the sequence. Not available for `.CRM` compressed data.";
    "8.2.7"  Timecode: u32 => u32 = same, "SMPTE time code, BCD `hhmmssff`. Not available for `.CRM` compressed data.";
    "8.2.8"  PlaybackFrameRate: u16 => u16 = same, "Playback frame rate.";
    "8.2.9"  PlaybackSystemFlag: CrmBool => bool = flag, "Whether the playback frame rate is on the 1000/1001 system (`false`: 1000/1000).";
    "8.2.10" CaptureFrameRate: u16 => u16 = same, "Capture frame rate; differs from the playback rate when over- or under-cranked.";
    "8.2.11" CaptureSystemFlag: CrmBool => bool = flag, "Whether the capture frame rate is on the 1000/1001 system (`false`: 1000/1000).";
    "8.2.12" PixelAspectRatio: [u32; 2] => [u32; 2] = same, "Pixel aspect ratio.";

    "8.2.13" CreationDateTime: [u8; 24] => String = text, "Shooting date and time of the frame, e.g. `2024:01:01:03:06:20+0100`.";
    "8.2.14" ShutterAngle: f32 => f32 = same, "Shutter angle in degrees.";
    "8.2.15" UserBits: u32 => u32 = same, "SMPTE user bits.";

    "8.2.16" DeviceModelName: [u8; 30] => String = text, "Model name of the camera, e.g. `Canon EOS C80`.";
    "8.2.17" DeviceFirmwareVersion: [u8; 30] => String = text, "Firmware version of the camera.";
    "8.2.18" DeviceSerialNumber: [u8; 30] => String = text, "Serial number of the camera.";
    "8.2.19" LensName: [u8; 80] => String = text, "Name of the lens.";
    "8.2.20" LensSqueezeFactor: u8 => u8 = same, "Squeeze ratio of an anamorphic lens (code).";
    "8.2.58" LensSerialNumber: [u8; 12] => String = text, "Serial number of the lens.";

    "8.2.21" RelatedClipName: [u8; 32] => String = text, "Name of the related clip. Always empty for `.CRM` compressed data.";
    "8.2.22" RelatedClipId: Umid32 => Umid32 = same, "ID of the related clip. Always zero for `.CRM` compressed data.";
    "8.2.23" RelatedFrameId: Umid64 => Umid64 = same, "ID of the frame the RAW frame corresponds to. Always zero for `.CRM` compressed data.";
    "8.2.45" ShootingGamutInformation: u8 => u8 = same, "Gamut at shooting (code).";
    "8.2.46" ShootingGammaType: u8 => u8 = same, "Gamma curve at shooting (code).";
    "8.2.59" ShootingColorMatrix: u8 => u8 = same, "Colour matrix setting at shooting (code; 255 is \"no information\").";

    "8.2.24" FocalLength: u16 => u16 = same, "Focal length.";
    "8.2.25" FocusPosition: f32 => f32 = same, "Focusing position, in metres.";
    "8.2.26" FNumber: Rational16 => Rational16 = same, "Aperture as an F number.";
    "8.2.27" NdFilter: u16 => u16 = same, "Status of the ND filter (code).";
    "8.2.43" TNumber: Rational16 => Rational16 = same, "Aperture as a T number.";

    "8.2.28" ShutterSpeedMode: u8 => u8 = same, "Mode of the shutter speed setting (code).";
    "8.2.29" ShutterSpeed: Rational16 => Rational16 = same, "Shutter speed, in the units of the shutter speed mode.";
    "8.2.30" WbMode: u8 => u8 = same, "Mode of the white balance setting (code).";
    "8.2.31" WbPreset: u8 => u8 = same, "Preset of the white balance setting (code).";
    "8.2.32" ColorTemperature: u16 => u16 = same, "Colour temperature of the white balance.";
    "8.2.33" ColorTemperatureStatus: u8 => u8 = same, "Reliability of the colour temperature value (code).";
    "8.2.34" IsoSpeed: u32 => u32 = same, "ISO speed.";
    "8.2.35" ImageStabilizer: u8 => u8 = same, "Status of the image stabilizer (code).";
    "8.2.36" PeripheralIlluminationCorrection: u8 => u8 = same, "Status of peripheral illumination correction (code).";
    "8.2.37" GamutInformation: u8 => u8 = same, "Gamut (code).";
    "8.2.38" GammaType: u8 => u8 = same, "Gamma curve (code).";
    "8.2.44" Gain: u16 => u16 = same, "Gain of the master control.";
    "8.2.47" ColorCompensation: i8 => i8 = same, "Colour compensation value.";
    "8.2.50" WbGainR: u16 => u16 = same, "White-balance gain applied to R in the Bayer image.";
    "8.2.51" WbGainB: u16 => u16 = same, "White-balance gain applied to B in the Bayer image.";
    "8.2.52" WbGainDenominator: u16 => u16 = same, "Denominator of the white-balance gains.";
    "8.2.53" ColorMatrix: [f32; 9] => [f32; 9] = same, "3 × 3 colour matrix, row-major, for third-party debayering of Group-2 to Group-5 clips.";
    "8.2.55" ChromaticAberrationCorrection: u8 => u8 = same, "Status of chromatic aberration correction (code).";
    "8.2.56" DistortionCorrection: u8 => u8 = same, "Status of distortion correction (code).";
    "8.2.57" FocusBreathingCorrection: u8 => u8 = same, "Status of focus breathing correction (code).";

    "8.2.39" VfxTilt: u16 => u16 = same, "Tilt angle of the camera.";
    "8.2.40" VfxRoll: u16 => u16 = same, "Roll angle of the camera.";

    "8.2.48" IsoAdjustAvailable: CrmBool => bool = flag, "Whether ISO speed adjustment is available.";
    "8.2.49" ActualIsoSpeed: u32 => u32 = same, "ISO speed corresponding to the master gain.";

    "8.2.54" RawType: u16 => u16 = same, "Type of the RAW data (code).";
}
