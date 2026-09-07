#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out_dir="$repo_root/android/harness/app/src/main/jniLibs"

if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo is required" >&2
    exit 1
fi

if ! cargo ndk --version >/dev/null 2>&1; then
    echo "cargo-ndk is required: cargo install cargo-ndk" >&2
    exit 1
fi

if ! command -v rustup >/dev/null 2>&1; then
    echo "rustup is required" >&2
    exit 1
fi

rustup target add aarch64-linux-android
rm -rf "$out_dir"
mkdir -p "$out_dir"

cd "$repo_root"
cargo ndk \
    --platform 28 \
    -t arm64-v8a \
    -o "$out_dir" \
    build \
    --release \
    -p apc-android-bridge

bridge="$out_dir/arm64-v8a/libapc_android_bridge.so"
if [[ ! -f "$bridge" ]]; then
    echo "expected Android bridge was not produced: $bridge" >&2
    exit 1
fi

echo "built $bridge"
