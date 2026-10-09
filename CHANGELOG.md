# Changelog

All notable changes to `canon-raw` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`crm-container` has [its own changelog](crm-container/CHANGELOG.md).

## [Unreleased]

### Fixed

- Metal init aborted the process: `CrmMetalInit` handed the SDK the bare `id<MTLDevice>` /
  `id<MTLCommandQueue>`, but `CrmMtlObject` takes an `NSArray` of devices and an `NSArray` of
  queues (Canon's Metal sample), and the SDK threw `NSInvalidArgumentException` through Rust
  frames. It now builds one-element arrays, and `initialize_with_metal` keeps them retained
  until `terminate_sdk`.

## [0.1.0]

Initial release, against Canon CRMSDK 2.10.

- `CrmSdkLibrary`: every `CrmSdk` entry point, resolved at runtime with `libloading`
  (Windows, Linux, macOS), including the platform-mangled `CrmGetImageAsync`. Entry
  points that take raw pointers, process refs or GPU handles are `unsafe fn`s with
  `# Safety` contracts; the rest are safe.
- Values the SDK writes back are read as plain integers and converted afterwards:
  `SdkError` keeps unknown codes as `Unrecognized`, `FormatId` is a newtype that holds
  any value, and `GpuInfo` / `AcesIdtInfo` report raw values alongside decoded ones.
- `get_image_info` takes typed keys from `image_info`, each fixing the type and width the
  SDK writes for its property, read through a zeroed scratch buffer.
- `Header` borrows the slice it describes, so header packs are built safely.
- CUDA threading (spec § 9.2.1): `ProcessRef` records its device, and the five SDK calls
  the spec restricts run under a process-wide, reentrant `cuda_exclusion` on CUDA refs.
  The guard is public so applications can cover their own CUDA calls. CPU, OpenCL and
  Metal refs, and `CrmGetImageAsync`, stay concurrent.
- Process refs are leased: `CrmSdkLibrary::lease_process_ref` returns a
  `ProcessRefLease`, which resets its ref and parks it for reuse when dropped. Parked
  refs are released only when the SDK is terminated. On Windows, releasing a ref soon
  after creating it can crash the process: the SDK frees a worker's state after a
  bounded wait, and a worker that has not finished starting then runs on freed memory.
- `DecodeContext`: one leased process ref, both parameter structs, the header pack and
  the resolved output raster kept together, with transactional geometry changes and the
  SDK's undocumented `ResParamV13` pair constraints checked before any decode — and
  before the ref is leased. `decode_frame_cpu_into` is safe; it refuses short buffers
  and non-CPU outputs.
- `crm_parser::CrmMovie`: the `.CRM` container view the SDK consumes, built on
  `crm-container`.
- Async decode through `GetImageFuture` / `OwnedGetImageFuture`, which wait for the job
  when dropped while pending, and owned CUDA / OpenCL / Metal init descriptors.

[Unreleased]: https://github.com/AdrianEddy/canon-raw/compare/canon-raw-v0.1.0...HEAD
[0.1.0]: https://github.com/AdrianEddy/canon-raw/releases/tag/canon-raw-v0.1.0
