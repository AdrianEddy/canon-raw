//! CRM decode throughput benchmark — concrete frames/sec across every viable
//! mechanism × develop-device × output-buffer combination. SDK-managed context
//! (initialize_sdk); measures pure decode throughput on the calling machine.
//!
//!   cargo run --release --example crm_bench -- <in.CRM> \
//!       --sdk-dll <CrmSdk.dll> --luts-dir <CanonLUTs> [--frames 24] [--pool-max 8]
//!       [--group sync,queue,pool,async]   (default: sync,queue,pool)
//!
//! Each result is printed (flushed) the instant it's measured, so a crash in a
//! later variant never erases earlier numbers.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use canon_raw::crm_parser::CrmMovie;
use canon_raw::*;

mod common;

macro_rules! line { ($($a:tt)*) => {{ eprintln!($($a)*); use std::io::Write; let _ = std::io::stderr().flush(); }} }

struct Frame { comp: Vec<u8>, hdr: Vec<u8> }

fn dev_param(format_id: FormatId) -> DevParamV13 {
    DevParamV13 {
        size: std::mem::size_of::<DevParamV13>() as _, struct_id: StructId::DevParamV13, format_id,
        wb_setting: WbSetting::Record, click_wb_point: Point { x: 0, y: 0 }, tint: 0.0, exposure: 0.0,
        cct: 0, sharpness: 0, gamma: Gamma::Rec709, noise: 0, aces_idt: AcesIdt::Auto,
        color_space: ColorSpace::Original, iso_setting: IsoSetting::Record, iso_speed: 0,
        wb_gain: WbGain { r: 0, g1: 0, g2: 0, b: 0 },
    }
}

#[allow(clippy::too_many_arguments)]
fn res_param(format_id: FormatId, w: u32, h: u32, in_dt: DataType, out_dt: DataType,
             in_bt: BufferType, out_bt: BufferType) -> ResParamV13 {
    ResParamV13 {
        size: std::mem::size_of::<ResParamV13>() as _, struct_id: StructId::ResParamV13, format_id,
        debayer_setting: DebayerSetting::HighQuality, downscale: Downscale::X1,
        crop: Rect { x: 0, y: 0, width: w, height: h }, frame_rendering: FrameRendering::Split,
        lens_squeeze: LensSqueeze::Off, in_data_type: in_dt, in_buffer_type: in_bt,
        out_data_type: out_dt, out_buffer_type: out_bt, decode_level: DecodeLevel::Full,
    }
}

