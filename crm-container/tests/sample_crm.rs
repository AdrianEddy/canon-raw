// SPDX-License-Identifier: MIT OR Apache-2.0

//! End-to-end checks against a real `.CRM` clip: sample table, `CMP1`
//! geometry, `CDI1`/`IAD1` crop, fps, the Canon `uuid` box and the LPCM audio
//! track.
//!
//! Point `CRM_TEST_CLIP` at any Canon Cinema RAW Light clip to run them; they
//! skip with a printed reason when it is unset. The structural checks hold for
//! any camera; clips from a sensor listed in [`KNOWN_SENSORS`] are additionally
//! checked against that sensor's exact active area.

use std::path::PathBuf;

use crm_container::CrmContainer;

/// `(left, top, width, height)` of an active area.
type Area = (u32, u32, u32, u32);

/// Coded read-out `(f_width, f_height)` → expected active area.
const KNOWN_SENSORS: &[((u32, u32), Area)] = &[
    // Canon EOS C80, 6K full frame.
    ((6024, 3184), (12, 10, 6000, 3164)),
];

fn open() -> Option<CrmContainer<std::fs::File>> {
    let Some(path) = std::env::var_os("CRM_TEST_CLIP").map(PathBuf::from) else {
        eprintln!("SKIP sample_crm: CRM_TEST_CLIP is not set");
        return None;
    };
    if !path.is_file() {
        eprintln!("SKIP sample_crm: {} absent", path.display());
        return None;
    }
    let f = std::fs::File::open(&path).expect("open CRM_TEST_CLIP");
    Some(CrmContainer::load(f).expect("parse CRM_TEST_CLIP"))
}

#[test]
fn parses_geometry_crop_and_fps() {
    let Some(c) = open() else { return };

    let cmp1 = c.cmp1();
    assert!(cmp1.f_width > 0 && cmp1.f_height > 0, "coded read-out {}x{}", cmp1.f_width, cmp1.f_height);
    assert_eq!(cmp1.n_planes, 4, "CFA RAW path (4-plane)");
    assert!(cmp1.is_cfa());

    // The active area is a sub-rectangle of the coded read-out.
    let crop = c.active_crop();
    assert!(crop.active_width > 0 && crop.active_height > 0);
    assert!(crop.left_margin + crop.active_width <= cmp1.f_width, "{crop:?} exceeds the read-out width");
    assert!(crop.top_margin + crop.active_height <= cmp1.f_height, "{crop:?} exceeds the read-out height");

    if let Some(&(_, expected)) = KNOWN_SENSORS.iter().find(|(coded, _)| *coded == (cmp1.f_width, cmp1.f_height)) {
        assert_eq!(
            (crop.left_margin, crop.top_margin, crop.active_width, crop.active_height),
            expected,
            "active area for a {}x{} read-out",
            cmp1.f_width,
            cmp1.f_height
        );
    }

    // fps numerator/denominator are both positive (timescale / sample_delta).
    let (num, den) = c.fps();
    assert!(num > 0 && den > 0, "fps = {num}/{den}");
}

#[test]
fn has_video_samples_and_reads_first_frame() {
    let Some(mut c) = open() else { return };
    let n = c.frame_count();
    assert!(n > 0, "video track has samples");

    // The first compressed frame reads with the size the sample table records,
    // without bulk-loading mdat.
    let want = c.compressed_frame_size(0).expect("frame-0 size") as usize;
    let frame = c.read_compressed_frame(0).expect("read frame 0");
    assert_eq!(frame.len(), want, "frame 0 size matches the sample table");
    assert!(!frame.is_empty());
}

#[test]
fn active_crop_symmetric_fallback_without_iad1() {
    let Some(c) = open() else { return };
    let entry = c.craw_entry().to_vec();
    let cmp1 = crm_container::parse_craw_cmp1(&entry).expect("CMP1");

    // Strip the IAD1 signature so the parser cannot find it → symmetric fallback.
    let mut no_iad1 = entry.clone();
    if let Some(p) = (0..no_iad1.len().saturating_sub(4)).find(|&i| &no_iad1[i..i + 4] == b"IAD1") {
        no_iad1[p..p + 4].copy_from_slice(b"XXXX");
    }
    let crop = crm_container::parse_craw_active_crop(&no_iad1, &cmp1);

    // The fallback centres the CRAW display rectangle in the read-out…
    assert_eq!(crop.left_margin, (cmp1.f_width - crop.active_width) / 2);
    assert_eq!(crop.top_margin, (cmp1.f_height - crop.active_height) / 2);
    // …and the display rectangle is exactly the extent `IAD1` is accepted for.
    let with_iad1 = c.active_crop();
    assert_eq!((crop.active_width, crop.active_height), (with_iad1.active_width, with_iad1.active_height));
}

