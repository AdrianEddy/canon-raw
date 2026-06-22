// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

//! Bundled per-decoder facade — hides the SDK's per-frame
//! `set_timed_meta` → `decode_frame_to_gpu` two-call sequence behind
//! one [`DecodeContext`] type. Resolves the output buffer size **once
//! at construction** and only re-resolves when geometry-affecting
//! parameters change; per-frame timed metadata updates (colour
//! transforms) do not invalidate the cached size.
//!
//! See [`DecodeContext`] for the public surface.

use std::sync::Arc;

use crate::{
    CrmSdkLibrary, CrmVoid, DevParamV13, ResParamV13, Header, HeaderId,
    FormatId, Device, Gpu, ProcessType, Resolution, SampleSequence,
    Downscale, DecodeLevel, Rect, ProcessRef, Result,
};

/// Geometry-only parameters that drive the output buffer size.
///
/// Mutating any field invalidates the cached `out_size`; mutating only
/// the colour-transform fields of `dev_param` (white balance, exposure,
/// gamma) via [`DecodeContext::dev_param_color_mut`] or
/// [`DecodeContext::res_param_color_mut`] does **not** invalidate.
///
/// Source: TestCrmSdk.cpp:1855-1864; `crm-rs/examples/decode_queue.rs:105-106`
/// — `pre_process` is called exactly once at setup and the output
/// buffer is reused across frames with the same dev/res config.
/// TimedMetadata carries per-frame WB / exposure / log-curve color
/// transforms — it does **not** change output geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputGeometry {
    pub crop:              Rect,
    pub downscale:         Downscale,
    pub decode_level:      DecodeLevel,
    pub bits_per_sample:   u32,
    pub samples_per_pixel: u32,
    pub bytes_per_line:    u32,
}

impl OutputGeometry {
    /// The SDK's output row pitch (`inBytesPerLine`) for a given output
    /// width / bit depth / channel count.
    ///
    /// `CrmGetImage` packs each row into a whole number of 32-bit words:
    /// `data_num_in_int = 32 / bits_per_sample` samples per `i32`, so a
    /// row is `out_width * samples_per_pixel * 4 / data_num_in_int`
    /// bytes. 12-bit is the documented special case — three 12-bit
    /// samples share two `i32`s (`* 4 * 3 / 8`). This mirrors the SDK
    /// sample code's `out_bytes_per_line` exactly (see
    /// `TestCrmSdk.cpp` / the crate's `examples/decode*.rs`); callers
    /// (e.g. `gpu-video`) must compute the same value to size the
    /// `CrmGetImage` destination buffer, so it lives here as the SDK
    /// wrapper's canonical formula rather than being re-derived per
    /// call site.
    #[inline]
    pub const fn bytes_per_line(out_width: u32, bits_per_sample: u32, samples_per_pixel: u32) -> u32 {
        let i32_bytes = core::mem::size_of::<i32>() as u32;
        if bits_per_sample == 12 {
            out_width * samples_per_pixel * i32_bytes * 3 / 8
        } else {
            let data_num_in_int = 32 / if bits_per_sample == 0 { 1 } else { bits_per_sample };
            out_width * samples_per_pixel * i32_bytes
                / if data_num_in_int == 0 { 1 } else { data_num_in_int }
        }
    }
}

/// Bundled per-decoder facade.
///
/// Owns the SDK `ProcessRef`, the cached header pack covering the
/// clip-static UUID + CRAW entry + most-recent TimedMetadata, the
/// `DevParamV13`/`ResParamV13` parameter structs, and the resolved
/// output buffer size for the current geometry.
///
/// # Lifecycle
///
/// - [`new`](Self::new) constructs and resolves `out_size` once.
/// - Per-frame: [`set_timed_meta`](Self::set_timed_meta) then
///   [`decode_frame_cpu_into`](Self::decode_frame_cpu_into) or
///   [`decode_frame_to_gpu`](Self::decode_frame_to_gpu). Neither
///   re-runs `pre_process`.
/// - Color-only param edits: [`dev_param_color_mut`](Self::dev_param_color_mut)
///   / [`res_param_color_mut`](Self::res_param_color_mut). Cheap; no
///   FFI hop.
/// - Geometry edits: [`set_geometry`](Self::set_geometry) re-runs
///   `pre_process` with the new geometry.
pub struct DecodeContext {
    sdk:             Arc<CrmSdkLibrary>,
    process_ref:     ProcessRef,
    /// Lane the process_ref was created against. Drives whether
    /// `resolve_output_size` calls `pre_process_for_get_image_with_gpu_memory`
    /// (GPU lanes) or computes the size analytically from `geometry`
    /// (CPU lane — `pre_process_for_get_image_with_gpu_memory` returns
    /// `InvalidParameter` when `device == Cpu` because the SDK reserves
    /// that pre-process path for GPU memory targets).
    device:          Device,
    dev_param:       DevParamV13,
    res_param:       ResParamV13,
    /// Pre-built header pack covering clip-static UUID + CRAW entry +
    /// the most-recently-set TimedMetadata. Rebuilt by
    /// [`set_timed_meta`](Self::set_timed_meta). Header-pack size does
    /// NOT affect output buffer size.
    header_pack:     Vec<u8>,
    uuid_box:        Vec<u8>,
    craw_entry:      Vec<u8>,
    format_id:       FormatId,
    sample_sequence: SampleSequence,
    /// Current geometry (drives both `pre_process` and `out_size`).
    geometry:        OutputGeometry,
    /// Resolved output buffer size for `self.geometry`. Always `Some`
    /// once the first frame has been resolved (always after `new()`).
    out_size:        Option<u32>,
}

