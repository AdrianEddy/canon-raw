// SPDX-License-Identifier: MIT OR Apache-2.0

//! `CrmMovie` parses a real `.CRM` and yields the SDK headers (`craw_entry` /
//! `canon_metadata_box`) plus per-frame samples WITHOUT the Canon SDK. The SDK
//! *decode* path additionally needs `CrmSdk`; this test covers only the
//! SDK-free container layer.
//!
//! Point `CRM_TEST_CLIP` at any Canon Cinema RAW Light clip to run it; it skips
//! with a printed reason when that is unset.

use canon_raw::crm_parser::CrmMovie;

#[test]
fn crm_movie_parses_a_real_clip() {
    let Some(clip) = std::env::var_os("CRM_TEST_CLIP").map(std::path::PathBuf::from) else {
        eprintln!("SKIP crm_container_parse: CRM_TEST_CLIP is not set");
        return;
    };
    if !clip.is_file() {
        eprintln!("SKIP crm_container_parse: {} absent", clip.display());
        return;
    }
    let f = std::fs::File::open(&clip).expect("open CRM_TEST_CLIP");
    let mut movie = CrmMovie::load(f).expect("CrmMovie::load");

    assert!(movie.frame_count() > 0, "video frames present");

    // The SDK headers the decode path feeds to `CrmGetImage`.
    let craw = movie.craw_entry();
    assert!(craw.len() > 8 && &craw[4..8] == b"CRAW", "craw_entry is the full CRAW box");
    let uuid = movie.canon_metadata_box().expect("Canon uuid box (kCrmHeaderUuid)");
    assert!(uuid.len() > 24 && &uuid[4..8] == b"uuid", "canon_metadata_box is the full uuid box");

    // First compressed frame reads at the recorded size (seek-per-frame; mdat
    // never bulk-loaded).
    let want = movie.compressed_frame_size(0).expect("frame-0 size") as usize;
    let frame = movie.read_compressed_frame(0).expect("read frame 0");
    assert_eq!(frame.len(), want, "frame 0 size matches the sample table");
}
