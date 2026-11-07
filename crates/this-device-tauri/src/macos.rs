//! macOS native device bridge via Swift C FFI.
//!
//! The macOS Swift code is compiled by `build.rs` into a static library. Each function
//! takes C strings (method name, JSON args) and returns a C string (JSON result). The
//! allocation is done by Swift's `strdup()`; Rust frees it with `libc::free()`.
//!
//! The two entry points mirror the iOS bridge:
//! - [`call`] — a family operation (contacts, photos, etc.), same method names as iOS
//! - [`status`] — the permission state of a capability

use std::ffi::{CStr, CString};

use serde_json::Value;

unsafe extern "C" {
    /// The single entry point for all device family calls on macOS.
    fn macos_device_call(
        method: *const libc::c_char,
        args: *const libc::c_char,
    ) -> *mut libc::c_char;

    /// Status check for a capability.
    fn macos_device_status(capability: *const libc::c_char) -> *mut libc::c_char;
}

/// Call a native method on macOS and return the JSON result.
///
/// This is synchronous: the Swift side blocks (with a semaphore) until any async operation
/// completes. The caller should run this from `spawn_blocking` to avoid blocking the Tokio
/// runtime.
pub fn call(method: &str, args: &Value) -> Value {
    let c_method = match CString::new(method) {
        Ok(s) => s,
        Err(_) => return serde_json::json!({"kind": "error", "code": "internal"}),
    };
    let args_json = serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string());
    let c_args = match CString::new(args_json) {
        Ok(s) => s,
        Err(_) => return serde_json::json!({"kind": "error", "code": "internal"}),
    };

    unsafe {
        let result = macos_device_call(c_method.as_ptr(), c_args.as_ptr());
        parse_and_free(result)
    }
}

/// Check the permission status of a capability on macOS.
pub fn status(capability: &str) -> Value {
    let c_cap = match CString::new(capability) {
        Ok(s) => s,
        Err(_) => return serde_json::json!({"kind": "status", "state": "unsupported"}),
    };

    unsafe {
        let result = macos_device_status(c_cap.as_ptr());
        parse_and_free(result)
    }
}

/// Parse a C string as JSON, then free it. Returns a fallback on any failure.
///
/// # Safety
/// `ptr` must be either null or a valid pointer returned by Swift's `strdup()`,
/// allocated with the C allocator.
unsafe fn parse_and_free(ptr: *mut libc::c_char) -> Value {
    if ptr.is_null() {
        return serde_json::json!({"kind": "error", "code": "internal"});
    }
    let json_str = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().to_string();
    unsafe { libc::free(ptr as *mut libc::c_void) };
    serde_json::from_str(&json_str)
        .unwrap_or_else(|_| serde_json::json!({"kind": "error", "code": "internal"}))
}