// DecodeContext — Send only. Mutators take `&mut self` (the read paths
// `decode_frame_*_into` take `&self`, where the SDK serializes internally
// per the per-decoder contract); the `Arc<CrmSdkLibrary>` keeps the SDK
// alive for `process_ref`. Not `Sync`: the cached header_pack / out_size
// fields would let an interleaved `set_timed_meta` + `decode_frame_to_gpu`
// against a *shared* reference violate the per-decoder single-consumer
// contract.
unsafe impl Send for DecodeContext {}

impl DecodeContext {
    /// Construct + immediately resolve `out_size` from
    /// `representative_compressed` (any frame from the clip — output
    /// size is geometry-only, the SDK just needs some valid compressed
    /// bytes to pass its parameter checks). The decode loop can then
    /// call [`required_output_size`](Self::required_output_size) (no
    /// FFI hop) every frame.
    pub fn new(
        sdk:                       Arc<CrmSdkLibrary>,
        format_id:                 FormatId,
        use_device:                Device,
        use_gpu:                   Gpu,
        process_type:              ProcessType,
        max_res:                   Resolution,
        dev_param:                 DevParamV13,
        res_param:                 ResParamV13,
        uuid_box:                  Vec<u8>,
        craw_entry:                Vec<u8>,
        initial_timed_meta:        &[u8],
        sample_sequence:           SampleSequence,
        geometry:                  OutputGeometry,
        representative_compressed: &[u8],
    ) -> Result<Self> {
        let process_ref = sdk.create_process_ref(format_id, use_device, use_gpu, process_type, max_res)?;
        let mut header_pack = Vec::with_capacity(64 * 1024);
        Self::build_header_pack_into(&sdk, &uuid_box, &craw_entry, initial_timed_meta, &mut header_pack)?;

        let mut me = Self {
            sdk,
            process_ref,
            device: use_device,
            dev_param,
            res_param,
            header_pack,
            uuid_box,
            craw_entry,
            format_id,
            sample_sequence,
            geometry,
            out_size: None,
        };
        me.resolve_output_size(representative_compressed)?;
        Ok(me)
    }

    /// The geometry the context was last resolved for.
    pub fn geometry(&self) -> OutputGeometry { self.geometry }

    /// The format id passed at construction.
    pub fn format_id(&self) -> FormatId { self.format_id }

    /// The active sample-sequence layout (output pixel order).
    pub fn sample_sequence(&self) -> SampleSequence { self.sample_sequence }

    /// Cheap accessor — returns the resolved output buffer size.
    /// Panics in debug if `resolve_output_size` has not been called yet
    /// (cannot happen via the public API — `new` always resolves).
    pub fn required_output_size(&self) -> u32 {
        self.out_size.expect("DecodeContext invariant: out_size resolved in new()")
    }

    /// Update geometry. Re-runs `pre_process` with the new geometry +
    /// the supplied representative frame. Returns the new size.
    pub fn set_geometry(&mut self, geometry: OutputGeometry, representative_compressed: &[u8]) -> Result<u32> {
        if geometry == self.geometry {
            return Ok(self.required_output_size());
        }
        self.geometry = geometry;
        self.out_size = None;
        self.resolve_output_size(representative_compressed)
    }

