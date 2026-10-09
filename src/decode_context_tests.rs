// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

//! Gates for [`super::DecodeContext`]'s raster authority.
//!
//! Two tiers, both in this one file because the validators they exercise
//! (`check_res_param_pairs`, `probe_res_param`, `resolve`) are private:
//!
//! 1. **Hardware-free** — everything the crate decides for itself: the
//!    pair-constraint table, the debayer derivation, and the degenerate-
//!    extent rules. All of it is settled before any FFI call, so all of it
//!    is testable with no `CrmSdk.dll` in sight, and it stays that way —
//!    a rule that needed the library to confirm it would be a rule the
//!    library was really making.
//! 2. **SDK-gated** — the parts that only mean something against the real
//!    library: that `CrmGetOutImageSize` really does accept what
//!    `CrmGetImage` rejects (the premise the validators exist for), that
//!    `set_geometry` refuses those combinations without moving, and that an
//!    edit through `edit_res_param_in_place` / `edit_res_param` re-resolves
//!    instead of letting the cached raster drift. These skip with a printed
//!    reason when the library or a clip is absent, like the crate's other
//!    SDK-touching tests.
//!
//! ## Provisioning the SDK tier
//!
//! | env | default | what it is |
//! |---|---|---|
//! | `CRM_TEST_SDK_ROOT` | `<repo>/sdk/CRMSDK210/CRMSDK` | the unpacked SDK zip's `CRMSDK` folder |
//! | `CRM_TEST_SDK_DLL` | `<sdk root>/Win_Lib/x64/CrmSdk.dll` | the library to load |
//! | `CRM_TEST_LUTS_PARENT` | `<sdk root>` | the directory CONTAINING `CanonLUTs` |
//! | `CRM_TEST_CLIP` | — | any `.CRM` the SDK can develop |

use super::*;
use crate::crm_parser::CrmMovie;
use crate::image_info;
use crate::{
    AcesIdt, BufferType, ColorSpace, DataType, DevParamV13, FrameRendering, Gamma, IsoSetting, LensSqueeze, Point,
    SdkError, StructId, WbGain, WbSetting,
};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

// ─────────────────────────────────────────────────────────────────────
// Tier 1 — hardware-free
// ─────────────────────────────────────────────────────────────────────

/// A `ResParamV13` that differs from the crate's usual develop recipe only
/// in the three fields under test.
fn res_param(debayer_setting: DebayerSetting, downscale: Downscale, decode_level: DecodeLevel) -> ResParamV13 {
    ResParamV13 {
        size: std::mem::size_of::<ResParamV13>() as u32,
        struct_id: StructId::ResParamV13,
        format_id: FormatId::MOVIE_TYPE_1,
        debayer_setting,
        downscale,
        crop: Rect { x: 0, y: 0, width: 6000, height: 3164 },
        frame_rendering: FrameRendering::Alternate,
        lens_squeeze: LensSqueeze::Off,
        in_data_type: DataType::CrmCompressed,
        in_buffer_type: BufferType::Cpu,
        out_data_type: DataType::RgbLutApplied,
        out_buffer_type: BufferType::Cpu,
        decode_level,
    }
}

fn rejection(p: &ResParamV13) -> Option<&'static str> {
    match check_res_param_pairs(p) {
        Ok(()) => None,
        Err(CrmError::InvalidParameter(msg)) => Some(msg),
        Err(other) => panic!("a pair-constraint refusal must be InvalidParameter, got {other:?}"),
    }
}

/// The full `debayer × decode_level` table. `HighQuality` exists only at
/// `Full`; it is not a slower option on the reduced rungs.
#[test]
fn high_quality_debayer_is_legal_only_at_full_decode_level() {
    for level in [DecodeLevel::Full, DecodeLevel::Half, DecodeLevel::Quarter] {
        let p = res_param(DebayerSetting::HighQuality, Downscale::X1, level);
        match level {
            DecodeLevel::Full => assert_eq!(rejection(&p), None, "HighQuality + Full is the default recipe"),
            _ => assert_eq!(
                rejection(&p),
                Some(ERR_HIGH_QUALITY_NEEDS_FULL),
                "HighQuality + {level:?} is what CrmGetImage rejects with \
                 \"CrmResParam::decodeLevel must be kCrmDecodeLevel_Full\""
            ),
        }
        let p = res_param(DebayerSetting::HighSpeed, Downscale::X1, level);
        assert_eq!(rejection(&p), None, "HighSpeed + {level:?} is the whole point of the reduced rungs");
    }
}

/// `downscale` and `decode_level` do not multiply: at most one of them may
/// scale, and asking for both fails the decode outright.
#[test]
fn downscale_and_decode_level_never_both_scale() {
    // Every non-`X1` value scales — the RGB resamplers, the named rasters
    // and the bayer-domain pair alike.
    let scaling_downscales = [
        Downscale::X2,
        Downscale::X4,
        Downscale::Hd,
        Downscale::Sd,
        Downscale::Qfhd,
        Downscale::K4Dci,
        Downscale::K2Dci,
        Downscale::Bayer2,
        Downscale::Bayer4,
    ];
    for downscale in scaling_downscales {
        // Legal: one scaler, and `Full` keeps the debayer free.
        assert_eq!(
            rejection(&res_param(DebayerSetting::HighQuality, downscale, DecodeLevel::Full)),
            None,
            "{downscale:?} alone is a fit-to-size knob, not a violation"
        );
        for level in [DecodeLevel::Half, DecodeLevel::Quarter] {
            // `HighSpeed` so only the two-scaler rule can fire.
            assert_eq!(
                rejection(&res_param(DebayerSetting::HighSpeed, downscale, level)),
                Some(ERR_TWO_SCALERS),
                "{downscale:?} + {level:?} stacks two scalers; the SDK refuses rather than composing them"
            );
        }
    }
    // The pairs measured against `CrmGetImage`, named explicitly so a
    // regression reads as the symptom it would be.
    assert_eq!(
        rejection(&res_param(DebayerSetting::HighSpeed, Downscale::X2, DecodeLevel::Half)),
        Some(ERR_TWO_SCALERS)
    );
    assert_eq!(
        rejection(&res_param(DebayerSetting::HighSpeed, Downscale::X4, DecodeLevel::Quarter)),
        Some(ERR_TWO_SCALERS)
    );
}

