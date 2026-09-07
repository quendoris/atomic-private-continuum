# A.P.C. Android harness

This directory is a deliberately small device harness for the Rust/Android boundary. It is not the A.P.C. application UI and does not define portable format semantics.

Current purpose:

- prove that an APK can load `libapc_android_bridge.so`;
- drive the native foreground-only synchronization gate from Android lifecycle callbacks;
- verify that a fresh process starts fail-closed before `Activity.onStart()`;
- provide the first ADB target for later durability/lost-ack process-kill tests.

The harness currently has no network permission and does not bind the still-open production GitHub HTTP client.

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

## First ADB probe

```bash
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am force-stop org.atomicprivatecontinuum.harness
adb shell monkey -p org.atomicprivatecontinuum.harness 1
```

On a fresh process the screen should report:

```text
process gate before onStart: false
process gate now: true
```

Going Home/background invokes the native background transition. A later foreground start re-enables new transport calls; it does not infer the result of any transport mutation that may already have been in flight.

The next harness slice will replace the lifecycle-only probe with a device-visible encrypted recovery/lost-ack scenario using the existing `ForegroundRecoveryRuntime` and durability contracts.