fn pick_gpu(lib: &CrmSdkLibrary) -> Option<Gpu> {
    let list = lib.get_gpu_info_list_with_device(Device::Cuda).ok()?;
    for (i, info) in list.iter().enumerate() {
        if info.support == 0 { continue; }
        return Some(match i { 0 => Gpu::Gpu1, 1 => Gpu::Gpu2, 2 => Gpu::Gpu3, 3 => Gpu::Gpu4, _ => break });
    }
    None
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let input = std::path::PathBuf::from(&args[1]);
    let sdk_dll = common::arg_value(&args, "--sdk-dll").expect("--sdk-dll");
    let luts_dir = common::arg_value(&args, "--luts-dir").expect("--luts-dir");
    let n = common::parse_u64_arg(&args, "--frames", 24) as usize;
    let pool_max = common::parse_u64_arg(&args, "--pool-max", 8) as usize;
    let group = common::arg_value(&args, "--group").unwrap_or_else(|| "sync,queue,pool".into());
    let do_grp = |g: &str| group.split(',').any(|x| x == g);

    if let Some(parent) = std::path::PathBuf::from(&sdk_dll).parent() {
        common::add_dll_directory(parent);
    }

    let lib = Arc::new(CrmSdkLibrary::load(&sdk_dll)?);
    lib.initialize_sdk()?;
    lib.set_luts_directory(&luts_dir, true)?;

    let f = std::fs::File::open(&input)?;
    let mut crm = CrmMovie::load(f)?;
    let uuid = crm.canon_metadata_box().ok_or(CrmError::MissingData("uuid"))?.to_vec();
    let craw = crm.craw_entry().ok_or(CrmError::MissingData("craw"))?.to_vec();
    let total = crm.frame_count();
    let n = n.min(total as usize).max(1);

    line!("Loading {n} distinct frames (clip has {total})...");
    let mut frames: Vec<Frame> = Vec::with_capacity(n);
    for i in 0..n as u64 {
        let comp = crm.read_compressed_frame(i)?;
        let tm = crm.read_timed_meta(i)?;
        let hdr = lib.create_header_pack(&[
            Header { header_id: HeaderId::Uuid, data: uuid.as_ptr() as *mut _, size: uuid.len() as u32 },
            Header { header_id: HeaderId::Craw, data: craw.as_ptr() as *mut _, size: craw.len() as u32 },
            Header { header_id: HeaderId::TimedMetadata, data: tm.as_ptr() as *mut _, size: tm.len() as u32 },
        ])?;
        if i == 0 || i + 1 == n as u64 { line!("  frame {i}: comp={} hdr={}", comp.len(), hdr.len()); }
        frames.push(Frame { comp, hdr });
    }
    let frames = Arc::new(frames);
    let max_comp = frames.iter().map(|f| f.comp.len()).max().unwrap() as u32;
    let max_hdr = frames.iter().map(|f| f.hdr.len()).max().unwrap() as u32;

    let format_id = lib.get_format_id(&frames[0].hdr)?;
    let w = lib.get_image_info::<u16>(format_id, &frames[0].hdr, ImageInfoId::CaptureWidth)? as u32;
    let h = lib.get_image_info::<u16>(format_id, &frames[0].hdr, ImageInfoId::CaptureHeight)? as u32;
    let gpu = pick_gpu(&lib).unwrap_or(Gpu::Gpu1);
    let dev = dev_param(format_id);

    // Develop output sizing (16-bit RGBA).
    let (bits, samples) = (16u32, 4u32);
    let rgb_res = res_param(format_id, w, h, DataType::CrmCompressed, DataType::RgbLutApplied, BufferType::Cpu, BufferType::Cpu);
    let (ow, oh) = lib.get_out_image_size(&rgb_res, &frames[0].hdr)?;
    let rgb_bpl = ow * samples * 4 / (32 / bits);
    let rgb_size = rgb_bpl * oh;

    line!("\n{w}x{h} -> develop {ow}x{oh}  RGBA16 {} MiB/frame  gpu={gpu:?}  N={n}\n", rgb_size/(1024*1024));
    let fps = |d: Duration| n as f64 / d.as_secs_f64();
    let mut results: Vec<(String, f64)> = Vec::new();
    let mut report = |results: &mut Vec<(String,f64)>, name: &str, d: Duration| {
        let f = fps(d);
        line!("  {name:<34} {:>7.1} ms/frame   {f:>6.2} fps", d.as_secs_f64()*1000.0/n as f64);
        results.push((name.to_string(), f));
    };

    // Warmup (absorbs CUDA JIT) on a CUDA develop ref.
    {
        let r = lib.create_process_ref(format_id, Device::Cuda, gpu, ProcessType::DEVELOP|ProcessType::DECOMPRESS, Resolution::K8)?;
        let res = res_param(format_id, w, h, DataType::CrmCompressed, DataType::RgbLutApplied, BufferType::Cpu, BufferType::Cpu);
        let mut dst = vec![0u8; rgb_size as usize];
        for i in 0..2.min(n) {
            lib.get_image(r, dst.as_mut_ptr() as *mut CrmVoid, rgb_size, bits, samples, rgb_bpl,
                SampleSequence::Rgba16BitLe, &frames[i].comp, &dev, &res, &frames[i].hdr)?;
        }
        let _ = lib.release_process_ref(r);
    }

    // ───────────────── SYNC (single thread) ─────────────────
    if do_grp("sync") {
        line!("[SYNC single-thread]");
        // device, out_buffer_type, label
        let cases: &[(Device, BufferType, &str)] = &[
            (Device::Cpu,  BufferType::Cpu,  "sync  CPU-dev   CPU-out"),
            (Device::Cuda, BufferType::Cpu,  "sync  CUDA-dev  CPU-out"),
            (Device::Cuda, BufferType::Cuda, "sync  CUDA-dev  CUDA-out (zero-copy)"),
        ];
        for &(d_dev, out_bt, label) in cases {
            let usegpu = if d_dev == Device::Cpu { Gpu::NotUse } else { gpu };
            let r = lib.create_process_ref(format_id, d_dev, usegpu, ProcessType::DEVELOP|ProcessType::DECOMPRESS, Resolution::K8)?;
            let res = res_param(format_id, w, h, DataType::CrmCompressed, DataType::RgbLutApplied, BufferType::Cpu, out_bt);
            let dst = CrmMemory::new(lib.clone(), rgb_size, if out_bt==BufferType::Cuda {Device::Cuda} else {Device::Cpu})?;
            // First frame separately (exposes one-time CUDA-out setup) from steady-state.
            let t0 = Instant::now();
            lib.get_image(r, dst.as_ptr(), rgb_size, bits, samples, rgb_bpl,
                SampleSequence::Rgba16BitLe, &frames[0].comp, &dev, &res, &frames[0].hdr)?;
            let first = t0.elapsed();
            let t = Instant::now();
            for i in 1..n {
                let fr = &frames[i % frames.len()];
                lib.get_image(r, dst.as_ptr(), rgb_size, bits, samples, rgb_bpl,
                    SampleSequence::Rgba16BitLe, &fr.comp, &dev, &res, &fr.hdr)?;
            }
            let steady = t.elapsed();
            line!("  {label:<34} first={:.0}ms steady={:.1}ms/frame ({:.2} fps)",
                  first.as_secs_f64()*1000.0, steady.as_secs_f64()*1000.0/(n-1).max(1) as f64,
                  (n-1) as f64 / steady.as_secs_f64());
            results.push((label.to_string(), (n-1) as f64 / steady.as_secs_f64()));
            let _ = lib.release_process_ref(r);
        }

        // Diagnostic: CUDA-out with a FRESH CrmAllocate'd CUDA buffer per frame
        // (vs the reused buffer above) — isolates whether reuse is the stall.
        {
            let r = lib.create_process_ref(format_id, Device::Cuda, gpu, ProcessType::DEVELOP|ProcessType::DECOMPRESS, Resolution::K8)?;
            let res = res_param(format_id, w, h, DataType::CrmCompressed, DataType::RgbLutApplied, BufferType::Cpu, BufferType::Cuda);
            let probe = n.min(4);
            let t = Instant::now();
            for i in 0..probe {
                let dst = CrmMemory::new(lib.clone(), rgb_size, Device::Cuda)?; // fresh each frame
                lib.get_image(r, dst.as_ptr(), rgb_size, bits, samples, rgb_bpl,
                    SampleSequence::Rgba16BitLe, &frames[i].comp, &dev, &res, &frames[i].hdr)?;
            }
            line!("  sync  CUDA-dev  CUDA-out FRESH-buf/frame   {:.1}ms/frame ({:.2} fps)",
                  t.elapsed().as_secs_f64()*1000.0/probe as f64, probe as f64 / t.elapsed().as_secs_f64());
            let _ = lib.release_process_ref(r);
        }
    }

    // ───────────────── QUEUE (single driver thread, CPU-out only) ─────────────────
    if do_grp("queue") {
        line!("[QUEUE single driver thread — CPU output only (spec)]");
        let seq = SampleSequence::Rgba16BitLe as u32;
        for &(d_dev, label) in &[(Device::Cpu, "queue CPU-dev   CPU-out"), (Device::Cuda, "queue CUDA-dev  CPU-out")] {
            let usegpu = if d_dev == Device::Cpu { Gpu::NotUse } else { gpu };
            let r = lib.create_process_ref(format_id, d_dev, usegpu, ProcessType::DEVELOP|ProcessType::DECOMPRESS, Resolution::K8)?;
            let res = res_param(format_id, w, h, DataType::CrmCompressed, DataType::RgbLutApplied, BufferType::Cpu, BufferType::Cpu);
            let mut ms = CrmMemory::new(lib.clone(), max_comp, Device::Cpu)?;
            let mut mh = CrmMemory::new(lib.clone(), max_hdr, Device::Cpu)?;
            let dst = CrmMemory::new(lib.clone(), rgb_size, Device::Cpu)?;
            let t = Instant::now();
            let (mut next, mut got) = (0usize, 0usize);
            loop {
                if next < n {
                    let fr = &frames[next];
                    unsafe {
                        std::ptr::copy_nonoverlapping(fr.comp.as_ptr(), ms.as_mut_ptr() as *mut u8, fr.comp.len());
                        std::ptr::copy_nonoverlapping(fr.hdr.as_ptr(), mh.as_mut_ptr() as *mut u8, fr.hdr.len());
                    }
                    lib.enqueue_image(r, bits, samples, rgb_bpl, seq, ms.as_ptr(), fr.comp.len() as u32,
                        &dev, &res, std::ptr::null_mut(), mh.as_ptr(), fr.hdr.len() as u32)?;
                    next += 1;
                } else {
                    lib.enqueue_image(r, bits, samples, rgb_bpl, seq, std::ptr::null_mut(), 0,
                        &dev, &res, std::ptr::null_mut(), std::ptr::null_mut(), 0)?;
                }
                match lib.dequeue_image(r, dst.as_ptr(), rgb_size) {
                    Ok(_) => got += 1,
                    Err(CrmError::SdkError(SdkError::DevelopNotCompleted)) => {}
                    Err(CrmError::SdkError(SdkError::DevelopEmptyQueue)) => break,
                    Err(e) => return Err(e),
                }
            }
            assert_eq!(got, n);
            report(&mut results, label, t.elapsed());
            let _ = lib.release_process_ref(r);
        }
    }

    // ───────────────── POOL (sync CrmGetImage, CPU dev/out, K threads) ─────────────────
    if do_grp("pool") {
        line!("[POOL sync threads — CUDA-dev CPU-out, work-stealing]");
        for k in [1usize, 2, 4, 6, 8].into_iter().filter(|&k| k <= pool_max) {
            let t = Instant::now();
            let counter = Arc::new(AtomicUsize::new(0));
            std::thread::scope(|s| -> Result<()> {
                let mut handles = Vec::new();
                for _ in 0..k {
                    let lib = lib.clone(); let frames = frames.clone(); let counter = counter.clone();
                    let dev = dev;
                    handles.push(s.spawn(move || -> Result<()> {
                        let r = lib.create_process_ref(format_id, Device::Cuda, gpu, ProcessType::DEVELOP|ProcessType::DECOMPRESS, Resolution::K8)?;
                        let res = res_param(format_id, w, h, DataType::CrmCompressed, DataType::RgbLutApplied, BufferType::Cpu, BufferType::Cpu);
                        let dst = CrmMemory::new(lib.clone(), rgb_size, Device::Cpu)?;
                        loop {
                            let i = counter.fetch_add(1, Ordering::Relaxed);
                            if i >= n { break; }
                            let fr = &frames[i];
                            lib.get_image(r, dst.as_ptr(), rgb_size, bits, samples, rgb_bpl,
                                SampleSequence::Rgba16BitLe, &fr.comp, &dev, &res, &fr.hdr)?;
                        }
                        let _ = lib.release_process_ref(r);
                        Ok(())
                    }));
                }
                for hdl in handles { hdl.join().unwrap()?; }
                Ok(())
            })?;
            report(&mut results, &format!("pool  K={k:<2}      CUDA-dev CPU-out"), t.elapsed());
        }
    }

    // ───────────────── MULTI-QUEUE (M driver threads, each its own queue) ─────────────────
    if do_grp("mqueue") {
        line!("[MULTI-QUEUE — M driver threads each running a queue, CUDA-dev CPU-out]");
        let seq = SampleSequence::Rgba16BitLe as u32;
        for m in [1usize, 2, 3, 4].into_iter().filter(|&m| m <= pool_max) {
            let t = Instant::now();
            let counter = Arc::new(AtomicUsize::new(0));
            std::thread::scope(|s| -> Result<()> {
                let mut handles = Vec::new();
                for _ in 0..m {
                    let lib = lib.clone(); let frames = frames.clone(); let counter = counter.clone(); let dev = dev;
                    handles.push(s.spawn(move || -> Result<()> {
                        let r = lib.create_process_ref(format_id, Device::Cuda, gpu, ProcessType::DEVELOP|ProcessType::DECOMPRESS, Resolution::K8)?;
                        let res = res_param(format_id, w, h, DataType::CrmCompressed, DataType::RgbLutApplied, BufferType::Cpu, BufferType::Cpu);
                        let mut ms = CrmMemory::new(lib.clone(), max_comp, Device::Cpu)?;
                        let mut mh = CrmMemory::new(lib.clone(), max_hdr, Device::Cpu)?;
                        let dst = CrmMemory::new(lib.clone(), rgb_size, Device::Cpu)?;
                        let mut inflight = 0usize;
                        let mut exhausted = false;
                        loop {
                            if !exhausted {
                                let i = counter.fetch_add(1, Ordering::Relaxed);
                                if i < n {
                                    let fr = &frames[i];
                                    unsafe {
                                        std::ptr::copy_nonoverlapping(fr.comp.as_ptr(), ms.as_mut_ptr() as *mut u8, fr.comp.len());
                                        std::ptr::copy_nonoverlapping(fr.hdr.as_ptr(), mh.as_mut_ptr() as *mut u8, fr.hdr.len());
                                    }
                                    lib.enqueue_image(r, bits, samples, rgb_bpl, seq, ms.as_ptr(), fr.comp.len() as u32,
                                        &dev, &res, std::ptr::null_mut(), mh.as_ptr(), fr.hdr.len() as u32)?;
                                    inflight += 1;
                                } else {
                                    exhausted = true;
                                    lib.enqueue_image(r, bits, samples, rgb_bpl, seq, std::ptr::null_mut(), 0,
                                        &dev, &res, std::ptr::null_mut(), std::ptr::null_mut(), 0)?;
                                }
                            } else {
                                lib.enqueue_image(r, bits, samples, rgb_bpl, seq, std::ptr::null_mut(), 0,
                                    &dev, &res, std::ptr::null_mut(), std::ptr::null_mut(), 0)?;
                            }
                            match lib.dequeue_image(r, dst.as_ptr(), rgb_size) {
                                Ok(_) => { if inflight > 0 { inflight -= 1; } }
                                Err(CrmError::SdkError(SdkError::DevelopNotCompleted)) => {}
                                Err(CrmError::SdkError(SdkError::DevelopEmptyQueue)) => if exhausted { break; },
                                Err(e) => return Err(e),
                            }
                        }
                        let _ = (&mut ms, &mut mh);
                        let _ = lib.release_process_ref(r);
                        Ok(())
                    }));
                }
                for hdl in handles { hdl.join().unwrap()?; }
                Ok(())
            })?;
            report(&mut results, &format!("mqueue M={m:<2}     CUDA-dev CPU-out"), t.elapsed());
        }
    }

    // ───────────────── TWO-STAGE ASYNC (decompress-only ∥ develop-only) ─────────────────
    if do_grp("async") {
        line!("[ASYNC stages — 1-way and 3-way concurrency]");
        run_async_stages(&lib, &frames, format_id, w, h, gpu, &dev, max_comp, max_hdr, rgb_size, rgb_bpl, bits, samples, &mut results, &mut report)?;
    }

    line!("\n================ SUMMARY (fps, higher=better) ================");
    let base = results.iter().find(|(n,_)| n.starts_with("sync  CUDA-dev  CPU")).map(|(_,f)| *f)
        .or_else(|| results.first().map(|(_,f)| *f)).unwrap_or(1.0);
    for (name, f) in &results {
        line!("  {name:<40} {f:>6.2} fps   {:>4.2}x", f / base);
    }
    line!("==============================================================");
    lib.terminate_sdk()?;
    Ok(())
}

