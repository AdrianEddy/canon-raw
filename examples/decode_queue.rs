// SPDX-License-Identifier: MIT OR Apache-2.0

//! Develop a whole `.CRM` clip through the SDK's own queue
//! (`CrmEnqueueImage` / `CrmDequeueImage`) and write each frame as a JPEG.
//!
//!   cargo run --release --example decode_queue -- <input.CRM> //!       --sdk-dll <CrmSdk> --luts-dir <dir containing CanonLUTs> --out <dir> //!       [--device cpu|cuda|opencl] [--frames <n>]
//!
//! The queue is CPU-output only (spec § 6.3.16). See [`Slot`] for the
//! undocumented rules the loop below has to follow.

use canon_raw::crm_parser::CrmMovie;
use canon_raw::*;
use std::sync::Arc;

mod common;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || common::has_flag(&args, "--help") {
        eprintln!(
            "Usage:\n  cargo run --example decode_queue -- <input.CRM> --sdk-dll <path> --luts-dir <dir containing CanonLUTs> --out <dir>\n\
             Optional:\n  --device <cpu|cuda|opencl> --frames <n>"
        );
        return Ok(());
    }

    let input = std::path::PathBuf::from(&args[1]);
    let out_dir =
        std::path::PathBuf::from(common::arg_value(&args, "--out").ok_or(CrmError::InvalidParameter("--out <dir>"))?);
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
    let canon_metadata_box = crm.canon_metadata_box().ok_or(CrmError::MissingData("Canon metadata box"))?.to_vec();
    let craw_entry = crm.craw_entry().to_vec();

    let lib = Arc::new(unsafe { CrmSdkLibrary::load(&sdk_dll) }?);
    lib.initialize_sdk()?;
    lib.set_luts_directory(&luts_dir, true)?;

    let timed0 = crm.read_timed_meta(0)?;
    let header_pack0 = lib.create_header_pack(&[
        Header::uuid(&canon_metadata_box),
        Header::craw(&craw_entry),
        Header::timed_metadata(&timed0),
    ])?;

    let format_id = lib.get_format_id(&header_pack0)?;
    let w = u32::from(lib.get_image_info(format_id, &header_pack0, image_info::CaptureWidth)?);
    let h = u32::from(lib.get_image_info(format_id, &header_pack0, image_info::CaptureHeight)?);

    let use_gpu = common::pick_first_supported_gpu(&lib, device).unwrap_or(Gpu::NotUse);
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

    let (out_w, out_h) = lib.get_out_image_size(&res_param, &header_pack0)?;
    let out_bits_per_sample = 8u32;
    let samples_per_pixel = 3u32;
    let out_bytes_per_line = OutputGeometry::bytes_per_line(out_w, out_bits_per_sample, samples_per_pixel);
    let dst_buffer_size = out_bytes_per_line * out_h;
    let dst_buffer = CrmMemory::new(lib.clone(), dst_buffer_size, Device::Cpu)?;

    let frame_count = crm.frame_count().min(frames_limit);

    let mut max_comp = 0u32;
    let mut max_hdr = 0u32;
    for i in 0..frame_count {
        max_comp = max_comp.max(crm.compressed_frame_size(i)?);
        max_hdr = max_hdr.max(header_pack0.len() as u32 + crm.timed_meta_size(i)?);
    }

    let mut slots: Vec<Slot> = Vec::new();
    let mut free: Vec<usize> = Vec::new();
    let mut next_enqueue = 0u64;
    let mut outputs_written = 0u64;
    let mut idle_polls = 0u32;

    loop {
        // ── exactly ONE enqueue per iteration (requirement 2) ────────────
        if next_enqueue < frame_count {
            let compressed = crm.read_compressed_frame(next_enqueue)?;
            let timed = crm.read_timed_meta(next_enqueue)?;
            let header_pack = lib.create_header_pack(&[
                Header::uuid(&canon_metadata_box),
                Header::craw(&craw_entry),
                Header::timed_metadata(&timed),
            ])?;

            // A slot is free only once its `inRefCon` has come back, i.e.
            // once the SDK has finished reading its source buffer, so
            // staging into it here can never disturb a pending job
            // (requirement 1). The pool grows only while the SDK holds
            // submissions without returning any, so it converges on the
            // process ref's internal queue depth — a handful of slots, not
            // one per frame of the movie.
            let slot = match free.pop() {
                Some(slot) => slot,
                None => {
                    if slots.len() == MAX_SLOTS {
                        return Err(CrmError::Other(format!(
                            "the SDK is holding {MAX_SLOTS} submissions without returning one at \
                             {outputs_written}/{frame_count} frames — the queue is not draining"
                        )));
                    }
                    slots.push(Slot::new(&lib, max_comp.max(1), max_hdr.max(1), slots.len())?);
                    slots.len() - 1
                }
            };
            slots[slot].stage(next_enqueue, &compressed, &header_pack);

            let staged = &slots[slot];
            unsafe {
                lib.enqueue_image(
                    process_ref,
                    out_bits_per_sample,
                    samples_per_pixel,
                    out_bytes_per_line,
                    SampleSequence::Rgb as u32,
                    staged.src.as_ptr(),
                    staged.src_len,
                    &dev_param,
                    &res_param,
                    staged.ref_con(),
                    staged.hdr.as_ptr(),
                    staged.hdr_len,
                )
            }?;
            next_enqueue += 1;
        } else {
            // End-of-stream sentinel — re-sent every drain iteration
            // (requirement 3).
            unsafe {
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
                )
            }?;
        }

        // ── exactly ONE dequeue per iteration ────────────────────────────
        match unsafe { lib.dequeue_image(process_ref, dst_buffer.as_ptr(), dst_buffer_size) } {
            Ok((dst_data_size, out_ref_con)) => {
                idle_polls = 0;
                // `CrmDequeueImage` hands back the `inRefCon` of the
                // submission that just completed. It names both the frame
                // these pixels came from and the slot the SDK has stopped
                // reading, so it is the one authority for recycling.
                let tag = unsafe { (out_ref_con as *const Tag).as_ref() }
                    .ok_or(CrmError::MissingData("CrmDequeueImage returned a developed frame with no inRefCon"))?;
                let Tag { frame, slot } = *tag;
                write_rgb8_jpeg(&out_dir, frame, dst_buffer.as_ptr(), dst_data_size, out_w, out_h)?;
                outputs_written += 1;
                free.push(slot);
            }
            Err(CrmError::SdkError(SdkError::DevelopNotCompleted)) => {
                // A real enqueue advanced the pipeline this iteration, so
                // only the drain phase can actually spin.
                if next_enqueue >= frame_count {
                    idle_polls += 1;
                    if idle_polls > MAX_IDLE_POLLS {
                        return Err(CrmError::Other(format!(
                            "the queue made no progress across {idle_polls} consecutive idle dequeues \
                             at {outputs_written}/{frame_count} frames — poll budget exhausted, and \
                             every idle poll leaks an SDK worker thread"
                        )));
                    }
                    if idle_polls > FREE_IDLE_POLLS {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
            }
            // The SDK's own end-of-drain — the ONLY termination condition.
            Err(CrmError::SdkError(SdkError::DevelopEmptyQueue)) => break,
            Err(e) => return Err(e),
        }
    }

    let peak_slots = slots.len();
    unsafe { lib.release_process_ref(process_ref) }?;
    // Every `CrmAllocate`d buffer goes back before `CrmTerminateSDK` tears
    // the allocator down.
    drop(slots);
    drop(dst_buffer);
    unsafe { lib.terminate_sdk() }?;

    println!("Wrote {outputs_written} frames to {} ({peak_slots} staging slots)", out_dir.display());
    Ok(())
}

