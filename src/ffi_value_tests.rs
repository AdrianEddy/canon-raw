// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

//! Gates for the values the SDK writes back. Every one is read as the plain
//! integer the C side produces and only then converted, so a value the
//! headers do not list must come out as a value, never as undefined behaviour.
//! Hardware-free.

use crate::image_info::{self, ImageInfo};
use crate::*;
use std::mem::{align_of, size_of};

#[test]
fn every_crm_error_code_round_trips_and_unknown_ones_are_kept() {
    assert_eq!(SdkError::from_code(CRM_ERR_OK), None, "CRM_ERR_OK is not an error");

    // Every code `CrmError.h` (CRMSDK 2.10) defines.
    let defined: [u32; 23] = [
        0x8000_0001,
        0x8000_0002,
        0x8000_0003,
        0x8000_0004,
        0x8000_0005,
        0x8000_0006,
        0x8000_0007,
        0x8000_0021,
        0x8000_0022,
        0x8000_0023,
        0x8000_0024,
        0x8000_0025,
        0x8000_0031,
        0x8000_0032,
        0x8000_0033,
        0x8000_0041,
        0x8000_0042,
        0x8000_0043,
        0x8000_0051,
        0x8000_0052,
        0x8000_0053,
        0x8000_0054,
        0x8000_0055,
    ];
    for code in defined {
        let e = SdkError::from_code(code).expect("a defined code is an error");
        assert!(!matches!(e, SdkError::Unrecognized(_)), "{code:#x} is defined in CrmError.h");
        assert_eq!(e.code(), code);
    }
    assert_eq!(SdkError::from_code(0x8000_0021), Some(SdkError::InvalidParameter));

    for code in [1, 0x8000_0000, 0x8000_0026, 0xFFFF_FFFF] {
        assert_eq!(SdkError::from_code(code), Some(SdkError::Unrecognized(code)));
        assert_eq!(SdkError::Unrecognized(code).code(), code);
    }
    assert_eq!(SdkError::InvalidParameter.to_string(), "InvalidParameter (0x80000021)");
    assert_eq!(SdkError::Unrecognized(0x8000_0099).to_string(), "unrecognized SDK error (0x80000099)");
}

#[test]
fn format_id_carries_any_value_the_sdk_returns() {
    assert_eq!((size_of::<FormatId>(), align_of::<FormatId>()), (4, 4), "laid out as the C enum it replaces");
    assert_eq!(FormatId::from_raw(0x0001_0002), FormatId::MOVIE_TYPE_2);
    assert_eq!(FormatId::MOVIE_TYPE_1.raw(), 0x0001_0001);
    assert_eq!(format!("{:?}", FormatId::MOVIE_TYPE_2), "FormatId::MOVIE_TYPE_2");
    assert_eq!(format!("{:?}", FormatId::from_raw(0x0001_0003)), "FormatId(0x00010003)");
}

#[test]
fn reported_enums_decode_known_values_and_refuse_the_rest() {
    assert_eq!(AcesIdt::try_from(1), Ok(AcesIdt::D55));
    assert_eq!(AcesIdt::try_from(3), Err(3));
    assert_eq!(ClSupportType::try_from(2), Ok(ClSupportType::Limited));
    // What the SDK reports for every CUDA device: not a `ClSupportType` at all.
    assert_eq!(ClSupportType::try_from(0), Err(0));

    let info = AcesIdtInfo { raw: 1, name: "D55 Ver. 2.0".into() };
    assert_eq!(info.idt(), Some(AcesIdt::D55));
    assert_eq!(AcesIdtInfo { raw: 7, name: String::new() }.idt(), None);
}

#[test]
fn gpu_info_matches_the_sdk_layout_and_tolerates_a_zero_cl_support_type() {
    // `CrmGpuInfo`: 256 + 4 + 2 (+2 padding) + 4 — the stride the SDK writes.
    assert_eq!(size_of::<GpuInfo>(), 268);

    let mut info = GpuInfo::ZEROED;
    assert_eq!(info.cl_support_type(), None);
    assert_eq!(info.cl_support_type_raw(), 0);
    assert!(!info.is_supported());
    assert_eq!(info.device_name(), "");

    info.cl_support_type = 1;
    info.support = 1;
    for (slot, byte) in info.device_name.iter_mut().zip(b"NVIDIA GeForce RTX 5090") {
        *slot = *byte as CrmASCII;
    }
    assert_eq!(info.cl_support_type(), Some(ClSupportType::Full));
    assert!(info.is_supported());
    assert_eq!(info.device_name(), "NVIDIA GeForce RTX 5090");

    // A name that fills the field leaves no NUL; it must still be bounded.
    info.device_name = [b'x' as CrmASCII; 256];
    assert_eq!(info.device_name().len(), 256);
}

