//! STEP files through OpenCASCADE, behind a C ABI.
//!
//! OpenCASCADE adds ~30 MB of code and C++ static initializers. Living in its own DLL, which
//! `polyloupe-core` loads only when a STEP file is opened, it costs nothing at startup or for
//! any other format. The reading itself is C++ (`reader.cpp`); `cadrum` builds and links the
//! OpenCASCADE libraries.
//!
//! [`polyloupe_step_load`] returns a byte buffer (little endian): `u32 part count`, then per
//! part `u32 name length`, the UTF-8 name, `f32 rgba[4]`, `u32 vertex count`, positions
//! `f32 xyz`, normals `f32 xyz`, `u32 index count`, indices `u32`. Lengths are in millimeters.
//! On failure the buffer is the UTF-8 error message and the status is non-zero. Free it with
//! [`polyloupe_step_free`].

use std::ffi::CString;

// Only for the OpenCASCADE libraries its build links in.
extern crate cadrum;

unsafe extern "C" {
    fn pl_step_read(path: *const std::ffi::c_char, linear: f64, angular: f64, out: *mut *mut u8, out_len: *mut usize) -> i32;
    fn pl_step_free(p: *mut u8);
}

/// Chord deflection relative to each edge's size, and angular deflection (radians): smooth
/// holes and fillets without bloating flat parts.
const LINEAR: f64 = 0.002;
const ANGULAR: f64 = 0.3;

/// Version of the buffer layout; the loader refuses a DLL with another one.
#[unsafe(no_mangle)]
pub extern "C" fn polyloupe_step_abi() -> u32 {
    2
}

/// # Safety
/// `path` must point to `path_len` bytes of UTF-8; `out_ptr` and `out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polyloupe_step_load(path: *const u8, path_len: usize, out_ptr: *mut *mut u8, out_len: *mut usize) -> i32 {
    let bytes = unsafe { std::slice::from_raw_parts(path, path_len) };
    // Windows paths can't hold NUL; if one did, the empty path fails with the reader's message.
    let path = CString::new(bytes).unwrap_or_default();
    // OpenCASCADE takes UTF-8 paths, Unicode folders included.
    unsafe { pl_step_read(path.as_ptr(), LINEAR, ANGULAR, out_ptr, out_len) }
}

/// # Safety
/// `ptr` must come from one [`polyloupe_step_load`] call, freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn polyloupe_step_free(ptr: *mut u8, _len: usize) {
    if !ptr.is_null() {
        unsafe { pl_step_free(ptr) };
    }
}
