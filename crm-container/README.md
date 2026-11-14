# crm-container — Canon Cinema RAW Light container parser

[![crates.io](https://img.shields.io/crates/v/crm-container.svg)](https://crates.io/crates/crm-container)
[![docs.rs](https://img.shields.io/docsrs/crm-container)](https://docs.rs/crm-container)
[![CI](https://github.com/AdrianEddy/canon-raw/actions/workflows/ci.yml/badge.svg)](https://github.com/AdrianEddy/canon-raw/actions/workflows/ci.yml)
![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-informational)

A pure-Rust parser for the ISOBMFF container of Canon **Cinema RAW Light** (`.CRM`)
clips. No Canon SDK, no FFI, one dependency ([`mp4-atom`](https://crates.io/crates/mp4-atom)).

It gives you everything a CRM decoder needs from the file except the decode itself:

- the per-frame **sample tables** for the CRX video track, the timed-metadata track and
  the uncompressed LPCM audio track, read one sample at a time — the multi-gigabyte `mdat`
  is never loaded;
- the **`CMP1` CRX geometry** (coded read-out size, tile size, bit depth, plane count, CFA
  layout) and the **active-area crop** from `CDI1`/`IAD1`;
- the frame rate, and the audio format;
- the raw Canon-private blobs — the full `CRAW` sample entry and the Canon `uuid` metadata
  box — exactly as Canon's own SDK expects to be handed them.

It is the container layer of [`canon-raw`](https://crates.io/crates/canon-raw), the
bindings for Canon's CRM SDK, and is equally usable on its own by a pure-Rust CRX decoder.

## Example

```rust,no_run
use crm_container::CrmContainer;

let mut clip = CrmContainer::load(std::fs::File::open("A001C001.CRM")?)?;

let cmp1 = clip.cmp1();
let crop = clip.active_crop();
let (num, den) = clip.fps();
println!(
    "{} frames at {num}/{den} fps, read-out {}x{}, active {}x{} at ({}, {})",
    clip.frame_count(),
    cmp1.f_width, cmp1.f_height,
    crop.active_width, crop.active_height, crop.left_margin, crop.top_margin,
);

// One compressed CRX frame and its colour metadata, straight from `mdat`.
let frame = clip.read_compressed_frame(0)?;
let timed_meta = clip.read_timed_meta(0)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Errors that tell "not mine" from "broken"

`CrmContainer::load` keeps three failure classes apart, so a caller that tries several
decoders in turn can act on the difference:

| Error | Meaning |
|---|---|
| `Unsupported` | a well-formed container that is not a CRX movie of the shape this crate parses — try another decoder |
| `MissingBox` / `Parse` | a standard ISOBMFF structure is absent or does not decode — on a clip identified as `.CRM`, the file is damaged |
| `Io` | the reader failed, including `UnexpectedEof` on a truncated file |

The `ftyp` brand is deliberately not checked: identifying the file is the caller's step.

## Tests

The synthetic-container tests run anywhere. The end-to-end tests run against a real clip
when `CRM_TEST_CLIP` points at one, and skip otherwise.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-Apache-2.0) at your option.