#[test]
fn canon_uuid_payload_present() {
    let Some(c) = open() else { return };
    // Canon cameras write the metadata `uuid` box; the payload (post-UUID) is
    // the CMT1…CMT4 / CNCV tree.
    let payload = c.canon_uuid_payload();
    assert!(payload.is_some(), "Canon uuid metadata box present");
    // The full box (the Canon SDK's `kCrmHeaderUuid` header) includes the 8-byte
    // box header + 16-byte UUID and is strictly longer than the payload.
    let full = c.canon_uuid_box().expect("full uuid box");
    assert!(full.len() >= payload.unwrap().len() + 24);
}

#[test]
fn surfaces_lpcm_audio_track() {
    let Some(mut c) = open() else { return };

    let Some(fmt) = c.audio_format() else {
        eprintln!("SKIP surfaces_lpcm_audio_track: the clip has no audio track");
        return;
    };

    // Plausible LPCM parameters: real integer PCM fourcc, sane channel/rate/depth.
    assert!(
        matches!(&fmt.codec_fourcc, b"in24" | b"in32" | b"twos" | b"sowt" | b"ipcm" | b"lpcm" | b"fpcm"),
        "unexpected audio fourcc {:?}",
        String::from_utf8_lossy(&fmt.codec_fourcc),
    );
    assert!((1..=16).contains(&fmt.channels), "plausible channel count, got {}", fmt.channels);
    assert!(matches!(fmt.bits_per_sample, 8 | 16 | 24 | 32), "plausible bit depth, got {}", fmt.bits_per_sample);
    assert!(fmt.sample_rate >= 8000, "plausible sample rate, got {}", fmt.sample_rate);

    // The mdhd audio timescale equals the stsd sample rate.
    assert_eq!(c.audio_timescale(), fmt.sample_rate, "mdhd timescale == sample rate");

    // `in24` is big-endian 24-bit PCM.
    if &fmt.codec_fourcc == b"in24" {
        assert!(fmt.is_big_endian(), "in24 is big-endian");
        assert_eq!(fmt.bits_per_sample, 24);
    }

    // Audio samples are present and readable, seek-per-sample (mdat never bulk-loaded).
    let count = c.audio_sample_count();
    assert!(count > 0, "audio track has samples");

    let mut buf = Vec::new();
    let n = c.read_audio_sample_into(0, &mut buf).expect("read audio sample 0");
    assert_eq!(n, buf.len());
    assert!(n > 0, "first audio sample is non-empty");
}

#[test]
fn audio_range_read_matches_per_sample_concat() {
    let Some(mut c) = open() else { return };
    let count = c.audio_sample_count();
    if count < 60 {
        return; // need a range to exercise coalescing
    }

    // The coalescing range read must be BYTE-IDENTICAL to concatenating
    // individual per-sample reads over the same range.
    let (start, n) = (10usize, 50usize);
    let mut per_sample = Vec::new();
    let mut one = Vec::new();
    for i in start..start + n {
        c.read_audio_sample_into(i, &mut one).expect("read audio sample");
        per_sample.extend_from_slice(&one);
    }

    let mut ranged = Vec::new();
    let total = c.read_audio_samples_into(start, n, &mut ranged).expect("range read");
    assert_eq!(total, ranged.len());
    assert_eq!(ranged, per_sample, "coalesced range read != per-sample concatenation");

    // Degenerate + bounds behaviour.
    let mut empty = vec![0xAAu8; 4];
    assert_eq!(c.read_audio_samples_into(0, 0, &mut empty).expect("empty range"), 0);
    assert!(empty.is_empty(), "count=0 clears the buffer");
    assert!(c.read_audio_samples_into(count - 1, 2, &mut ranged).is_err(), "past-end range errors");
}
