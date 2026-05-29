//! TEMP diagnostic — find why CrmEnqueueImage/CrmDequeueImage access-violates.
//! Faithfully mirrors TestCrmSdk.cpp: CrmAllocate'd src/header/dst buffers
//! (kept alive for the whole run), strict 1:1 enqueue/dequeue, CPU device,
//! heavy stderr tracing so we see exactly which FFI call faults.
//!
//!   cargo run --release --example queue_probe -- <in.CRM> \
//!       --sdk-dll <CrmSdk.dll> --luts-dir <CanonLUTs> [--frames 12] [--device cpu|cuda]

use std::sync::Arc;
use canon_raw::crm_parser::CrmMovie;
use canon_raw::*;

mod common;

macro_rules! trace { ($($a:tt)*) => {{ eprintln!($($a)*); use std::io::Write; let _ = std::io::stderr().flush(); }} }

use std::sync::atomic::{AtomicBool, Ordering};
static ASYNC_DONE: AtomicBool = AtomicBool::new(false);
static ASYNC_RESULT: std::sync::Mutex<Option<SdkError>> = std::sync::Mutex::new(None);

unsafe extern "system" fn async_cb(
    _pr: ProcessRef, _db: *mut CrmVoid, _dbs: CrmUInt32, dds: CrmUInt32,
    _bps: CrmUInt32, _spp: CrmUInt32, _bpl: CrmUInt32, _ss: CrmUInt32,
    _sb: *mut CrmVoid, _sds: CrmUInt32, _dp: *mut CrmVoid, _rp: *mut CrmVoid,
    _rc: *mut CrmVoid, _hb: *mut CrmVoid, _hds: CrmUInt32, err: SdkError,
) -> SdkError {
    eprintln!("    [async callback fired] err={err:?} dst_data_size={dds}");
    *ASYNC_RESULT.lock().unwrap() = Some(err);
    ASYNC_DONE.store(true, Ordering::SeqCst);
    SdkError::Ok
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let input = std::path::PathBuf::from(&args[1]);
    let sdk_dll = common::arg_value(&args, "--sdk-dll").expect("--sdk-dll");
    let luts_dir = common::arg_value(&args, "--luts-dir").expect("--luts-dir");
    let frames = common::parse_u64_arg(&args, "--frames", 12) as usize;
    let use_async = common::has_flag(&args, "--async");
    let use_measure = common::has_flag(&args, "--measure");
    let device = match common::arg_value(&args, "--device").as_deref() {
        Some("cuda") => Device::Cuda,
        _ => Device::Cpu,
    };

    if let Some(parent) = std::path::PathBuf::from(&sdk_dll).parent() {
        common::add_dll_directory(parent);
    }

    trace!("[1] load + init");
    let lib = Arc::new(CrmSdkLibrary::load(&sdk_dll)?);
    lib.initialize_sdk()?;
    lib.set_luts_directory(&luts_dir, true)?;

    trace!("[2] open movie + metadata");
    let f = std::fs::File::open(&input)?;
    let mut crm = CrmMovie::load(f)?;
    let uuid = crm.canon_metadata_box().ok_or(CrmError::MissingData("uuid"))?.to_vec();
    let craw = crm.craw_entry().ok_or(CrmError::MissingData("craw"))?.to_vec();

    // Frame 0: compressed bytes + timed meta + header pack (Rust Vecs).
    let comp = crm.read_compressed_frame(0)?;
    let timed = crm.read_timed_meta(0)?;
    let header_pack = lib.create_header_pack(&[
        Header { header_id: HeaderId::Uuid, data: uuid.as_ptr() as *mut _, size: uuid.len() as u32 },
        Header { header_id: HeaderId::Craw, data: craw.as_ptr() as *mut _, size: craw.len() as u32 },
        Header { header_id: HeaderId::TimedMetadata, data: timed.as_ptr() as *mut _, size: timed.len() as u32 },
    ])?;

    let format_id = lib.get_format_id(&header_pack)?;
    let w = lib.get_image_info::<u16>(format_id, &header_pack, ImageInfoId::CaptureWidth)? as u32;
    let h = lib.get_image_info::<u16>(format_id, &header_pack, ImageInfoId::CaptureHeight)? as u32;
    trace!("    format_id ok, {w}x{h}, comp={} hdr={}", comp.len(), header_pack.len());

    let use_gpu = if device == Device::Cpu { Gpu::NotUse } else { pick_gpu(&lib).unwrap_or(Gpu::Gpu1) };
    let process_type = ProcessType::DEVELOP | ProcessType::DECOMPRESS;

    // 16-bit RGBA output (what gpu-video actually wants).
    let bits = 16u32;
    let samples = 4u32;
    let seq = SampleSequence::Rgba16BitLe as u32;

    let dev = DevParamV13 {
        size: std::mem::size_of::<DevParamV13>() as _, struct_id: StructId::DevParamV13, format_id,
        wb_setting: WbSetting::Record, click_wb_point: Point { x: 0, y: 0 }, tint: 0.0, exposure: 0.0,
        cct: 0, sharpness: 0, gamma: Gamma::Rec709, noise: 0, aces_idt: AcesIdt::Auto,
        color_space: ColorSpace::Original, iso_setting: IsoSetting::Record, iso_speed: 0,
        wb_gain: WbGain { r: 0, g1: 0, g2: 0, b: 0 },
    };
    // Output on the GPU when developing on CUDA (what gpu-video wants: one
    // on-device copy into our wgpu buffer, no host round-trip).
    let out_buffer_type = if device == Device::Cpu { BufferType::Cpu } else { BufferType::Cuda };
    let res = ResParamV13 {
        size: std::mem::size_of::<ResParamV13>() as _, struct_id: StructId::ResParamV13, format_id,
        debayer_setting: DebayerSetting::HighQuality, downscale: Downscale::X1,
        crop: Rect { x: 0, y: 0, width: w, height: h }, frame_rendering: FrameRendering::Split,
        lens_squeeze: LensSqueeze::Off, in_data_type: DataType::CrmCompressed, in_buffer_type: BufferType::Cpu,
        out_data_type: DataType::RgbLutApplied, out_buffer_type, decode_level: DecodeLevel::Full,
    };

    let (out_w, out_h) = lib.get_out_image_size(&res, &header_pack)?;
    let out_bpl = out_w * samples * 4 / (32 / bits);
    let out_size = out_bpl * out_h;
    trace!("    out {out_w}x{out_h}, bpl={out_bpl}, dst={} MiB", out_size / (1024*1024));

    // ── CrmAllocate src + header + dst (the deviation from decode_queue.rs) ──
    // Compressed input + header are CPU (in_buffer_type=Cpu); developed output
    // lands on `device` (CUDA buffer when developing on GPU).
    trace!("[3] CrmAllocate src/header (CPU) + dst ({device:?})");
    let mut src_mem = CrmMemory::new(lib.clone(), comp.len() as u32, Device::Cpu)?;
    let mut hdr_mem = CrmMemory::new(lib.clone(), header_pack.len() as u32, Device::Cpu)?;
    let dst_mem = CrmMemory::new(lib.clone(), out_size, device)?;
    unsafe {
        std::ptr::copy_nonoverlapping(comp.as_ptr(), src_mem.as_mut_ptr() as *mut u8, comp.len());
        std::ptr::copy_nonoverlapping(header_pack.as_ptr(), hdr_mem.as_mut_ptr() as *mut u8, header_pack.len());
    }

    trace!("[4] create_process_ref ({device:?}, {use_gpu:?})");
    let process_ref = lib.create_process_ref(format_id, device, use_gpu, process_type, Resolution::K8)?;

    // ── MEASURE: single-thread sync throughput vs queue throughput, distinct
    //    frames, to see how much the SDK-internal pipeline overlaps.
    if use_measure {
        let m = frames; // frames to time
        trace!("[5-measure] loading {m} distinct frames ({device:?} out)...");
        let mut fr: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(m);
        for i in 0..m as u64 {
            let c = crm.read_compressed_frame(i)?;
            let tm = crm.read_timed_meta(i)?;
            let hp = lib.create_header_pack(&[
                Header { header_id: HeaderId::Uuid, data: uuid.as_ptr() as *mut _, size: uuid.len() as u32 },
                Header { header_id: HeaderId::Craw, data: craw.as_ptr() as *mut _, size: craw.len() as u32 },
                Header { header_id: HeaderId::TimedMetadata, data: tm.as_ptr() as *mut _, size: tm.len() as u32 },
            ])?;
            fr.push((c, hp));
        }
        let max_c = fr.iter().map(|f| f.0.len()).max().unwrap() as u32;
        let max_h = fr.iter().map(|f| f.1.len()).max().unwrap() as u32;
        let mut ms = CrmMemory::new(lib.clone(), max_c, Device::Cpu)?;
        let mut mh = CrmMemory::new(lib.clone(), max_h, Device::Cpu)?;
        let copy_in = |ms: &mut CrmMemory, mh: &mut CrmMemory, f: &(Vec<u8>, Vec<u8>)| unsafe {
            std::ptr::copy_nonoverlapping(f.0.as_ptr(), ms.as_mut_ptr() as *mut u8, f.0.len());
            std::ptr::copy_nonoverlapping(f.1.as_ptr(), mh.as_mut_ptr() as *mut u8, f.1.len());
        };

        // Warmup (also absorbs CUDA JIT).
        for i in 0..2.min(m) {
            lib.get_image(process_ref, dst_mem.as_ptr(), out_size, bits, samples, out_bpl,
                SampleSequence::Rgba16BitLe, &fr[i].0, &dev, &res, &fr[i].1)?;
        }

        // Sequential sync baseline.
        let t = std::time::Instant::now();
        for i in 0..m {
            let f = &fr[i % fr.len()];
            lib.get_image(process_ref, dst_mem.as_ptr(), out_size, bits, samples, out_bpl,
                SampleSequence::Rgba16BitLe, &f.0, &dev, &res, &f.1)?;
        }
        let seq_dur = t.elapsed();

        // Queue: strict 1:1, copy each frame into pinned buffers before enqueue.
        let t = std::time::Instant::now();
        let mut next = 0usize; let mut got = 0usize;
        loop {
            if next < m {
                copy_in(&mut ms, &mut mh, &fr[next]);
                lib.enqueue_image(process_ref, bits, samples, out_bpl, seq,
                    ms.as_ptr(), fr[next].0.len() as u32, &dev, &res, std::ptr::null_mut(),
                    mh.as_ptr(), fr[next].1.len() as u32)?;
                next += 1;
            } else {
                lib.enqueue_image(process_ref, bits, samples, out_bpl, seq,
                    std::ptr::null_mut(), 0, &dev, &res, std::ptr::null_mut(), std::ptr::null_mut(), 0)?;
            }
            match lib.dequeue_image(process_ref, dst_mem.as_ptr(), out_size) {
                Ok(_) => got += 1,
                Err(CrmError::SdkError(SdkError::DevelopNotCompleted)) => {}
                Err(CrmError::SdkError(SdkError::DevelopEmptyQueue)) => break,
                Err(e) => return Err(e),
            }
        }
        let que = t.elapsed();

        let seq_fps = m as f64 / seq_dur.as_secs_f64();
        let que_fps = got as f64 / que.as_secs_f64();
        trace!("\n================ CRM QUEUE THROUGHPUT ({device:?}) ================");
        trace!("frames={m}  out={} MiB/frame", out_size/(1024*1024));
        trace!("sequential sync : {seq_dur:?}  ({:.2} ms/frame, {seq_fps:.1} fps)", seq_dur.as_secs_f64()*1000.0/m as f64);
        trace!("queue (1 thread): {que:?}  ({:.2} ms/frame, {que_fps:.1} fps), got {got}", que.as_secs_f64()*1000.0/got.max(1) as f64);
        trace!("queue speedup over single-thread sync: {:.2}x", que_fps/seq_fps);
        trace!("================================================================\n");
        let _ = (&mut ms, &mut mh);
        lib.release_process_ref(process_ref)?;
        lib.terminate_sdk()?;
        return Ok(());
    }

    // ── ASYNC path: CrmGetImageAsync with the SAME CrmAllocate'd buffers that
    //    fixed the queue. Settles whether async's InvalidParameter was the
    //    non-pinned-buffer bug (same as queue) or something else (binding/mangle).
    if use_async {
        let null_cb = common::has_flag(&args, "--null-cb");
        let decompress_only = common::has_flag(&args, "--decompress");

        // The SDK rejects a COMBINED develop|decompress ref for async
        // ("CrmGetImageAsync can execute either decompress or develop"). Build a
        // single-stage DECOMPRESS-only ref + bayer output when --decompress.
        let (a_ref, a_res, a_bits, a_samples, a_bpl, a_seq, a_dst, a_dst_size) = if decompress_only {
            let dres = ResParamV13 {
                size: std::mem::size_of::<ResParamV13>() as _, struct_id: StructId::ResParamV13, format_id,
                debayer_setting: DebayerSetting::HighQuality, downscale: Downscale::X1,
                crop: Rect { x: 0, y: 0, width: w, height: h }, frame_rendering: FrameRendering::Split,
                lens_squeeze: LensSqueeze::Off, in_data_type: DataType::CrmCompressed, in_buffer_type: BufferType::Cpu,
                out_data_type: DataType::CrmDecompressed, out_buffer_type: BufferType::Cpu, decode_level: DecodeLevel::Full,
            };
            let (dw, dh) = lib.get_out_image_size(&dres, &header_pack)?;
            let dbpl = dw * 2; // 16-bit, 1 sample bayer
            let dsize = dbpl * dh;
            let dref = lib.create_process_ref(format_id, device, use_gpu, ProcessType::DECOMPRESS, Resolution::K8)?;
            let dbuf = CrmMemory::new(lib.clone(), dsize, Device::Cpu)?;
            trace!("    DECOMPRESS-only ref, bayer out {dw}x{dh} bpl={dbpl} ({} MiB)", dsize/(1024*1024));
            (dref, dres, 16u32, 1u32, dbpl, 0u32, dbuf, dsize)
        } else {
            (process_ref, res.clone(), bits, samples, out_bpl, seq,
             CrmMemory::new(lib.clone(), out_size, device)?, out_size)
        };

        let mut refcon_slot: *mut CrmVoid = std::ptr::null_mut();
        let refcon_arg = &mut refcon_slot as *mut *mut CrmVoid as *mut CrmVoid;
        let cb: GetImageCallback = if null_cb { None } else { Some(async_cb) };
        trace!("[5-async] submit CrmGetImageAsync (process={}, callback={})",
               if decompress_only { "DECOMPRESS-only" } else { "DEVELOP|DECOMPRESS" },
               if null_cb { "NULL(sync)" } else { "async_cb" });
        let submit = lib.get_image_async_raw(
            a_ref, a_dst.as_ptr(), a_dst_size, a_bits, a_samples, a_bpl, a_seq,
            src_mem.as_ptr(), comp.len() as u32,
            &dev as *const _ as *mut CrmVoid, &a_res as *const _ as *mut CrmVoid,
            refcon_arg, hdr_mem.as_ptr(), header_pack.len() as u32,
            cb,
        );
        let _ = &a_dst;
        match submit {
            Ok(_) => {
                trace!("    submit OK -> waiting for callback (<=30s)");
                let t = std::time::Instant::now();
                while !ASYNC_DONE.load(Ordering::SeqCst) {
                    if t.elapsed() > std::time::Duration::from_secs(30) {
                        trace!("    TIMEOUT: callback never fired in 30s");
                        break;
                    }
                    std::thread::yield_now();
                }
                if ASYNC_DONE.load(Ordering::SeqCst) {
                    trace!("    callback result: {:?} (after {:?})", *ASYNC_RESULT.lock().unwrap(), t.elapsed());
                }
            }
            Err(e) => {
                trace!("    submit REJECTED synchronously: {e:?}");
                match lib.get_last_error_string() {
                    Ok(msg) => trace!("    CrmGetLastError says: {msg:?}"),
                    Err(le) => trace!("    CrmGetLastError failed: {le:?}"),
                }
            }
        }
        let _ = &mut src_mem; let _ = &mut hdr_mem;
        lib.release_process_ref(process_ref)?;
        lib.terminate_sdk()?;
        trace!("[7] async probe done");
        return Ok(());
    }

    trace!("[5] strict 1:1 enqueue/dequeue loop, {frames} frames (C++ pattern, drain to EMPTY_QUEUE)");
    let mut next = 0usize;
    let mut got = 0usize;
    loop {
        // Always enqueue exactly once per iteration; NULL once inputs exhausted.
        if next < frames {
            trace!("    enqueue #{next} (src={} bytes)", comp.len());
            lib.enqueue_image(
                process_ref, bits, samples, out_bpl, seq,
                src_mem.as_ptr(), comp.len() as u32,
                &dev, &res, std::ptr::null_mut(),
                hdr_mem.as_ptr(), header_pack.len() as u32,
            )?;
            next += 1;
        } else {
            trace!("    enqueue NULL (drain)");
            lib.enqueue_image(
                process_ref, bits, samples, out_bpl, seq,
                std::ptr::null_mut(), 0,
                &dev, &res, std::ptr::null_mut(),
                std::ptr::null_mut(), 0,
            )?;
        }
        // Always dequeue exactly once per iteration.
        match lib.dequeue_image(process_ref, dst_mem.as_ptr(), out_size) {
            Ok((sz, _)) => { got += 1; trace!("    dequeue OK -> frame {got}, {sz} bytes"); }
            Err(CrmError::SdkError(SdkError::DevelopNotCompleted)) => trace!("    dequeue NOT_COMPLETED"),
            Err(CrmError::SdkError(SdkError::DevelopEmptyQueue)) => { trace!("    dequeue EMPTY_QUEUE -> drained"); break; }
            Err(e) => { trace!("    dequeue ERR {e:?}"); return Err(e); }
        }
    }

    trace!("[6] done: produced {got}/{frames} frames");
    let _ = &mut src_mem; let _ = &mut hdr_mem; // keep alive to here
    lib.release_process_ref(process_ref)?;
    lib.terminate_sdk()?;
    trace!("[7] clean exit");
    Ok(())
}

fn pick_gpu(lib: &CrmSdkLibrary) -> Option<Gpu> {
    let list = lib.get_gpu_info_list_with_device(Device::Cuda).ok()?;
    for (i, info) in list.iter().enumerate() {
        if info.support == 0 { continue; }
        return Some(match i { 0 => Gpu::Gpu1, 1 => Gpu::Gpu2, 2 => Gpu::Gpu3, 3 => Gpu::Gpu4, _ => break });
    }
    None
}