/// `X1` + `Full` is the one combination with no scaler at all.
#[test]
fn the_unscaled_recipe_is_accepted_under_either_debayer() {
    for debayer in [DebayerSetting::HighQuality, DebayerSetting::HighSpeed] {
        assert_eq!(rejection(&res_param(debayer, Downscale::X1, DecodeLevel::Full)), None, "{debayer:?}");
    }
}

/// `OutputGeometry` cannot name the debayer, so `probe_res_param` derives
/// it. The post-condition is what matters: whatever the live `res_param`
/// carried, the probe a scaling rung produces always satisfies the first
/// pair rule — which is what keeps `set_geometry` from having to reject a
/// caller over a field it was given no way to set.
#[test]
fn a_probe_for_a_reduced_rung_always_carries_a_legal_debayer() {
    for base_debayer in [DebayerSetting::HighQuality, DebayerSetting::HighSpeed] {
        let base = res_param(base_debayer, Downscale::X1, DecodeLevel::Full);
        for level in [DecodeLevel::Full, DecodeLevel::Half, DecodeLevel::Quarter] {
            let geometry = OutputGeometry {
                crop: base.crop,
                downscale: Downscale::X1,
                decode_level: level,
                bits_per_sample: 16,
                samples_per_pixel: 4,
            };
            let probe = probe_res_param(&base, &geometry);
            assert_eq!(probe.decode_level, level);
            assert_eq!(
                rejection(&probe),
                None,
                "a probe built from {base_debayer:?} for {level:?} must be decodable as-is"
            );
            match level {
                // Derived: `HighQuality` does not exist down here.
                DecodeLevel::Half | DecodeLevel::Quarter => {
                    assert_eq!(probe.debayer_setting, DebayerSetting::HighSpeed);
                }
                // Untouched: at `Full` the debayer stays the caller's choice.
                DecodeLevel::Full => assert_eq!(probe.debayer_setting, base_debayer),
            }
        }
    }
}

/// A degenerate extent is settled in Rust, with **no SDK at all** — which
/// is the point: `CrmGetOutImageSize` is not a validator, and an input we
/// already know has no raster is not a question to put to it. This test
/// runs on every machine, including ones with no `CrmSdk.dll`, so the rule
/// cannot quietly become "whatever the library happened to answer".
#[test]
fn a_degenerate_extent_is_refused_before_any_ffi() {
    let full = Rect { x: 0, y: 0, width: 6000, height: 3164 };

    // A crop edge of 1 cannot survive the SDK's even-flooring.
    for crop in [Rect { width: 1, height: 1, ..full }, Rect { width: 1, ..full }, Rect { height: 1, ..full }] {
        assert!(
            matches!(check_extent(&crop, 4), Err(CrmError::InvalidParameter(m)) if m == ERR_DEGENERATE_CROP),
            "{crop:?} has no output raster"
        );
    }

    // A crop edge of ZERO is not degenerate — it is the SDK's own "whole
    // frame" spelling, the one `TestCrmSdk.cpp` uses for its decompress
    // pass. Refusing it would break a legal recipe.
    for crop in [Rect { x: 0, y: 0, width: 0, height: 0 }, Rect { width: 0, ..full }, Rect { height: 0, ..full }] {
        assert!(check_extent(&crop, 4).is_ok(), "{crop:?} means the whole frame, not an empty one");
    }

    // No samples, no pitch, no buffer — at any width.
    assert!(
        matches!(check_extent(&full, 0), Err(CrmError::InvalidParameter(m)) if m == ERR_EMPTY_PITCH),
        "samples_per_pixel: 0 sizes the destination at 0 bytes"
    );
    for samples in [1, 3, 4] {
        assert!(check_extent(&full, samples).is_ok(), "{samples} samples/pixel is a real layout");
    }
    // 2 is the small odd one out only in that nothing uses it; the rule is
    // about zero, not about the SDK's supported set.
    assert!(check_extent(&full, 2).is_ok());
}

/// The probe carries the rest of the live struct through unchanged — the
/// GPU lane's `out_buffer_type` above all, which a rung change must not
/// silently reset to CPU.
#[test]
fn a_probe_preserves_every_field_the_geometry_does_not_name() {
    let mut base = res_param(DebayerSetting::HighSpeed, Downscale::X1, DecodeLevel::Full);
    base.out_buffer_type = BufferType::Cuda;
    base.lens_squeeze = LensSqueeze::X13;
    base.out_data_type = DataType::CrmDecompressed;
    base.frame_rendering = FrameRendering::Split;

    let geometry = OutputGeometry {
        crop: Rect { x: 8, y: 8, width: 4096, height: 2160 },
        downscale: Downscale::X2,
        decode_level: DecodeLevel::Full,
        bits_per_sample: 16,
        samples_per_pixel: 4,
    };
    let probe = probe_res_param(&base, &geometry);

    assert_eq!(probe.crop, geometry.crop);
    assert_eq!(probe.downscale, geometry.downscale);
    assert_eq!(probe.decode_level, geometry.decode_level);
    assert_eq!(probe.out_buffer_type, BufferType::Cuda, "a rung change must not move the destination domain");
    assert_eq!(probe.lens_squeeze, LensSqueeze::X13);
    assert_eq!(probe.out_data_type, DataType::CrmDecompressed);
    assert_eq!(probe.frame_rendering, FrameRendering::Split);
    assert_eq!(probe.size, base.size);
    assert_eq!(probe.struct_id, base.struct_id);
    assert_eq!(probe.format_id, base.format_id);
}

// ─────────────────────────────────────────────────────────────────────
// Tier 2 — SDK-gated
// ─────────────────────────────────────────────────────────────────────

/// The SDK zip as unpacked into the (git-ignored) `sdk/` folder of this repo.
const DEFAULT_SDK_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/sdk/CRMSDK210/CRMSDK");

/// Process-wide, because `CrmInitializeSDK` is: it loads ~29 MB of
/// resources (plus the LUT folder) and is not a per-test operation.
static SHARED: OnceLock<Option<Shared>> = OnceLock::new();
/// The SDK's per-decoder contract is single-consumer; these gates each
/// build their own `DecodeContext` against the shared library, so they run
/// one at a time rather than racing inside `CrmSdk.dll`.
static SERIALIZE: Mutex<()> = Mutex::new(());

struct Shared {
    sdk: Arc<CrmSdkLibrary>,
    format_id: FormatId,
    uuid: Vec<u8>,
    craw: Vec<u8>,
    timed: Vec<u8>,
    header_pack: Vec<u8>,
    capture: Rect,
    frame0: Vec<u8>,
}

