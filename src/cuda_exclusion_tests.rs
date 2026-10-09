// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2025 Adrian <adrian.eddy at gmail>

//! Gates for the process-wide CUDA exclusion. Hardware-free.

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[test]
fn the_holding_thread_can_take_it_again() {
    let outer = cuda_exclusion();
    let inner = cuda_exclusion();
    drop(inner);
    // Still held: another thread must not get in yet.
    let entered = Arc::new(AtomicBool::new(false));
    let other = {
        let entered = entered.clone();
        thread::spawn(move || {
            let _g = cuda_exclusion();
            entered.store(true, Ordering::SeqCst);
        })
    };
    thread::sleep(Duration::from_millis(100));
    assert!(!entered.load(Ordering::SeqCst), "released by the inner guard, not the outer one");
    drop(outer);
    other.join().unwrap();
    assert!(entered.load(Ordering::SeqCst));
}

#[test]
fn another_thread_waits_until_it_is_released() {
    let held = cuda_exclusion();
    let entered = Arc::new(AtomicBool::new(false));
    let other = {
        let entered = entered.clone();
        thread::spawn(move || {
            let _g = cuda_exclusion();
            entered.store(true, Ordering::SeqCst);
        })
    };
    thread::sleep(Duration::from_millis(100));
    assert!(!entered.load(Ordering::SeqCst), "two threads inside the exclusion at once");
    drop(held);
    other.join().unwrap();
    assert!(entered.load(Ordering::SeqCst));
}

#[test]
fn only_cuda_process_refs_take_it() {
    let held = cuda_exclusion();
    let (tx, rx) = std::sync::mpsc::channel();
    let other = thread::spawn(move || {
        exclusive_if_cuda(crate::Device::Cpu, || tx.send("cpu").unwrap());
        exclusive_if_cuda(crate::Device::OpenCl, || tx.send("opencl").unwrap());
        exclusive_if_cuda(crate::Device::Cuda, || tx.send("cuda").unwrap());
    });
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "cpu");
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "opencl");
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err(), "a CUDA call ran inside another thread's exclusion");
    drop(held);
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "cuda");
    other.join().unwrap();
}
