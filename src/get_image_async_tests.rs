// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

//! Lifecycle tests for the private `GetImageAsyncState` co-ownership model
//! shared by `GetImageFuture` / `OwnedGetImageFuture`.
//!
//! Exercises the real `Arc::into_raw` / `Arc::from_raw` balance used by
//! `get_image_future*` (the SDK is handed an owned refcount via `ref_con`)
//! and the `get_image_async_callback` dispatcher — without the SDK ever
//! running a job, so it needs no codec or GPU. It is the hardware-free
//! proof that dropping a pending future neither frees the state out from
//! under a not-yet-fired callback nor returns before the job is done.
//!
//! Lives inside the crate (not in `tests/`) because `GetImageAsyncState`
//! and its fields are private implementation details of the async path.

use crate::{
    CRM_ERR_OK, CrmError, CrmErrorCode, CrmVoid, GetImageAsyncState, GetImageFuture, SdkError, get_image_async_callback,
};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

/// Fire the SDK's completion callback for `ref_con`, as an SDK worker would.
fn fire_callback(ref_con: *const GetImageAsyncState, error: CrmErrorCode) -> CrmErrorCode {
    let null = std::ptr::null_mut();
    // SAFETY: `ref_con` is an `Arc::into_raw` handed over exactly once, which
    // is all the callback requires of it.
    unsafe {
        get_image_async_callback(
            null,
            null,
            0,
            1234,
            0,
            0,
            0,
            0,
            null,
            0,
            null,
            null,
            ref_con as *mut CrmVoid,
            null,
            0,
            error,
        )
    }
}

/// A future over `state`, as `get_image_future` would build one.
fn future_over(state: Arc<GetImageAsyncState>) -> GetImageFuture<'static> {
    GetImageFuture { state, _out_dst_data_size: Box::new(0), _inputs: PhantomData }
}

/// Dropping the future while the SDK still holds its handed-out refcount
/// must keep `GetImageAsyncState` alive; the callback then reclaims that
/// refcount via `Arc::from_raw` and completes the state.
#[test]
fn dropping_future_keeps_state_alive_until_callback() {
    let state = Arc::new(GetImageAsyncState::new());

    // get_image_future*(): hand the SDK an owned refcount via `ref_con`.
    let ref_con = Arc::into_raw(state.clone());
    assert_eq!(Arc::strong_count(&state), 2, "future + SDK both own the state");

    // The caller drops its handle before the callback fires (e.g. a seek).
    drop(state);

    // The SDK's reference still keeps the state alive — reconstruct it the
    // way `get_image_async_callback` does.
    let reclaimed = unsafe { Arc::from_raw(ref_con) };
    assert_eq!(Arc::strong_count(&reclaimed), 1, "SDK holds the last reference");
    assert!(!reclaimed.done.load(Ordering::Acquire), "not yet completed");

    // Final release frees the state exactly once — no double-free / UAF.
    drop(reclaimed);
}

/// The guarantee every `unsafe` async contract leans on: dropping a pending
/// future does not return until the SDK has called back, so the inputs and
/// destination the job uses outlive it.
#[test]
fn dropping_a_pending_future_waits_for_the_callback() {
    let state = Arc::new(GetImageAsyncState::new());
    let ref_con = Arc::into_raw(state.clone()) as usize;
    let future = future_over(state.clone());

    let delay = Duration::from_millis(150);
    let sdk_worker = std::thread::spawn(move || {
        std::thread::sleep(delay);
        fire_callback(ref_con as *const GetImageAsyncState, CRM_ERR_OK)
    });

    let started = Instant::now();
    drop(future);
    assert!(state.done.load(Ordering::Acquire), "drop returned before the job finished");
    assert!(started.elapsed() >= delay - Duration::from_millis(20), "drop did not wait: {:?}", started.elapsed());
    assert_eq!(sdk_worker.join().unwrap(), CRM_ERR_OK);
    assert_eq!(Arc::strong_count(&state), 1, "the SDK's refcount was reclaimed exactly once");
}

/// A completed future resolves with the SDK's result, and an error code
/// `CrmError.h` does not define arrives as a value, not as UB.
#[test]
fn a_completed_future_reports_the_callbacks_result() {
    for (code, expected) in [
        (CRM_ERR_OK, None),
        (0x8000_0043, Some(SdkError::DecompressNoResource)),
        (0x8000_0099, Some(SdkError::Unrecognized(0x8000_0099))),
    ] {
        let state = Arc::new(GetImageAsyncState::new());
        let ref_con = Arc::into_raw(state.clone());
        let mut future = pin!(future_over(state));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        assert_eq!(fire_callback(ref_con, code), CRM_ERR_OK);
        match (future.as_mut().poll(&mut cx), expected) {
            (Poll::Ready(Ok(out)), None) => assert_eq!(out.dst_data_size, 1234),
            (Poll::Ready(Err(CrmError::SdkError(e))), Some(want)) => assert_eq!(e, want),
            (other, _) => panic!("code {code:#x}: unexpected {other:?}"),
        }
    }
}