struct Fixture<'a> {
    s: &'a Shared,
    _lock: MutexGuard<'a, ()>,
}

impl Fixture<'_> {
    /// The crate's usual full-resolution develop recipe for this clip.
    fn recipe(&self) -> (DevParamV13, ResParamV13, OutputGeometry) {
        let dev_param = DevParamV13 {
            size: std::mem::size_of::<DevParamV13>() as u32,
            struct_id: StructId::DevParamV13,
            format_id: self.s.format_id,
            wb_setting: WbSetting::Record,
            click_wb_point: Point { x: 0, y: 0 },
            tint: 0.0,
            exposure: 0.0,
            cct: 0,
            sharpness: 0,
            noise: 0,
            gamma: Gamma::Rec709,
            wb_gain: WbGain { r: 0, g1: 0, g2: 0, b: 0 },
            aces_idt: AcesIdt::Auto,
            color_space: ColorSpace::Bt709,
            iso_setting: IsoSetting::Record,
            iso_speed: 0,
        };
        let mut res_param = res_param(DebayerSetting::HighQuality, Downscale::X1, DecodeLevel::Full);
        res_param.format_id = self.s.format_id;
        res_param.crop = self.s.capture;
        let geometry = OutputGeometry {
            crop: self.s.capture,
            downscale: Downscale::X1,
            decode_level: DecodeLevel::Full,
            bits_per_sample: 16,
            samples_per_pixel: 4,
        };
        (dev_param, res_param, geometry)
    }

    fn context(&self) -> DecodeContext {
        let (dev_param, res_param, geometry) = self.recipe();
        DecodeContext::new(
            self.s.sdk.clone(),
            self.s.format_id,
            Device::Cpu,
            Gpu::NotUse,
            ProcessType::DEVELOP | ProcessType::DECOMPRESS,
            Resolution::K8,
            dev_param,
            res_param,
            self.s.uuid.clone(),
            self.s.craw.clone(),
            &self.s.timed,
            SampleSequence::Rgba16BitLe,
            geometry,
        )
        .expect("the full-resolution recipe is the one this clip was shot at")
    }

    /// A DECOMPRESS-only context — the one kind `CrmGetImageAsync` accepts —
    /// producing the 16-bit single-channel Bayer raster.
    fn decompress_context(&self) -> DecodeContext {
        let (dev_param, mut res_param, mut geometry) = self.recipe();
        res_param.out_data_type = DataType::CrmDecompressed;
        geometry.bits_per_sample = 16;
        geometry.samples_per_pixel = 1;
        DecodeContext::new(
            self.s.sdk.clone(),
            self.s.format_id,
            Device::Cpu,
            Gpu::NotUse,
            ProcessType::DECOMPRESS,
            Resolution::K8,
            dev_param,
            res_param,
            self.s.uuid.clone(),
            self.s.craw.clone(),
            &self.s.timed,
            SampleSequence::Rgb,
            geometry,
        )
        .expect("a full-resolution decompress of this clip")
    }

    /// A CPU develop/decompress process ref, leased like `DecodeContext`
    /// leases its own: one per test, reused across every `decode_with` in
    /// it, parked again on drop.
    fn process_ref(&self) -> Result<ProcessRefLease> {
        self.s.sdk.lease_process_ref(
            self.s.format_id,
            Device::Cpu,
            Gpu::NotUse,
            ProcessType::DEVELOP | ProcessType::DECOMPRESS,
            Resolution::K8,
        )
    }

    /// One real `CrmGetImage` of frame 0 with an arbitrary `ResParamV13` —
    /// the only way to learn what the SDK actually refuses.
    ///
    /// `CrmResetProcessRef` afterwards, unconditionally: a refused decode
    /// leaves the ref mid-recipe, and the next call on it must start clean.
    fn decode_with(
        &self,
        process_ref: &ProcessRefLease,
        res_param: &ResParamV13,
        bits: u32,
        samples: u32,
    ) -> Result<u32> {
        let dev_param = self.recipe().0;
        let (w, h) = self.s.sdk.get_out_image_size(res_param, &self.s.header_pack)?;
        let bytes_per_line = OutputGeometry::bytes_per_line(w, bits, samples);
        let size = bytes_per_line.saturating_mul(h).max(4096);
        let dst = crate::CrmMemory::new(self.s.sdk.clone(), size, Device::Cpu)?;
        // SAFETY: `process_ref` is live for the test; `dst` is a CPU
        // allocation of `size` bytes and `res_param` names a CPU output.
        let got = unsafe {
            self.s.sdk.get_image(
                process_ref.process_ref(),
                dst.as_ptr(),
                size,
                bits,
                samples,
                bytes_per_line,
                SampleSequence::Rgba16BitLe,
                &self.s.frame0,
                &dev_param,
                res_param,
                &self.s.header_pack,
            )
        };
        // SAFETY: as above; nothing is pending on the ref.
        let _ = unsafe { self.s.sdk.reset_process_ref(process_ref.process_ref()) };
        got
    }
}

