// SPDX-License-Identifier: MIT OR Apache-2.0

//! Sizes a damaged or hostile `.CRM` states must never become allocations the
//! file can't back, nor reads past its end. A failed allocation aborts a Rust
//! process (it doesn't unwind), so a decoder embedded in an application can't
//! catch it: every case here must come back as an `Err` — quickly, and without
//! touching more memory than the file is long.
//!
//! Synthetic files throughout, plus one truncated copy of a real clip (skipped
//! when no clip is at hand).

use std::io::{Cursor, ErrorKind};

use crm_container::{CrmContainer, CrmContainerError};

// ─────────────────────────────────────────────────────────────────────────────
// A minimal ISOBMFF builder (as in `error_classification.rs`)
// ─────────────────────────────────────────────────────────────────────────────

fn bx(typ: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + body.len());
    v.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
    v.extend_from_slice(typ);
    v.extend_from_slice(body);
    v
}

fn ftyp() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"crx ");
    b.extend_from_slice(&1u32.to_be_bytes());
    b.extend_from_slice(b"crx ");
    b.extend_from_slice(b"isom");
    bx(b"ftyp", &b)
}

fn hdlr(handler: &[u8; 4]) -> Vec<u8> {
    let mut body = vec![0u8; 8];
    body.extend_from_slice(handler);
    body.extend_from_slice(&[0u8; 12]);
    bx(b"hdlr", &body)
}

fn mdhd(timescale: u32) -> Vec<u8> {
    let mut body = vec![0u8; 12];
    body.extend_from_slice(&timescale.to_be_bytes());
    body.extend_from_slice(&0u32.to_be_bytes());
    body.extend_from_slice(&[0u8; 4]);
    bx(b"mdhd", &body)
}

fn stts(count: u32, delta: u32) -> Vec<u8> {
    let mut body = vec![0u8; 4];
    body.extend_from_slice(&1u32.to_be_bytes());
    body.extend_from_slice(&count.to_be_bytes());
    body.extend_from_slice(&delta.to_be_bytes());
    bx(b"stts", &body)
}

/// `stsz` with one `size` for `count` samples (the identical-size form).
fn stsz_identical(size: u32, count: u32) -> Vec<u8> {
    let mut body = vec![0u8; 4];
    body.extend_from_slice(&size.to_be_bytes());
    body.extend_from_slice(&count.to_be_bytes());
    bx(b"stsz", &body)
}

/// `stsz` listing each sample's size.
fn stsz_sizes(sizes: &[u32]) -> Vec<u8> {
    let mut body = vec![0u8; 4];
    body.extend_from_slice(&0u32.to_be_bytes());
    body.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
    for s in sizes {
        body.extend_from_slice(&s.to_be_bytes());
    }
    bx(b"stsz", &body)
}

fn stsc(per_chunk: u32) -> Vec<u8> {
    let mut body = vec![0u8; 4];
    body.extend_from_slice(&1u32.to_be_bytes());
    body.extend_from_slice(&1u32.to_be_bytes());
    body.extend_from_slice(&per_chunk.to_be_bytes());
    body.extend_from_slice(&1u32.to_be_bytes());
    bx(b"stsc", &body)
}

fn stco(offsets: &[u32]) -> Vec<u8> {
    let mut body = vec![0u8; 4];
    body.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
    for o in offsets {
        body.extend_from_slice(&o.to_be_bytes());
    }
    bx(b"stco", &body)
}

fn craw_entry() -> Vec<u8> {
    let mut body = vec![0u8; 0x18];
    body.extend_from_slice(&64u16.to_be_bytes());
    body.extend_from_slice(&32u16.to_be_bytes());
    body.extend_from_slice(&[0u8; 0x2E]);
    let mut cmp1 = vec![0u8; 48];
    cmp1[4..6].copy_from_slice(&0x100u16.to_be_bytes());
    cmp1[8..12].copy_from_slice(&64u32.to_be_bytes());
    cmp1[12..16].copy_from_slice(&32u32.to_be_bytes());
    cmp1[16..20].copy_from_slice(&64u32.to_be_bytes());
    cmp1[20..24].copy_from_slice(&32u32.to_be_bytes());
    cmp1[24] = 12;
    cmp1[25] = 0x40;
    body.extend_from_slice(&bx(b"CMP1", &cmp1));
    bx(b"CRAW", &body)
}

