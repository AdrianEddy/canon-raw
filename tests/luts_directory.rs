// SPDX-License-Identifier: MIT OR Apache-2.0

//! `CrmSetLUTsDirectory` takes the directory that **contains** `CanonLUTs` —
//! not the `CanonLUTs` folder itself.
//!
//! The SDK appends the folder name (spec § 7.2.18: *"a path of the directory
//! where the CanonLUTs folder is stored"*; inside `CrmSdk.dll` the argument is
//! only ever consumed as `sprintf(buf, "%s\CanonLUTs", path)`), so pointing at
//! the LUT folder makes it look for a `CanonLUTs/CanonLUTs` that is not there.
//!
//! ## Why this needs a develop, and a specific staging layout
//!
//! `CrmSetLUTsDirectory` returns `CRM_ERR_OK` for **any** path, including one
//! that does not exist: a directory it cannot use is a silent no-op that leaves
//! the previously loaded LUTs in force. So the return code proves nothing and
//! the only honest probe is a develop whose recipe needs a 3-D LUT.
//!
//! And the LUTs the SDK loads by default — `<directory of CrmSdk.dll>/CanonLUTs`,
//! read during `CrmInitializeSDK*` — would satisfy that develop no matter what
//! this test then passes; that default silently masks a wrong argument. So the
//! gate **requires** a `CrmSdk.dll` with no `CanonLUTs` beside it (the SDK zip's
//! own `Win_Lib/x64/` layout is one) and skips otherwise, saying why.
//!
//! ## ONE process ref, held for the whole gate — this is not a style choice
//!
//! Both develops run on a single [`ScopedProcessRef`], with
//! `CrmResetProcessRef` between them. The obvious shape — mint a ref inside the
//! develop helper and release it on the way out — made this binary crash **2 in
//! 24** runs under an eight-way parallel load, always inside the SDK and always
//! before the first line of output, which is easy to mistake for a problem
//! with whichever SDK call ran last.
//!
//! Measured with a vectored exception handler on the crashing process:
//!
//! * `CrmGetImage` **lazily builds a worker pool on the process ref** — 76 extra
//!   OS threads and ~2 GB of committed arena on this 6000 × 3164 clip, on the
//!   first decode and not before (a create/release pair with no decode in
//!   between moves neither number).
//! * `CrmResetProcessRef` leaves that pool up. `CrmReleaseProcessRef` tears it
//!   down — threads and arena both — and **returns before every worker has
//!   exited**.
//! * The fault is an `ACCESS_VIOLATION` writing `0x24` on a thread whose whole
//!   stack is `ntdll!RtlUserThreadStart` → `BaseThreadInitThunk` →
//!   `CrmSdk.dll+0x241804` → `CrmSdk.dll+0x1f434` →
//!   `ntdll!RtlEnterCriticalSection`'s contended path. `0x24` is
//!   `RTL_CRITICAL_SECTION_DEBUG::ContentionCount`, reached through a
//!   `DebugInfo` that `RtlDeleteCriticalSection` has already NULLed: a straggler
//!   worker entering a critical section that the release deleted under it.
//!
//! So every `CrmReleaseProcessRef` after a decode is a dice roll, and it needs
//! contention to lose — which is why the crash looks random and only shows up on
//! a loaded box. One ref per gate is one roll instead of one per develop, and it
//! happens when nothing else is in flight. `src/decode_context_tests.rs` holds
//! its ref the same way and for the same reason; `DecodeContext` itself owns
//! exactly one for its whole life.
//!
//! **Do not record a crash here as an SDK limitation without first ruling this
//! out.**
//!
//! ## What the gate additionally establishes
//!
//! Holding one ref across both develops means the second `set_luts_directory`
//! lands on a process ref that already exists. It takes effect anyway — the
//! refused develop and the successful one run on the *same* ref. The LUT set is
//! therefore process-global state, not something a ref captures at creation.
//!
//! ## Provisioning
//!
//! | env | default | what it is |
//! |---|---|---|
//! | `CRM_TEST_SDK_ROOT` | `<repo>/sdk/CRMSDK210/CRMSDK` | the unpacked SDK zip's `CRMSDK` folder |
//! | `CRM_TEST_SDK_DLL` | `<sdk root>/Win_Lib/x64/CrmSdk.dll` | the library to load |
//! | `CRM_TEST_LUTS_PARENT` | `<sdk root>` | the directory CONTAINING `CanonLUTs` |
//! | `CRM_TEST_CLIP` | — | any `.CRM` whose camera supports BT.2100 PQ |
//!
//! Anything required that is unset or absent ⇒ printed skip, like the crate's
//! other SDK-touching tests.

