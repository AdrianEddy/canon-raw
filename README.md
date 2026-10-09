# canon-raw — Rust bindings for the Canon Cinema RAW Light SDK

Safe, dynamically-loaded Rust bindings for Canon's **CRM SDK** — decode and develop
`.CRM` (Cinema RAW Light) and `.RMF` clips from Canon Cinema EOS cameras, on the CPU or
straight into CUDA / OpenCL / Metal memory.

[![crates.io](https://img.shields.io/crates/v/canon-raw.svg)](https://crates.io/crates/canon-raw)
[![docs.rs](https://img.shields.io/docsrs/canon-raw)](https://docs.rs/canon-raw)
[![CI](https://github.com/AdrianEddy/canon-raw/actions/workflows/ci.yml/badge.svg)](https://github.com/AdrianEddy/canon-raw/actions/workflows/ci.yml)
![MSRV 1.88](https://img.shields.io/badge/MSRV-1.88-blue)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20Linux%20%7C%20macOS-success)
![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-informational)

**Based on CRMSDK 2.10.**

| Crate | What it is |
|---|---|
| [`canon-raw`](https://crates.io/crates/canon-raw) (this directory) | the SDK bindings |
| [`crm-container`](https://crates.io/crates/crm-container) | the pure-Rust, SDK-free `.CRM` container parser it builds on |

## Overview

The SDK is not included. Obtain it from Canon. It is loaded at runtime from a path you
pass to `CrmSdkLibrary::load`, so the crate builds and your app starts without it.

- `crm_parser::CrmMovie` reads the `.CRM` container: frames and per-frame metadata. No SDK
  needed.
- `CrmSdkLibrary` and `DecodeContext` decode and develop frames. `DecodeContext` keeps the
  per-clip state and reports the output size, so your buffer always matches the current
  settings.
- `CrmSdkLibrary` methods that take raw pointers or process handles are `unsafe`, with the
  SDK's requirements in their docs. `DecodeContext::decode_frame_cpu_into` is the safe way
  to decode.

## Setup

- **Library:** `CrmSdk.dll` (Windows), `libCrmSdk.so` (Linux, beta) or
  `CrmSdk.framework/CrmSdk` (macOS). Keep it together with the files it ships with, and
  put its directory on the library search path.
- **LUTs:** call `set_luts_directory` with the directory that *contains* `CanonLUTs`, not
  `CanonLUTs` itself. Call it after initialization and before creating a `DecodeContext`.
  The LUTs are needed for HDR and colour-space conversions. A wrong path is not reported
  as an error, so check that `CanonLUTs` exists yourself.

## Example

```rust,no_run
use std::sync::Arc;
use canon_raw::{crm_parser::CrmMovie, *};

fn main() -> Result<()> {
    // Container: no SDK involved.
    let mut movie = CrmMovie::load(std::fs::File::open("sample.CRM")?)?;
    let uuid_box   = movie.canon_metadata_box()
        .ok_or(CrmError::MissingData("Canon uuid box"))?
        .to_vec();
    let craw_entry = movie.craw_entry().to_vec();
    let timed_meta = movie.read_timed_meta(0)?;

    // SDK.
    // SAFETY: Canon's SDK; loading it runs its initialization code.
    let sdk = Arc::new(unsafe { CrmSdkLibrary::load("CrmSdk.dll") }?);
    sdk.initialize_with_cpu()?;
    sdk.set_luts_directory("C:/CRMSDK210/CRMSDK", true)?;

    // Clip format and size.
    let header_pack = sdk.create_header_pack(&[
        Header::uuid(&uuid_box),
        Header::craw(&craw_entry),
        Header::timed_metadata(&timed_meta),
    ])?;
    let format_id = sdk.get_format_id(&header_pack)?;
    let width  = u32::from(sdk.get_image_info(format_id, &header_pack, image_info::CaptureWidth)?);
    let height = u32::from(sdk.get_image_info(format_id, &header_pack, image_info::CaptureHeight)?);

    // Develop settings.
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
    // Overrides crop, downscale and decode level in `res_param`.
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

    let (out_width, out_height) = ctx.output_dimensions();
    println!("{out_width} x {out_height}");
    let mut rgb = vec![0u8; ctx.required_output_size() as usize];

    for frame_no in 0..movie.frame_count() {
        let compressed = movie.read_compressed_frame(frame_no)?;
        ctx.set_timed_meta(&movie.read_timed_meta(frame_no)?)?;
        ctx.decode_frame_cpu_into(&compressed, &mut rgb)?;
    }

    // Faster, half-resolution decode.
    let half = OutputGeometry { decode_level: DecodeLevel::Half, ..geometry };
    rgb.resize(ctx.set_geometry(half)? as usize, 0);

    Ok(())
}
```

## Decode resolution

- `DecodeLevel::Half` / `Quarter` decode less data and are much faster. Use them for
  previews and proxies.
- `Downscale` resizes *after* a full decode. Use it to fit a target size, not for speed.
- The two cannot be combined.

`output_dimensions_for(&geometry)` reports the output size of a setting without applying
it.

## GPU output

Initialize with `initialize_with_cuda`, `initialize_with_opencl` or
`initialize_with_metal`, set `res_param.out_buffer_type` to the same API, and call
`decode_frame_to_gpu` with a device buffer of at least `required_output_size()` bytes.

`decode_frame_to_gpu` is `unsafe`: the SDK does not limit its writes to the buffer size you
pass (the context checks it), and it returns before the GPU has finished writing. Keep the
buffer alive until you have synchronized the stream or queue you gave the SDK.

With CUDA, the SDK forbids its decode calls from overlapping each other or your own CUDA
calls on other threads (spec § 9.2.1). The crate serializes its CUDA calls through
`cuda_exclusion()`. Hold that guard around your own CUDA work that can overlap a decode,
such as the stream synchronize after `decode_frame_to_gpu`.

`decode_frame_future` decodes asynchronously, but the SDK only allows it on a
decompress-only context. Develop calls are synchronous.

## Examples

Each example takes `--sdk-dll <path to CrmSdk>` and `--luts-dir <directory containing
CanonLUTs>` (or `CRM_SDK_DLL` / `CRM_LUTS_DIR`). Run one with `--help` for its options.

| Example | What it shows |
|---|---|
| `decode` | develop one frame on the CPU and save it as an image |
| `decode_gpu` | the same on a CUDA or OpenCL device |
| `decode_queue` | decode a whole clip through the SDK's queue |
| `metadata` | dump clip metadata |
| `crm_bench` | decode speed across the available decode paths |

```sh
cargo run --release --example decode -- A001C001.CRM \
    --sdk-dll CRMSDK210/CRMSDK/Win_Lib/x64/CrmSdk.dll --luts-dir CRMSDK210/CRMSDK
```

## Testing

`cargo test --workspace` runs everywhere. Tests that need the SDK or real footage print
`SKIP …` and pass. To run them, set:

| Variable | Default | What it is |
|---|---|---|
| `CRM_TEST_CLIP` | — | any `.CRM` clip |
| `CRM_TEST_SDK_ROOT` | `<repo>/sdk/CRMSDK210/CRMSDK` | the unpacked SDK |
| `CRM_TEST_SDK_DLL` | `<sdk root>/Win_Lib/x64/CrmSdk.dll` | the library; required off Windows |
| `CRM_TEST_LUTS_PARENT` | `<sdk root>` | the directory containing `CanonLUTs` |

## Minimum supported Rust version

Rust **1.88**. Raising it is not treated as a breaking change, but is noted in the
changelog.

## License

Dual-licensed under **MIT** or **Apache-2.0** at your option.

> The Canon RAW SDK is distributed by Canon Inc. under its own licence, is not redistributed
> here, and is not covered by the above. You must obtain it from Canon and comply with their
> terms.
