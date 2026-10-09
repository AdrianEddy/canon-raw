// SPDX-License-Identifier: MIT OR Apache-2.0

//! Which failure [`CrmContainer::load`] reports, and why it matters.
//!
//! Consumers escalate on this distinction: a load failure on a `crx `-branded
//! file becomes either a **"your file is damaged"** verdict that stops the open,
//! or a **decline** that lets the caller fall through to another decoder. So a
//! container that is merely a variant this parser does not decode MUST report
//! [`CrmContainerError::Unsupported`] and never `MissingBox` / `Parse`.
//! Otherwise an intact Canon clip whose video samples are not CRX (no `CRAW`
//! sample entry) would be declared damaged, and denied a decoder — such as the
//! Canon SDK — that could have played it.
//!
//! Synthetic containers, deliberately: each case isolates exactly one absent
//! structure, which no real clip does, and they need no sample footage.

use std::io::Cursor;

use crm_container::{CrmContainer, CrmContainerError};

// ─────────────────────────────────────────────────────────────────────────────
// A minimal ISOBMFF builder — just enough for `load` to walk
// ─────────────────────────────────────────────────────────────────────────────

/// `[u32 size][type][body]`.
fn bx(typ: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + body.len());
    v.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
    v.extend_from_slice(typ);
    v.extend_from_slice(body);
    v
}

fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

/// `hdlr` — only the handler type at body offset 8 is read.
fn hdlr(handler: &[u8; 4]) -> Vec<u8> {
    let mut body = vec![0u8; 8];
    body.extend_from_slice(handler);
    body.extend_from_slice(&[0u8; 12]); // reserved[3] + name terminator
    bx(b"hdlr", &body)
}

/// `mdhd` v0 — timescale at body offset 12.
fn mdhd(timescale: u32) -> Vec<u8> {
    let mut body = vec![0u8; 12]; // version/flags + creation + modification
    body.extend_from_slice(&timescale.to_be_bytes());
    body.extend_from_slice(&0u32.to_be_bytes()); // duration
    body.extend_from_slice(&[0u8; 4]); // language + quality
    bx(b"mdhd", &body)
}

/// `stts` with one run of `count` samples at `delta` ticks.
fn stts(count: u32, delta: u32) -> Vec<u8> {
    let mut body = vec![0u8; 4]; // version/flags
    body.extend_from_slice(&1u32.to_be_bytes()); // entry_count
    body.extend_from_slice(&count.to_be_bytes());
    body.extend_from_slice(&delta.to_be_bytes());
    bx(b"stts", &body)
}

/// `stsz` with a constant `size` for `count` samples.
fn stsz(size: u32, count: u32) -> Vec<u8> {
    let mut body = vec![0u8; 4];
    body.extend_from_slice(&size.to_be_bytes());
    body.extend_from_slice(&count.to_be_bytes());
    bx(b"stsz", &body)
}

/// `stsc` — one entry: chunk 1 onwards holds `per_chunk` samples.
fn stsc(per_chunk: u32) -> Vec<u8> {
    let mut body = vec![0u8; 4];
    body.extend_from_slice(&1u32.to_be_bytes()); // entry_count
    body.extend_from_slice(&1u32.to_be_bytes()); // first_chunk
    body.extend_from_slice(&per_chunk.to_be_bytes());
    body.extend_from_slice(&1u32.to_be_bytes()); // sample_description_index
    bx(b"stsc", &body)
}

/// `stco` with a single chunk at `offset`.
fn stco(offset: u32) -> Vec<u8> {
    let mut body = vec![0u8; 4];
    body.extend_from_slice(&1u32.to_be_bytes()); // entry_count
    body.extend_from_slice(&offset.to_be_bytes());
    bx(b"stco", &body)
}

/// A `CRAW` VisualSampleEntry. `with_cmp1` selects whether it carries a usable
/// `CMP1` geometry box (the 32-byte minimum the parser demands) or none at all.
fn craw_entry(with_cmp1: bool) -> Vec<u8> {
    // VisualSampleEntry prefix up to the display dims at +0x20 / +0x22.
    let mut body = vec![0u8; 0x18];
    body.extend_from_slice(&64u16.to_be_bytes()); // width  @ entry +0x20
    body.extend_from_slice(&32u16.to_be_bytes()); // height @ entry +0x22
    body.extend_from_slice(&[0u8; 0x2E]); // the rest of the VisualSampleEntry
    if with_cmp1 {
        // CMP1 payload: `parse_cmp1` needs `box_size >= 8 + 32`.
        let mut cmp1 = vec![0u8; 48];
        cmp1[4..6].copy_from_slice(&0x100u16.to_be_bytes()); // version
        cmp1[8..12].copy_from_slice(&64u32.to_be_bytes()); // f_width
        cmp1[12..16].copy_from_slice(&32u32.to_be_bytes()); // f_height
        cmp1[16..20].copy_from_slice(&64u32.to_be_bytes()); // tile_width
        cmp1[20..24].copy_from_slice(&32u32.to_be_bytes()); // tile_height
        cmp1[24] = 12; // n_bits
        cmp1[25] = 0x40; // n_planes = 4, cfa_layout = 0
        body.extend_from_slice(&bx(b"CMP1", &cmp1));
    }
    bx(b"CRAW", &body)
}

/// Which structural pieces the synthetic movie carries.
#[derive(Clone, Copy)]
struct Shape {
    handler: &'static [u8; 4],
    /// `None` = no sample entry at all; `Some(false)` = a `CRAW` without `CMP1`.
    craw: Option<bool>,
    samples: u32,
    with_stsz: bool,
}

impl Default for Shape {
    fn default() -> Self {
        Self { handler: b"vide", craw: Some(true), samples: 3, with_stsz: true }
    }
}