use canon_raw::crm_parser::CrmMovie;
use canon_raw::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The SDK zip as unpacked into the (git-ignored) `sdk/` folder of this repo.
/// `CanonLUTs/` lives here; the library lives two levels down in
/// `Win_Lib/x64/`, which is what makes this tree usable as the "default cannot
/// rescue us" fixture.
const DEFAULT_SDK_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/sdk/CRMSDK210/CRMSDK");

struct Fixture {
    dll: PathBuf,
    /// The directory that contains `CanonLUTs`. The RIGHT argument.
    luts_parent: PathBuf,
    clip: PathBuf,
}

impl Fixture {
    /// The WRONG argument — the `CanonLUTs` folder itself.
    fn luts_folder_itself(&self) -> PathBuf {
        self.luts_parent.join("CanonLUTs")
    }
}

fn skip(why: &str) -> Option<Fixture> {
    eprintln!("SKIP luts_directory: {why}");
    None
}

fn fixture() -> Option<Fixture> {
    let sdk_root = PathBuf::from(env_or("CRM_TEST_SDK_ROOT", DEFAULT_SDK_ROOT));

    let dll = match std::env::var_os("CRM_TEST_SDK_DLL") {
        Some(p) => PathBuf::from(p),
        None if cfg!(windows) => sdk_root.join("Win_Lib").join("x64").join("CrmSdk.dll"),
        // The Linux / macOS SDK bundles ship as archives with no fixed unpacked
        // layout, so there is nothing to guess. Point `CRM_TEST_SDK_DLL` at the
        // library and the gate runs there too.
        None => return skip("CRM_TEST_SDK_DLL unset (no default layout off Windows)"),
    };
    if !dll.is_file() {
        return skip(&format!("{} absent", dll.display()));
    }

    let luts_parent = PathBuf::from(env_or("CRM_TEST_LUTS_PARENT", &sdk_root.to_string_lossy()));
    if !luts_parent.join("CanonLUTs").is_dir() {
        return skip(&format!("{}/CanonLUTs absent", luts_parent.display()));
    }

    // THE precondition. With `CanonLUTs` beside the library, `CrmInitializeSDK`
    // loads it and every later argument — right or wrong — develops fine, so the
    // gate would pass while asserting nothing.
    let dll_dir = dll.parent().unwrap_or(Path::new("."));
    if dll_dir.join("CanonLUTs").is_dir() {
        return skip(&format!(
            "{} has `CanonLUTs` beside it — the SDK's own default would load those and mask the argument \
             under test. Point CRM_TEST_SDK_DLL at a copy of the library that has no `CanonLUTs` sibling.",
            dll_dir.display()
        ));
    }

    let Some(clip) = std::env::var_os("CRM_TEST_CLIP").map(PathBuf::from) else {
        return skip("CRM_TEST_CLIP is not set");
    };
    if !clip.is_file() {
        return skip(&format!("{} absent", clip.display()));
    }

    // The SDK dlopens `libiomp5md.dll` / `CESScalingLib.dll` / `crxdec.dll` /
    // the CUDA runtime from beside itself by bare name.
    add_to_loader_search_path(dll_dir);

    Some(Fixture { dll, luts_parent, clip })
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Make the SDK's siblings findable: `CrmSdk.dll` `LoadLibrary`s
/// `crxdec.dll` / `libiomp5md.dll` / `CESScalingLib.dll` / the CUDA runtime by
/// bare name, and the standard search order does not include the directory of
/// the DLL doing the loading.
///
/// `SetDllDirectoryW` rather than an edit to `PATH`: `std::env::set_var` is
/// unsound in a process that has other threads running — and a `#[test]` binary
/// always does — whereas this is a documented, thread-safe, process-wide loader
/// setting that does exactly the same job.
#[cfg(windows)]
fn add_to_loader_search_path(dir: &Path) {
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
    wide.push(0);
    // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the call.
    unsafe { windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW(wide.as_ptr()) };
}

/// Off Windows there is nothing useful to do here: `dlopen` resolves a
/// library's `DT_NEEDED` against the search path as it stood at `exec` time, so
/// a run-time `LD_LIBRARY_PATH` edit would not help even if it were sound.
/// Export it before `cargo test`, alongside the `CRM_TEST_SDK_DLL` this gate
/// already requires there.
#[cfg(not(windows))]
fn add_to_loader_search_path(_dir: &Path) {}

/// A process ref released exactly once, on drop.
///
/// See the module header: `CrmReleaseProcessRef` is the call that races, so the
/// gate makes exactly one of them and makes it last.
struct ScopedProcessRef {
    sdk: Arc<CrmSdkLibrary>,
    raw: ProcessRef,
}

impl Drop for ScopedProcessRef {
    fn drop(&mut self) {
        // SAFETY: created by the gate and released only here, with nothing
        // pending on it.
        let _ = unsafe { self.sdk.release_process_ref(self.raw) };
    }
}

/// Everything one develop of frame 0 needs that does not depend on the LUT
/// directory — read once, so the two develops differ in nothing but the
/// directory that was in force.
struct Recipe {
    format_id: FormatId,
    frame: Vec<u8>,
    header_pack: Vec<u8>,
    dev_param: DevParamV13,
    res_param: ResParamV13,
    bits: u32,
    samples: u32,
    bytes_per_line: u32,
    size: u32,
}

/// The develop recipe that NEEDS a 3-D LUT: BT.2100 PQ into BT.2020.
fn recipe(sdk: &Arc<CrmSdkLibrary>, clip: &Path) -> Result<Recipe> {
    let mut movie = CrmMovie::load(std::fs::File::open(clip)?)?;
    let frame = movie.read_compressed_frame(0)?;
    let timed_meta = movie.read_timed_meta(0)?;
    let uuid = movie.canon_metadata_box().ok_or(CrmError::MissingData("Canon uuid box"))?.to_vec();
    let craw = movie.craw_entry().to_vec();

    let header_pack =
        sdk.create_header_pack(&[Header::uuid(&uuid), Header::craw(&craw), Header::timed_metadata(&timed_meta)])?;
    let format_id = sdk.get_format_id(&header_pack)?;
    let w = u32::from(sdk.get_image_info(format_id, &header_pack, image_info::CaptureWidth)?);
    let h = u32::from(sdk.get_image_info(format_id, &header_pack, image_info::CaptureHeight)?);

    let dev_param = DevParamV13 {
        size: std::mem::size_of::<DevParamV13>() as _,
        struct_id: StructId::DevParamV13,
        format_id,
        wb_setting: WbSetting::Record,
        click_wb_point: Point { x: 0, y: 0 },
        tint: 0.0,
        exposure: 0.0,
        cct: 0,
        sharpness: 0,
        // The pair that pulls in `CinemaGamut_*-to-BT2020_BT2100PQ_*.bin`.
        gamma: Gamma::Bt2100Pq,
        noise: 0,
        aces_idt: AcesIdt::Auto,
        color_space: ColorSpace::Bt2020,
        iso_setting: IsoSetting::Record,
        iso_speed: 0,
        wb_gain: WbGain { r: 0, g1: 0, g2: 0, b: 0 },
    };
    let res_param = ResParamV13 {
        size: std::mem::size_of::<ResParamV13>() as _,
        struct_id: StructId::ResParamV13,
        format_id,
        debayer_setting: DebayerSetting::HighQuality,
        downscale: Downscale::X1,
        crop: Rect { x: 0, y: 0, width: w, height: h },
        frame_rendering: FrameRendering::Alternate,
        lens_squeeze: LensSqueeze::Off,
        in_data_type: DataType::CrmCompressed,
        in_buffer_type: BufferType::Cpu,
        out_data_type: DataType::RgbLutApplied,
        out_buffer_type: BufferType::Cpu,
        decode_level: DecodeLevel::Full,
    };

    let (ow, oh) = sdk.get_out_image_size(&res_param, &header_pack)?;
    let (bits, samples) = (8u32, 3u32);
    let bytes_per_line = OutputGeometry::bytes_per_line(ow, bits, samples);
    let size = bytes_per_line * oh;

    Ok(Recipe { format_id, frame, header_pack, dev_param, res_param, bits, samples, bytes_per_line, size })
}

/// One develop of frame 0 on the caller's long-lived ref. `Ok(mean)` on success
/// so the caller can also see that the develop produced a real image,
/// `Err(sdk message)` otherwise.
///
/// `CrmResetProcessRef` afterwards, unconditionally: a refused develop leaves
/// the ref mid-recipe, and the next call on it must start clean. Reset is the
/// operation that does NOT tear the worker pool down, which is exactly why this
/// gate can reuse the ref at all.
fn develop_hdr_frame0(sdk: &Arc<CrmSdkLibrary>, r: &Recipe, process_ref: ProcessRef) -> Result<f64> {
    let dst = CrmMemory::new(sdk.clone(), r.size, Device::Cpu)?;
    // SAFETY: `process_ref` is the gate's live ref; `dst` is a CPU allocation
    // of `r.size` bytes and the recipe names a CPU output.
    let written = unsafe {
        sdk.get_image(
            process_ref,
            dst.as_ptr(),
            r.size,
            r.bits,
            r.samples,
            r.bytes_per_line,
            SampleSequence::Rgb,
            &r.frame,
            &r.dev_param,
            &r.res_param,
            &r.header_pack,
        )
    };
    // SAFETY: as above; nothing is pending on the ref.
    let _ = unsafe { sdk.reset_process_ref(process_ref) };

    let written = written?;
    // SAFETY: `CrmGetImage` reported `written` bytes into `dst`, which is
    // `size >= written` bytes of CRM-owned memory alive until `dst` drops.
    let pixels = unsafe { std::slice::from_raw_parts(dst.as_ptr() as *const u8, written as usize) };
    Ok(pixels.iter().map(|b| *b as u64).sum::<u64>() as f64 / pixels.len() as f64)
}

/// The gate. Pointing at `CanonLUTs` itself must NOT work; pointing at its
/// parent must.
///
/// Order matters and is the point: the wrong directory goes first, while the
/// SDK holds no LUTs at all, so its develop can only fail. Setting the right
/// one afterwards loads them, and the second develop can only succeed.
#[test]
fn set_luts_directory_wants_the_parent_of_canon_luts() {
    let Some(fx) = fixture() else { return };

    // SAFETY: the SDK the provisioning table names.
    let sdk = Arc::new(unsafe { CrmSdkLibrary::load(fx.dll.as_path()) }.expect("load CrmSdk"));
    sdk.initialize_sdk().expect("CrmInitializeSDK");
    let recipe = recipe(&sdk, &fx.clip).expect("read frame 0 and resolve its develop recipe");

    // ── The wrong level: `<parent>/CanonLUTs` ────────────────────────────
    // Set BEFORE the first `create_process_ref`, which is the order § 7.2.18
    // asks for.
    let wrong = fx.luts_folder_itself();
    assert_eq!(
        sdk.set_luts_directory(&wrong, true).map_err(|e| e.to_string()),
        Ok(()),
        "the SDK accepts ANY directory — a wrong one is a silent no-op, never an error. \
         If this ever starts failing, the caller-side existence checks this crate documents can go."
    );

    // The one ref both develops run on, and the one `CrmReleaseProcessRef` this
    // binary performs. See the module header for why that count matters.
    let process_ref = ScopedProcessRef {
        sdk: sdk.clone(),
        raw: sdk
            .create_process_ref(
                recipe.format_id,
                Device::Cpu,
                Gpu::NotUse,
                ProcessType::DEVELOP | ProcessType::DECOMPRESS,
                Resolution::K8,
            )
            .expect("CPU develop/decompress process ref"),
    };

    let err = match develop_hdr_frame0(&sdk, &recipe, process_ref.raw) {
        Err(e) => format!("{e}: {}", sdk.get_last_error_string().unwrap_or_default()),
        Ok(mean) => panic!(
            "a BT.2100 PQ develop SUCCEEDED (mean {mean:.4}) with the LUT directory set to `{}` and no \
             `CanonLUTs` beside the library — so `CrmSetLUTsDirectory` does NOT append the folder name \
             after all, and this crate's documentation is wrong the other way",
            wrong.display()
        ),
    };
    if !err.contains("LUT is not available") {
        eprintln!(
            "SKIP luts_directory: the BT.2100 PQ develop failed for an unrelated reason ({err}) — this \
             clip's camera does not reach the 3-D LUT path, so it cannot witness the argument under test"
        );
        return;
    }

    // ── The right level: the parent ──────────────────────────────────────
    // On the ref that already exists, deliberately: a second `set_luts_directory`
    // is what the gate is measuring, and it takes effect on a live ref.
    sdk.set_luts_directory(&fx.luts_parent, true).expect("set_luts_directory(parent)");
    let mean = develop_hdr_frame0(&sdk, &recipe, process_ref.raw).unwrap_or_else(|e| {
        panic!(
            "the BT.2100 PQ develop still failed with the LUT directory set to `{}` — the directory that \
             CONTAINS `CanonLUTs`: {e}: {}",
            fx.luts_parent.display(),
            sdk.get_last_error_string().unwrap_or_default()
        )
    });
    assert!(mean > 0.0, "the develop produced an all-black frame (mean {mean})");
    eprintln!(
        "luts_directory: `{}` develops (mean {mean:.4}); `{}` does not",
        fx.luts_parent.display(),
        wrong.display()
    );
}
