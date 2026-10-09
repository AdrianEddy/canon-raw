# Changelog

All notable changes to `crm-container` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1]

First release on crates.io.

- `CrmContainer`: parses `moov` only and reads video, timed-metadata and LPCM audio
  samples one at a time, never loading `mdat`.
- `Cmp1` CRX geometry, `ActiveCrop` from `CDI1`/`IAD1` (with a symmetric fallback),
  frame rate, `AudioFormat`, and the raw `CRAW` entry and Canon `uuid` box.
- `CrmContainerError` separates `Unsupported` (not a CRX movie) from `MissingBox` /
  `Parse` (damaged).

[Unreleased]: https://github.com/AdrianEddy/canon-raw/compare/crm-container-v0.2.1...HEAD
[0.2.1]: https://github.com/AdrianEddy/canon-raw/releases/tag/crm-container-v0.2.1
