# canon-raw — Rust bindings for the Canon Cinema RAW Light SDK

Safe, dynamically-loaded Rust bindings for Canon's **CRM SDK** — decode and develop
`.CRM` (Cinema RAW Light) and `.RMF` clips from Canon Cinema EOS cameras, on the CPU or
straight into CUDA / OpenCL / Metal memory.

<p align="center">
  <a href="#"><img alt="License: MIT/Apache-2.0" src="https://img.shields.io/badge/license-MIT%2FApache--2.0-informational"></a>
  <a href="#"><img alt="Rust Edition" src="https://img.shields.io/badge/rust-Edition_2024-blue"></a>
  <a href="#"><img alt="Platforms" src="https://img.shields.io/badge/platforms-Windows%20%7C%20Linux%20%7C%20macOS-success"></a>
</p>

**Based on CRMSDK 2.10.**

## What is this

Canon ships the CRM SDK as a closed binary with a flat C entry-point table: a handful of
init calls, an opaque `CrmProcessRef`, and one fourteen-argument `CrmGetImage` that takes
its real configuration through two `#[repr(C)]` parameter structs and a *serialized header
pack* — a blob you build from raw ISOBMFF boxes lifted out of the clip file. There is no
"open a clip" call. The SDK never reads your file; you hand it the boxes.

So this crate is two layers, and you will use both:

| Layer | What it does | Needs the SDK? |
|---|---|---|
| `crm_parser::CrmMovie` | Walks the `.CRM` container, hands you the Canon `uuid` box, the `CRAW` sample entry, and the per-frame compressed + timed-metadata samples. | **No** |
| `CrmSdkLibrary`, `DecodeContext` | Loads the SDK, resolves output geometry, decompresses and develops frames. | Yes |

`CrmMovie` is thin glue over the SDK-free `crm-container` crate, so the container layer is
shared with pure-Rust CRM decoders and stays usable —
for probing, indexing, thumbnail scheduling — on machines that have no Canon SDK at all.

## Why it looks like this

**The SDK is loaded, never linked.** `libloading` resolves every entry point at runtime
from a path you supply. Canon distributes the SDK under its own licence and it is not
redistributable, so a build-time link would make the crate unbuildable for anyone without
it. Your binary starts fine on a machine with no SDK; it fails at `CrmSdkLibrary::load`
with an error you can act on, which is what a per-format backend in a larger stack needs.

**`DecodeContext` exists because the per-frame call is a trap.** Developing frame *n* is a
two-call sequence — rebuild the header pack around frame *n*'s timed metadata, then call
`CrmGetImage` — and the destination buffer size depends on parameters spread across the
resolution struct. Written out longhand at the call site, the size and the struct drift
apart the first time someone changes a knob, and the SDK writes past a buffer sized for the
old geometry. `DecodeContext` owns the process ref, both parameter structs, the header
pack and the resolved raster together, and makes the coupling structural.

**The raster is the SDK's answer; only the packing is arithmetic.** `OutputGeometry` is a
pure *input*: crop, downscale, decode level, and your sample layout. The output **width and
height** are never computed from it — they are asked of the SDK's own `CrmGetOutImageSize`,
which returns those two numbers and nothing else, and read back through
`output_dimensions()`. This is not pedantry — `crop / factor` is wrong. The SDK floors each
edge to an even number *after* dividing, so a 6000 × 3164 clip at quarter comes back
1500 × **790**, not 791, and the missing row is dropped off the bottom rather than rounded.
Re-deriving that by hand is a second, wrong source of truth.

