use std::sync::Arc;
use canon_raw::*;
use canon_raw::crm_parser::*;

fn main() -> Result<()> {
    let f = std::fs::File::open("F:/canon/A_0002C015X250527_153335DT_CANON.CRM").unwrap();
    let mut crm = CrmMovie::load(f).unwrap();
    let frame = crm.read_compressed_frame(0)?;
    let timed_meta = crm.read_timed_meta(0).unwrap();
    let metadata_box = crm.canon_metadata_box().unwrap();
    let craw_entry = crm.craw_entry();
    //dbg!(timed_meta);

	let device = Device::Cpu;

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

	//Get GPU information
	//let sys_info = lib.get_gpu_system_info_with_device(device).unwrap();
	//dbg!(&sys_info);
	//let gpu_info = lib.get_gpu_info_list_with_device(device).unwrap();
	//dbg!(&gpu_info);

	let headers = [
		Header {
			header_id: HeaderId::Uuid,
			data: metadata_box.as_ptr() as *mut _,
			size: metadata_box.len() as u32,
		},
		Header {
			header_id: HeaderId::Craw,
			data: craw_entry.as_ptr() as *mut _,
			size: craw_entry.len() as u32,
		},
		Header {
			header_id: HeaderId::TimedMetadata,
			data: timed_meta.as_ptr() as *mut _,
			size: timed_meta.len() as u32,
		},
	];

	let header_buffer = lib.create_header_pack(&headers).unwrap();

	let format_id = lib.get_format_id(&header_buffer).unwrap();
	dbg!(format_id);

	let w = lib.get_image_info::<u32>(format_id, &header_buffer, ImageInfoId::CaptureWidth)?;
	let h = lib.get_image_info::<u32>(format_id, &header_buffer, ImageInfoId::CaptureHeight)?;
	dbg!(w, h);
    let process_type = ProcessType::DEVELOP | ProcessType::DECOMPRESS;

	let process_ref = lib.create_process_ref(format_id, device, Gpu::NotUse, process_type, Resolution::K8).unwrap();

	// Set development parameters and resolution Parameters to develop RAW frames
	let dev_param = DevParamV13 {
		size: std::mem::size_of::<DevParamV13>() as _,
		struct_id: StructId::DevParamV13,
		format_id: format_id,
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
		wb_gain: WbGain { r: 0, g1: 0, g2: 0, b: 0 }
	};
	let res_param = ResParamV13 {
		size: std::mem::size_of::<ResParamV13>() as _,
		struct_id: StructId::ResParamV13,
		format_id: format_id,
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

	let (out_width, out_height) = lib.get_out_image_size(&res_param, &header_buffer).unwrap();
	dbg!(out_width, out_height);

	let out_bits_per_sample = 8; // 8-bit
	let samples_per_pixel = 3; // RGB

	let out_bytes_per_line =
		OutputGeometry::bytes_per_line(out_width, out_bits_per_sample, samples_per_pixel);
	let dst_buffer_size = out_bytes_per_line * out_height;
	let dst_buffer = CrmMemory::new(lib.clone(), dst_buffer_size, device).unwrap();

	//Get RGB image data
	let dst_data_size = lib.get_image(process_ref,
		dst_buffer.as_ptr(),
		dst_buffer_size,
		out_bits_per_sample,
		samples_per_pixel,
		out_bytes_per_line,
		SampleSequence::Rgb,
		&frame,
		&dev_param,
		&res_param,
		&header_buffer).unwrap();

	// save the rgb to jpeg using `image`
	let dst_slice = unsafe { std::slice::from_raw_parts(dst_buffer.as_ptr() as *mut u8, dst_data_size as usize) };
	let img_buffer = image::ImageBuffer::<image::Rgb<u8>, _>::from_raw(out_width, out_height, dst_slice.to_vec()).unwrap();
	img_buffer.save("F:/test.jpg").unwrap();

	// deallocate
	drop(dst_buffer);

	lib.release_process_ref(process_ref).unwrap();

	lib.terminate_sdk().unwrap();

    Ok(())
}