fn skip(why: &str) -> Option<Shared> {
    eprintln!("SKIP decode_context: {why}");
    None
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Make the SDK's siblings findable: `CrmSdk.dll` `LoadLibrary`s
/// `crxdec.dll` / `libiomp5md.dll` / `CESScalingLib.dll` / the CUDA runtime
/// by bare name, and the standard search order does not include the
/// directory of the DLL doing the loading.
///
/// `SetDllDirectoryW` rather than an edit to `PATH`: `std::env::set_var` is
/// unsound in a process that has other threads running — and a `#[test]`
/// binary always does — whereas this is a documented, thread-safe,
/// process-wide loader setting that does exactly the same job.
#[cfg(windows)]
fn add_to_loader_search_path(dir: &Path) {
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
    wide.push(0);
    // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the call.
    unsafe { windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW(wide.as_ptr()) };
}

/// Off Windows there is nothing useful to do here: `dlopen` resolves a
/// library's `DT_NEEDED` against the search path as it stood at `exec`
/// time, so a run-time `LD_LIBRARY_PATH` edit would not help even if it
/// were sound. Export it before `cargo test`, alongside the
/// `CRM_TEST_SDK_DLL` this gate already requires there.
#[cfg(not(windows))]
fn add_to_loader_search_path(_dir: &Path) {}

fn build_shared() -> Option<Shared> {
    let sdk_root = PathBuf::from(env_or("CRM_TEST_SDK_ROOT", DEFAULT_SDK_ROOT));
    let dll = match std::env::var_os("CRM_TEST_SDK_DLL") {
        Some(p) => PathBuf::from(p),
        None if cfg!(windows) => sdk_root.join("Win_Lib").join("x64").join("CrmSdk.dll"),
        // The Linux / macOS bundles ship as archives with no fixed unpacked
        // layout, so there is nothing to guess.
        None => return skip("CRM_TEST_SDK_DLL unset (no default layout off Windows)"),
    };
    if !dll.is_file() {
        return skip(&format!("{} absent", dll.display()));
    }
    let Some(clip) = std::env::var_os("CRM_TEST_CLIP").map(PathBuf::from) else {
        return skip("CRM_TEST_CLIP is not set");
    };
    if !clip.is_file() {
        return skip(&format!("{} absent", clip.display()));
    }

    add_to_loader_search_path(dll.parent().unwrap_or(Path::new(".")));

    // SAFETY: the SDK the provisioning table names.
    let sdk = match unsafe { CrmSdkLibrary::load(dll.as_path()) } {
        Ok(s) => Arc::new(s),
        Err(e) => return skip(&format!("{} did not load: {e}", dll.display())),
    };
    if let Err(e) = sdk.initialize_sdk() {
        return skip(&format!("CrmInitializeSDK failed: {e}"));
    }
    // `CrmSetLUTsDirectory` wants the directory that CONTAINS `CanonLUTs`,
    // and returns `CRM_ERR_OK` for a path it cannot use — a successful
    // return proves nothing, so point it at a tree that has the folder and
    // let a failing develop be the signal.
    let luts_parent = PathBuf::from(env_or("CRM_TEST_LUTS_PARENT", &sdk_root.to_string_lossy()));
    let _ = sdk.set_luts_directory(&luts_parent, true);

    let mut movie = match std::fs::File::open(&clip).map_err(CrmError::from).and_then(CrmMovie::load) {
        Ok(m) => m,
        Err(e) => return skip(&format!("{} did not parse: {e}", clip.display())),
    };
    let frame0 = match movie.read_compressed_frame(0) {
        Ok(f) => f,
        Err(e) => return skip(&format!("frame 0: {e}")),
    };
    let timed = match movie.read_timed_meta(0) {
        Ok(t) => t,
        Err(e) => return skip(&format!("timed metadata 0: {e}")),
    };
    let uuid = match movie.canon_metadata_box() {
        Some(u) => u.to_vec(),
        None => return skip("clip has no Canon uuid box"),
    };
    let craw = movie.craw_entry().to_vec();

    let header_pack =
        match sdk.create_header_pack(&[Header::uuid(&uuid), Header::craw(&craw), Header::timed_metadata(&timed)]) {
            Ok(h) => h,
            Err(e) => return skip(&format!("CrmCreateHeaderPack: {e}")),
        };
    let format_id = match sdk.get_format_id(&header_pack) {
        Ok(f) => f,
        Err(e) => return skip(&format!("CrmGetFormatId: {e}")),
    };
    let (w, h) = match (
        sdk.get_image_info(format_id, &header_pack, image_info::CaptureWidth),
        sdk.get_image_info(format_id, &header_pack, image_info::CaptureHeight),
    ) {
        (Ok(w), Ok(h)) => (u32::from(w), u32::from(h)),
        (Err(e), _) | (_, Err(e)) => return skip(&format!("CrmGetImageInfo: {e}")),
    };

    Some(Shared {
        sdk,
        format_id,
        uuid,
        craw,
        timed,
        header_pack,
        capture: Rect { x: 0, y: 0, width: w, height: h },
        frame0,
    })
}

fn fixture() -> Option<Fixture<'static>> {
    let lock = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
    let s = SHARED.get_or_init(build_shared).as_ref()?;
    Some(Fixture { s, _lock: lock })
}

/// The premise the validators exist for, restated against the real
/// library: `CrmGetOutImageSize` reports a raster for combinations
/// `CrmGetImage` then refuses on every frame. If this ever stops holding,
/// the pre-check would be enough on its own and
/// [`check_res_param_pairs`] could go.
#[test]
fn get_out_image_size_accepts_what_get_image_rejects() {
    let Some(fx) = fixture() else { return };
    let base = fx.recipe().1;
    let pr = fx.process_ref().expect("CrmCreateProcessRef(CPU, DEVELOP|DECOMPRESS)");

    // The control: the recipe this clip was shot at develops.
    let written = fx.decode_with(&pr, &base, 16, 4).unwrap_or_else(|e| {
        panic!(
            "the full-resolution develop failed ({e}: {}) — nothing below can be attributed to the \
             pair constraints",
            fx.s.sdk.get_last_error_string().unwrap_or_default()
        )
    });
    assert!(written > 0, "the control develop wrote no bytes");

    for (name, debayer, downscale, level) in [
        ("X2 + Half", DebayerSetting::HighSpeed, Downscale::X2, DecodeLevel::Half),
        ("X4 + Quarter", DebayerSetting::HighSpeed, Downscale::X4, DecodeLevel::Quarter),
        ("HighQuality + Half", DebayerSetting::HighQuality, Downscale::X1, DecodeLevel::Half),
    ] {
        let mut p = base;
        p.debayer_setting = debayer;
        p.downscale = downscale;
        p.decode_level = level;

        let probed = fx.s.sdk.get_out_image_size(&p, &fx.s.header_pack);
        assert!(
            matches!(probed, Ok((w, h)) if w > 0 && h > 0),
            "{name}: CrmGetOutImageSize is supposed to answer for this (it validates neither pair \
             constraint); got {probed:?}"
        );

        let decoded = fx.decode_with(&pr, &p, 16, 4);
        assert!(
            matches!(decoded, Err(CrmError::SdkError(SdkError::InvalidParameter))),
            "{name}: CrmGetImage is supposed to refuse this with InvalidParameter, got {decoded:?} \
             ({})",
            fx.s.sdk.get_last_error_string().unwrap_or_default()
        );

        // And our validator agrees with the library, which is the whole
        // point of it existing.
        assert!(check_res_param_pairs(&p).is_err(), "{name}: the SDK refuses it but check_res_param_pairs does not");

        eprintln!(
            "decode_context: {name}: CrmGetOutImageSize -> {probed:?}, CrmGetImage -> {}",
            match &decoded {
                Ok(n) => format!("Ok({n})"),
                Err(e) => format!("Err({e})"),
            }
        );
    }
}

