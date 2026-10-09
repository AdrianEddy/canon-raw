// SPDX-License-Identifier: MIT OR Apache-2.0

//! Dump a `.CRM` clip's SDK header blobs (Canon `uuid` box, `CRAW` entry,
//! per-frame timed metadata) to disk — no SDK required — and, given the SDK,
//! print the basic image info it reads from them.

use canon_raw::crm_parser::CrmMovie;
use canon_raw::*;
use std::path::PathBuf;

mod common;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || common::has_flag(&args, "--help") {
        eprintln!(
            "Usage:\n  cargo run --example metadata -- <input.CRM> [--out <dir>] [--frame <n> | --all-frames]\n\
             Optional SDK info extraction:\n  --sdk-dll <path> --luts-dir <dir containing CanonLUTs> [--sdk-dir <dir>]"
        );
        return Ok(());
    }

    let input = PathBuf::from(&args[1]);
    let out_dir = common::arg_value(&args, "--out").map(PathBuf::from).unwrap_or_else(|| ".".into());
    let frame_no = common::parse_u64_arg(&args, "--frame", 0);
    let dump_all_frames = common::has_flag(&args, "--all-frames");

    std::fs::create_dir_all(&out_dir)?;

    let f = std::fs::File::open(&input)?;
    let mut crm = CrmMovie::load(f)?;

    let canon_metadata_box = crm.canon_metadata_box().ok_or(CrmError::MissingData("Canon metadata box"))?.to_vec();
    let craw_entry = crm.craw_entry().to_vec();

    std::fs::write(out_dir.join("canon_metadata_box.bin"), &canon_metadata_box)?;
    std::fs::write(out_dir.join("craw_entry.bin"), &craw_entry)?;

    if dump_all_frames {
        for i in 0..crm.frame_count() {
            let timed = crm.read_timed_meta(i)?;
            std::fs::write(out_dir.join(format!("timed_meta_{i:06}.bin")), timed)?;
        }
    } else {
        let timed = crm.read_timed_meta(frame_no)?;
        std::fs::write(out_dir.join(format!("timed_meta_{frame_no:06}.bin")), timed)?;
    }

    if let (Some(sdk_dll), Some(luts_dir)) = (
        common::arg_value(&args, "--sdk-dll")
            .or_else(|| common::env_path("CRM_SDK_DLL").map(|p| p.to_string_lossy().to_string())),
        common::arg_value(&args, "--luts-dir")
            .or_else(|| common::env_path("CRM_LUTS_DIR").map(|p| p.to_string_lossy().to_string())),
    ) {
        if let Some(sdk_dir) = common::arg_value(&args, "--sdk-dir")
            .map(PathBuf::from)
            .or_else(|| PathBuf::from(&sdk_dll).parent().map(|p| p.to_path_buf()))
        {
            common::add_dll_directory(&sdk_dir);
        }

        let lib = unsafe { CrmSdkLibrary::load(&sdk_dll) }?;
        lib.initialize_sdk()?;
        lib.set_luts_directory(&luts_dir, true)?;

        let timed = crm.read_timed_meta(frame_no)?;
        let headers = [Header::uuid(&canon_metadata_box), Header::craw(&craw_entry), Header::timed_metadata(&timed)];

        let header_pack = lib.create_header_pack(&headers)?;
        let format_id = lib.get_format_id(&header_pack)?;
        let w = u32::from(lib.get_image_info(format_id, &header_pack, image_info::CaptureWidth)?);
        let h = u32::from(lib.get_image_info(format_id, &header_pack, image_info::CaptureHeight)?);

        println!("Frames: {}", crm.frame_count());
        println!("Format: {format_id:?}");
        println!("Frame {frame_no}: {w}x{h}");
    } else {
        println!("Frames: {}", crm.frame_count());
        println!("Wrote metadata blobs to: {}", out_dir.display());
        println!(
            "Tip: pass --sdk-dll and --luts-dir (or set CRM_SDK_DLL/CRM_LUTS_DIR) to extract basic image info via Canon RAW SDK."
        );
    }

    Ok(())
}
