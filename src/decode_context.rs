// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

//! Bundled per-decoder facade — hides the SDK's per-frame
//! `set_timed_meta` → `decode_frame_to_gpu` two-call sequence behind
//! one [`DecodeContext`] type.
//!
//! # The one authority for the output raster
//!
//! `ResParamV13` is the **only** copy of the raster inputs the context
//! keeps: [`OutputGeometry`] is a *view* built from it on demand, not a
//! second store that has to be mirrored back. Every path that can write
//! `res_param` — [`DecodeContext::new`],
//! [`DecodeContext::set_geometry`], [`DecodeContext::edit_res_param`],
//! [`DecodeContext::edit_res_param_in_place`] — funnels through one private
//! resolve+commit that asks `CrmGetOutImageSize` and only then writes.
//! So the struct the SDK is handed per frame and the cached
//! width/height/pitch/size cannot describe different rasters: there is
//! no reachable code path that moves one without the other.
//!
//! `dev_param` (white balance, exposure, gamma) and the per-frame
//! TimedMetadata carry colour transforms only — neither can move the
//! raster, so [`DecodeContext::dev_param_color_mut`] hands out a plain
//! `&mut` and [`DecodeContext::set_timed_meta`] never re-resolves.
//!
//! See [`DecodeContext`] for the public surface.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use crate::{
    CrmError, CrmSdkLibrary, CrmVoid, DebayerSetting, DecodeLevel, DevParamV13, Device, Downscale,
    FormatId, Gpu, Header, HeaderId, ProcessRef, ProcessType, Rect, ResParamV13, Resolution,
    Result, SampleSequence,
};

// Crate-internal gates for the private validators + the resolve/commit
// funnel (project rule: tests never inline with the implementation). The
// pair-constraint tables are hardware-free; the SDK sections skip with a
// reason when the library or a clip is absent.
#[cfg(test)]
#[path = "decode_context_tests.rs"]
mod tests;

/// The three `ResParamV13` scaling fields a caller picks a decode rung
/// with, plus the sample layout the SDK is asked to write.
///
/// This is an **input view**, and the only one the ergonomic path needs:
/// the resulting width / height / row pitch / buffer size are DERIVED
/// from the context's whole `ResParamV13` by the SDK's own
/// `CrmGetOutImageSize`, and read back with
/// [`DecodeContext::output_dimensions`] /
/// [`DecodeContext::bytes_per_line`] /
/// [`DecodeContext::required_output_size`].
///
/// It is deliberately **not** the complete list of raster-affecting
/// fields — `lens_squeeze` (§ 6.3.17: *"Use CrmGetOutImageSize to know
/// actual size of the output image"*) and `out_data_type` (a
/// `CrmDecompressed` output is the RAW sensor raster, 6024 × 3184 for a
/// 6000 × 3164 capture) move it too. Those reach the same resolve
/// through [`DecodeContext::edit_res_param`] /
/// [`DecodeContext::edit_res_param_in_place`] rather than being duplicated
/// here, which is why no field of `ResParamV13` needs a "do not touch
/// this one" convention.
///
/// `debayer_setting` is the odd one out: it does **not** move the raster
/// (`CrmGetOutImageSize` ignores it — `Half` reports 3000 × 1582 under
/// both settings), but the SDK refuses to *decode* a `HighQuality`
/// debayer at a reduced `decode_level`, so it is validated together with
/// this struct. See [`DecodeContext::set_geometry`].
///
/// `frame_rendering` is neither: § 6.3.14 has it as a **deprecated**
/// multi-GPU scheduling mode (alternate frames vs. split halves across
/// GPUs). It changes neither the raster nor the colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputGeometry {
    pub crop:              Rect,
    pub downscale:         Downscale,
    pub decode_level:      DecodeLevel,
    pub bits_per_sample:   u32,
    pub samples_per_pixel: u32,
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