/// `set_geometry` must refuse a two-scaler rung like `X2 + Half` and not
/// move. Committing it would report a buffer size (`CrmGetOutImageSize`
/// answers for it) and leave every later decode failing with nothing to
/// restore.
#[test]
fn set_geometry_refuses_a_rung_the_sdk_would_reject_and_does_not_move() {
    let Some(fx) = fixture() else { return };
    let mut ctx = fx.context();

    let before_geometry = ctx.geometry();
    let before_dims = ctx.output_dimensions();
    let before_bpl = ctx.bytes_per_line();
    let before_size = ctx.required_output_size();
    let before_res = *ctx.res_param();
    assert!(before_size > 0 && before_dims.0 > 0 && before_dims.1 > 0);

    for (name, downscale, level) in
        [("X2 + Half", Downscale::X2, DecodeLevel::Half), ("X4 + Quarter", Downscale::X4, DecodeLevel::Quarter)]
    {
        let rung = OutputGeometry { downscale, decode_level: level, ..before_geometry };

        // The ladder query refuses it too — a UI must not advertise a step
        // the decoder cannot then serve.
        assert!(
            ctx.output_dimensions_for(&rung).is_err(),
            "{name}: output_dimensions_for must answer what set_geometry would commit"
        );

        let err = ctx.set_geometry(rung).expect_err(&format!("{name} must not be committable"));
        assert!(
            matches!(err, CrmError::InvalidParameter(msg) if msg == ERR_TWO_SCALERS),
            "{name}: expected the two-scaler refusal, got {err:?}"
        );

        assert_eq!(ctx.geometry(), before_geometry, "{name}: a refused rung moved the geometry");
        assert_eq!(ctx.output_dimensions(), before_dims, "{name}: a refused rung moved the raster");
        assert_eq!(ctx.bytes_per_line(), before_bpl, "{name}: a refused rung moved the pitch");
        assert_eq!(ctx.required_output_size(), before_size, "{name}: a refused rung moved the size");
        assert_eq!(ctx.res_param().downscale, before_res.downscale, "{name}: a refused rung moved res_param");
        assert_eq!(ctx.res_param().decode_level, before_res.decode_level, "{name}: a refused rung moved res_param");
    }

    // And the context still decodes at what it had.
    let mut dst = vec![0u8; ctx.required_output_size() as usize];
    let written = ctx
        .decode_frame_cpu_into(&fx.s.frame0, &mut dst)
        .expect("a context that refused a rung must still decode at the previous one");
    assert!(written > 0);
}

/// A reduced rung is committable, and committing it derives the debayer
/// half — the caller never has to write `HighSpeed` first, and the context
/// never holds the pair the SDK refuses.
#[test]
fn a_reduced_rung_commits_and_derives_its_debayer() {
    let Some(fx) = fixture() else { return };
    let mut ctx = fx.context();
    assert_eq!(
        ctx.res_param().debayer_setting,
        DebayerSetting::HighQuality,
        "the fixture opens at the full-resolution recipe"
    );

    let full = ctx.geometry();
    let half = OutputGeometry { decode_level: DecodeLevel::Half, ..full };

    let queried = ctx.output_dimensions_for(&half).expect("Half is a real rung");
    let size = ctx.set_geometry(half).expect("Half must commit even from a HighQuality open");

    assert_eq!(ctx.output_dimensions(), queried, "the ladder query must match what the commit installed");
    assert_eq!(ctx.res_param().debayer_setting, DebayerSetting::HighSpeed, "the debayer half of the rung");
    assert_eq!(ctx.res_param().decode_level, DecodeLevel::Half);
    assert_eq!(ctx.geometry().decode_level, DecodeLevel::Half, "geometry() reads out of res_param");
    assert_eq!(size, ctx.required_output_size());
    assert_eq!(size, ctx.bytes_per_line().saturating_mul(ctx.output_dimensions().1), "size is pitch x height");

    // The proof that the committed pair is decodable, which is what
    // `CrmGetOutImageSize` alone could not have told us.
    let mut dst = vec![0u8; size as usize];
    let written = ctx.decode_frame_cpu_into(&fx.s.frame0, &mut dst).unwrap_or_else(|e| {
        panic!("the committed Half rung must decode: {e}: {}", fx.s.sdk.get_last_error_string().unwrap_or_default())
    });
    assert!(written > 0);

    // Back up the ladder: `Full` leaves the debayer at `HighSpeed` (it is
    // legal there) and restoring `HighQuality` stays the caller's move.
    ctx.set_geometry(full).expect("Full must commit");
    assert_eq!(ctx.res_param().debayer_setting, DebayerSetting::HighSpeed);
    ctx.edit_res_param(|p| p.debayer_setting = DebayerSetting::HighQuality).expect("HighQuality is legal at Full");
    assert_eq!(ctx.geometry(), full);
}

/// The two degenerate refusals, and the end-to-end proof that a refused
/// geometry does not move the context.
///
/// The SDK is deliberately never asked about either input — that is the
/// design (see [`check_extent`]) and it is what makes the refusal testable
/// without hardware in
/// [`a_degenerate_extent_is_refused_before_any_ffi`]. Here we only check
/// that `set_geometry` / `output_dimensions_for` route through it and that
/// a refusal leaves everything where it was.
#[test]
fn a_degenerate_geometry_is_refused_and_does_not_move_the_context() {
    let Some(fx) = fixture() else { return };
    let mut ctx = fx.context();
    let before = (ctx.geometry(), ctx.output_dimensions(), ctx.bytes_per_line(), ctx.required_output_size());

    for (name, geometry, expected) in [
        (
            "a 1x1 crop",
            OutputGeometry { crop: Rect { x: 0, y: 0, width: 1, height: 1 }, ..ctx.geometry() },
            ERR_DEGENERATE_CROP,
        ),
        ("samples_per_pixel: 0", OutputGeometry { samples_per_pixel: 0, ..ctx.geometry() }, ERR_EMPTY_PITCH),
    ] {
        let err = ctx.set_geometry(geometry).expect_err(&format!("{name} must not be committable"));
        assert!(
            matches!(err, CrmError::InvalidParameter(msg) if msg == expected),
            "{name}: wrong refusal, got {err:?}"
        );
        assert!(ctx.output_dimensions_for(&geometry).is_err(), "{name}: the ladder query must refuse it too");
        assert_eq!(
            (ctx.geometry(), ctx.output_dimensions(), ctx.bytes_per_line(), ctx.required_output_size()),
            before,
            "{name}: a refused geometry moved the context"
        );
    }

    // Still decoding at what it had.
    let mut dst = vec![0u8; ctx.required_output_size() as usize];
    assert!(ctx.decode_frame_cpu_into(&fx.s.frame0, &mut dst).expect("still decodes") > 0);
}