    /// Update the per-frame TimedMetadata. Rebuilds `header_pack` in
    /// place; the pre-warmed `Vec<u8>` does not grow after the first
    /// frame. **Does NOT invalidate `out_size`** — TimedMetadata carries
    /// color-transform fields (WB, exposure, gamma) that do not affect
    /// output geometry.
    pub fn set_timed_meta(&mut self, timed_meta: &[u8]) -> Result<()> {
        Self::build_header_pack_into(
            &self.sdk, &self.uuid_box, &self.craw_entry, timed_meta, &mut self.header_pack,
        )
    }

    /// Decode one frame to a CPU destination buffer. The bits-per-sample
    /// / samples-per-pixel / bytes-per-line fields of the active
    /// `OutputGeometry` drive the layout.
    pub fn decode_frame_cpu_into(&self, compressed: &[u8], dst: &mut [u8]) -> Result<u32> {
        self.sdk.get_image(
            self.process_ref,
            dst.as_mut_ptr() as *mut CrmVoid, dst.len() as u32,
            self.geometry.bits_per_sample,
            self.geometry.samples_per_pixel,
            self.geometry.bytes_per_line,
            self.sample_sequence,
            compressed,
            &self.dev_param,
            &self.res_param,
            &self.header_pack,
        )
    }

    /// Decode one frame directly into the GPU buffer `gpu_dst` of size
    /// `gpu_dst_size` bytes. The buffer's actual memory backend must
    /// match the lane the SDK was initialized for (CUDA / OpenCL / Metal)
    /// **and** `self.res_param.out_buffer_type` must name that same lane
    /// (the SDK's `CrmGetImage` reads `out_buffer_type` to decide whether
    /// the destination pointer is a CPU pointer, a `CUdeviceptr`, a
    /// `cl_mem`, or an `id<MTLBuffer>`).
    ///
    /// The compressed source is a CPU buffer — the SDK uploads/streams
    /// it into the device internally. This is the modern path (the
    /// deprecated `CrmGetImageWithGpuMemory` required a GPU source
    /// buffer too, but `CrmGetImage` with `out_buffer_type=<gpu>` and
    /// `in_buffer_type=Cpu` is functionally equivalent and the one
    /// `TestCrmSdk.cpp` uses for every CRM clip — see the
    /// `testCrmGetImageWithProvidedQueue` path § 1953-2367).
    pub fn decode_frame_to_gpu(&self, compressed: &[u8], gpu_dst: *mut CrmVoid, gpu_dst_size: u32) -> Result<u32> {
        self.sdk.get_image(
            self.process_ref,
            gpu_dst,
            gpu_dst_size,
            self.geometry.bits_per_sample,
            self.geometry.samples_per_pixel,
            self.geometry.bytes_per_line,
            self.sample_sequence,
            compressed,
            &self.dev_param,
            &self.res_param,
            &self.header_pack,
        )
    }