// ─────────────────────────────────────────────────────────────────────
// The pair constraints `CrmGetOutImageSize` does NOT check
//
// `CrmGetOutImageSize` answers "how big would the output be", not "is
// this `ResParamV13` decodable". It happily reports a raster for pairs
// `CrmGetImage` then rejects with `InvalidParameter` on EVERY frame,
// measured against CrmSdk.dll 2.10 on a 6000 × 3164 clip:
//
//     X2 + Half            GetOutImageSize Ok(1500 x 790)   GetImage Err(InvalidParameter)
//     X4 + Quarter         GetOutImageSize Ok( 374 x 196)   GetImage Err(InvalidParameter)
//     HighQuality + Half   GetOutImageSize Ok(3000 x 1582)  GetImage Err(InvalidParameter)
//
// So the pre-check alone makes `set_geometry` "transactional" over a
// question that rejects almost nothing: it would commit the geometry,
// return the new size, and leave every later decode failing with no
// previous state to go back to. The two rules below are checked here,
// before anything is written.
//
// Sourcing: both are quoted verbatim from `CrmSdk.dll`'s own error
// strings (found in the binary; reachable through `CrmGetLastError`).
// The interface spec documents **neither** — § 6.3.10 and § 6.3.20
// describe `CrmDownscale` and `CrmDecodeLevel` as independent enums and
// state no cross-constraint — so these are attributed to the library,
// not to a §.
// ─────────────────────────────────────────────────────────────────────

/// `CrmSdk.dll` error string: *"When CrmResParam::debayerSetting is 1,
/// CrmResParam::decodeLevel must be kCrmDecodeLevel_Full."* (`1` is
/// `kCrmDebayerSettingHighQuality`.)
const ERR_HIGH_QUALITY_NEEDS_FULL: &str =
    "ResParamV13: DebayerSetting::HighQuality is legal only at DecodeLevel::Full — the SDK reports \
     \"When CrmResParam::debayerSetting is 1, CrmResParam::decodeLevel must be kCrmDecodeLevel_Full.\" \
     A reduced rung is a PAIR: it takes DebayerSetting::HighSpeed";

/// `CrmSdk.dll` error string: *"Either CrmResParam::downscale or
/// CrmResParam::decodeLevel must be non-scaling."*
const ERR_TWO_SCALERS: &str =
    "ResParamV13: at most one of Downscale / DecodeLevel may scale — they do NOT multiply, and the \
     SDK reports \"Either CrmResParam::downscale or CrmResParam::decodeLevel must be non-scaling.\" \
     Keep one of Downscale::X1 / DecodeLevel::Full";

/// A crop edge of 1 — see [`check_extent`].
const ERR_DEGENERATE_CROP: &str =
    "OutputGeometry: a crop edge of 1 has no output raster — the SDK floors every output edge to \
     an even number, and the only even value at or below 1 is 0. (A crop edge of 0 is a different \
     thing and is legal: it is the SDK's own \"whole frame\" spelling.)";

/// A sample layout whose row pitch works out to zero. [`check_extent`]
/// catches the one case that needs no width — `samples_per_pixel: 0` —
/// before any FFI; [`DecodeContext::resolve`] re-checks the computed pitch
/// afterwards for the width-dependent one (a `bits_per_sample` that packs
/// so many samples per 32-bit word that the row divides away to nothing).
const ERR_EMPTY_PITCH: &str =
    "OutputGeometry: the sample layout sizes the destination at 0 bytes — samples_per_pixel is 0, \
     or bits_per_sample packs enough samples per 32-bit word that the resolved width divides the \
     row pitch away entirely";

/// The backstop in [`DecodeContext::resolve`]: `CrmGetOutImageSize` can
/// report `(0, 0)` — and return `CRM_ERR_OK` — for a recipe whose scaling
/// leaves nothing, which is the one part of this that only it can know.
const ERR_EMPTY_RASTER: &str =
    "ResParamV13: the SDK resolves this crop/downscale/decode_level to an empty output raster \
     (CrmGetOutImageSize reports a zero edge and still returns CRM_ERR_OK)";

/// Neither pair constraint is a preference: both are hard SDK rules, and
/// a `ResParamV13` that breaks either one fails `CrmGetImage` for as long
/// as it stays installed.
fn check_res_param_pairs(p: &ResParamV13) -> Result<()> {
    if p.debayer_setting == DebayerSetting::HighQuality && p.decode_level != DecodeLevel::Full {
        return Err(CrmError::InvalidParameter(ERR_HIGH_QUALITY_NEEDS_FULL));
    }
    // "Non-scaling" is `Downscale::X1` / `DecodeLevel::Full`; every other
    // value of either enum scales, the named rasters (`Hd`/`Sd`/`Qfhd`/
    // `*Dci`) and the bayer-domain `Bayer2`/`Bayer4` included.
    if p.downscale != Downscale::X1 && p.decode_level != DecodeLevel::Full {
        return Err(CrmError::InvalidParameter(ERR_TWO_SCALERS));
    }
    Ok(())
}