/// Idle drain polls that run unpaced before the backoff kicks in.
const FREE_IDLE_POLLS: u32 = 64;

/// Absolute cap on consecutive idle drain polls — each one costs a leaked
/// SDK thread, so this is the bound that keeps a wedged queue from taking
/// the machine with it.
const MAX_IDLE_POLLS: u32 = 2_000;

/// Upper bound on the staging pool, i.e. on submissions the SDK may hold
/// without returning one. Far above any real queue depth; it exists so a
/// wedged queue fails loudly instead of allocating a slot per frame.
const MAX_SLOTS: usize = 32;

/// The `inRefCon` handed to `CrmEnqueueImage` and handed straight back by
/// `CrmDequeueImage`. Owned by its [`Slot`] — never `Box::into_raw`'d — so
/// no exit path out of the drain loop can leak it.
#[derive(Clone, Copy)]
struct Tag {
    frame: u64,
    slot: usize,
}

/// One in-flight submission's `CrmAllocate`d staging.
///
/// The queue API's contract (CRMSDK 2.10). Requirements 1–3 are in neither
/// the header nor the Interface Spec; all three were established
/// empirically:
///
///   1. Source and header buffers must come from `CrmAllocate`
///      ([`CrmMemory`]) — a plain Rust `Vec` faults the SDK's worker — and
///      must be **per submission**. `CrmEnqueueImage` does *not* copy the
///      source: `CrmImpl_EnqueueImage` stores the caller's `inSrcBuffer`
///      pointer verbatim in its ring node (its one `memcpy` copies the
///      header params `CrmGetParam` had just parsed out — which is why the
///      header, unlike the source, *is* consumed eagerly), and a worker
///      reads the frame later. Measured on CRMSDK 2.10 / RTX 5090:
///      scribbling over a submitted source between `CrmEnqueueImage` and
///      the next `CrmDequeueImage` fails the develop with `Unknown`;
///      scribbling after that dequeue is harmless. A single shared staging
///      buffer therefore survives *this exact* loop shape only because the
///      overwrite happens to land before the enqueue — an undocumented
///      window, and this ref keeps six submissions in flight inside it. One
///      slot per submission costs a few MB and makes the question moot.
///   2. Exactly one `CrmEnqueueImage` and one `CrmDequeueImage` per
///      iteration — the enqueue is what advances the internal pipeline.
///   3. The end-of-stream sentinel is a NULL enqueue on **every** drain
///      iteration, not once. Termination is the SDK answering
///      `DEVELOP_EMPTY_QUEUE`, never our own frame count.
///
/// Driving it any other way does not return an error: `CrmDequeueImage`
/// answers `DEVELOP_NOT_COMPLETED` forever, and every such poll spawns —
/// and leaks — an OS thread inside the SDK. 39 000 idle polls once produced
/// 38 907 threads and 9.4 GB, so the drain below bounds the poll *count*,
/// not just wall-clock.
struct Slot {
    src: CrmMemory,
    hdr: CrmMemory,
    src_len: u32,
    hdr_len: u32,
    tag: Box<Tag>,
}

