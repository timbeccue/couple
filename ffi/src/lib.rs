//! couple-ffi — the C ABI porting surface (§4.9). Generated header: couple.h.

#![allow(clippy::missing_safety_doc)]

use std::ffi::{c_char, CString};

/// ABI version gate; bump on any breaking change (§4.9).
pub const CPL_ABI_VERSION: u32 = 1;

#[no_mangle]
pub extern "C" fn cpl_abi_version() -> u32 {
    CPL_ABI_VERSION
}

/// Returns a core version string; free with `cpl_string_free`.
#[no_mangle]
pub extern "C" fn cpl_version() -> *mut c_char {
    CString::new(couple_core::version()).unwrap().into_raw()
}

#[no_mangle]
pub unsafe extern "C" fn cpl_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}