// ---- async stage throughput (manual 3-way concurrency via per-slot callbacks) ----
// inRefCon is *void** in the DLL — it must be a VALID pointer, so we hand each
// slot a writable scratch cell and identify the completed slot by matching the
// dst-buffer pointer the callback reports back (each slot has a unique bayer buf).
const SLOTS: usize = 3;
static SLOT_DONE: [AtomicBool; SLOTS] = [AtomicBool::new(true), AtomicBool::new(true), AtomicBool::new(true)];
static SLOT_DST: [AtomicUsize; SLOTS] = [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];
static SLOT_SCRATCH: [AtomicUsize; SLOTS] = [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];

unsafe extern "system" fn slot_cb(
    _pr: ProcessRef, dst: *mut CrmVoid, _dbs: CrmUInt32, _dds: CrmUInt32,
    _b: CrmUInt32, _s: CrmUInt32, _bpl: CrmUInt32, _ss: CrmUInt32,
    _sb: *mut CrmVoid, _sds: CrmUInt32, _dp: *mut CrmVoid, _rp: *mut CrmVoid,
    _rc: *mut CrmVoid, _hb: *mut CrmVoid, _hds: CrmUInt32, _err: SdkError,
) -> SdkError {
    let dptr = dst as usize;
    for s in 0..SLOTS {
        if SLOT_DST[s].load(Ordering::SeqCst) == dptr { SLOT_DONE[s].store(true, Ordering::SeqCst); break; }
    }
    SdkError::Ok
}

