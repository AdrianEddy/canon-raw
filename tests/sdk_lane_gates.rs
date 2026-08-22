// SPDX-License-Identifier: MIT OR Apache-2.0

//! Which [`CrmError`] shape [`CrmMovie::load`]'s **SDK-lane header gates**
//! report — and why exactly one of them may say "damaged".
//!
//! `oxivideo::decoder::crm::container_error` reads this crate's error enum as a
//! two-class verdict:
//!
//! | `CrmError`                        | `CrmFault`    | what the user is told                |
//! |-----------------------------------|---------------|--------------------------------------|
//! | `MissingData`                     | `Unsupported` | DECLINE → `Configuration`; the dispatcher falls through to the pure-Rust `crm-oxiraw` lane |
//! | `InvalidFormat` / `InvalidLength` | `Integrity`   | on a `crx `-branded carrier: **`Fatal` — "your clip is corrupt"**, and the open STOPS |
//!
//! [`CrmMovie::load`]'s gates are all **lane requirements** — the Canon `uuid`
//! box, a non-empty video index, and a timed-metadata track that is
//! index-parallel with the video track — that the shared container parser and
//! the oxiraw lane do without. None of them is evidence of damage, so every one
//! of them must land in the first row.
//!
//! The defect this file pins: the **count-mismatch** gate raised `InvalidFormat`.
//! A readable `.CRM` whose timed-metadata sample count was one off the video
//! frame count was therefore reported to the user as a CORRUPT FILE, and the
//! `Fatal` class also denied it the oxiraw lane that had no such requirement.
//!
//! Synthetic containers, deliberately: each case isolates exactly one deviation
//! (no real clip does), and they need neither `CrmSdk.dll` nor the multi-hundred-MB
//! dev corpus. `crm_container::CrmContainer` has the mirror-image suite in
//! `crm-container/tests/error_classification.rs`; this one covers the SDK glue's
//! own gates, which live only here.

use std::io::Cursor;

use canon_raw::crm_parser::CrmMovie;
use canon_raw::CrmError;
use crm_container::CANON_METADATA_BOX_UUID;

// ─────────────────────────────────────────────────────────────────────────────
// A minimal ISOBMFF builder — just enough for `CrmContainer::load` to walk.
// Deliberately a near-twin of `crm-container/tests/error_classification.rs`'s:
// the two crates share no test-support crate, and a gate that silently drifted
// onto a *different* synthetic movie would stop testing the same thing.
// ─────────────────────────────────────────────────────────────────────────────

/// `[u32 size][type][body]`.
fn bx(typ: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + body.len());
    v.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
    v.extend_from_slice(typ);
    v.extend_from_slice(body);
    v
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

/// A `CRAW` VisualSampleEntry carrying the 32-byte-minimum `CMP1` geometry box
/// the container parser demands before it will call this a CRM at all.
fn craw_entry() -> Vec<u8> {
    let mut body = vec![0u8; 0x18];
    body.extend_from_slice(&64u16.to_be_bytes()); // width  @ entry +0x20
    body.extend_from_slice(&32u16.to_be_bytes()); // height @ entry +0x22
    body.extend_from_slice(&[0u8; 0x2E]); // the rest of the VisualSampleEntry
    let mut cmp1 = vec![0u8; 48];
    cmp1[4..6].copy_from_slice(&0x100u16.to_be_bytes()); // version
    cmp1[8..12].copy_from_slice(&64u32.to_be_bytes()); // f_width
    cmp1[12..16].copy_from_slice(&32u32.to_be_bytes()); // f_height
    cmp1[16..20].copy_from_slice(&64u32.to_be_bytes()); // tile_width
    cmp1[20..24].copy_from_slice(&32u32.to_be_bytes()); // tile_height
    cmp1[24] = 12; // n_bits
    cmp1[25] = 0x40; // n_planes = 4, cfa_layout = 0
    body.extend_from_slice(&bx(b"CMP1", &cmp1));
    bx(b"CRAW", &body)
}

/// The Canon `uuid` metadata box (`kCrmHeaderUuid`) — a direct child of `moov`.
/// The payload past the 16-byte UUID is never decoded by the container parser
/// (the SDK is handed the box verbatim), so a stub is enough.
fn canon_uuid_box() -> Vec<u8> {
    let mut body = CANON_METADATA_BOX_UUID.to_vec();
    body.extend_from_slice(&bx(b"CNCV", b"synthetic"));
    bx(b"uuid", &body)
}

/// One `trak` with `handler`, `samples` samples of `sample_size` bytes each,
/// and an optionally CORRUPT `stsz` (a 4-byte body `mp4-atom` cannot decode —
/// the one way to reach `CrmContainerError::Parse`, i.e. genuine damage).
fn trak(handler: &[u8; 4], sample_entry: Vec<u8>, samples: u32, sample_size: u32, corrupt_stsz: bool) -> Vec<u8> {
    let stsz_box = if corrupt_stsz { bx(b"stsz", &[0u8; 4]) } else { stsz(sample_size, samples) };
    let stbl = bx(
        b"stbl",
        &[
            // `stsd` is never DECODED (it carries Canon-private entries); the
            // parser recovers `CRAW` by signature scan over the whole `moov`.
            bx(b"stsd", &sample_entry),
            stsz_box,
            stts(samples.max(1), 1001),
            stsc(samples.max(1)),
            stco(0),
        ]
        .concat(),
    );
    let mdia = bx(b"mdia", &[hdlr(handler), mdhd(24_000), bx(b"minf", &stbl)].concat());
    bx(b"trak", &mdia)
}

