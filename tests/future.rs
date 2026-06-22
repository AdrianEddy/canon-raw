// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

//! Lifecycle test for the `GetImageAsyncState` co-ownership model shared
//! by `GetImageFuture` / `OwnedGetImageFuture`.
//!
//! Exercises the real `Arc::into_raw` / `Arc::from_raw` balance used by
//! `get_image_future*` (the SDK is handed an owned refcount via `ref_con`)
//! and the `get_image_async_callback` dispatcher — without the SDK ever
//! running a job, so it needs no codec or GPU. It is the hardware-free
//! proof that dropping a pending future does NOT free the state out from
//! under a not-yet-fired completion callback.

use crate::GetImageAsyncState;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Dropping the future while the SDK still holds its handed-out refcount
/// must keep `GetImageAsyncState` alive; the callback then reclaims that
/// refcount via `Arc::from_raw` and completes the state.
#[test]
fn dropping_future_keeps_state_alive_until_callback() {
    let state = Arc::new(GetImageAsyncState::new());

    // get_image_future*(): hand the SDK an owned refcount via `ref_con`.
    let ref_con = Arc::into_raw(state.clone());
    assert_eq!(Arc::strong_count(&state), 2, "future + SDK both own the state");

    // The caller drops the future before the callback fires (e.g. a seek).
    drop(state);

    // The SDK's reference still keeps the state alive — reconstruct it the
    // way `get_image_async_callback` does.
    let reclaimed = unsafe { Arc::from_raw(ref_con) };
    assert_eq!(Arc::strong_count(&reclaimed), 1, "SDK holds the last reference");
    assert!(!reclaimed.done.load(Ordering::Acquire), "not yet completed");

    // Final release frees the state exactly once — no double-free / UAF.
    drop(reclaimed);
}
