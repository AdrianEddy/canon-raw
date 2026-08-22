// SPDX-License-Identifier: MIT OR Apache-2.0

//! Crate-internal gates for [`sdk_path_cstring`] — the `Path` → `CrmChar*`
//! encoder every SDK path argument goes through. Private, so it cannot live in
//! `tests/`; hardware-free, so it runs everywhere.

use super::sdk_path_cstring;
use std::path::Path;

#[test]
fn an_ascii_path_encodes_byte_for_byte() {
    let p = if cfg!(windows) { r"C:\CRMSDK210\CRMSDK" } else { "/opt/CRMSDK210/CRMSDK" };
    let c = sdk_path_cstring(Path::new(p)).expect("an ASCII path is always encodable");
    assert_eq!(c.to_bytes(), p.as_bytes(), "the SDK must receive exactly the path it was given");
}

/// A path is caller data. An interior NUL truncates a C string, so it cannot be
/// handed to the SDK — but it must come back as an error, never a panic, which
/// is what `CString::new(..).unwrap()` used to do.
#[test]
fn an_interior_nul_is_an_error_not_a_panic() {
    let err = sdk_path_cstring(Path::new("luts\0dir")).expect_err("an interior NUL cannot be a C string");
    assert!(
        matches!(err, crate::CrmError::InvalidParameter(_)),
        "an unencodable path is an InvalidParameter, got {err:?}"
    );
}

/// The SDK reads `""` as "no directory" (its own `CrmInitializeSDK*` default
/// resolves the bare `CanonLUTs` against the module directory), so an empty
/// path is legal input and must not error.
#[test]
fn an_empty_path_encodes_to_an_empty_c_string() {
    let c = sdk_path_cstring(Path::new("")).expect("an empty path is legal");
    assert!(c.to_bytes().is_empty());
}

/// The behaviour [`sdk_path_cstring`]'s Windows arm exists for: a character
/// the SDK's ANSI code page cannot express must come back as an error, never
/// as *different* bytes.
///
/// `WideCharToMultiByte` has two ways of losing such a character, and the
/// encoder has to defeat both:
///
///   * **best fit** — the OS default, which quietly substitutes a lookalike
///     (`ﬁ` → `fi`, `Ａ` → `A`) and leaves `lpUsedDefaultChar` *clear*.
///     `WC_NO_BEST_FIT_CHARS` turns it off;
///   * **the default character** — `?`, which the OS does report, via the
///     `lpUsedDefaultChar` out-parameter the encoder checks.
///
/// Drop either and the SDK is handed a path that is silently not the one the
/// caller asked for; the gates below fail in exactly that case. They are
/// written against the *thread* ANSI code page rather than a hardcoded one, so
/// they stay meaningful under an `activeCodePage=UTF-8` manifest — where
/// `GetACP()` is 65001, every scalar is representable, and nothing is expected
/// to be rejected at all.
#[cfg(target_os = "windows")]
mod ansi_code_page {
    use super::sdk_path_cstring;
    use std::path::Path;

    /// A character embedded where a real caller would put one.
    fn path_with(c: char) -> String {
        format!(r"C:\CRMSDK210\{c}")
    }

    /// Every scalar a path could plausibly carry: the whole BMP bar the
    /// surrogate range (`char::from_u32` drops those), plus astral samples.
    fn corpus() -> impl Iterator<Item = char> {
        (0x20u32..=0xFFFF).chain([0x1_0000, 0x1_D11E, 0x1_F600, 0x10_FFFF]).filter_map(char::from_u32)
    }

    /// Encode in the thread ANSI code page with best-fit substitution left
    /// **on** — the OS default, and precisely what [`sdk_path_cstring`]
    /// suppresses. Returns the bytes plus the `lpUsedDefaultChar` the OS
    /// reported, which is what separates a best-fit substitution (flag clear,
    /// bytes changed) from a fallback to the default character (flag set).
    ///
    /// `None` when the code page cannot encode `s` at all — including the
    /// UTF-8 ACP case, where the documented CP_UTF8 restriction on
    /// `lpUsedDefaultChar` may reject the call.
    fn encode_with_best_fit(s: &str) -> Option<(Vec<u8>, bool)> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Globalization::{CP_THREAD_ACP, WideCharToMultiByte};