/// The extent refusals we make **ourselves**, before any FFI call.
///
/// `CrmGetOutImageSize` is not a validator and nothing is routed through it
/// as though it were: it under-validates (it answers happily for the pairs
/// [`check_res_param_pairs`] catches) and it is not the place to *discover*
/// that an input is degenerate. What is decidable in Rust is decided here:
///
/// * **A crop edge of 1.** The SDK floors every output edge to an even
///   number — measured even where nothing scales (5999 × 3163 →
///   5998 × 3162) — and the only even value at or below 1 is 0. So an edge
///   of 1 has no raster whatever the rest of the recipe says, and that
///   needs no asking. A crop edge of **zero** is a different thing and
///   stays legal: it is the SDK's own "whole frame" spelling
///   (`TestCrmSdk.cpp` memsets `CrmResParamV13` to zero for its decompress
///   pass and gets the full sensor raster back).
/// * **A sample layout with no samples.** `samples_per_pixel: 0` makes the
///   row pitch 0 at any width, so [`DecodeContext::required_output_size`]
///   would be 0 — a size every caller reads as "allocate nothing", and one
///   `CrmDeviceAllocate` / `cudaMalloc` rejects outright.
///
/// What is genuinely the SDK's to answer is the raster itself, which it
/// returns correctly for every legal geometry, its even-flooring included.
/// [`DecodeContext::resolve`] keeps a zero-edge check on that answer for
/// what only it can know (a small crop at `Quarter`, say), but that is a
/// belt over these braces, not the guard.
fn check_extent(crop: &Rect, samples_per_pixel: u32) -> Result<()> {
    if crop.width == 1 || crop.height == 1 {
        return Err(CrmError::InvalidParameter(ERR_DEGENERATE_CROP));
    }
    if samples_per_pixel == 0 {
        return Err(CrmError::InvalidParameter(ERR_EMPTY_PITCH));
    }
    Ok(())
}

/// The `ResParamV13` `geometry` describes, against a live `base`: the
/// three scaling fields overwritten, and the debayer *derived* where a
/// reduced `decode_level` leaves it no choice.
///
/// `OutputGeometry` cannot name the debayer, and `HighQuality` at
/// `Half`/`Quarter` is not a slower alternative — it does not exist. So
/// a scaling rung takes the only legal value rather than handing the
/// caller a rejection for a field it was given no way to set. The
/// post-condition is the point: whatever `base` carried, the result
/// always satisfies [`check_res_param_pairs`]'s first rule.
///
/// Shared by [`DecodeContext::set_geometry`] and
/// [`DecodeContext::output_dimensions_for`] so the ladder query can never
/// describe a different struct than the commit would install.
fn probe_res_param(base: &ResParamV13, geometry: &OutputGeometry) -> ResParamV13 {
    let mut probe = *base;
    probe.crop = geometry.crop;
    probe.downscale = geometry.downscale;
    probe.decode_level = geometry.decode_level;
    if probe.decode_level != DecodeLevel::Full {
        probe.debayer_setting = DebayerSetting::HighSpeed;
    }
    probe
}

/// Everything one `CrmGetOutImageSize` answer determines. Produced by
/// [`DecodeContext::resolve`] into a local, so a caller can be told "no"
/// with nothing written yet.
#[derive(Clone, Copy, Debug)]
struct Resolved {
    dims:           (u32, u32),
    bytes_per_line: u32,
    out_size:       u32,
}