/// `out_data_type` moves the raster but is not part of `OutputGeometry`, so
/// editing it must still re-resolve. Otherwise `required_output_size()`
/// would describe the RGB raster while `CrmGetImage` renders the (larger)
/// RAW one, which fails `MemAllocFailed`.
#[test]
fn editing_out_data_type_re_resolves_the_raster() {
    let Some(fx) = fixture() else { return };
    let mut ctx = fx.context();

    // The decompressed bayer output is single-channel 16-bit.
    ctx.set_geometry(OutputGeometry { bits_per_sample: 16, samples_per_pixel: 1, ..ctx.geometry() })
        .expect("16/1 is a legal sample layout");
    let rgb_dims = ctx.output_dimensions();
    let rgb_size = ctx.required_output_size();

    let decompressed_size = ctx
        .edit_res_param(|p| p.out_data_type = DataType::CrmDecompressed)
        .expect("CrmDecompressed is a legal output for a CrmCompressed input");

    let raw_dims = ctx.output_dimensions();
    assert_ne!(raw_dims, rgb_dims, "the RAW sensor raster is not the develop raster");
    assert!(
        raw_dims.0 >= rgb_dims.0 && raw_dims.1 >= rgb_dims.1,
        "the decompressed raster is the whole sensor readout, larger than the capture rect: \
         {raw_dims:?} vs {rgb_dims:?}"
    );
    assert_eq!(decompressed_size, ctx.required_output_size());
    assert_ne!(decompressed_size, rgb_size, "the cached size must have followed the raster");
    assert_eq!(
        ctx.bytes_per_line(),
        OutputGeometry::bytes_per_line(raw_dims.0, 16, 1),
        "the pitch must have followed the width"
    );

    // And the size we now publish is one `CrmGetImage` actually accepts —
    // the check that catches a stale-size `MemAllocFailed`.
    let mut dst = vec![0u8; decompressed_size as usize];
    let written = ctx.decode_frame_cpu_into(&fx.s.frame0, &mut dst).unwrap_or_else(|e| {
        panic!(
            "required_output_size() must be sufficient for the CrmDecompressed lane: {e}: {}",
            fx.s.sdk.get_last_error_string().unwrap_or_default()
        )
    });
    assert!(written > 0);

    // The in-place editor is the same funnel: going back re-resolves too.
    ctx.edit_res_param_in_place().out_data_type = DataType::RgbLutApplied;
    assert_eq!(ctx.output_dimensions(), rgb_dims, "the guard must have re-resolved on drop");
    assert_eq!(ctx.required_output_size(), rgb_size);

    eprintln!(
        "decode_context: 16/1 RgbLutApplied {}x{} size {rgb_size}; CrmDecompressed {}x{} size \
         {decompressed_size} (decoded {written} bytes into exactly that)",
        rgb_dims.0, rgb_dims.1, raw_dims.0, raw_dims.1,
    );
}

/// `edit_res_param_in_place` has nowhere to return an error to, so an edit the
/// pair rules refuse is rolled back rather than installed — the context
/// keeps decoding at what it had instead of failing every later frame.
/// `edit_res_param` is the form that reports.
#[test]
fn an_illegal_in_place_edit_is_rolled_back_and_the_checked_form_reports_it() {
    let Some(fx) = fixture() else { return };
    let mut ctx = fx.context();
    ctx.set_geometry(OutputGeometry { decode_level: DecodeLevel::Half, ..ctx.geometry() }).expect("Half commits");
    let dims = ctx.output_dimensions();
    let size = ctx.required_output_size();
    assert_eq!(ctx.res_param().debayer_setting, DebayerSetting::HighSpeed);

    // `HighQuality` does not exist at `Half`.
    ctx.edit_res_param_in_place().debayer_setting = DebayerSetting::HighQuality;
    assert_eq!(
        ctx.res_param().debayer_setting,
        DebayerSetting::HighSpeed,
        "an edit that would make every later CrmGetImage fail must not stick"
    );
    assert_eq!(ctx.output_dimensions(), dims);
    assert_eq!(ctx.required_output_size(), size);

    let err = ctx
        .edit_res_param(|p| p.debayer_setting = DebayerSetting::HighQuality)
        .expect_err("the checked form reports what the guard swallows");
    assert!(matches!(err, CrmError::InvalidParameter(msg) if msg == ERR_HIGH_QUALITY_NEEDS_FULL), "{err:?}");

    // Still decodable, at the rung it kept.
    let mut dst = vec![0u8; ctx.required_output_size() as usize];
    assert!(ctx.decode_frame_cpu_into(&fx.s.frame0, &mut dst).expect("still decodes") > 0);
}

/// A legal in-place edit that does NOT move the raster still goes through
/// the funnel, and the funnel leaves everything where it was — the
/// `out_buffer_type` case the GPU path needs.
#[test]
fn editing_out_buffer_type_is_allowed_and_leaves_the_raster_alone() {
    let Some(fx) = fixture() else { return };
    let mut ctx = fx.context();
    let before = (ctx.output_dimensions(), ctx.bytes_per_line(), ctx.required_output_size());

    for buffer_type in [BufferType::Cuda, BufferType::OpenCl, BufferType::Metal, BufferType::Cpu] {
        ctx.edit_res_param_in_place().out_buffer_type = buffer_type;
        assert_eq!(
            ctx.res_param().out_buffer_type,
            buffer_type,
            "{buffer_type:?} is a legal destination domain; the edit must stick"
        );
        assert_eq!(
            (ctx.output_dimensions(), ctx.bytes_per_line(), ctx.required_output_size()),
            before,
            "{buffer_type:?} does not move the output raster"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────
// Tier 2 — what the safe and unsafe decode entry points rely on
// ─────────────────────────────────────────────────────────────────────

/// Drive `future` to completion on this thread.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let std::task::Poll::Ready(out) = future.as_mut().poll(&mut cx) {
            return out;
        }
        std::thread::park();
    }
}