/// Which pieces the synthetic SDK-lane movie carries.
#[derive(Clone, Copy)]
struct Shape {
    video_samples: u32,
    /// `None` = no timed-metadata track at all.
    meta_samples: Option<u32>,
    with_canon_uuid: bool,
    /// Corrupt the VIDEO track's `stsz` so the shared container parser raises
    /// `Parse` — the damage control.
    corrupt_video_stsz: bool,
}

impl Default for Shape {
    fn default() -> Self {
        Self { video_samples: 3, meta_samples: Some(3), with_canon_uuid: true, corrupt_video_stsz: false }
    }
}

/// A whole `ftyp` + `moov` + `mdat` file for `shape`. `load` never reads a
/// sample, so the chunk offsets need not point anywhere real.
fn movie(shape: Shape) -> Vec<u8> {
    let sample_size = 4u32;
    let mut moov_children =
        vec![trak(b"vide", craw_entry(), shape.video_samples, sample_size, shape.corrupt_video_stsz)];
    if let Some(n) = shape.meta_samples {
        moov_children.push(trak(b"meta", Vec::new(), n, sample_size, false));
    }
    if shape.with_canon_uuid {
        moov_children.push(canon_uuid_box());
    }
    let moov = bx(b"moov", &moov_children.concat());

    let ftyp = {
        let mut b = Vec::new();
        b.extend_from_slice(b"crx ");
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(b"crx ");
        b.extend_from_slice(b"isom");
        bx(b"ftyp", &b)
    };
    let mdat = bx(b"mdat", &vec![0xAAu8; (sample_size * shape.video_samples.max(1)) as usize]);
    [ftyp, moov, mdat].concat()
}

fn load_err(shape: Shape) -> CrmError {
    CrmMovie::load(Cursor::new(movie(shape))).err().expect("this shape must not load")
}

/// The assertion every decline case makes: a `MissingData`, and specifically
/// NOT one of the two shapes `container_error` escalates to `Fatal`.
#[track_caller]
fn assert_declines(e: &CrmError, what: &str) {
    assert!(
        matches!(e, CrmError::MissingData(_)),
        "{what} must DECLINE (CrmError::MissingData → CrmFault::Unsupported → Configuration), got {e:?}"
    );
    assert!(
        !matches!(e, CrmError::InvalidFormat(_) | CrmError::InvalidLength { .. }),
        "{what} must never travel on a damage shape — those become a Fatal \"your clip is corrupt\" \
         verdict on a `crx `-branded carrier and stop the dispatcher falling through to crm-oxiraw"
    );
}

// ─────────────────────────────────────────────────────────────────────────────

/// The builder is load-bearing: the DEFAULT shape must actually load, or every
/// "deviation X ⇒ error Y" case below would pass for the wrong reason.
#[test]
fn the_complete_synthetic_sdk_lane_movie_loads() {
    let m = CrmMovie::load(Cursor::new(movie(Shape::default()))).expect("the complete SDK-lane shape parses");
    assert_eq!(m.frame_count(), 3, "video samples");
    assert_eq!(m.timed_meta_count(), 3, "timed-metadata samples");
    assert!(m.canon_metadata_box().is_some(), "Canon uuid box present");
    assert_eq!(&m.craw_entry()[4..8], b"CRAW", "craw_entry is the full CRAW box");
}

/// **The regression.** A timed-metadata track that is not index-parallel with
/// the video track breaks an SDK-LANE requirement — `timed_meta[frame_no]` per
/// video frame — that the oxiraw lane does not have. Declining lets the
/// dispatcher play the clip; reporting damage (what `InvalidFormat` did here)
/// both libelled an intact file and terminated the open.
///
/// Both directions, because the SDK lane needs the tracks index-parallel, not
/// merely long enough: a metadata track with SPARE samples is no more usable
/// per-frame than a short one.
#[test]
fn a_timed_metadata_count_mismatch_declines_rather_than_reporting_damage() {
    for meta in [2u32, 4] {
        let e = load_err(Shape { video_samples: 3, meta_samples: Some(meta), ..Shape::default() });
        assert_declines(&e, &format!("a {meta}-sample timed-metadata track against 3 video frames"));
    }
}

/// The sibling gates were already declines; they are pinned here so the whole
/// gate block is covered by one contract rather than one of four being right by
/// accident.
#[test]
fn the_other_sdk_lane_header_gates_decline_too() {
    assert_declines(
        &load_err(Shape { with_canon_uuid: false, ..Shape::default() }),
        "a container with no Canon `uuid` metadata box",
    );
    assert_declines(
        &load_err(Shape { meta_samples: None, ..Shape::default() }),
        "a container with no timed-metadata track",
    );
    assert_declines(
        &load_err(Shape { video_samples: 0, meta_samples: Some(0), ..Shape::default() }),
        "a container with an empty video index",
    );
}

/// The control that keeps the fix honest: moving the count-mismatch gate off
/// `InvalidFormat` must NOT have turned genuine container damage into a silent
/// decline. A sample table `mp4-atom` cannot decode is `CrmContainerError::Parse`
/// — the ONE shape `map_err` still routes to `InvalidFormat`, and hence the one
/// that still reaches `CrmFault::Integrity`.
#[test]
fn a_corrupt_sample_table_is_still_reported_as_damage() {
    let e = load_err(Shape { corrupt_video_stsz: true, ..Shape::default() });
    assert!(
        matches!(e, CrmError::InvalidFormat(_)),
        "a `stsz` that does not decode is DAMAGE (CrmError::InvalidFormat → CrmFault::Integrity), got {e:?}"
    );
}