/// A one-track movie: `ftyp` + `moov` (video track with the given `stsz`, a
/// `stsc` of `per_chunk`, chunks at `chunks`) + an `mdat` of `mdat_len` bytes.
/// Chunk offsets are absolute; `mdat_body_offset` tells a caller where the
/// `mdat` payload starts so it can aim chunks into (or past) it.
fn movie(stsz: Vec<u8>, per_chunk: u32, chunks: &[u32], mdat_len: usize) -> Vec<u8> {
    let stbl = bx(b"stbl", &[bx(b"stsd", &craw_entry()), stts(1, 1001), stsz, stsc(per_chunk), stco(chunks)].concat());
    let mdia = bx(b"mdia", &[hdlr(b"vide"), mdhd(24_000), bx(b"minf", &stbl)].concat());
    let moov = bx(b"moov", &bx(b"trak", &mdia));
    [ftyp(), moov, bx(b"mdat", &vec![0xAAu8; mdat_len])].concat()
}

/// Where `movie`'s `mdat` payload starts, for a given `stsz` box and chunk count.
fn mdat_body_offset(stsz_len: usize, chunk_count: usize) -> u32 {
    let probe = movie(vec![0u8; stsz_len], 1, &vec![0; chunk_count], 0);
    probe.len() as u32 // the empty mdat's header is the last 8 bytes; its body starts at the end
}