/// A whole `ftyp` + `moov` + `mdat` file for `shape`. `mdat` is real so a sample
/// read would succeed; `load` never touches it.
fn movie(shape: Shape) -> Vec<u8> {
    let sample_size = 4u32;
    let mut stbl_parts = vec![
        // `stsd` is never DECODED (it carries Canon-private entries); the parser
        // recovers `CRAW` by signature scan over the whole moov.
        bx(b"stsd", &shape.craw.map(craw_entry).unwrap_or_default()),
        stts(shape.samples.max(1), 1001),
        stsc(shape.samples.max(1)),
        stco(0), // patched below to the real mdat body offset
    ];
    if shape.with_stsz {
        stbl_parts.insert(1, stsz(sample_size, shape.samples));
    }
    let stbl = bx(b"stbl", &cat(&stbl_parts));
    let minf = bx(b"minf", &stbl);
    let mdia = bx(b"mdia", &cat(&[hdlr(shape.handler), mdhd(24_000), minf]));
    let trak = bx(b"trak", &mdia);
    let moov = bx(b"moov", &trak);

    let ftyp = {
        let mut b = Vec::new();
        b.extend_from_slice(b"crx ");
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(b"crx ");
        b.extend_from_slice(b"isom");
        bx(b"ftyp", &b)
    };
    let mdat = bx(b"mdat", &vec![0xAAu8; (sample_size * shape.samples.max(1)) as usize]);
    cat(&[ftyp, moov, mdat])
}

fn load_err(shape: Shape) -> CrmContainerError {
    CrmContainer::load(Cursor::new(movie(shape))).expect_err("this shape must not load")
}

// ─────────────────────────────────────────────────────────────────────────────

/// The builder itself is load-bearing: the DEFAULT shape must actually parse, or
/// every "absent piece X ⇒ error Y" case below would pass for the wrong reason.
#[test]
fn the_complete_synthetic_movie_loads() {
    let c = CrmContainer::load(Cursor::new(movie(Shape::default()))).expect("the complete shape parses");
    assert_eq!(c.frame_count(), 3);
    assert_eq!((c.cmp1().f_width, c.cmp1().f_height), (64, 32));
    assert_eq!(c.fps(), (24_000, 1001));
}

/// An otherwise complete movie whose video sample entry is not `CRAW` (its
/// samples are not CRX) is an UNSUPPORTED variant — the bytes are intact and
/// nothing about them is damaged.
#[test]
fn a_movie_without_a_craw_entry_is_unsupported_not_missing() {
    let e = load_err(Shape { craw: None, ..Shape::default() });
    assert!(
        matches!(e, CrmContainerError::Unsupported(_)),
        "no CRAW sample entry means 'not a variant I decode', not a missing box: {e:?}",
    );
    assert!(e.to_string().contains("CRAW"), "{e}");
}

/// Same for a `CRAW` entry carrying no `CMP1` geometry.
#[test]
fn a_craw_entry_without_cmp1_is_unsupported() {
    let e = load_err(Shape { craw: Some(false), ..Shape::default() });
    assert!(matches!(e, CrmContainerError::Unsupported(_)), "{e:?}");
    assert!(e.to_string().contains("CMP1"), "{e}");
}

/// A container with no video track at all (an audio-only / metadata-only `crx `
/// file) likewise declines rather than claiming damage.
#[test]
fn a_movie_without_a_video_track_is_unsupported() {
    let e = load_err(Shape { handler: b"soun", ..Shape::default() });
    assert!(matches!(e, CrmContainerError::Unsupported(_)), "{e:?}");
    assert!(e.to_string().contains("vide"), "{e}");
}

/// An empty video sample table has nothing to decode, but nothing is corrupt
/// either, so it must not be a `Parse` (which consumers read as damage).
#[test]
fn a_video_track_with_zero_samples_is_unsupported() {
    let e = load_err(Shape { samples: 0, ..Shape::default() });
    assert!(matches!(e, CrmContainerError::Unsupported(_)), "{e:?}");
    assert!(e.to_string().contains("zero samples"), "{e}");
}

/// The other side of the split, which must NOT regress: a STANDARD ISOBMFF box
/// every movie carries being absent is still `MissingBox`, so an identified
/// `.CRM` missing it is still reported damaged.
#[test]
fn an_absent_standard_sample_table_box_is_still_a_missing_box() {
    let e = load_err(Shape { with_stsz: false, ..Shape::default() });
    assert!(matches!(e, CrmContainerError::MissingBox("stsz (sample sizes)")), "{e:?}");
}

/// …and so is a file with no `moov` at all.
#[test]
fn a_file_without_moov_is_still_a_missing_box() {
    let ftyp_only = {
        let mut b = Vec::new();
        b.extend_from_slice(b"crx ");
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(b"crx ");
        bx(b"ftyp", &b)
    };
    let e = CrmContainer::load(Cursor::new(ftyp_only)).expect_err("no moov must fail");
    assert!(matches!(e, CrmContainerError::MissingBox("moov")), "{e:?}");
}

/// A file truncated inside `moov` is an I/O short read — the shape that is
/// unambiguously damage.
#[test]
fn a_truncated_moov_is_a_short_read() {
    let whole = movie(Shape::default());
    // Cut inside the moov body (past the `ftyp` + the moov header).
    let cut = whole.len() - 24;
    let e = CrmContainer::load(Cursor::new(whole[..cut].to_vec())).expect_err("a truncated moov must fail");
    match e {
        CrmContainerError::Io(io) => assert_eq!(io.kind(), std::io::ErrorKind::UnexpectedEof, "{io}"),
        other => panic!("expected a short read, got {other:?}"),
    }
}