Row pitch and buffer size are then ours, computed off that answer: `bytes_per_line()` is the
`inBytesPerLine` packing rule (`OutputGeometry::bytes_per_line`, a `const fn` mirroring
Canon's own `TestCrmSdk.cpp`) applied to the SDK's width, and `required_output_size()` is
that pitch times the SDK's height. There is no API that hands you either; the sample code
does the same two multiplications. What `DecodeContext` buys you is that they are done once,
from the SDK's dimensions, at the moment the geometry changes. And on the GPU lanes the
pitch is not even read: spec § 7.3.1 says `inBytesPerLine` "is ignored" for any
`out_buffer_type` other than `BufferType::Cpu`. There it is not a layout you can choose —
it survives only inside `required_output_size()`, as the number your device allocation has
to cover.

**There is one store, so there is nothing to keep in sync.** `ResParamV13` is the only copy
of the raster inputs the context holds; `crop` / `downscale` / `decode_level` live there and
nowhere else, and `geometry()` is a *view* read back out of it rather than a second field
that has to agree. So the struct the SDK is handed per frame cannot describe a raster
different from the one your buffer was sized for — not because two copies are mirrored, but
because there is only one.

Every path that writes it — `new()`, `set_geometry()`, `edit_res_param()`,
`edit_res_param_in_place()` — funnels through the same private resolve-then-commit pair. The
resolve applies both `ResParamV13` pair constraints, then asks `CrmGetOutImageSize`, and
computes width / height / pitch / size **into a local**; the commit is the only code that
assigns, and it runs after every fallible step has passed. Transactionality is therefore
structural: a geometry the rules or the SDK reject leaves the context exactly as it was, and
no call site has to restate the guarantee.

`edit_res_param(|p| …)` is the checked form — every field is fair game and you see the
error. `edit_res_param_in_place()` is the same funnel through a guard, for the common
single-field write; it reaches every field too (`out_buffer_type` included — the GPU lane
needs to name its destination domain — and `lens_squeeze` / `out_data_type` re-resolve like
anything else), and because `Drop` has nowhere to return an error to, an edit the rules or
the SDK refuse is **rolled back** rather than installed. There is no forbidden-field list.

What genuinely never re-resolves is colour: white balance, exposure and gamma via
`dev_param_color_mut()`, and per-frame timed metadata via `set_timed_meta()`. Neither can
move the raster, so neither pays for an FFI hop.

## Runtime requirements

### The SDK library

`CrmSdk.dll` (Windows), `libCrmSdk.so` (Linux, beta in 2.10), or `CrmSdk.framework`
(macOS). Pass the path to `CrmSdkLibrary::load`.

macOS ships the framework and only the framework — `Mac_Lib.zip` contains no
`libCrmSdk.dylib`, and the release note lists the framework alone — so the path you hand
`load` is the Mach-O *inside* the bundle: `…/CrmSdk.framework/CrmSdk`, a symlink to
`Versions/A/CrmSdk`. Unpack the zip with something that preserves symlinks, or name
`Versions/A/CrmSdk` directly. The bundle lives under `Mac_Lib/x86_64/`, but do not read the
folder name as a limit — the binary in it is universal, x86_64 and arm64 both.

On Windows the SDK also loads a CUDA runtime and the Intel OpenMP runtime from beside
itself, so point the loader at the SDK's own directory (`AddDllDirectory`, or put it on
`PATH`) rather than copying out the single DLL. The `CrmRsc*.bin` resource files that ship
next to the library — inside the bundle's `Resources/`, extension-less, on macOS — resolve
on their own and need no configuration.

### The `CanonLUTs` directory

This is the one that costs people an afternoon.

`CanonLUTs` is a folder of `.bin` 3D colour-transform LUTs shipped with the SDK, one per
conversion and named `<inGamut>_<inGamma>-to-<outGamut>_<outGamma>` — 45 of them in 2.10,
e.g. `CinemaGamut_BT2100PQ-to-BT2020_BT2100PQ_V2.bin`, which is what an HDR PQ develop of a
CinemaGamut clip reaches for. The set is closed: `CrmSdk.dll` carries exactly those 45 names
as a table and opens each as `<CanonLUTs>/<name>.bin`, so the lookup is by table entry and a
file you add yourself is never opened — nor is there a LUT for every combination you can
name. CanonLog2 → BT.2100 PQ, for instance, does not exist.

The SDK reads the whole set up front — during `CrmInitializeSDK*`, and again in full on
every `set_luts_directory` — rather than opening a LUT when a develop asks for one. That is
measurable from outside: with `CanonLUTs` beside the library, `CrmInitializeSDK` reads
9.7 MB more than it does without, exactly the size of the folder, and every later
`set_luts_directory` that resolves reads the same 9.7 MB again. A develop consults one only
when the recipe asks for a transform that needs it: an `out_data_type` of
`DataType::RgbLutApplied` combined with a colour space and gamma that are not the clip's
own. A plain Rec.709 / `ColorSpace::Original` recipe needs no 3D LUT and produces
byte-identical output with the directory misconfigured — which is exactly why the problem
surfaces later, on the first HDR (BT.2020 PQ / HLG) develop, as
`"The necessary LUT is not available"` from
`get_last_error_string()`.

