// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

//! Compile-only proof that `CrmMemory` (post-owned-rewrite from
//! stage-6c.md § 6c.0.4.A) and `DecodeContext` (§ 6c.0.4.F) carry the
//! Send classification gpu-video's `Arc<BackendFrameAnchor>` requires.

use canon_raw::*;
use static_assertions::assert_impl_all;

#[test]
fn crm_sdk_library_is_send_sync() {
    // Required for `Arc<CrmSdkLibrary>` to be `Send + Sync` so it can
    // be cloned into per-decoder facades from any thread.
    assert_impl_all!(CrmSdkLibrary: Send, Sync);
}

#[test]
fn crm_memory_is_send() {
    assert_impl_all!(CrmMemory: Send);
}

#[test]
fn decode_context_is_send() {
    assert_impl_all!(DecodeContext: Send);
}

#[test]
fn owned_get_image_future_is_send() {
    // A pipelined scheduler parks `depth` of these in a cross-thread pool.
    assert_impl_all!(OwnedGetImageFuture: Send);
}

#[test]
fn init_descriptors_are_send() {
    #[cfg(not(target_vendor = "apple"))]
    assert_impl_all!(CrmCudaInit: Send);
    assert_impl_all!(CrmOpenClInit: Send);
    #[cfg(target_vendor = "apple")]
    assert_impl_all!(CrmMetalInit: Send);
}
