// SPDX-License-Identifier: MIT OR Apache-2.0

//! Stage-9 §9.9 cross-repo consolidation — the SDK lane's `CrmMovie` (now thin
//! glue over the shared `crm_container::CrmContainer`) parses the real C80
//! `sample.CRM` and yields the SDK headers (`craw_entry` / `canon_metadata_box`)
//! + per-frame samples WITHOUT the Canon SDK. (The SDK *decode* path additionally
//! needs `CrmSdk.dll`; this test covers only the SDK-free container layer, which
//! is exactly the part the consolidation moved.)

use canon_raw::crm_parser::CrmMovie;

const SAMPLE_CRM: &str = r"D:\programowanie\projekty\Rust\oxivideo\_dev\sdks\sample.CRM";

#[test]
fn crm_movie_parses_sample_via_shared_container() {
    if !std::path::Path::new(SAMPLE_CRM).exists() {
        eprintln!("{SAMPLE_CRM} absent — skipping");
        return;
    }
    let f = std::fs::File::open(SAMPLE_CRM).expect("open sample.CRM");
    let mut movie = CrmMovie::load(f).expect("CrmMovie::load via crm-container");

    assert!(movie.frame_count() > 0, "video frames present");

    // The SDK headers the decode lane feeds to `CrmGetImage`.
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