    /// Async sibling of [`decode_frame_to_gpu`](Self::decode_frame_to_gpu):
    /// submit the decode via `CrmGetImageAsync` and return a future that
    /// resolves when the SDK fires its completion callback. The combined
    /// `DEVELOP|DECOMPRESS` ref accepts up to **3** such jobs in flight; a
    /// 4th submission fails immediately with
    /// [`SdkError::DecompressNoResource`](crate::SdkError::DecompressNoResource)
    /// — submit returns `Err` at the call site, not via the future.
    ///
    /// Borrows `self` (process ref + params + header pack) and `compressed`
    /// for the future's whole lifetime: the SDK reads both until completion,
    /// so neither may move while the future is pending. The header pack here
    /// is the context's most-recently-set TimedMetadata, so concurrent
    /// jobs all share that colour metadata — to pipeline *distinct* frames
    /// with per-frame TimedMetadata, give each its own context or header
    /// pack (see the gpu-video pipelining plan § 5.2).
    pub fn decode_frame_to_gpu_future<'a>(
        &'a self,
        compressed:   &'a [u8],
        gpu_dst:      *mut CrmVoid,
        gpu_dst_size: u32,
    ) -> Result<crate::GetImageFuture<'a>> {
        self.sdk.get_image_future(
            self.process_ref,
            gpu_dst,
            gpu_dst_size,
            self.geometry.bits_per_sample,
            self.geometry.samples_per_pixel,
            self.geometry.bytes_per_line,
            self.sample_sequence,
            compressed,
            &self.dev_param,
            &self.res_param,
            &self.header_pack,
        )
    }

    /// Mutate color-transform fields of `dev_param` (WB, exposure, gamma,
    /// tint, etc.). Caller MUST NOT mutate geometry fields (`size`,
    /// `struct_id`, `format_id`) through this accessor — geometry
    /// changes go through [`set_geometry`](Self::set_geometry).
    pub fn dev_param_color_mut(&mut self) -> &mut DevParamV13 { &mut self.dev_param }

    /// Read-only access to `dev_param`.
    pub fn dev_param(&self) -> &DevParamV13 { &self.dev_param }

    /// Mutate color-only fields of `res_param` (e.g. `debayer_setting`,
    /// `frame_rendering`). Geometry-affecting fields (`crop`,
    /// `downscale`, `decode_level`, `in_data_type`, `out_data_type`,
    /// `in_buffer_type`, `out_buffer_type`) must be changed through
    /// [`set_geometry`](Self::set_geometry).
    pub fn res_param_color_mut(&mut self) -> &mut ResParamV13 { &mut self.res_param }

    /// Read-only access to `res_param`.
    pub fn res_param(&self) -> &ResParamV13 { &self.res_param }

    /// The underlying SDK handle, useful for callers that need to invoke
    /// methods outside the bundled facade.
    pub fn sdk(&self) -> &Arc<CrmSdkLibrary> { &self.sdk }

    /// The active `ProcessRef`. Released automatically on `Drop`.
    pub fn process_ref(&self) -> ProcessRef { self.process_ref }

    fn resolve_output_size(&mut self, _representative_compressed: &[u8]) -> Result<u32> {
        // Output size is `bytes_per_line × output_height` regardless of
        // device — `CrmGetImage` writes exactly that many bytes per the
        // SDK spec (§ 7.3.1, "outDstBuffer must be large enough to hold
        // inBytesPerLine × outHeight bytes"). The legacy GPU path used
        // `CrmPreProcessForGetImageWithGpuMemory` to size the SDK's own
        // scratch — that's the deprecated `CrmGetImageWithGpuMemory`
        // flow (spec § 7.3.4 / 7.3.5); the modern `CrmGetImage` flow
        // sizes the output identically across CPU and GPU lanes.
        let _ = self.device; // kept; future per-lane padding may differ
        let h = effective_output_height(&self.res_param);
        let sz = self.geometry.bytes_per_line.saturating_mul(h);
        self.out_size = Some(sz);
        Ok(sz)
    }

    fn build_header_pack_into(
        sdk:        &CrmSdkLibrary,
        uuid:       &[u8],
        craw:       &[u8],
        timed_meta: &[u8],
        dst:        &mut Vec<u8>,
    ) -> Result<()> {
        let headers = [
            Header { header_id: HeaderId::Uuid,          data: uuid.as_ptr()       as *mut _, size: uuid.len()       as u32 },
            Header { header_id: HeaderId::Craw,          data: craw.as_ptr()       as *mut _, size: craw.len()       as u32 },
            Header { header_id: HeaderId::TimedMetadata, data: timed_meta.as_ptr() as *mut _, size: timed_meta.len() as u32 },
        ];
        sdk.create_header_pack_into(&headers, dst)
    }
}

impl Drop for DecodeContext {
    fn drop(&mut self) {
        let _ = self.sdk.release_process_ref(self.process_ref);
    }
}

/// CPU-output-size helper — `crop.height` divided by the active
/// downscale factor. Mirrors the GPU pre-process's height arithmetic
/// without an FFI hop.
fn effective_output_height(res_param: &ResParamV13) -> u32 {
    let h = res_param.crop.height;
    let factor: u32 = match res_param.downscale {
        Downscale::X1                                 => 1,
        Downscale::X2 | Downscale::Bayer2             => 2,
        Downscale::X4 | Downscale::Bayer4             => 4,
        // The named output formats (HD/SD/QFHD/4K-DCI/2K-DCI) target
        // specific frame heights regardless of input — the SDK rescales
        // to fit. The CPU lane in gpu-video only uses X1/X2/X4 in v1;
        // for the named modes we fall back to the SDK pre_process path
        // (which works for CPU device when the output is a named-mode
        // target — InvalidParameter only triggers on the X1/X2/X4
        // bytes-per-line × height shape).
        Downscale::Hd                                 => return 1080,
        Downscale::Sd                                 => return 480,
        Downscale::Qfhd                               => return 2160,
        Downscale::K4Dci                              => return 2160,
        Downscale::K2Dci                              => return 1080,
    };
    h.div_ceil(factor)
}
