// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

//! The SDK's CUDA threading contract, spec § 9.2.1:
//!
//! > When you use CRMSDK in multi-thread environments with CUDA processing,
//! > you must call following CRMSDK APIs and CUDA APIs which you use in your
//! > system exclusively: `CrmCreateProcessRef`, `CrmGetImage`,
//! > `CrmEnqueueImage`, `CrmDequeueImage`, `CrmReleaseProcessRef`.
//!
//! OpenCL and Metal processing carry no such restriction (§ 9.2.2, § 9.2.3),
//! and neither does `CrmGetImageAsync`, which is not on the list.
//!
//! This crate keeps the SDK's half of the contract itself: every one of those
//! five calls on a [`Device::Cuda`](crate::Device::Cuda) process ref runs
//! under one process-wide exclusion. The application's half — its own CUDA
//! API calls — is [`cuda_exclusion()`]: hold the guard around CUDA work that can
//! overlap an SDK call on another thread.
//!
//! The exclusion is reentrant on the thread that holds it, so an application
//! can hold it across an SDK call and its own CUDA calls — a decode and the
//! stream synchronize after it, say — and make the pair one critical section.

use std::fmt;
use std::marker::PhantomData;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread::{self, ThreadId};

struct Exclusion {
    owner: Mutex<Owner>,
    released: Condvar,
}

struct Owner {
    thread: Option<ThreadId>,
    depth: usize,
}

/// Process-wide, like the SDK and the CUDA state it protects: every
/// [`CrmSdkLibrary`](crate::CrmSdkLibrary) in the process shares it.
static CUDA_EXCLUSION: Exclusion =
    Exclusion { owner: Mutex::new(Owner { thread: None, depth: 0 }), released: Condvar::new() };

fn lock_owner() -> MutexGuard<'static, Owner> {
    // Nothing that holds this lock can panic, but a poisoned owner record is
    // still a valid one.
    CUDA_EXCLUSION.owner.lock().unwrap_or_else(|e| e.into_inner())
}

/// Proof that this thread holds the process-wide CUDA exclusion. Released on
/// drop; not `Send`, because it belongs to the thread that took it.
#[must_use = "the exclusion is released as soon as the guard is dropped"]
pub struct CudaExclusionGuard {
    _thread_bound: PhantomData<*const ()>,
}

/// Take the process-wide CUDA exclusion of spec § 9.2.1, blocking until no
/// other thread holds it.
///
/// Hold it around the application's own CUDA API calls that can run while
/// another thread is inside the SDK — synchronizing the stream the SDK
/// develops on, recording an event on it, copying out of a buffer it wrote.
/// The SDK calls this crate makes take it themselves.
///
/// Reentrant: a thread that already holds it gets it again at once, so it can
/// wrap an SDK call. Dropping the last guard on the thread releases it.
pub fn cuda_exclusion() -> CudaExclusionGuard {
    let me = thread::current().id();
    let mut owner = lock_owner();
    loop {
        match owner.thread {
            None => {
                owner.thread = Some(me);
                owner.depth = 1;
                break;
            }
            Some(holder) if holder == me => {
                owner.depth += 1;
                break;
            }
            Some(_) => owner = CUDA_EXCLUSION.released.wait(owner).unwrap_or_else(|e| e.into_inner()),
        }
    }
    CudaExclusionGuard { _thread_bound: PhantomData }
}

impl Drop for CudaExclusionGuard {
    fn drop(&mut self) {
        let mut owner = lock_owner();
        owner.depth -= 1;
        if owner.depth == 0 {
            owner.thread = None;
            drop(owner);
            CUDA_EXCLUSION.released.notify_one();
        }
    }
}

impl fmt::Debug for CudaExclusionGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CudaExclusionGuard").finish_non_exhaustive()
    }
}

/// Run `call` under the exclusion when it is a § 9.2.1 call on a CUDA process
/// ref, and as-is otherwise.
pub(crate) fn exclusive_if_cuda<R>(device: crate::Device, call: impl FnOnce() -> R) -> R {
    let _exclusion = (device == crate::Device::Cuda).then(cuda_exclusion);
    call()
}

// Kept in their own file rather than inline with the implementation.
#[cfg(test)]
#[path = "cuda_exclusion_tests.rs"]
mod tests;