**Pass the directory that *contains* `CanonLUTs`, not `CanonLUTs` itself.** The SDK
appends the folder name for you (spec § 7.2.18: *"Sets a path of the directory where the
CanonLUTs folder is stored"*). Pointing at the LUT folder makes it look for a
`CanonLUTs/CanonLUTs` that does not exist:

| What you do | Where the SDK looks |
|---|---|
| never call `set_luts_directory` | `<directory of CrmSdk.dll>/CanonLUTs` |
| `set_luts_directory(p, false)` | `<directory of CrmSdk.dll>/<p>/CanonLUTs` |
| `set_luts_directory(p, true)` | `<p>/CanonLUTs` |

**Canon's own layout does not satisfy that default, so on the shipped SDK the call is
mandatory.** The zip stages `CanonLUTs` as a sibling of `Win_Lib`, while the library sits two
levels further down in `Win_Lib/x64/CrmSdk.dll` — the directory beside the DLL holds the
`CrmRsc*.bin` resources, `crxdec.dll` and the CUDA/OpenMP runtimes, and no LUTs whatsoever.
Unpack the SDK as it comes and the argument you want is the `CRMSDK` folder itself, two
levels up from the library. (`examples/decode.rs` passes exactly that, and
`tests/luts_directory.rs` depends on the same shape from the other side: it *requires* a
`CrmSdk.dll` with no `CanonLUTs` beside it, because otherwise the init-time default would
load the LUTs and the argument under test would prove nothing.)

Staging `CanonLUTs` beside the library is still a perfectly good thing to do — flatten
library, resources and LUTs into one directory when you ship your own app and the default
resolves, and you can skip the call. Just know that it is a layout you chose, not the one
Canon hands you.

**A wrong directory is silent.** `CrmSetLUTsDirectory` returns `CRM_ERR_OK` for a
directory with no `CanonLUTs` in it, and for a path that does not exist at all; the call
simply leaves the previously loaded LUTs in force. So an `Ok` proves nothing, and on a
tree that stages `CanonLUTs` beside the library the default masks a wrong argument
completely. If you need the LUTs, check `dir.join("CanonLUTs")` yourself.

Ordering: the call must come **after** SDK initialization, and spec § 7.2.18 asks for it
**before** the first `create_process_ref`. That is still the order to write, but the second
half is not something the 2.10 library enforces — the loaded LUT set is process-global, not
state a ref captures when it is made. `tests/luts_directory.rs` measures it from the other
side: one process ref, created while the directory was wrong, refuses the HDR develop and
then serves it once the *parent* is set, with no new ref in between.

### A clip

`CrmMovie::load` requires a per-video-frame timed-metadata track — the SDK reads
`timed_meta[frame_no]` for every frame as its colour-development metadata — and refuses a
clip whose timed-metadata sample count does not match its video frame count.

## Example

Container parse, SDK init, and a per-frame develop loop. It is `no_run`: it compiles
against the real API, but running it needs Canon's SDK, its `CanonLUTs`, and an actual
`.CRM` file on disk, none of which a `cargo test` can assume.

```rust,no_run
use std::sync::Arc;
use canon_raw::{crm_parser::CrmMovie, *};

fn main() -> Result<()> {
    // ── 1. The container layer — no SDK involved ────────────────────────────
    let mut movie = CrmMovie::load(std::fs::File::open("sample.CRM")?)?;

    let uuid_box   = movie.canon_metadata_box()
        .ok_or(CrmError::MissingData("Canon uuid box"))?
        .to_vec();
    let craw_entry = movie.craw_entry().to_vec();
    let timed_meta = movie.read_timed_meta(0)?;

    // ── 2. The SDK ──────────────────────────────────────────────────────────
    let sdk = Arc::new(CrmSdkLibrary::load("CrmSdk.dll")?);
    sdk.initialize_with_cpu()?;
    // The directory CONTAINING `CanonLUTs` — the SDK appends the folder name.
    // Must precede the first process ref, which `DecodeContext::new` creates.
    sdk.set_luts_directory("C:/CRMSDK210/CRMSDK", true)?;

    // ── 3. Clip-static facts come out of the header pack ────────────────────
    let headers = [
        Header { header_id: HeaderId::Uuid,          data: uuid_box.as_ptr()   as *mut _, size: uuid_box.len()   as u32 },
        Header { header_id: HeaderId::Craw,          data: craw_entry.as_ptr() as *mut _, size: craw_entry.len() as u32 },
        Header { header_id: HeaderId::TimedMetadata, data: timed_meta.as_ptr() as *mut _, size: timed_meta.len() as u32 },
    ];
    let header_pack = sdk.create_header_pack(&headers)?;
    let format_id   = sdk.get_format_id(&header_pack)?;
    // Ask for the spec's exact type: `CaptureWidth` / `CaptureHeight` are
    // `CrmUInt16` (§ 8.2.1-8.2.2), so the SDK writes two bytes. Widen yourself.
    let width  = u32::from(sdk.get_image_info::<u16>(format_id, &header_pack, ImageInfoId::CaptureWidth)?);
    let height = u32::from(sdk.get_image_info::<u16>(format_id, &header_pack, ImageInfoId::CaptureHeight)?);

    // ── 4. The develop recipe, and the geometry that drives the raster ──────
    let dev_param = DevParamV13 {
        size:           std::mem::size_of::<DevParamV13>() as u32,
        struct_id:      StructId::DevParamV13,
        format_id,
        wb_setting:     WbSetting::Record,
        click_wb_point: Point { x: 0, y: 0 },
        tint:           0.0,
        exposure:       0.0,
        cct:            0,
        sharpness:      0,
        noise:          0,
        gamma:          Gamma::Rec709,
        wb_gain:        WbGain { r: 0, g1: 0, g2: 0, b: 0 },
        aces_idt:       AcesIdt::Auto,
        color_space:    ColorSpace::Original,
        iso_setting:    IsoSetting::Record,
        iso_speed:      0,
    };
    let res_param = ResParamV13 {
        size:            std::mem::size_of::<ResParamV13>() as u32,
        struct_id:       StructId::ResParamV13,
        format_id,
        debayer_setting: DebayerSetting::HighQuality,
        downscale:       Downscale::X1,
        crop:            Rect { x: 0, y: 0, width, height },
        frame_rendering: FrameRendering::Split,
        lens_squeeze:    LensSqueeze::Off,
        in_data_type:    DataType::CrmCompressed,
        in_buffer_type:  BufferType::Cpu,
        out_data_type:   DataType::RgbLutApplied,
        out_buffer_type: BufferType::Cpu,
        decode_level:    DecodeLevel::Full,
    };
    // These three OVERWRITE what `res_param` above carries — `res_param` stays
    // the single store, and `ctx.geometry()` reads them back out of it — so a
    // caller that fills both structs cannot make them disagree.
    let geometry = OutputGeometry {
        crop:              Rect { x: 0, y: 0, width, height },
        downscale:         Downscale::X1,
        decode_level:      DecodeLevel::Full,
        bits_per_sample:   8,
        samples_per_pixel: 3, // RGB
    };

    let mut ctx = DecodeContext::new(
        Arc::clone(&sdk),
        format_id,
        Device::Cpu,
        Gpu::NotUse,
        ProcessType::DEVELOP | ProcessType::DECOMPRESS,
        Resolution::K8,
        dev_param,
        res_param,
        uuid_box,
        craw_entry,
        &timed_meta,
        SampleSequence::Rgb,
        geometry,
    )?;

    // ── 5. The SDK's raster, our packing — resolved once ────────────────────
    let (out_width, out_height) = ctx.output_dimensions();
    println!("{out_width} x {out_height}, {} bytes per row", ctx.bytes_per_line());
    let mut rgb = vec![0u8; ctx.required_output_size() as usize];

    // ── 6. Per frame: this frame's colour metadata, then develop ────────────
    for frame_no in 0..movie.frame_count() {
        let compressed = movie.read_compressed_frame(frame_no)?;
        let timed_meta = movie.read_timed_meta(frame_no)?;
        ctx.set_timed_meta(&timed_meta)?;   // colour only — no re-resolve
        ctx.decode_frame_cpu_into(&compressed, &mut rgb)?;
    }

    // ── 7. Dropping to a cheaper rung ───────────────────────────────────────
    // A rung is a PAIR: the SDK rejects `decode_level != Full` unless the
    // debayer is `HighSpeed`. `set_geometry` derives that half itself —
    // `OutputGeometry` cannot name the debayer, and at `Half` there is no other
    // legal value — so you are never handed a rejection for a field you were
    // given no way to set. Ask before you commit: a rung can change the
    // framing, not just the size.
    let half = OutputGeometry { decode_level: DecodeLevel::Half, ..geometry };
    println!("half would give {:?}", ctx.output_dimensions_for(&half)?);
    rgb.resize(ctx.set_geometry(half)? as usize, 0);

    // Climbing back is a deliberate act — `Full` is legal with EITHER debayer,
    // so nothing restores `HighQuality` for you. Name both halves in one
    // checked edit and the pair is committed, or refused, together.
    rgb.resize(
        ctx.edit_res_param(|p| {
            p.decode_level = DecodeLevel::Full;
            p.debayer_setting = DebayerSetting::HighQuality;
        })? as usize,
        0,
    );

    Ok(())
}
```

## Decode scale

Three knobs look like they reduce decode cost. Only one of them does.

`DecodeLevel::{Full, Half, Quarter}` truncates the *decompression* itself — the spec calls
it "decompressing the .CRM compressed data and then getting half-/quarter-resolution bayer
or RGB image" (§ 6.3.20). The work is never done, so it is the real decode-scale knob, and
it is worth roughly 9× at half and 27× at quarter on the CPU lane (measured on a
6000 × 3164 clip).

`Downscale::{X2, X4}` is a **post-develop RGB resampler** ("downscaling developed RGB image
to 1/2 size", § 6.3.10). Every pixel is decoded at full resolution and then thrown away, so
it costs *more* than a full-resolution decode. It is a fit-to-size knob, not a decode
scale. `Hd`, `Sd`, `Qfhd`, `K4Dci` and `K2Dci` are the same resampler aimed at named
rasters. `Bayer2` / `Bayer4` resample before demosaic and land on exactly the dimensions
`Half` / `Quarter` already produce, but slower — 84 ms against 39 ms at half, 64 ms against
11 ms at quarter, on that same clip. They duplicate the ladder; they do not extend it.

Two constraints the SDK enforces, both of which shape the ladder rather than merely
annoying you. The quoted text in each is `CrmSdk.dll`'s own validation message, not spec
prose — the spec documents neither rule, so the library is the only place they are written
down:

- **`decode_level != Full` is legal only with `DebayerSetting::HighSpeed`** — *"When
  CrmResParam::debayerSetting is 1, CrmResParam::decodeLevel must be
  kCrmDecodeLevel_Full."* (`1` is `HighQuality`.) A scale step is therefore a *pair*, and
  the two must be changed together. `Full` keeps `HighQuality`. Note also that `sharpness`
  only applies under `HighQuality` (§ 6.2.5), so it silently stops doing anything on the
  reduced rungs.
- **`downscale != X1` cannot be combined with `decode_level != Full`** — *"Either
  CrmResParam::downscale or CrmResParam::decodeLevel must be non-scaling."* At most one of
  the two may scale. They do **not** multiply: ask for both and `CrmGetImage` fails the
  call outright rather than applying one after the other, so there is no "eighth" to be had
  by stacking `Quarter` on `Bayer2`. `Quarter` is the bottom of the ladder.

Because a reduced rung can change the *framing* and not just the size (the even-height
flooring above), query before you commit: `output_dimensions_for(&geometry)` reports what a
rung would produce without applying it, which is what lets a UI advertise a whole ladder
before anything is selected.

## GPU output

`decode_frame_to_gpu` writes straight into device memory. Two things must agree, and the
SDK checks neither for you: the buffer must live in the lane the SDK was initialized for
(`initialize_with_cuda` / `initialize_with_opencl` / `initialize_with_metal`), and
`res_param.out_buffer_type` must *name* that lane — `CrmGetImage` reads it to decide
whether your destination pointer is a CPU pointer, a `CUdeviceptr`, a `cl_mem` or an
`id<MTLBuffer>`. The compressed source stays a CPU buffer either way; the SDK streams it in
itself.

Initialization takes the CPU bit *or*-ed with the GPU bit: spec § 7.2.15 spells the
argument out as `kCrmDeviceCpu | kCrmDeviceOpenCl` and Canon's sample builds every mask
that way, CUDA and Metal included. `initialize_with_cuda` / `_opencl` / `_metal` do it for
you, and no safe entry point in this crate takes a raw device mask, so this is background
rather than a rule you can trip over.

`decode_frame_to_gpu_future` is the async sibling, over `CrmGetImageAsync`. A combined
`DEVELOP | DECOMPRESS` process ref accepts **three** jobs in flight; a fourth fails
immediately with `SdkError::DecompressNoResource` at the submission site rather than
through the future. All jobs on one context share that context's current timed metadata, so
pipelining frames with distinct per-frame colour metadata needs a context each.

`enqueue_image` / `dequeue_image` expose the SDK's own queue. It is CPU-output only
(§ 6.3.16 — the CUDA buffer type is valid for `CrmGetImage` / `CrmGetImageAsync` and not
for the queue), so it cannot be zero-copy.

## Errors

`CrmError` covers container parsing, I/O, library loading, and `SdkError` — the SDK's own
`CrmError` codes, kept as a typed enum. Error strings are *not* formatted eagerly: several
codes (`DevelopEmptyQueue`, `WaitTimeoutError`) are ordinary flow control on the queue path
and fetching a string for each would allocate 8 KiB per call. Call
`get_last_error_string()` yourself when you want the SDK's detail text.

## License

Dual-licensed under **MIT** or **Apache-2.0** at your option.

> The Canon RAW SDK is distributed by Canon Inc. under its own licence, is not redistributed
> here, and is not covered by the above. You must obtain it from Canon and comply with their
> terms.