impl Slot {
    fn new(lib: &Arc<CrmSdkLibrary>, src_capacity: u32, hdr_capacity: u32, index: usize) -> Result<Self> {
        Ok(Self {
            src: CrmMemory::new(lib.clone(), src_capacity, Device::Cpu)?,
            hdr: CrmMemory::new(lib.clone(), hdr_capacity, Device::Cpu)?,
            src_len: 0,
            hdr_len: 0,
            tag: Box::new(Tag { frame: 0, slot: index }),
        })
    }

    /// Stage one frame. Only ever called on a slot whose previous
    /// submission has been dequeued, so the SDK is not reading it.
    fn stage(&mut self, frame: u64, compressed: &[u8], header_pack: &[u8]) {
        // SAFETY: both allocations were sized to the movie's largest frame
        // and largest header pack respectively.
        unsafe {
            std::ptr::copy_nonoverlapping(compressed.as_ptr(), self.src.as_mut_ptr() as *mut u8, compressed.len());
            std::ptr::copy_nonoverlapping(header_pack.as_ptr(), self.hdr.as_mut_ptr() as *mut u8, header_pack.len());
        }
        self.src_len = compressed.len() as u32;
        self.hdr_len = header_pack.len() as u32;
        self.tag.frame = frame;
    }

    fn ref_con(&self) -> *mut CrmVoid {
        &*self.tag as *const Tag as *mut CrmVoid
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
