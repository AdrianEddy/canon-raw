use std::sync::Arc;
use canon_raw::crm_parser::CrmMovie;
use canon_raw::*;

mod common;

fn main() -> Result<()> {
    let f = std::fs::File::open("F:/canon/A_0002C015X250527_153335DT_CANON.CRM").unwrap();
    let mut crm = CrmMovie::load(f).unwrap();
    let frame = crm.read_compressed_frame(0)?;
    let timed_meta = crm.read_timed_meta(0).unwrap();
    let metadata_box = crm.canon_metadata_box().unwrap();
    let craw_entry = crm.craw_entry();
    //dbg!(timed_meta);

	let device = Device::Cuda;

	unsafe {
		windows_sys::Win32::System::LibraryLoader::SetDefaultDllDirectories(windows_sys::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | windows_sys::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_USER_DIRS);
		let path = "D:\\programowanie\\projekty\\Rust\\crm-rs\\sdk\\CRMSDK210\\CRMSDK\\Win_Lib\\x64\\";
		let wide = path.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
		windows_sys::Win32::System::LibraryLoader::AddDllDirectory(wide.as_ptr());
	}

    let lib = Arc::new(CrmSdkLibrary::load("D:\\programowanie\\projekty\\Rust\\crm-rs\\sdk\\CRMSDK210\\CRMSDK\\Win_Lib\\x64\\CrmSdk.dll").unwrap());
    lib.initialize_sdk().unwrap();
	// The directory that CONTAINS `CanonLUTs` — the SDK appends the folder name.
	lib.set_luts_directory("D:\\programowanie\\projekty\\Rust\\crm-rs\\sdk\\CRMSDK210\\CRMSDK", true).unwrap();

    let header_pack = lib.create_header_pack(&[
        Header { header_id: HeaderId::Uuid, data: metadata_box.as_ptr() as *mut _, size: metadata_box.len() as u32 },
        Header { header_id: HeaderId::Craw, data: craw_entry.as_ptr() as *mut _, size: craw_entry.len() as u32 },
        Header { header_id: HeaderId::TimedMetadata, data: timed_meta.as_ptr() as *mut _, size: timed_meta.len() as u32 },
    ])?;

    let format_id = lib.get_format_id(&header_pack)?;
    let w = lib.get_image_info::<u32>(format_id, &header_pack, ImageInfoId::CaptureWidth)?;
    let h = lib.get_image_info::<u32>(format_id, &header_pack, ImageInfoId::CaptureHeight)?;

    let use_gpu = pick_first_supported_gpu(&lib, device).unwrap_or(Gpu::NotUse);
    dbg!(&use_gpu);
    let process_type = ProcessType::DEVELOP | ProcessType::DECOMPRESS;
    let process_ref = lib.create_process_ref(format_id, device, use_gpu, process_type, Resolution::K8)?;

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
    dbg!(&dst_buffer_size);
    let dst_buffer = CrmMemory::new(lib.clone(), dst_buffer_size, Device::Cpu)?;

    let dst_data_size = lib.get_image(
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
    )?;

    println!("decoded");

    let src = unsafe { std::slice::from_raw_parts(dst_buffer.as_ptr() as *const u8, dst_data_size as usize) };
    let img = image::ImageBuffer::<image::Rgb<u8>, _>::from_raw(out_w, out_h, src.to_vec())
        .ok_or_else(|| CrmError::Other("failed to create RGB image buffer".into()))?;
    img.save("F:/test.jpg")
        .map_err(|e| CrmError::Other(format!("failed to save image: {e}")))?;

    lib.release_process_ref(process_ref)?;
    lib.terminate_sdk()?;

    Ok(())
}

fn pick_first_supported_gpu(lib: &CrmSdkLibrary, device: Device) -> Option<Gpu> {
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