/// `decode_frame_cpu_into` is safe because it refuses the two inputs the SDK
/// would mishandle, before calling it: a buffer short of the output (the SDK
/// writes nothing and reports success) and a recipe whose output is not CPU
/// memory (the SDK would treat the slice as a device handle).
#[test]
fn decode_frame_cpu_into_refuses_what_the_sdk_would_mishandle() {
    let Some(fx) = fixture() else { return };
    let mut ctx = fx.context();
    let size = ctx.required_output_size() as usize;

    let err = ctx
        .decode_frame_cpu_into(&fx.s.frame0, &mut vec![0u8; size - 1])
        .expect_err("a destination one byte short must be refused");
    assert!(matches!(err, CrmError::BufferTooSmall { needed, got } if needed == size && got == size - 1), "{err:?}");

    ctx.edit_res_param_in_place().out_buffer_type = BufferType::Cuda;
    let err = ctx
        .decode_frame_cpu_into(&fx.s.frame0, &mut vec![0u8; size])
        .expect_err("a CPU slice must not be handed over as a CUDA destination");
    assert!(matches!(err, CrmError::InvalidParameter(m) if m == ERR_NOT_CPU_DESTINATION), "{err:?}");

    // And with both right, it decodes.
    ctx.edit_res_param_in_place().out_buffer_type = BufferType::Cpu;
    let mut dst = vec![0u8; size];
    assert!(ctx.decode_frame_cpu_into(&fx.s.frame0, &mut dst).expect("decodes") > 0);
}

/// The typed `image_info` keys against the real library: values that agree
/// with what the fixture already knows, text decoded within its field, and a
/// property `.CRM` data does not carry refused rather than read as garbage.
#[test]
fn image_info_reads_typed_properties_from_a_real_clip() {
    let Some(fx) = fixture() else { return };
    let (sdk, fid, hp) = (&fx.s.sdk, fx.s.format_id, &fx.s.header_pack[..]);

    let width = sdk.get_image_info(fid, hp, image_info::CaptureWidth).expect("CaptureWidth");
    assert_eq!(u32::from(width), fx.s.capture.width);
    let raw_width = sdk.get_image_info(fid, hp, image_info::RawFrameWidth).expect("RawFrameWidth");
    assert!(raw_width >= width, "the RAW frame includes the effective image");

    let model = sdk.get_image_info(fid, hp, image_info::DeviceModelName).expect("DeviceModelName");
    assert!(!model.is_empty() && model.len() <= 30 && model.is_ascii(), "{model:?}");
    let shot_at = sdk.get_image_info(fid, hp, image_info::CreationDateTime).expect("CreationDateTime");
    assert!(!shot_at.is_empty() && shot_at.len() <= 24 && shot_at.is_ascii(), "{shot_at:?}");
    let matrix = sdk.get_image_info(fid, hp, image_info::ColorMatrix).expect("ColorMatrix");
    assert!(matrix.iter().all(|v| v.is_finite()), "{matrix:?}");
    let _: bool = sdk.get_image_info(fid, hp, image_info::CaptureSystemFlag).expect("CaptureSystemFlag");

    let missing = sdk.get_image_info(fid, hp, image_info::TotalImageSize);
    assert!(
        matches!(missing, Err(CrmError::SdkError(SdkError::InvalidParameter))),
        "TotalImageSize is not available for .CRM data: {missing:?}"
    );
    eprintln!("decode_context: {model}, shot {shot_at}, {width} px wide ({raw_width} raw)");
}

/// Async decode runs on a DECOMPRESS-only context and produces exactly what
/// the synchronous path does.
#[test]
fn decode_frame_future_decompresses_like_the_sync_path() {
    let Some(fx) = fixture() else { return };
    let ctx = fx.decompress_context();
    let size = ctx.required_output_size() as usize;

    let mut sync = vec![0u8; size];
    ctx.decode_frame_cpu_into(&fx.s.frame0, &mut sync).expect("sync decompress");

    let mut dst = vec![0u8; size];
    // SAFETY: `dst` is a CPU buffer of the full output size, the context
    // names a CPU output, and the future is awaited here, not leaked.
    let submitted = unsafe { ctx.decode_frame_future(&fx.s.frame0, dst.as_mut_ptr().cast(), size as u32) };
    let out = block_on(submitted.expect("submit"))
        .unwrap_or_else(|e| panic!("async decompress: {e}: {}", fx.s.sdk.get_last_error_string().unwrap_or_default()));
    assert!(out.dst_data_size > 0);
    assert!(dst == sync, "async and sync decompress differ");
}

/// Dropping a pending future waits for the SDK: by the time `drop` returns,
/// the job has finished writing the destination. This is what lets the
/// destination and inputs be freed right after.
#[test]
fn dropping_a_pending_decode_future_waits_for_the_job() {
    let Some(fx) = fixture() else { return };
    let ctx = fx.decompress_context();
    let size = ctx.required_output_size() as usize;

    let mut sync = vec![0u8; size];
    ctx.decode_frame_cpu_into(&fx.s.frame0, &mut sync).expect("sync decompress");

    let mut dst = vec![0u8; size];
    // SAFETY: as above; the future is dropped, not leaked.
    let future =
        unsafe { ctx.decode_frame_future(&fx.s.frame0, dst.as_mut_ptr().cast(), size as u32) }.expect("submit");
    drop(future);
    assert!(dst == sync, "drop returned before the SDK finished writing the destination");
}

/// The SDK refuses `CrmGetImageAsync` on a combined `DEVELOP | DECOMPRESS`
/// ref ("CrmGetImageAsync can execute either decompress or develop processing
/// asynchronously."). Pinned so a future SDK that lifts the restriction is
/// noticed, and the docs updated with it.
#[test]
fn decode_frame_future_is_refused_on_a_combined_context() {
    let Some(fx) = fixture() else { return };
    let ctx = fx.context();
    let size = ctx.required_output_size() as usize;
    let mut dst = vec![0u8; size];
    // SAFETY: `dst` is a CPU buffer of the full output size; a refused
    // submission starts no job.
    let refused = unsafe { ctx.decode_frame_future(&fx.s.frame0, dst.as_mut_ptr().cast(), size as u32) };
    assert!(
        matches!(refused, Err(CrmError::SdkError(SdkError::InvalidParameter))),
        "CRMSDK 2.10 runs async decode on single-stage refs only: {refused:?}"
    );
}