#[allow(clippy::too_many_arguments)]
fn run_async_stages(
    lib: &Arc<CrmSdkLibrary>, frames: &Arc<Vec<Frame>>, format_id: FormatId, w: u32, h: u32, gpu: Gpu,
    dev: &DevParamV13, max_comp: u32, max_hdr: u32, rgb_size: u32, rgb_bpl: u32, bits: u32, samples: u32,
    results: &mut Vec<(String,f64)>, report: &mut impl FnMut(&mut Vec<(String,f64)>, &str, Duration),
) -> Result<()> {
    let n = frames.len();
    // DECOMPRESS-only ref + bayer sizing.
    let dres = res_param(format_id, w, h, DataType::CrmCompressed, DataType::CrmDecompressed, BufferType::Cpu, BufferType::Cpu);
    let (dw, dh) = lib.get_out_image_size(&dres, &frames[0].hdr)?;
    let dbpl = dw * 2; let dsize = dbpl * dh;
    // Decompress is CPU-only (spec) — a CUDA decompress ref is rejected.
    let _ = gpu;
    let dref = lib.create_process_ref(format_id, Device::Cpu, Gpu::NotUse, ProcessType::DECOMPRESS, Resolution::K8)?;

    // Per-slot pinned input/header/bayer buffers.
    let mut src: Vec<CrmMemory> = (0..SLOTS).map(|_| CrmMemory::new(lib.clone(), max_comp, Device::Cpu)).collect::<Result<_>>()?;
    let mut hdr: Vec<CrmMemory> = (0..SLOTS).map(|_| CrmMemory::new(lib.clone(), max_hdr, Device::Cpu)).collect::<Result<_>>()?;
    let bay: Vec<CrmMemory> = (0..SLOTS).map(|_| CrmMemory::new(lib.clone(), dsize, Device::Cpu)).collect::<Result<_>>()?;

    let submit_decompress = |lib: &CrmSdkLibrary, slot: usize, fi: usize, src: &mut [CrmMemory], hdr: &mut [CrmMemory], bay: &[CrmMemory]| -> Result<()> {
        let fr = &frames[fi];
        unsafe {
            std::ptr::copy_nonoverlapping(fr.comp.as_ptr(), src[slot].as_mut_ptr() as *mut u8, fr.comp.len());
            std::ptr::copy_nonoverlapping(fr.hdr.as_ptr(), hdr[slot].as_mut_ptr() as *mut u8, fr.hdr.len());
        }
        SLOT_DST[slot].store(bay[slot].as_ptr() as usize, Ordering::SeqCst);
        SLOT_DONE[slot].store(false, Ordering::SeqCst);
        let refcon = &SLOT_SCRATCH[slot] as *const AtomicUsize as *mut CrmVoid;
        lib.get_image_async_raw(dref, bay[slot].as_ptr(), dsize, 16, 1, dbpl, 0,
            src[slot].as_ptr(), fr.comp.len() as u32,
            dev as *const _ as *mut CrmVoid, &dres as *const _ as *mut CrmVoid,
            refcon, hdr[slot].as_ptr(), fr.hdr.len() as u32, Some(slot_cb))?;
        Ok(())
    };

    for &conc in &[1usize, SLOTS] {
        for s in 0..SLOTS { SLOT_DONE[s].store(true, Ordering::SeqCst); }
        let mut busy = [false; SLOTS];
        let mut submitted = 0usize;
        let mut completed = 0usize;
        let t = Instant::now();
        while completed < n {
            for slot in 0..conc {
                if !busy[slot] && submitted < n {
                    submit_decompress(lib, slot, submitted, &mut src, &mut hdr, &bay)?;
                    busy[slot] = true;
                    submitted += 1;
                }
            }
            for slot in 0..conc {
                if busy[slot] && SLOT_DONE[slot].load(Ordering::SeqCst) {
                    busy[slot] = false;
                    completed += 1;
                }
            }
            std::thread::yield_now();
        }
        let d = t.elapsed();
        let fps = n as f64 / d.as_secs_f64();
        line!("  async DECOMPRESS-only x{conc} (CPU bayer)   {:.1}ms/frame ({fps:.2} fps)",
              d.as_secs_f64()*1000.0/n as f64);
        results.push((format!("async DECOMPRESS x{conc} (bayer)"), fps));
    }
    let _ = (&mut src, &mut hdr);
    let _ = lib.release_process_ref(dref);
    Ok(())
}
