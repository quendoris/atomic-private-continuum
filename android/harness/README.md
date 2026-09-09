# A.P.C. Android harness

This directory is a deliberately small device harness for the Rust/Android boundary. It is not the A.P.C. application UI and does not define portable format semantics.

Current purpose:

- prove that an APK can load `libapc_android_bridge.so`;
- drive the native foreground-only synchronization gate from Android lifecycle callbacks;
- verify that a fresh process starts fail-closed before `Activity.onStart()`;
- exercise the real protected recovery store in Android app-private storage;
- provide an ADB process-death test for the accepted-publication/lost-ack recovery path.

The harness currently has no network permission and does not bind the still-open production GitHub HTTP client. Its lost-ack transport is a deliberately local, opaque, crash-durable simulator used to exercise the same `OpaqueTransport`/`ForegroundRecoveryRuntime` contracts without making GitHub credentials part of the first device test.

The recovery probe uses a fixed development key. That key is test fixture material only; it is not the production Android key-management design.

## Toolchain

The checked-in Gradle project targets Android API 37 with Android Gradle Plugin 9.4.0, JDK 17 and NDK 28.2.13676358. The Rust bridge is built separately with `cargo-ndk` and copied into the generated `jniLibs` directory.

Install the Rust Android target/build helper once:

```bash
cargo install cargo-ndk
rustup target add aarch64-linux-android
```

Install Android SDK platform 37 and NDK 28.2.13676358 with Android Studio or `sdkmanager`.

## Build

From the repository root:

```bash
./tools/build-android-bridge.sh
cd android/harness
gradle :app:assembleDebug
```

The Gradle wrapper is intentionally not committed yet; use Android Studio or a local Gradle 9.6 installation for this first harness slice. Once the device path is proven, the wrapper can be pinned without making its binary JAR part of the semantic repository history prematurely.

The debug APK is expected under:

```text
android/harness/app/build/outputs/apk/debug/app-debug.apk
```

Install it from `android/harness` with:

```bash
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

For clean logs before a probe:

```bash
adb logcat -c
```

The Activity writes compact state lines under the `APC-HARNESS` tag.

## Lifecycle probe

```bash
adb shell am force-stop org.atomicprivatecontinuum.harness
adb shell monkey -p org.atomicprivatecontinuum.harness 1
```

On a fresh process the screen/log should report:

```text
process gate before onStart: false
process gate now: true
```

Going Home/background invokes the native background transition. A later foreground start re-enables new transport calls; it does not infer the result of any transport mutation that may already have been in flight.

## Encrypted recovery restart probe

The basic recovery probe performs real semantic handoff/exposure, builds real protected sync bytes, and commits the complete `DurableSyncRecord` through `ProtectedSyncRecordStore<UnixFsDurabilityBackend>` under the app-private files directory.

Stage it:

```bash
adb shell am force-stop org.atomicprivatecontinuum.harness
adb shell am start \
  -n org.atomicprivatecontinuum.harness/.MainActivity \
  --es apc_probe stage
```

Expected probe result:

```text
PASS staged encrypted exposed state + exact outbox
```

Kill the entire app process, then verify from a new process:

```bash
adb shell am force-stop org.atomicprivatecontinuum.harness
adb shell am start \
  -n org.atomicprivatecontinuum.harness/.MainActivity \
  --es apc_probe verify
```

Expected result:

```text
PASS reopened encrypted state; cursor/exposure/outbox intact
```

This proves the process-restart path only when it is actually run on a device. Host CI exercises the same Rust probe against the real Unix development filesystem backend, but it is not evidence about Android's storage stack by itself.

## Lost-ack process-death probe

This is the more important scenario. The harness first durably stages the local exposed state and exact protected outbox. A simulated opaque remote transport then durably accepts that exact publication and deliberately loses the response. Local state therefore remains at the old cursor with the publication still pending.

Start the uncertain-outcome phase:

```bash
adb shell am force-stop org.atomicprivatecontinuum.harness
adb shell am start \
  -n org.atomicprivatecontinuum.harness/.MainActivity \
  --es apc_probe lost-ack-stage
```

The command is deferred until `Activity.onStart()` has opened the native foreground gate. Expected result:

```text
PASS remote accepted once; response lost; durable local outbox still pending
```

Now kill the process before any acknowledgement can be remembered in RAM:

```bash
adb shell am force-stop org.atomicprivatecontinuum.harness
```

Start a new process and recover from durable facts:

```bash
adb shell am start \
  -n org.atomicprivatecontinuum.harness/.MainActivity \
  --es apc_probe lost-ack-resume
```

Expected result:

```text
PASS authenticated refetch reconciled lost ACK; no second publish
```

The Rust probe also keeps a durable transport-side `publish_count` and requires it to remain exactly `1`, so successful recovery cannot silently pass by publishing the same accepted change a second time.

Inspect the latest harness lines with, for example:

```bash
adb logcat -d -s APC-HARNESS:I '*:S'
```

## What these probes do and do not prove

If the device runs pass, it is evidence for Android process-death recovery using app-private filesystem storage, JNI lifecycle gating, real A.P.C. AEAD/recovery framing and the current runtime orchestration.

It is **not** yet proof of sudden device power-loss durability. `adb shell am force-stop` kills the app process but does not remove power from the storage stack. The simulated remote also lives in the harness app-private filesystem and therefore is not an independent fault domain. Production GitHub transport, Android credential/key ownership, real network cancellation and physical power-loss campaigns remain separate later tests.