fn assert_short_read(e: CrmContainerError) {
    match e {
        CrmContainerError::Io(io) => assert_eq!(io.kind(), ErrorKind::UnexpectedEof, "{io}"),
        other => panic!("expected an UnexpectedEof I/O error, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// A `moov` larger than the file
// ─────────────────────────────────────────────────────────────────────────────

/// The reviewers' `bigmoov.crm`: 104 bytes, a `moov` with a 64-bit size of 2^44.
/// Before the fix this tried to allocate 16 TiB and aborted the process.
#[test]
fn a_moov_with_a_2_pow_44_extended_size_is_a_short_read() {
    let mut f = ftyp();
    f.extend_from_slice(&1u32.to_be_bytes());
    f.extend_from_slice(b"moov");
    f.extend_from_slice(&(1u64 << 44).to_be_bytes());
    f.resize(104, 0);
    assert_eq!(f.len(), 104);
    assert_short_read(CrmContainer::load(Cursor::new(f)).expect_err("must not load"));
}

#[test]
fn a_moov_with_a_2_pow_50_extended_size_is_a_short_read() {
    let mut f = ftyp();
    f.extend_from_slice(&1u32.to_be_bytes());
    f.extend_from_slice(b"moov");
    f.extend_from_slice(&(1u64 << 50).to_be_bytes());
    f.extend_from_slice(&[0u8; 64]);
    assert_short_read(CrmContainer::load(Cursor::new(f)).expect_err("must not load"));
}

/// A 32-bit size of almost 4 GiB in a file of a few hundred bytes.
#[test]
fn a_moov_with_a_4_gib_size_in_a_tiny_file_is_a_short_read() {
    let mut f = ftyp();
    f.extend_from_slice(&0xFFFF_FFF0u32.to_be_bytes());
    f.extend_from_slice(b"moov");
    f.extend_from_slice(&[0u8; 256]);
    assert_short_read(CrmContainer::load(Cursor::new(f)).expect_err("must not load"));
}

/// A `moov` whose 64-bit size field the file ends inside of: cut inside `moov`.
#[test]
fn a_cut_off_extended_moov_size_is_a_short_read() {
    let mut f = ftyp();
    f.extend_from_slice(&1u32.to_be_bytes());
    f.extend_from_slice(b"moov");
    f.extend_from_slice(&[0u8; 3]);
    assert_short_read(CrmContainer::load(Cursor::new(f)).expect_err("must not load"));
}

/// A box before `moov` whose size runs past the end hides everything after it.
#[test]
fn a_box_running_past_the_end_ends_the_top_level_scan() {
    let mut f = ftyp();
    f.extend_from_slice(&0x7FFF_FFFFu32.to_be_bytes());
    f.extend_from_slice(b"free");
    f.extend_from_slice(&movie(stsz_identical(4, 1), 1, &[0], 4));
    let e = CrmContainer::load(Cursor::new(f)).expect_err("must not load");
    assert!(matches!(e, CrmContainerError::MissingBox("moov")), "{e:?}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Sample tables larger than the file
// ─────────────────────────────────────────────────────────────────────────────

/// An identical-size `stsz` of `u32::MAX` samples: 64 GiB of table before the
/// fix (16 bytes per entry), in a file of a few hundred bytes.
#[test]
fn an_identical_size_stsz_of_u32_max_samples_is_refused() {
    let f = movie(stsz_identical(4, u32::MAX), u32::MAX, &[0], 16);
    let e = CrmContainer::load(Cursor::new(f)).expect_err("must not load");
    assert!(matches!(e, CrmContainerError::Parse(_)), "{e:?}");
}

/// A per-sample `stsz` whose count is larger than its entries: `mp4-atom` reads
/// each entry from the box, so this ends in a decode error, not an allocation.
#[test]
fn a_per_sample_stsz_count_larger_than_its_box_is_refused() {
    let mut stsz = stsz_sizes(&[4, 4]);
    // The count field sits after the box header (8), version/flags (4) and the
    // zero identical-size (4).
    stsz[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
    let f = movie(stsz, 1, &[0, 4], 16);
    let e = CrmContainer::load(Cursor::new(f)).expect_err("must not load");
    assert!(matches!(e, CrmContainerError::Parse(_)), "{e:?}");
}

/// A `stsc` saying every chunk holds `u32::MAX` samples, with a sane sample count:
/// the walk stops at the count.
#[test]
fn a_huge_samples_per_chunk_stops_at_the_sample_count() {
    let f = movie(stsz_identical(4, 3), u32::MAX, &[0], 16);
    let c = CrmContainer::load(Cursor::new(f)).expect("loads");
    assert_eq!(c.frame_count(), 3);
}

/// Many `stsc` runs over many chunks: the walk is linear, not chunks × runs.
#[test]
fn the_stsc_walk_is_linear_in_chunks_and_runs() {
    // 200k chunks of one sample each and one run: before, each chunk rescanned
    // every run; this only checks that the result is right at that scale.
    // The mdat is large enough for the table's 16 bytes per sample (the bound a
    // track's sample count must stay under).
    let n = 200_000u32;
    let chunks: Vec<u32> = (0..n).map(|i| i * 4).collect();
    let f = movie(stsz_identical(4, n), 1, &chunks, (n * 16) as usize);
    let c = CrmContainer::load(Cursor::new(f)).expect("loads");
    assert_eq!(c.frame_count(), u64::from(n));
    assert_eq!(c.video_samples()[n as usize - 1].offset, u64::from((n - 1) * 4));
}

// ─────────────────────────────────────────────────────────────────────────────
// Samples past the end of the file
// ─────────────────────────────────────────────────────────────────────────────

/// One 4 GiB sample in a small file: reading it must fail before 4 GiB is
/// allocated, and it isn't a readable frame.
#[test]
fn a_4_gib_sample_is_a_short_read_without_allocating() {
    let probe = movie(stsz_sizes(&[16, 0xFFFF_FFFF]), 1, &[0, 0], 0);
    let base = probe.len() as u32; // the mdat body starts here (empty mdat in the probe)
    let f = movie(stsz_sizes(&[16, 0xFFFF_FFFF]), 1, &[base, base + 16], 64);
    let mut c = CrmContainer::load(Cursor::new(f)).expect("loads: only the table is read at load");
    assert_eq!(c.frame_count(), 2);
    assert_eq!(c.readable_frame_count(), 1);
    assert_eq!(c.read_compressed_frame(0).expect("frame 0 is inside the file").len(), 16);
    assert_short_read(c.read_compressed_frame(1).expect_err("frame 1 runs past the end"));
    let mut dst = vec![0u8; 16];
    let e = c.read_compressed_frame_into(1, &mut dst).expect_err("too small and past the end");
    assert!(matches!(e, CrmContainerError::BufferTooSmall { .. }), "{e:?}");
}

/// A recording cut short: the table describes 5 frames, the file holds 3.
#[test]
fn readable_frame_count_stops_at_the_first_frame_past_the_end() {
    let base = mdat_body_offset(stsz_identical(10, 5).len(), 5);
    let chunks: Vec<u32> = (0..5).map(|i| base + i * 10).collect();
    let mut whole = movie(stsz_identical(10, 5), 1, &chunks, 50);
    assert_eq!(CrmContainer::load(Cursor::new(whole.clone())).unwrap().readable_frame_count(), 5);
    whole.truncate(base as usize + 35); // frames 0..3 whole, frame 3 cut, frame 4 gone
    let mut c = CrmContainer::load(Cursor::new(whole)).expect("an intact moov loads");
    assert_eq!(c.frame_count(), 5);
    assert_eq!(c.readable_frame_count(), 3);
    c.read_compressed_frame(2).expect("frame 2 is whole");
    assert_short_read(c.read_compressed_frame(3).expect_err("frame 3 is cut"));
}

/// A chunk aimed far past the end of the file: its sample is never readable.
#[test]
fn a_sample_far_past_the_end_is_unreadable() {
    let f = movie(stsz_identical(8, 2), 1, &[0, u32::MAX], 16);
    let mut c = CrmContainer::load(Cursor::new(f)).expect("loads");
    assert_eq!(c.readable_frame_count(), 1);
    assert_short_read(c.read_compressed_frame(1).expect_err("past the end"));
}

// ─────────────────────────────────────────────────────────────────────────────
// A real clip, cut short
// ─────────────────────────────────────────────────────────────────────────────

/// The first 200 MB of a real clip (its `moov` is at the front): it loads, says
/// how many frames it holds, reads the last of them, and refuses the next one with
/// a short read. Source: `CRM_TEST_CLIP`, else the C80 sample clip on this
/// machine; skipped when neither exists. The copy goes to the temp directory,
/// never beside the clip.
#[test]
fn a_truncated_real_clip_reports_its_readable_frames() {
    let src = std::env::var_os("CRM_TEST_CLIP")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| r"F:\clips\canon\A_0002C015X250527_153335DT_CANON.CRM".into());
    let Ok(mut input) = std::fs::File::open(&src) else {
        eprintln!("SKIP a_truncated_real_clip_reports_its_readable_frames: {} not found", src.display());
        return;
    };
    let full = CrmContainer::load(std::fs::File::open(&src).unwrap()).expect("the whole clip loads");
    let total = full.frame_count();
    assert_eq!(full.readable_frame_count(), total, "an intact clip holds all its frames");

    let cut = 200u64 << 20;
    let len = input.metadata().unwrap().len();
    if len <= cut {
        eprintln!("SKIP: {} is not longer than 200 MB", src.display());
        return;
    }
    let tmp = std::env::temp_dir().join(format!("crm-container-truncated-{}.crm", std::process::id()));
    {
        use std::io::{Read, Write};
        let mut out = std::fs::File::create(&tmp).unwrap();
        let mut left = cut;
        let mut buf = vec![0u8; 8 << 20];
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            let n = input.read(&mut buf[..want]).unwrap();
            assert!(n > 0);
            out.write_all(&buf[..n]).unwrap();
            left -= n as u64;
        }
    }
    let result = std::panic::catch_unwind(|| {
        let mut c = CrmContainer::load(std::fs::File::open(&tmp).unwrap()).expect("a clip cut after its moov loads");
        assert_eq!(c.frame_count(), total, "the moov still describes every frame");
        let readable = c.readable_frame_count();
        assert!(readable > 0 && readable < total, "readable {readable} of {total}");
        let last = c.video_samples()[readable as usize - 1];
        assert!(last.offset + u64::from(last.size) <= cut);
        c.read_compressed_frame(readable - 1).expect("the last readable frame reads");
        c.read_timed_meta(readable - 1).expect("and its metadata");
        let e = c
            .read_compressed_frame(readable)
            .err()
            .or_else(|| c.read_timed_meta(readable).err())
            .expect("the first unreadable frame fails");
        assert_short_read(e);
        eprintln!("truncated clip: {readable} of {total} frames readable");
    });
    let _ = std::fs::remove_file(&tmp);
    if let Err(p) = result {
        std::panic::resume_unwind(p);
    }
}