#[test]
fn fixed_width_text_stops_at_the_first_nul_or_the_end_of_the_field() {
    // What the SDK wrote for `CreationDateTime`: 24 characters, no terminator.
    assert_eq!(fixed_ascii(b"2024:01:01:03:06:20+0100"), "2024:01:01:03:06:20+0100");
    // What it wrote for `DeviceModelName`: the name, a NUL, then stack bytes.
    assert_eq!(fixed_ascii(b"Canon EOS C80\0\0\0\x04\0\0\0\0\0\0\0\x01\0\0\0\0\0"), "Canon EOS C80");
    assert_eq!(fixed_ascii(b""), "");
    assert_eq!(fixed_ascii(b"\0abc"), "");
}

#[test]
fn a_header_is_laid_out_as_crm_header_and_only_built_from_a_slice() {
    #[repr(C)]
    struct CrmHeader {
        _header_id: u32,
        _data: *mut core::ffi::c_void,
        _size: u32,
    }
    assert_eq!(size_of::<Header<'_>>(), size_of::<CrmHeader>());
    assert_eq!(align_of::<Header<'_>>(), align_of::<CrmHeader>());

    let bytes = [1u8, 2, 3, 4, 5];
    let h = Header::craw(&bytes);
    assert_eq!(h.header_id(), HeaderId::Craw);
    assert_eq!(h.data(), &bytes);
    assert_eq!(Header::new(HeaderId::Uuid, &[]).data(), &[] as &[u8]);
}

/// How many bytes `get_image_info` reads for `info` — the width of its raw
/// type.
fn width<I: ImageInfo>(_: I) -> usize {
    size_of::<I::Raw>()
}

/// Each key's raw type is exactly as wide as what CRMSDK 2.10 writes for the
/// property: measured by canary-filling the output buffer and calling
/// `CrmGetImageInfo` on a Canon EOS C80 clip. A key narrower than the SDK's
/// write would lose data; the scratch buffer means none can overrun.
#[test]
fn every_image_info_key_is_as_wide_as_what_the_sdk_writes() {
    macro_rules! widths {
        ($($key:ident = $bytes:literal,)*) => {$(
            assert_eq!(width(image_info::$key), $bytes, stringify!($key));
            assert!(width(image_info::$key) <= crate::IMAGE_INFO_SCRATCH);
        )*};
    }
    widths! {
        CaptureWidth = 2, CaptureHeight = 2, BitDepth = 1, TotalImageSize = 4, OffsetToImageData = 4,
        RawFrameWidth = 2, RawFrameHeight = 2,
        Position = 4, Timecode = 4, PlaybackFrameRate = 2, PlaybackSystemFlag = 4, CaptureFrameRate = 2,
        CaptureSystemFlag = 4, PixelAspectRatio = 8,
        CreationDateTime = 24, ShutterAngle = 4, UserBits = 4,
        DeviceModelName = 30, DeviceFirmwareVersion = 30, DeviceSerialNumber = 30, LensName = 80,
        LensSqueezeFactor = 1, LensSerialNumber = 12,
        RelatedClipName = 32, RelatedClipId = 32, RelatedFrameId = 64,
        ShootingGamutInformation = 1, ShootingGammaType = 1, ShootingColorMatrix = 1,
        FocalLength = 2, FocusPosition = 4, FNumber = 4, NdFilter = 2, TNumber = 4,
        ShutterSpeedMode = 1, ShutterSpeed = 4, WbMode = 1, WbPreset = 1, ColorTemperature = 2,
        ColorTemperatureStatus = 1, IsoSpeed = 4, ImageStabilizer = 1, PeripheralIlluminationCorrection = 1,
        GamutInformation = 1, GammaType = 1, Gain = 2, ColorCompensation = 1, WbGainR = 2, WbGainB = 2,
        WbGainDenominator = 2, ColorMatrix = 36, ChromaticAberrationCorrection = 1, DistortionCorrection = 1,
        FocusBreathingCorrection = 1, VfxTilt = 2, VfxRoll = 2, IsoAdjustAvailable = 4, ActualIsoSpeed = 4,
        RawType = 2,
    }
}

#[test]
fn image_info_keys_name_their_sdk_ids() {
    assert_eq!(image_info::CaptureWidth::ID as u32, 0x0000_0001);
    assert_eq!(image_info::LensName::ID as u32, 0x0003_0004);
    assert_eq!(image_info::RawType::ID as u32, 0x0009_0001);
    assert!(image_info::PlaybackSystemFlag::from_raw(1));
    assert!(!image_info::IsoAdjustAvailable::from_raw(0));
    assert_eq!(image_info::CreationDateTime::from_raw(*b"2024:01:01:03:06:20+0100"), "2024:01:01:03:06:20+0100");
}
