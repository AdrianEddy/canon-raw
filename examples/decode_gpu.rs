// SPDX-License-Identifier: MIT OR Apache-2.0

//! Develop one frame of a `.CRM` clip on a GPU (CUDA by default, or OpenCL)
//! and read it back to the CPU as an 8-bit RGB image.
//!
//!   cargo run --release --example decode_gpu -- <input.CRM> \
//!       --sdk-dll <CrmSdk> --luts-dir <dir containing CanonLUTs> \
//!       [--device cuda|opencl] [--frame <n>] [--out frame.jpg]
//!
//! `--sdk-dll` / `--luts-dir` fall back to `CRM_SDK_DLL` / `CRM_LUTS_DIR`.
//! The develop runs on the GPU, but `out_buffer_type` stays `Cpu` so the result
//! lands in host memory; name `BufferType::Cuda` / `OpenCl` there and pass a
//! device allocation to keep the frame on the GPU instead.

use canon_raw::crm_parser::CrmMovie;
use canon_raw::*;
use std::path::PathBuf;

mod common;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || common::has_flag(&args, "--help") {
        eprintln!(
            "Usage:\n  cargo run --example decode_gpu -- <input.CRM> --sdk-dll <path> \
             --luts-dir <dir containing CanonLUTs> [--device cuda|opencl] [--frame <n>] [--out <image>]"
        );
        return Ok(());
    }
    let input = PathBuf::from(&args[1]);
    let frame_no = common::parse_u64_arg(&args, "--frame", 0);
    let out = common::arg_value(&args, "--out").map(PathBuf::from).unwrap_or_else(|| "frame.jpg".into());
    let device = common::device_arg(&args, Device::Cuda);

    let mut crm = CrmMovie::load(std::fs::File::open(&input)?)?;
    let frame = crm.read_compressed_frame(frame_no)?;
    let timed_meta = crm.read_timed_meta(frame_no)?;
    let metadata_box = crm.canon_metadata_box().ok_or(CrmError::MissingData("Canon metadata box"))?;
    let craw_entry = crm.craw_entry();

    let lib = common::load_sdk(&args)?;

    let header_pack = lib.create_header_pack(&[
        Header::uuid(metadata_box),
        Header::craw(craw_entry),
        Header::timed_metadata(&timed_meta),
    ])?;

    let format_id = lib.get_format_id(&header_pack)?;
    let w = u32::from(lib.get_image_info(format_id, &header_pack, image_info::CaptureWidth)?);
    let h = u32::from(lib.get_image_info(format_id, &header_pack, image_info::CaptureHeight)?);

    let use_gpu = common::pick_first_supported_gpu(&lib, device)
        .ok_or(CrmError::MissingData("no GPU the SDK supports for the requested device"))?;
    println!("{}: {w}x{h}, developing on {device:?} / {use_gpu:?}", input.display());
    let process_type = ProcessType::DEVELOP | ProcessType::DECOMPRESS;
    // Leased rather than created: dropping the lease parks the ref for reuse
    // instead of releasing it (see `CrmSdkLibrary::create_process_ref`).
    let lease = lib.lease_process_ref(format_id, device, use_gpu, process_type, Resolution::K8)?;
    let process_ref = lease.process_ref();

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
        gamma: Gamma::Rec709,
        noise: 0,
        aces_idt: AcesIdt::Auto,
        color_space: ColorSpace::Original,
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

    let (out_w, out_h) = lib.get_out_image_size(&res_param, &header_pack)?;
    let out_bits_per_sample = 8u32;
    let samples_per_pixel = 3u32;
    let out_bytes_per_line = OutputGeometry::bytes_per_line(out_w, out_bits_per_sample, samples_per_pixel);
    let dst_buffer_size = out_bytes_per_line * out_h;
    let dst_buffer = CrmMemory::new(lib.clone(), dst_buffer_size, Device::Cpu)?;

    // SAFETY: `process_ref` is live; `dst_buffer` is a CPU allocation of
    // `dst_buffer_size` bytes — the full output — and `res_param` names a CPU
    // output, which the SDK writes synchronously.
    let dst_data_size = unsafe {
        lib.get_image(
            process_ref,
            dst_buffer.as_ptr(),
            dst_buffer_size,
            out_bits_per_sample,
            samples_per_pixel,
            out_bytes_per_line,
            SampleSequence::Rgb,
            &frame,
            &dev_param,
            &res_param,
            &header_pack,
        )
    }?;

    // SAFETY: `CrmGetImage` wrote `dst_data_size` bytes into `dst_buffer`,
    // a CPU allocation of `dst_buffer_size >= dst_data_size` bytes.
    let pixels = unsafe { std::slice::from_raw_parts(dst_buffer.as_ptr() as *const u8, dst_data_size as usize) };
    let img = image::RgbImage::from_raw(out_w, out_h, pixels.to_vec())
        .ok_or_else(|| CrmError::Other("the developed frame does not fill an RGB image".into()))?;
    img.save(&out).map_err(|e| CrmError::Other(format!("failed to save {}: {e}", out.display())))?;
    println!("wrote {out_w}x{out_h} frame {frame_no} to {}", out.display());

    drop(dst_buffer);
    // Nothing is pending on the ref, so the lease can park it.
    drop(lease);
    Ok(())
}