/// `get_aces_idt` reads the applied IDT as a raw value plus its name, after
/// an ACES develop selected one automatically.
#[test]
fn get_aces_idt_reports_the_idt_an_aces_develop_applied() {
    let Some(fx) = fixture() else { return };
    let pr = fx.process_ref().expect("CrmCreateProcessRef(CPU, DEVELOP|DECOMPRESS)");
    let (mut dev_param, res_param, geometry) = fx.recipe();
    dev_param.color_space = ColorSpace::Aces;
    dev_param.gamma = Gamma::Linear;
    dev_param.aces_idt = AcesIdt::Auto;

    let (w, h) = fx.s.sdk.get_out_image_size(&res_param, &fx.s.header_pack).expect("out size");
    let bpl = OutputGeometry::bytes_per_line(w, geometry.bits_per_sample, geometry.samples_per_pixel);
    let mut dst = vec![0u8; (bpl * h) as usize];
    // SAFETY: `pr` is live; `dst` is a CPU buffer of the full output size and
    // the recipe names a CPU output.
    let developed = unsafe {
        fx.s.sdk.get_image(
            pr.process_ref(),
            dst.as_mut_ptr().cast(),
            dst.len() as u32,
            geometry.bits_per_sample,
            geometry.samples_per_pixel,
            bpl,
            SampleSequence::Rgba16BitLe,
            &fx.s.frame0,
            &dev_param,
            &res_param,
            &fx.s.header_pack,
        )
    };
    developed.unwrap_or_else(|e| panic!("ACES develop: {e}: {}", fx.s.sdk.get_last_error_string().unwrap_or_default()));

    // SAFETY: `pr` is live, and the develop above has completed.
    let idt = unsafe { fx.s.sdk.get_aces_idt(pr.process_ref()) }.expect("CrmGetAcesIDT");
    assert!(idt.idt().is_some(), "an IDT CrmSdkTypes.h defines: {idt:?}");
    assert!(!idt.name.is_empty() && idt.name.is_ascii(), "{idt:?}");
    // SAFETY: as above; nothing is pending on the ref.
    let _ = unsafe { fx.s.sdk.reset_process_ref(pr.process_ref()) };
    eprintln!("decode_context: ACES develop applied {:?} ({:?})", idt.idt(), idt.name);
}

/// The SDK's half of spec § 9.2.1 against the real library: while another
/// thread holds the CUDA exclusion, creating a CUDA process ref waits, and
/// creating a CPU one does not — CPU processing is outside the rule.
///
/// Through `lease_process_ref`, so the refs are parked rather than released
/// moments after creation. Nothing else in the suite leases a DEVELOP-only
/// ref, so both leases really do create one.
#[test]
fn cuda_process_ref_calls_wait_for_the_cuda_exclusion() {
    let Some(fx) = fixture() else { return };
    let (sdk, format_id) = (fx.s.sdk.clone(), fx.s.format_id);
    let create_on = |device: Device| {
        let sdk = sdk.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let gpu = if device == Device::Cuda { Gpu::Gpu1 } else { Gpu::NotUse };
            let _ = tx.send(sdk.lease_process_ref(format_id, device, gpu, ProcessType::DEVELOP, Resolution::K8));
        });
        rx
    };

    let held = crate::cuda_exclusion();

    let cpu = create_on(Device::Cpu)
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("a CPU process ref must not wait for the CUDA exclusion")
        .expect("CrmCreateProcessRef(CPU)");

    let cuda = create_on(Device::Cuda);
    assert!(
        cuda.recv_timeout(std::time::Duration::from_millis(500)).is_err(),
        "CrmCreateProcessRef(CUDA) ran while another thread held the CUDA exclusion"
    );
    drop(held);
    let cuda = cuda.recv_timeout(std::time::Duration::from_secs(120)).expect("released, so it proceeds");

    assert_eq!(cpu.process_ref().device(), Device::Cpu);
    match cuda {
        Ok(lease) => assert_eq!(lease.process_ref().device(), Device::Cuda),
        Err(e) => eprintln!("decode_context: no CUDA process ref on this machine ({e}); the wait was still observed"),
    }
}

/// Dropping a context parks its ref, and the next context with the same
/// parameters gets that ref back instead of a new one. This is what keeps the
/// suite — and every caller — off the create-then-release path that crashes
/// the Windows SDK (see `CrmSdkLibrary::create_process_ref`).
#[test]
fn a_dropped_contexts_ref_is_parked_and_reused_not_released() {
    let Some(fx) = fixture() else { return };
    let first = fx.context();
    let leased = first.process_ref();
    let idle = fx.s.sdk.idle_process_refs();

    drop(first);
    assert_eq!(fx.s.sdk.idle_process_refs(), idle + 1, "dropping the context must park its ref, not release it");

    let second = fx.context();
    assert_eq!(second.process_ref(), leased, "the parked ref must be the one the next context gets");
    assert_eq!(fx.s.sdk.idle_process_refs(), idle);
}

/// Leases held at the same time never share a ref, and a lease never gets a
/// ref created with different parameters, even when one is parked.
#[test]
fn leases_never_share_a_ref_or_cross_parameters() {
    let Some(fx) = fixture() else { return };
    let (a, b) = (fx.context(), fx.context());
    assert_ne!(a.process_ref(), b.process_ref(), "two live contexts were handed the same ref");
    let develop_refs = [a.process_ref(), b.process_ref()];

    let decompress = fx.decompress_context();
    assert!(!develop_refs.contains(&decompress.process_ref()), "a DECOMPRESS lease got a DEVELOP|DECOMPRESS ref");

    drop((a, b));
    let again = fx.decompress_context();
    assert!(
        !develop_refs.contains(&again.process_ref()),
        "with DEVELOP|DECOMPRESS refs parked, a DECOMPRESS lease must still not get one"
    );
}

/// A reused ref decodes exactly as it did before it was parked: the reset on
/// parking leaves nothing of the previous holder behind.
#[test]
fn a_reused_ref_decodes_bit_identically() {
    let Some(fx) = fixture() else { return };
    let decode = |ctx: &DecodeContext| {
        let mut dst = vec![0u8; ctx.required_output_size() as usize];
        let written = ctx.decode_frame_cpu_into(&fx.s.frame0, &mut dst).expect("the full-resolution develop");
        assert!(written > 0);
        dst
    };

    let first = fx.context();
    let leased = first.process_ref();
    let before = decode(&first);
    drop(first);

    let second = fx.context();
    assert_eq!(second.process_ref(), leased, "the test needs the parked ref back");
    assert!(decode(&second) == before, "the reused ref developed different pixels");
}
