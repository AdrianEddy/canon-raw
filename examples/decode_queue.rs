use std::sync::Arc;
use canon_raw::crm_parser::CrmMovie;
use canon_raw::*;

mod common;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || common::has_flag(&args, "--help") {
        eprintln!(
            "Usage:\n  cargo run --example decode_queue -- <input.CRM> --sdk-dll <path> --luts-dir <path> --out <dir>\n\
             Optional:\n  --device <cpu|cuda|opencl> --frames <n>"
        );
        return Ok(());
    }

    let input = std::path::PathBuf::from(&args[1]);
    let out_dir = std::path::PathBuf::from(common::arg_value(&args, "--out").ok_or(CrmError::InvalidParameter("--out <dir>"))?);
    let sdk_dll = common::arg_value(&args, "--sdk-dll")
        .or_else(|| common::env_path("CRM_SDK_DLL").map(|p| p.to_string_lossy().to_string()))
        .ok_or(CrmError::InvalidParameter("--sdk-dll <path>"))?;
    let luts_dir = common::arg_value(&args, "--luts-dir")
        .or_else(|| common::env_path("CRM_LUTS_DIR").map(|p| p.to_string_lossy().to_string()))
        .ok_or(CrmError::InvalidParameter("--luts-dir <path>"))?;
    let frames_limit = common::parse_u64_arg(&args, "--frames", u64::MAX);

    std::fs::create_dir_all(&out_dir)?;

    if let Some(sdk_dir) = common::arg_value(&args, "--sdk-dir")
        .map(std::path::PathBuf::from)
        .or_else(|| std::path::PathBuf::from(&sdk_dll).parent().map(|p| p.to_path_buf()))
    {
        common::add_dll_directory(&sdk_dir);
    }

    let device = match common::arg_value(&args, "--device").as_deref() {
        Some("cuda") => Device::Cuda,
        Some("opencl") => Device::OpenCl,
        _ => Device::Cpu,
    };

    let f = std::fs::File::open(&input)?;
    let mut crm = CrmMovie::load(f)?;
    let canon_metadata_box = crm
        .canon_metadata_box()
        .ok_or(CrmError::MissingData("Canon metadata box"))?
        .to_vec();
    let craw_entry = crm.craw_entry().ok_or(CrmError::MissingData("CRAW entry"))?.to_vec();

    let lib = Arc::new(CrmSdkLibrary::load(&sdk_dll)?);
    lib.initialize_sdk()?;
    lib.set_luts_directory(&luts_dir, true)?;

    let timed0 = crm.read_timed_meta(0)?;
    let header_pack0 = lib.create_header_pack(&[
        Header { header_id: HeaderId::Uuid, data: canon_metadata_box.as_ptr() as *mut _, size: canon_metadata_box.len() as u32 },
        Header { header_id: HeaderId::Craw, data: craw_entry.as_ptr() as *mut _, size: craw_entry.len() as u32 },
        Header { header_id: HeaderId::TimedMetadata, data: timed0.as_ptr() as *mut _, size: timed0.len() as u32 },
    ])?;

    let format_id = lib.get_format_id(&header_pack0)?;
    let w = lib.get_image_info::<u32>(format_id, &header_pack0, ImageInfoId::CaptureWidth)?;
    let h = lib.get_image_info::<u32>(format_id, &header_pack0, ImageInfoId::CaptureHeight)?;

    let use_gpu = pick_first_supported_gpu(&lib, device).unwrap_or(Gpu::NotUse);
    let process_type = ProcessType::DEVELOP | ProcessType::DECOMPRESS;
    let process_ref = lib.create_process_ref(format_id, device, use_gpu, process_type, Resolution::K8)?;

    let mut dev_param = DevParamV13 {
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
    let mut res_param = ResParamV13 {
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

    let (out_w, out_h) = lib.get_out_image_size(&res_param, &header_pack0)?;
    let out_bits_per_sample = 8u32;
    let samples_per_pixel = 3u32;
    let out_bytes_per_line = out_bytes_per_line(out_w, out_bits_per_sample, samples_per_pixel);
    let dst_buffer_size = out_bytes_per_line * out_h;
    let dst_buffer = CrmMemory::new(lib.clone(), dst_buffer_size, Device::Cpu)?;

    let mut next_enqueue = 0u64;
    let mut outputs_written = 0u64;
    let mut queue_draining = false;
    let frame_count = crm.frame_count().min(frames_limit);

    loop {
        if !queue_draining {
            if next_enqueue < frame_count {
                let frame = crm.read_compressed_frame(next_enqueue)?;
                let timed = crm.read_timed_meta(next_enqueue)?;
                let header_pack = lib.create_header_pack(&[
                    Header { header_id: HeaderId::Uuid, data: canon_metadata_box.as_ptr() as *mut _, size: canon_metadata_box.len() as u32 },
                    Header { header_id: HeaderId::Craw, data: craw_entry.as_ptr() as *mut _, size: craw_entry.len() as u32 },
                    Header { header_id: HeaderId::TimedMetadata, data: timed.as_ptr() as *mut _, size: timed.len() as u32 },
                ])?;

                let ref_con = Box::into_raw(Box::new(next_enqueue)) as *mut CrmVoid;
                lib.enqueue_image(
                    process_ref,
                    out_bits_per_sample,
                    samples_per_pixel,
                    out_bytes_per_line,
                    SampleSequence::Rgb as u32,
                    frame.as_ptr() as *mut CrmVoid,
                    frame.len() as u32,
                    &dev_param,
                    &res_param,
                    ref_con,
                    header_pack.as_ptr() as *mut CrmVoid,
                    header_pack.len() as u32,
                )?;
                next_enqueue += 1;
            } else {
                queue_draining = true;
                lib.enqueue_image(
                    process_ref,
                    out_bits_per_sample,
                    samples_per_pixel,
                    out_bytes_per_line,
                    SampleSequence::Rgb as u32,
                    std::ptr::null_mut(),
                    0,
                    &dev_param,
                    &res_param,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                )?;
            }
        }

        loop {
            match lib.dequeue_image(process_ref, dst_buffer.as_ptr(), dst_buffer_size) {
                Ok((dst_data_size, out_ref_con)) => {
                    let frame_no = if out_ref_con.is_null() { outputs_written } else { unsafe { *Box::from_raw(out_ref_con as *mut u64) } };
                    write_rgb8_jpeg(&out_dir, frame_no, dst_buffer.as_ptr(), dst_data_size, out_w, out_h)?;
                    outputs_written += 1;
                }
                Err(CrmError::SdkError(SdkError::DevelopNotCompleted)) => break,
                Err(CrmError::SdkError(SdkError::DevelopEmptyQueue)) => {
                    queue_draining = true;
                    break;
                }
                Err(e) => return Err(e),
            }
        }

        if queue_draining && outputs_written >= frame_count {
            break;
        }
    }

    lib.release_process_ref(process_ref)?;
    lib.terminate_sdk()?;

    println!("Wrote {outputs_written} frames to {}", out_dir.display());
    Ok(())
}

fn pick_first_supported_gpu(lib: &CrmSdkLibrary, device: Device) -> Option<Gpu> {
    if device == Device::Cpu {
        return None;
    }
    let list = lib.get_gpu_info_list_with_device(device).ok()?;
    for (idx, info) in list.iter().enumerate() {
        if info.support == 0 {
            continue;
        }
        return Some(match idx {
            0 => Gpu::Gpu1,
            1 => Gpu::Gpu2,
            2 => Gpu::Gpu3,
            3 => Gpu::Gpu4,
            4 => Gpu::Gpu5,
            5 => Gpu::Gpu6,
            6 => Gpu::Gpu7,
            7 => Gpu::Gpu8,
            _ => break,
        });
    }
    None
}

fn out_bytes_per_line(out_width: u32, bits_per_sample: u32, samples_per_pixel: u32) -> u32 {
    if bits_per_sample == 12 {
        out_width * samples_per_pixel * std::mem::size_of::<i32>() as u32 * 3 / 8
    } else {
        let data_num_in_int = 32 / bits_per_sample;
        out_width * samples_per_pixel * std::mem::size_of::<i32>() as u32 / data_num_in_int
    }
}

fn write_rgb8_jpeg(
    out_dir: &std::path::Path,
    frame_no: u64,
    src_ptr: *mut CrmVoid,
    byte_len: u32,
    width: u32,
    height: u32,
) -> Result<()> {
    let src = unsafe { std::slice::from_raw_parts(src_ptr as *const u8, byte_len as usize) };
    let img = image::ImageBuffer::<image::Rgb<u8>, _>::from_raw(width, height, src.to_vec())
        .ok_or_else(|| CrmError::Other("failed to create RGB image buffer".into()))?;
    img.save(out_dir.join(format!("frame_{frame_no:06}.jpg")))
        .map_err(|e| CrmError::Other(format!("failed to save image: {e}")))?;
    Ok(())
}