/// Bundled per-decoder facade.
///
/// Owns the SDK `ProcessRef`, the cached header pack covering the
/// clip-static UUID + CRAW entry + most-recent TimedMetadata, the
/// `DevParamV13`/`ResParamV13` parameter structs, and the resolved
/// output raster for the current `res_param`.
///
/// # Lifecycle
///
/// - [`new`](Self::new) constructs and resolves the output raster once.
/// - Per-frame: [`set_timed_meta`](Self::set_timed_meta) then
///   [`decode_frame_cpu_into`](Self::decode_frame_cpu_into) or
///   [`decode_frame_to_gpu`](Self::decode_frame_to_gpu). Neither
///   re-resolves.
/// - Colour-transform edits: [`dev_param_color_mut`](Self::dev_param_color_mut).
///   Cheap; no FFI hop.
/// - Rung changes: [`set_geometry`](Self::set_geometry).
/// - Anything else in `res_param` (`lens_squeeze`, `out_data_type`,
///   `out_buffer_type`, `debayer_setting`, `frame_rendering`):
///   [`edit_res_param`](Self::edit_res_param) or
///   [`edit_res_param_in_place`](Self::edit_res_param_in_place). Both
///   re-resolve; both are transactional.
pub struct DecodeContext {
    sdk:             Arc<CrmSdkLibrary>,
    process_ref:     ProcessRef,
    /// Lane the process_ref was created against. Kept for callers that
    /// need to know which memory domain `decode_frame_to_gpu`'s
    /// destination must live in; the output raster itself is
    /// device-independent (`CrmGetOutImageSize` takes no device).
    device:          Device,
    dev_param:       DevParamV13,
    /// **The** store for the raster inputs. `crop` / `downscale` /
    /// `decode_level` live here and nowhere else —
    /// [`geometry`](DecodeContext::geometry) reads them back out — so
    /// there is no second copy to keep in sync and no way for the two to
    /// drift apart. Every write goes through
    /// [`commit_res_param`](DecodeContext::commit_res_param).
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
    /// Sample layout the SDK is asked to write. Not part of
    /// `ResParamV13` — `CrmGetImage` takes both as separate arguments —
    /// so it is stored here and surfaced through
    /// [`geometry`](DecodeContext::geometry).
    bits_per_sample:   u32,
    samples_per_pixel: u32,
    /// Output raster the SDK reports for `res_param`
    /// (`CrmGetOutImageSize`). Not `crop / factor` — the SDK floors each
    /// edge to an even number after the division, so an odd quotient
    /// (3164 / 4 = 791) comes back one smaller (790), and it floors even
    /// at `X1`/`Full` (5999 × 3163 → 5998 × 3162).
    out_dims:        (u32, u32),
    /// Row pitch for `out_dims.0` at the active sample layout.
    bytes_per_line:  u32,
    /// `bytes_per_line × out_dims.1` — the destination size to allocate
    /// for this `res_param`.
    out_size:        u32,
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
    /// Construct + immediately resolve the output raster.
    ///
    /// `geometry`'s `crop` / `downscale` / `decode_level` overwrite
    /// whatever `res_param` carries, so a caller that fills both cannot
    /// make them disagree; from then on `res_param` is the single store
    /// and [`geometry`](Self::geometry) reads back out of it.
    ///
    /// Fails — releasing the process ref — if the pair constraints or the
    /// resolved raster reject the combination, rather than handing back a
    /// context whose every decode would fail. See
    /// [`set_geometry`](Self::set_geometry) for the rules.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sdk:                Arc<CrmSdkLibrary>,
        format_id:          FormatId,
        use_device:         Device,
        use_gpu:            Gpu,
        process_type:       ProcessType,
        max_res:            Resolution,
        dev_param:          DevParamV13,
        res_param:          ResParamV13,
        uuid_box:           Vec<u8>,
        craw_entry:         Vec<u8>,
        initial_timed_meta: &[u8],
        sample_sequence:    SampleSequence,
        geometry:           OutputGeometry,
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
            bits_per_sample: 0,
            samples_per_pixel: 0,
            out_dims: (0, 0),
            bytes_per_line: 0,
            out_size: 0,
        };
        let probe = probe_res_param(&me.res_param, &geometry);
        // On `Err`, `me` drops here and `Drop` releases `process_ref`.
        me.commit_res_param(probe, geometry.bits_per_sample, geometry.samples_per_pixel)?;
        Ok(me)
    }

    /// The scaling fields + sample layout the context is resolved for,
    /// read straight out of the live `ResParamV13`. Not a cache — there
    /// is nothing here that could go stale.
    pub fn geometry(&self) -> OutputGeometry {
        OutputGeometry {
            crop:              self.res_param.crop,
            downscale:         self.res_param.downscale,
            decode_level:      self.res_param.decode_level,
            bits_per_sample:   self.bits_per_sample,
            samples_per_pixel: self.samples_per_pixel,
        }
    }

    /// The format id passed at construction.
    pub fn format_id(&self) -> FormatId { self.format_id }

    /// The lane the `ProcessRef` was created against — the memory domain
    /// [`decode_frame_to_gpu`](Self::decode_frame_to_gpu)'s destination
    /// has to live in. The output *raster* is device-independent
    /// (`CrmGetOutImageSize` takes no device), so nothing in the resolve
    /// consults this.
    pub fn device(&self) -> Device { self.device }

    /// The active sample-sequence layout (output pixel order).
    pub fn sample_sequence(&self) -> SampleSequence { self.sample_sequence }

    /// The output raster the SDK produces for the active `res_param` —
    /// `CrmGetOutImageSize`'s answer, cached at the last resolve.
    pub fn output_dimensions(&self) -> (u32, u32) { self.out_dims }

    /// The output row pitch (`inBytesPerLine`) for the active raster.
    pub fn bytes_per_line(&self) -> u32 { self.bytes_per_line }

    /// Cheap accessor — the destination byte count to allocate for the
    /// active raster (`bytes_per_line × output height`).
    pub fn required_output_size(&self) -> u32 { self.out_size }

    /// The output raster this context WOULD produce for `geometry`,
    /// without applying it — the query behind a decode-scale ladder,
    /// which must report every advertised step before any of them is
    /// selected. Non-mutating: neither `res_param` nor the cached raster
    /// moves.
    ///
    /// Answers exactly what [`set_geometry`](Self::set_geometry) would
    /// commit, refusals included and in the same order: a rung that breaks
    /// a pair constraint ([`check_res_param_pairs`]), has a degenerate
    /// extent ([`check_extent`]) or resolves to an empty raster comes back
    /// `Err` here rather than as a step the UI advertises and the decoder
    /// then cannot serve.
    pub fn output_dimensions_for(&self, geometry: &OutputGeometry) -> Result<(u32, u32)> {
        let probe = probe_res_param(&self.res_param, geometry);
        Ok(self
            .resolve(&probe, geometry.bits_per_sample, geometry.samples_per_pixel)?
            .dims)
    }

    /// Update the decode rung: validate, re-resolve the output raster
    /// through the SDK, then commit both. Returns the new buffer size.
    ///
    /// **Transactional** — every fallible step runs against a local copy,
    /// so a geometry the rules or the SDK reject leaves the context
    /// exactly as it was rather than half-moved.
    ///
    /// # The debayer half of the rung
    ///
    /// A reduced `decode_level` is only decodable with
    /// [`DebayerSetting::HighSpeed`]; `HighQuality` at `Half`/`Quarter`
    /// does not exist, it is not a slower alternative. `OutputGeometry`
    /// does not carry the debayer, so when this commits a scaling
    /// `decode_level` it *derives* the only legal value and writes
    /// `HighSpeed` — the caller is never handed a rejection for a field
    /// it was given no way to name, and the context never holds a pair
    /// the SDK refuses. `res_param().debayer_setting` reports the result.
    /// Restoring `HighQuality` on the way back up to `Full` is a
    /// deliberate act, so it stays the caller's, through
    /// [`edit_res_param`](Self::edit_res_param).
    pub fn set_geometry(&mut self, geometry: OutputGeometry) -> Result<u32> {
        if geometry == self.geometry() {
            return Ok(self.out_size);
        }
        let probe = probe_res_param(&self.res_param, &geometry);
        self.commit_res_param(probe, geometry.bits_per_sample, geometry.samples_per_pixel)
    }

    /// Update the per-frame TimedMetadata. Rebuilds `header_pack` in
    /// place; the pre-warmed `Vec<u8>` does not grow after the first
    /// frame. **Does NOT invalidate the raster** — TimedMetadata carries
    /// colour-transform fields (WB, exposure, gamma) that do not affect
    /// output geometry.
    pub fn set_timed_meta(&mut self, timed_meta: &[u8]) -> Result<()> {
        Self::build_header_pack_into(
            &self.sdk, &self.uuid_box, &self.craw_entry, timed_meta, &mut self.header_pack,
        )
    }

    /// Decode one frame to a CPU destination buffer. The bits-per-sample
    /// / samples-per-pixel / bytes-per-line fields of the active
    /// [`geometry`](Self::geometry) drive the layout.
    pub fn decode_frame_cpu_into(&self, compressed: &[u8], dst: &mut [u8]) -> Result<u32> {
        self.sdk.get_image(
            self.process_ref,
            dst.as_mut_ptr() as *mut CrmVoid, dst.len() as u32,
            self.bits_per_sample,
            self.samples_per_pixel,
            self.bytes_per_line,
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
    /// **and** `res_param.out_buffer_type` must name that same lane
    /// (the SDK's `CrmGetImage` reads `out_buffer_type` to decide whether
    /// the destination pointer is a CPU pointer, a `CUdeviceptr`, a
    /// `cl_mem`, or an `id<MTLBuffer>`). Set it through
    /// [`edit_res_param`](Self::edit_res_param).
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
            self.bits_per_sample,
            self.samples_per_pixel,
            self.bytes_per_line,
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
            self.bits_per_sample,
            self.samples_per_pixel,
            self.bytes_per_line,
            self.sample_sequence,
            compressed,
            &self.dev_param,
            &self.res_param,
            &self.header_pack,
        )
    }

    /// Mutate colour-transform fields of `dev_param` (WB, exposure,
    /// gamma, tint, etc.). A plain `&mut` because `DevParamV13` holds
    /// nothing the output raster depends on — unlike `res_param`, which
    /// is why that one is not handed out this way.
    ///
    /// Caller MUST NOT rewrite the structural fields (`size`,
    /// `struct_id`, `format_id`): the SDK reads them to decide which
    /// struct version it was handed.
    pub fn dev_param_color_mut(&mut self) -> &mut DevParamV13 { &mut self.dev_param }

    /// Read-only access to `dev_param`.
    pub fn dev_param(&self) -> &DevParamV13 { &self.dev_param }

    /// Edit `res_param` and re-resolve — the **checked** form.
    ///
    /// `edit` runs against a copy; the result is validated and put to
    /// `CrmGetOutImageSize`, and only a combination that survives both
    /// becomes the live `res_param`. Returns the new
    /// [`required_output_size`](Self::required_output_size).
    ///
    /// Every field is fair game, `crop` / `downscale` / `decode_level`
    /// included — the raster is re-derived from whatever the closure
    /// leaves behind, so there is no field that has to be avoided here
    /// and no convention for a caller to obey.
    /// [`set_geometry`](Self::set_geometry) is just the ergonomic entry
    /// for the three scaling ones (and the one that derives the debayer
    /// half of a reduced rung).
    ///
    /// ```no_run
    /// # fn f(ctx: &mut canon_raw::DecodeContext) -> canon_raw::Result<()> {
    /// // The GPU lane names its destination domain; the raster is re-resolved
    /// // even though this particular field does not move it.
    /// ctx.edit_res_param(|p| p.out_buffer_type = canon_raw::BufferType::Cuda)?;
    /// # Ok(()) }
    /// ```
    pub fn edit_res_param<F>(&mut self, edit: F) -> Result<u32>
    where
        F: FnOnce(&mut ResParamV13),
    {
        let mut probe = self.res_param;
        edit(&mut probe);
        self.commit_res_param(probe, self.bits_per_sample, self.samples_per_pixel)
    }

    /// Edit `res_param` in place through a guard — the ergonomic form of
    /// [`edit_res_param`](Self::edit_res_param), for the common
    /// single-field write:
    ///
    /// ```no_run
    /// # fn f(ctx: &mut canon_raw::DecodeContext) {
    /// ctx.edit_res_param_in_place().debayer_setting = canon_raw::DebayerSetting::HighSpeed;
    /// # }
    /// ```
    ///
    /// The raster is re-resolved when the guard drops, so the cached
    /// size / pitch / dimensions can never fall behind the struct the SDK
    /// is handed. `Drop` cannot report, so an edit the pair rules or the
    /// SDK reject is **rolled back** and the context keeps decoding at
    /// what it had; reach for [`edit_res_param`](Self::edit_res_param)
    /// when you need to see that error.
    pub fn edit_res_param_in_place(&mut self) -> ResParamEdit<'_> {
        let before = self.res_param;
        ResParamEdit { ctx: self, before }
    }

    /// Read-only access to `res_param`.
    pub fn res_param(&self) -> &ResParamV13 { &self.res_param }

    /// The underlying SDK handle, useful for callers that need to invoke
    /// methods outside the bundled facade.
    pub fn sdk(&self) -> &Arc<CrmSdkLibrary> { &self.sdk }

    /// The active `ProcessRef`. Released automatically on `Drop`.
    pub fn process_ref(&self) -> ProcessRef { self.process_ref }

    /// Ask the SDK what `probe` produces, and reject what it cannot
    /// usefully produce. Touches nothing.
    ///
    /// `CrmGetOutImageSize` (§ 7.3.6) is the SDK's own derivation — it
    /// takes the whole `ResParamV13` plus the header pack and needs NO
    /// process ref and no frame bytes, which is what lets it run at
    /// construction and, through
    /// [`output_dimensions_for`](Self::output_dimensions_for), for rungs
    /// that have not been applied.
    ///
    /// It is not `crop / factor`. At most one of `downscale` /
    /// `decode_level` may scale; they do **not** multiply — ask for both
    /// and `CrmGetImage` fails the call outright rather than applying one
    /// after the other (see [`check_res_param_pairs`]), so exactly one
    /// scaler is ever in play. And the SDK floors each edge to an even
    /// number after dividing: 6000 × 3164 at `Quarter` is 1500 × 790, not
    /// 791, and the flooring applies even where nothing scales
    /// (5999 × 3163 → 5998 × 3162). It also owns the named modes
    /// (`Hd`/`Sd`/`Qfhd`/`*Dci`, fixed rasters), the `CrmDecompressed`
    /// output (the RAW sensor raster, *larger* than `crop`) and
    /// `lens_squeeze`. Re-deriving any of that here would be a second,
    /// wrong source of truth.
    ///
    /// # Who decides what
    ///
    /// The order of the three steps below is the design, not an accident.
    /// The SDK is asked **last**, about an input the two Rust checks have
    /// already accepted, because `CrmGetOutImageSize` is not a validator:
    /// it answers `CRM_ERR_OK` for the pairs `CrmGetImage` then refuses
    /// (see [`check_res_param_pairs`]) and it answers `CRM_ERR_OK` with
    /// `(0, 0)` for recipes that resolve to nothing. So
    /// [`check_res_param_pairs`] and [`check_extent`] settle everything
    /// derivable in Rust with no FFI at all, and the SDK is left the one
    /// question it alone can answer — the raster, which it gets right for
    /// every legal geometry, its even-flooring included.
    ///
    /// The zero-edge check on its answer stays as a **backstop**, for
    /// results the local rules cannot predict: a small-but-legal crop at
    /// `Quarter` can still floor away to nothing. Caching that would
    /// publish a `required_output_size()` of 0 that no caller can tell from
    /// a real answer — they allocate nothing, `CrmDeviceAllocate` /
    /// `cudaMalloc` reject a 0-byte request, and a CPU `Vec` sized 0 makes
    /// a decode that produced no pixels look like it worked. There is no
    /// legitimate zero-byte raster, so it is refused here, where the caller
    /// still has the geometry it had before.
    ///
    /// # Per-lane buffer sizing
    ///
    /// § 7.3.6 gives `CrmGetOutImageSize` a width and a height and
    /// nothing else — there is no SDK call that returns a pitch or a
    /// buffer size, so both are the caller's to compute, which is what
    /// `TestCrmSdk.cpp` does at :319-330 and what
    /// [`OutputGeometry::bytes_per_line`] mirrors.
    ///
    /// `bytes_per_line × out_height` is the destination size on **every**
    /// lane — it is what `TestCrmSdk.cpp` allocates through
    /// `CrmAllocate(…, useDevice)` for CUDA / OpenCL / Metal destinations
    /// just as for CPU ones (`dstBufferSize = outBytesPerLine *
    /// outHeight`, :2247-2254, the `testCrmGetImageWithProvidedQueue`
    /// path).
    ///
    /// What is **not** device-independent is the pitch *argument*: § 7.3.1
    /// says of `inBytesPerLine` *"If you designate a value except
    /// kCrmBuferType_CPU at outBufferType in inResParam, this argument is
    /// ignored."* So off the CPU lane it is not a layout you can choose —
    /// the SDK packs the rows itself and reports what it wrote through
    /// `outDstDataSize`, and our `bytes_per_line` survives only inside
    /// [`required_output_size`](Self::required_output_size), as the
    /// number the device allocation has to cover. Either way the size
    /// holds, which is why nothing here consults `self.device`.
    ///
    /// The legacy per-lane `CrmPreProcessForGetImageWithGpuMemory` sizing
    /// is gone because § 7.3.4 marks it **Deprecated** and binds it to the
    /// deprecated `CrmGetImageWithGpuMemory` flow (§ 7.3.5) — it demands
    /// `kCrmDeviceOpenCl` and has no bearing on `CrmGetImage`.
    fn resolve(&self, probe: &ResParamV13, bits_per_sample: u32, samples_per_pixel: u32) -> Result<Resolved> {
        // Everything decidable in Rust, FIRST — the SDK is only ever asked
        // about inputs we already believe in.
        check_res_param_pairs(probe)?;
        check_extent(&probe.crop, samples_per_pixel)?;
        let (w, h) = self.sdk.get_out_image_size(probe, &self.header_pack)?;
        if w == 0 || h == 0 {
            return Err(CrmError::InvalidParameter(ERR_EMPTY_RASTER));
        }
        let bytes_per_line = OutputGeometry::bytes_per_line(w, bits_per_sample, samples_per_pixel);
        let out_size = bytes_per_line.saturating_mul(h);
        if out_size == 0 {
            return Err(CrmError::InvalidParameter(ERR_EMPTY_PITCH));
        }
        Ok(Resolved { dims: (w, h), bytes_per_line, out_size })
    }

    /// The single write path for `res_param` + the cached raster.
    ///
    /// [`resolve`](Self::resolve) runs first and entirely against locals,
    /// so every `?` above the assignments leaves `self` untouched — that
    /// is what makes `new` / `set_geometry` / `edit_res_param` /
    /// [`ResParamEdit`] transactional without each restating the
    /// derivation (a second copy of it is exactly how the cached raster
    /// came to be able to drift from the struct in the first place).
    fn commit_res_param(
        &mut self,
        probe:             ResParamV13,
        bits_per_sample:   u32,
        samples_per_pixel: u32,
    ) -> Result<u32> {
        let resolved = self.resolve(&probe, bits_per_sample, samples_per_pixel)?;
        self.res_param = probe;
        self.bits_per_sample = bits_per_sample;
        self.samples_per_pixel = samples_per_pixel;
        self.out_dims = resolved.dims;
        self.bytes_per_line = resolved.bytes_per_line;
        self.out_size = resolved.out_size;
        Ok(self.out_size)
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

/// In-place `res_param` editor handed out by
/// [`DecodeContext::edit_res_param_in_place`].
///
/// Derefs to the context's live `ResParamV13`; on drop the context
/// re-resolves against whatever was written, so no edit can leave the
/// cached raster behind. An edit the pair rules or the SDK reject is
/// rolled back — `Drop` has nowhere to return an error to, and a
/// context left holding a `ResParamV13` that fails every `CrmGetImage`
/// is strictly worse than one that ignored the write. Use
/// [`DecodeContext::edit_res_param`] to see the error instead.
///
/// The re-resolve rides `Drop`, so — as with every RAII guard —
/// `mem::forget`ing one is the single way to skip it. Nothing in this
/// crate does.
pub struct ResParamEdit<'a> {
    ctx:    &'a mut DecodeContext,
    before: ResParamV13,
}

impl Deref for ResParamEdit<'_> {
    type Target = ResParamV13;
    #[inline]
    fn deref(&self) -> &ResParamV13 { &self.ctx.res_param }
}

impl DerefMut for ResParamEdit<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut ResParamV13 { &mut self.ctx.res_param }
}

impl Drop for ResParamEdit<'_> {
    fn drop(&mut self) {
        let edited = self.ctx.res_param;
        // Hand the commit a local and put the context back to its
        // pre-edit struct, so a rejected edit is a no-op rather than a
        // half-applied one: `commit_res_param` only writes on success.
        self.ctx.res_param = self.before;
        let (bits, samples) = (self.ctx.bits_per_sample, self.ctx.samples_per_pixel);
        let _ = self.ctx.commit_res_param(edited, bits, samples);
    }
}
