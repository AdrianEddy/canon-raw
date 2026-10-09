// SPDX-License-Identifier: MIT OR Apache-2.0

//! Process-ref reuse: the pool behind [`CrmSdkLibrary::lease_process_ref`].
//!
//! A process ref is expensive to create — 76 threads and ~2 GB of committed
//! memory on CRMSDK 2.10 — and on Windows it is not safe to release soon after
//! creating it (see [`CrmSdkLibrary::create_process_ref`]). So the crate does
//! not create and release refs per decoder: it leases them. Dropping a
//! [`ProcessRefLease`] resets its ref and parks it for the next lease with the
//! same parameters, and parked refs are released only when the SDK is
//! terminated — by [`CrmSdkLibrary::terminate_sdk`] or by dropping the
//! library.
//!
//! The pool grows to the most leases with the same parameters ever held at
//! once, and keeps that many refs — and their memory — until then.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::{CrmSdkLibrary, Device, FormatId, Gpu, ProcessRef, ProcessType, Resolution, Result};

/// What `CrmCreateProcessRef` was called with. Two refs are interchangeable
/// exactly when these match; everything else a decode needs is passed to each
/// `CrmGetImage` call.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct RefKey {
    format_id: FormatId,
    device: Device,
    gpu: Gpu,
    process_type: ProcessType,
    max_resolution: Resolution,
}

/// The parked refs of one [`CrmSdkLibrary`]. Each is live, reset, and leased
/// by no one.
#[derive(Default)]
pub(crate) struct IdleRefs(Mutex<Vec<(RefKey, ProcessRef)>>);

impl IdleRefs {
    fn lock(&self) -> MutexGuard<'_, Vec<(RefKey, ProcessRef)>> {
        // Every critical section is a single `Vec` operation, so a poisoned
        // lock cannot be guarding a half-made change.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The most recently parked ref for `key`: the one whose memory is likeliest
    /// still resident.
    fn take(&self, key: RefKey) -> Option<ProcessRef> {
        let mut idle = self.lock();
        let i = idle.iter().rposition(|(k, _)| *k == key)?;
        Some(idle.remove(i).1)
    }

    fn park(&self, key: RefKey, process_ref: ProcessRef) {
        self.lock().push((key, process_ref));
    }

    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }

    fn drain(&self) -> Vec<ProcessRef> {
        self.lock().drain(..).map(|(_, r)| r).collect()
    }
}

/// A process ref leased from its library's pool by
/// [`CrmSdkLibrary::lease_process_ref`].
///
/// Dropping the lease resets the ref (`CrmResetProcessRef`) and parks it for
/// the next lease with the same parameters instead of releasing it; see
/// [`CrmSdkLibrary::create_process_ref`] for why. [`DecodeContext`] holds one
/// for its whole life.
///
/// [`process_ref`](Self::process_ref) is the handle for the raw methods. It is
/// live until the lease is dropped, so every job submitted on it must be
/// complete, and every [`device_allocate`](CrmSdkLibrary::device_allocate)
/// buffer allocated against it freed, before then.
///
/// [`DecodeContext`]: crate::DecodeContext
pub struct ProcessRefLease {
    sdk: Arc<CrmSdkLibrary>,
    key: RefKey,
    process_ref: ProcessRef,
}

impl ProcessRefLease {
    /// The leased ref.
    pub fn process_ref(&self) -> ProcessRef {
        self.process_ref
    }
}

impl std::fmt::Debug for ProcessRefLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessRefLease")
            .field("process_ref", &self.process_ref)
            .field("format_id", &self.key.format_id)
            .field("gpu", &self.key.gpu)
            .field("process_type", &self.key.process_type)
            .field("max_resolution", &self.key.max_resolution)
            .finish()
    }
}

impl Drop for ProcessRefLease {
    fn drop(&mut self) {
        // SAFETY: the ref is live — created or un-parked by `lease_process_ref`
        // on `self.sdk` and not parked since — and the lease's contract has
        // every job on it complete by now.
        match unsafe { self.sdk.reset_process_ref(self.process_ref) } {
            Ok(()) => self.sdk.idle_refs.park(self.key, self.process_ref),
            // A ref the SDK will not reset is not one to hand out again.
            // SAFETY: as above; it is not used after this.
            Err(_) => {
                let _ = unsafe { self.sdk.release_process_ref(self.process_ref) };
            }
        }
    }
}

impl CrmSdkLibrary {
    /// Lease a process ref: a parked one created with exactly these
    /// parameters if there is one, otherwise a new one from
    /// [`create_process_ref`](Self::create_process_ref) (and so under the
    /// [`cuda_exclusion()`](crate::cuda_exclusion()) for [`Device::Cuda`]).
    ///
    /// This is how to get a ref that is dropped rather than released: the
    /// lease parks it again on drop, and parked refs are released only when
    /// the SDK is terminated. [`DecodeContext::new`](crate::DecodeContext::new)
    /// leases its ref here.
    pub fn lease_process_ref(
        self: &Arc<Self>,
        format_id: FormatId,
        use_device: Device,
        use_gpu: Gpu,
        process_type: ProcessType,
        max_resolution: Resolution,
    ) -> Result<ProcessRefLease> {
        let key = RefKey { format_id, device: use_device, gpu: use_gpu, process_type, max_resolution };
        let process_ref = match self.idle_refs.take(key) {
            Some(parked) => parked,
            None => self.create_process_ref(format_id, use_device, use_gpu, process_type, max_resolution)?,
        };
        Ok(ProcessRefLease { sdk: Arc::clone(self), key, process_ref })
    }

    /// How many refs are parked — created, not leased, and not yet released.
    pub fn idle_process_refs(&self) -> usize {
        self.idle_refs.len()
    }

    /// Release every parked ref. Run on termination, so a parked ref never
    /// outlives the SDK state it belongs to.
    pub(crate) fn release_idle_process_refs(&self) {
        for process_ref in self.idle_refs.drain() {
            // SAFETY: a parked ref is live, leased by no one, and has nothing
            // pending (its lease reset it before parking it).
            let _ = unsafe { self.release_process_ref(process_ref) };
        }
    }
}
