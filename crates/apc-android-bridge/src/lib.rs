#![deny(unsafe_op_in_unsafe_fn)]

//! Narrow Android/JNI boundary for A.P.C.
//!
//! Portable semantics, durability and synchronization remain in the existing
//! Rust crates. This crate translates Android process/lifecycle calls into the
//! foreground-only synchronization gate and exposes development-only encrypted
//! recovery probes for ADB process-death testing.
//!
//! Unlike the portable crates, this FFI boundary cannot forbid every use of an
//! unsafe attribute: exporting stable JNI symbol names requires `no_mangle`.
//! There are deliberately no unsafe blocks in this crate.

mod recovery_probe;

use std::path::PathBuf;
use std::sync::OnceLock;

use apc_sync::ForegroundSyncLifecycle;
use jni::objects::{JClass, JString};
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

fn java_path(env: &mut JNIEnv<'_>, value: &JString<'_>) -> Result<PathBuf, String> {
    let text: String = env
        .get_string(value)
        .map_err(|error| format!("read Android filesDir: {error}"))?
        .into();
    if text.is_empty() {
        return Err("Android filesDir is empty".to_owned());
    }
    Ok(PathBuf::from(text))
}

fn probe_result_string(env: &JNIEnv<'_>, result: Result<String, String>) -> jstring {
    let text = match result {
        Ok(message) => message,
        Err(error) => format!("FAIL {error}"),
    };
    match env.new_string(text) {
        Ok(value) => value.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn foreground_probe_path(
    env: &mut JNIEnv<'_>,
    files_dir: &JString<'_>,
) -> Result<PathBuf, String> {
    if !is_foreground() {
        return Err("transport recovery probe refused before Android foreground entry".to_owned());
    }
    java_path(env, files_dir)
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

/// Build and durably stage one real protected scalar recovery state under the
/// Android app-private files directory. No transport I/O occurs in this probe.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_atomicprivatecontinuum_harness_NativeBridge_nativeStageRecoveryProbe(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    files_dir: JString<'_>,
) -> jstring {
    let result = java_path(&mut env, &files_dir).and_then(|path| recovery_probe::stage(&path));
    probe_result_string(&env, result)
}

/// Reopen, authenticate, decode and validate the recovery state written by a
/// previous process. This is the basic ADB force-stop/restart observation point.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_atomicprivatecontinuum_harness_NativeBridge_nativeVerifyRecoveryProbe(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    files_dir: JString<'_>,
) -> jstring {
    let result = java_path(&mut env, &files_dir).and_then(|path| recovery_probe::verify(&path));
    probe_result_string(&env, result)
}

/// Durably stage a real protected outbox, let the simulated opaque transport
/// accept it, then deliberately lose the response. Android must already have
/// opened the foreground gate before this transport mutation may run.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_atomicprivatecontinuum_harness_NativeBridge_nativeStageLostAckProbe(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    files_dir: JString<'_>,
) -> jstring {
    let result = foreground_probe_path(&mut env, &files_dir)
        .and_then(|path| recovery_probe::stage_lost_ack(&path));
    probe_result_string(&env, result)
}

/// After process death, reopen the exact durable outbox and use authenticated
/// refetch to prove the previous transport acceptance without publishing again.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_atomicprivatecontinuum_harness_NativeBridge_nativeResumeLostAckProbe(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    files_dir: JString<'_>,
) -> jstring {
    let result = foreground_probe_path(&mut env, &files_dir)
        .and_then(|path| recovery_probe::resume_lost_ack(&path));
    probe_result_string(&env, result)
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