        let wide: Vec<u16> = std::ffi::OsStr::new(s).encode_wide().collect();
        if wide.is_empty() {
            return Some((Vec::new(), false));
        }
        // SAFETY: `wide` is a valid `[u16]` of `wide.len()`; a null
        // destination with size 0 is the documented length query.
        let len = unsafe {
            WideCharToMultiByte(CP_THREAD_ACP, 0, wide.as_ptr(), wide.len() as i32, std::ptr::null_mut(), 0, std::ptr::null(), std::ptr::null_mut())
        };
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        let mut used_default: i32 = 0;
        // SAFETY: `buf` has exactly the `len` bytes the query asked for.
        let written = unsafe {
            WideCharToMultiByte(CP_THREAD_ACP, 0, wide.as_ptr(), wide.len() as i32, buf.as_mut_ptr(), len, std::ptr::null(), &mut used_default)
        };
        if written != len {
            return None;
        }
        Some((buf, used_default != 0))
    }

    /// Decode with the same code page the SDK will use, so a round-trip
    /// mismatch means the SDK would open a different path than we were asked
    /// for.
    fn decode(bytes: &[u8]) -> Option<String> {
        use windows_sys::Win32::Globalization::{CP_THREAD_ACP, MultiByteToWideChar};

        if bytes.is_empty() {
            return Some(String::new());
        }
        // SAFETY: `bytes` is a valid `[u8]` of `bytes.len()`; a null
        // destination with size 0 is the documented length query.
        let len = unsafe { MultiByteToWideChar(CP_THREAD_ACP, 0, bytes.as_ptr(), bytes.len() as i32, std::ptr::null_mut(), 0) };
        if len <= 0 {
            return None;
        }
        let mut wide = vec![0u16; len as usize];
        // SAFETY: `wide` has exactly the `len` units the query asked for.
        let written = unsafe { MultiByteToWideChar(CP_THREAD_ACP, 0, bytes.as_ptr(), bytes.len() as i32, wide.as_mut_ptr(), len) };
        if written != len {
            return None;
        }
        String::from_utf16(&wide).ok()
    }

    /// `c` is one the code page substitutes a *lookalike* for: it encodes
    /// without the OS flagging a default-character fallback, yet does not
    /// decode back. These are the characters `lpUsedDefaultChar` cannot see,
    /// and the only reason `WC_NO_BEST_FIT_CHARS` has to be passed.
    fn is_best_fit_only(c: char) -> bool {
        let s = c.to_string();
        match encode_with_best_fit(&s) {
            // Reported by the OS, or not encodable at all — the
            // `lpUsedDefaultChar` check already covers it.
            None | Some((_, true)) => false,
            Some((bytes, false)) => decode(&bytes).as_deref() != Some(s.as_str()),
        }
    }

    /// The gate for `WC_NO_BEST_FIT_CHARS`. Without the flag the character
    /// found here encodes to a lookalike with `lpUsedDefaultChar` clear, so
    /// `sdk_path_cstring` returns `Ok` with bytes that are not the caller's
    /// path — the silent mangling the flag exists to prevent.
    #[test]
    fn a_best_fit_lookalike_is_rejected_not_substituted() {
        let victims: Vec<char> = corpus().filter(|c| is_best_fit_only(*c)).collect();
        let Some(&c) = victims.first() else {
            // A code page with no best-fit table — UTF-8 ACP is the common
            // case, where nothing is unrepresentable in the first place.
            eprintln!(
                "skipped: the thread ANSI code page substitutes no characters, so there is no \
                 best-fit case to reject (an activeCodePage=UTF-8 manifest does this)"
            );
            return;
        };
        eprintln!("{} best-fit-substituted scalars in this code page; testing U+{:04X}", victims.len(), c as u32);

        let path = path_with(c);
        let err = sdk_path_cstring(Path::new(&path))
            .map(|c| c.into_bytes())
            .expect_err("a character the code page only best-fits must be rejected, not substituted");
        assert!(
            matches!(err, crate::CrmError::InvalidParameter(_)),
            "an unrepresentable path is an InvalidParameter, got {err:?}"
        );
    }

    /// The invariant behind both the flag and the `lpUsedDefaultChar` check,
    /// stated over the whole scalar range: whatever `sdk_path_cstring`
    /// *accepts* must decode back, under the SDK's own code page, to exactly
    /// the path it was handed. Anything else is a path the SDK would open
    /// that the caller never named.
    ///
    /// Meaningful on either kind of ACP: on a legacy one most of the range is
    /// rejected and the handful that is accepted must survive the round trip;
    /// on a UTF-8 ACP everything is accepted and everything must survive.
    #[test]
    fn nothing_sdk_path_cstring_accepts_is_silently_mangled() {
        let (mut accepted, mut rejected, mut mangled) = (0usize, 0usize, Vec::new());

        for c in corpus() {
            let path = path_with(c);
            match sdk_path_cstring(Path::new(&path)) {
                Ok(encoded) => {
                    accepted += 1;
                    if decode(encoded.to_bytes()).as_deref() != Some(path.as_str()) {
                        mangled.push(c);
                    }
                }
                Err(_) => rejected += 1,
            }
        }

        eprintln!("ANSI code page sweep: accepted={accepted} rejected={rejected} mangled={}", mangled.len());
        assert!(
            mangled.is_empty(),
            "{} scalars were accepted but do not decode back — first offenders: {:?}",
            mangled.len(),
            mangled.iter().take(8).map(|c| format!("U+{:04X}", *c as u32)).collect::<Vec<_>>()
        );
        assert!(accepted > 0, "the sweep encoded nothing at all — the ACP probe itself is broken");
    }
}
