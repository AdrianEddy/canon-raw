use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
pub fn add_dll_directory(dir: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::{
        AddDllDirectory, SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_USER_DIRS,
    };

    unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_USER_DIRS);
    }

    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        AddDllDirectory(wide.as_ptr());
    }
}

#[cfg(not(target_os = "windows"))]
pub fn add_dll_directory(_dir: &Path) {}

pub fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).map(PathBuf::from)
}

pub fn arg_value(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|idx| args.get(idx + 1).cloned())
}

pub fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

pub fn parse_u64_arg(args: &[String], key: &str, default: u64) -> u64 {
    arg_value(args, key)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

