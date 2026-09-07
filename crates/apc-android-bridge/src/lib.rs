#![deny(unsafe_op_in_unsafe_fn)]

//! Narrow Android/JNI boundary for A.P.C.
//!
//! Portable semantics, durability and synchronization remain in the existing
//! Rust crates. This crate only translates Android process/lifecycle calls into
//! the foreground-only synchronization gate.
//!
//! Unlike the portable crates, this FFI boundary cannot forbid every use of an
//! unsafe attribute: exporting stable JNI symbol names requires `no_mangle`.
//! There are deliberately no unsafe blocks in this crate.

use std::sync::OnceLock;

use apc_sync::ForegroundSyncLifecycle;
use jni::objects::JClass;
use jni::sys::{jboolean, jstring, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;

const BRIDGE_VERSION: &str = concat!("apc-android-bridge/", env!("CARGO_PKG_VERSION"));

static LIFECYCLE: OnceLock<ForegroundSyncLifecycle> = OnceLock::new();

fn lifecycle() -> &'static ForegroundSyncLifecycle {
    LIFECYCLE.get_or_init(ForegroundSyncLifecycle::new)
}

fn enter_foreground() {
    lifecycle().enter_foreground();
}

fn enter_background() {
    lifecycle().enter_background();
}

fn is_foreground() -> bool {
    lifecycle().is_foreground()
}

/// Return a small bridge identity so the Android harness can prove that the APK
/// loaded the Rust library it intended to load.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_atomicprivatecontinuum_harness_NativeBridge_nativeVersion(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    match env.new_string(BRIDGE_VERSION) {
        Ok(value) => value.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Mark the current process foreground-eligible for future synchronization I/O.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_atomicprivatecontinuum_harness_NativeBridge_nativeEnterForeground(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    enter_foreground();
}

/// Fail closed for future synchronization I/O as soon as Android backgrounds the
/// harness process. In-flight outcome uncertainty remains a durable recovery
/// problem; this call does not invent an ACK result.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_atomicprivatecontinuum_harness_NativeBridge_nativeEnterBackground(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    enter_background();
}

/// Report only the process-local lifecycle gate. This is diagnostics for the
/// harness, not portable A.P.C. state.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_atomicprivatecontinuum_harness_NativeBridge_nativeIsForeground(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jboolean {
    if is_foreground() {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_lifecycle_is_fail_closed_and_reversible() {
        enter_background();
        assert!(!is_foreground());

        enter_foreground();
        assert!(is_foreground());

        enter_background();
        assert!(!is_foreground());
    }

    #[test]
    fn bridge_version_is_nonempty_and_names_the_boundary() {
        assert!(BRIDGE_VERSION.starts_with("apc-android-bridge/"));
        assert!(BRIDGE_VERSION.len() > "apc-android-bridge/".len());
    }
}
