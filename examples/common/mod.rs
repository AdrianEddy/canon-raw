// SPDX-License-Identifier: MIT OR Apache-2.0

//! Helpers shared by the examples. Each example compiles its own copy, so not
//! every helper is used everywhere.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use canon_raw::{CrmError, CrmSdkLibrary, Device, Gpu};

/// Add `dir` to the DLL search path, so `CrmSdk.dll` can find the runtimes it
/// loads by bare name from beside itself (`crxdec.dll`, CUDA, OpenMP).
#[cfg(target_os = "windows")]
pub fn add_dll_directory(dir: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::{
        AddDllDirectory, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_USER_DIRS, SetDefaultDllDirectories,
    };

    unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_USER_DIRS);
    }

    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        AddDllDirectory(wide.as_ptr());
    }
}

/// Off Windows the dynamic loader's search path is fixed at process start:
/// export `LD_LIBRARY_PATH` / `DYLD_LIBRARY_PATH` before running instead.
#[cfg(not(target_os = "windows"))]
pub fn add_dll_directory(_dir: &Path) {}

pub fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).map(PathBuf::from)
}

pub fn arg_value(args: &[String], key: &str) -> Option<String> {
    args.iter().position(|a| a == key).and_then(|idx| args.get(idx + 1).cloned())
}

/// `--flag <path>`, falling back to the environment variable `env`.
pub fn arg_or_env(args: &[String], flag: &str, env: &str) -> Option<PathBuf> {
    arg_value(args, flag).map(PathBuf::from).or_else(|| env_path(env))
}

pub fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

pub fn parse_u64_arg(args: &[String], key: &str, default: u64) -> u64 {
    arg_value(args, key).and_then(|v| v.parse::<u64>().ok()).unwrap_or(default)
}

/// Load and initialize the SDK named by `--sdk-dll` (or `CRM_SDK_DLL`), and
/// point it at the directory CONTAINING `CanonLUTs`, given by `--luts-dir` (or
/// `CRM_LUTS_DIR`).
pub fn load_sdk(args: &[String]) -> canon_raw::Result<Arc<CrmSdkLibrary>> {
    let sdk_dll = arg_or_env(args, "--sdk-dll", "CRM_SDK_DLL")
        .ok_or(CrmError::InvalidParameter("--sdk-dll <path to CrmSdk> (or CRM_SDK_DLL)"))?;
    let luts_dir = arg_or_env(args, "--luts-dir", "CRM_LUTS_DIR")
        .ok_or(CrmError::InvalidParameter("--luts-dir <directory containing CanonLUTs> (or CRM_LUTS_DIR)"))?;

    if let Some(sdk_dir) = sdk_dll.parent() {
        add_dll_directory(sdk_dir);
    }
    // SAFETY: the caller names a Canon CrmSdk build; loading runs its initializers.
    let lib = Arc::new(unsafe { CrmSdkLibrary::load(&sdk_dll) }?);
    lib.initialize_sdk()?;
    // The directory that CONTAINS `CanonLUTs` — the SDK appends the folder name.
    lib.set_luts_directory(&luts_dir, true)?;
    Ok(lib)
}

/// `--device cpu|cuda|opencl`, defaulting to `default`.
pub fn device_arg(args: &[String], default: Device) -> Device {
    match arg_value(args, "--device").as_deref() {
        Some("cpu") => Device::Cpu,
        Some("cuda") => Device::Cuda,
        Some("opencl") => Device::OpenCl,
        _ => default,
    }
}

/// The first GPU the SDK reports as supported for `device`, as the `Gpu` bit
/// `create_process_ref` takes. `None` for the CPU, or when there is none.
pub fn pick_first_supported_gpu(lib: &CrmSdkLibrary, device: Device) -> Option<Gpu> {
    if device == Device::Cpu {
        return None;
    }
    const GPUS: [Gpu; 8] = [Gpu::Gpu1, Gpu::Gpu2, Gpu::Gpu3, Gpu::Gpu4, Gpu::Gpu5, Gpu::Gpu6, Gpu::Gpu7, Gpu::Gpu8];
    let list = lib.get_gpu_info_list_with_device(device).ok()?;
    list.iter().zip(GPUS).find(|(info, _)| info.is_supported()).map(|(_, gpu)| gpu)
}
