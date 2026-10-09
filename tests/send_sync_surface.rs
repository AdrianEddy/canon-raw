// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

//! Compile-time proof of the `Send` / `Sync` classification of the types that
//! cross threads: the SDK handle, owned SDK memory, the decode context, the
//! owned async future and the GPU init descriptors.

use canon_raw::*;
use static_assertions::{assert_impl_all, assert_not_impl_any};

#[test]
fn crm_sdk_library_is_send_sync() {
    // Required for `Arc<CrmSdkLibrary>` to be `Send + Sync` so it can
    // be cloned into per-decoder facades from any thread.
    assert_impl_all!(CrmSdkLibrary: Send, Sync);
}

#[test]
fn process_ref_is_a_freely_shareable_handle() {
    // A value; every use of it is an `unsafe` call with its own contract.
    assert_impl_all!(ProcessRef: Send, Sync, Copy);
}

#[test]
fn cuda_exclusion_guard_stays_on_its_thread() {
    // Released on drop by the thread that took it.
    assert_not_impl_any!(CudaExclusionGuard: Send, Sync);
}

#[test]
fn crm_memory_is_send() {
    // A decoded frame's backing allocation can be handed to another thread.
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
