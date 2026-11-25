#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
export ANDROID_HOME="/opt/homebrew/share/android-commandlinetools"
export JAVA_HOME="/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home"
NDK_DIR=$(ls -d "$ANDROID_HOME"/ndk/* | sort -V | tail -n 1)
export PATH="$NDK_DIR/toolchains/llvm/prebuilt/darwin-x86_64/bin:$PATH"

echo "==> Building Sway Rust native libraries (armeabi-v7a + arm64-v8a)..."
cd "$ROOT_DIR/rust"

CC_armv7_linux_androideabi=armv7a-linux-androideabi26-clang \
AR_armv7_linux_androideabi=llvm-ar \
CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER=armv7a-linux-androideabi26-clang \
cargo build --release --target armv7-linux-androideabi

CC_aarch64_linux_android=aarch64-linux-android26-clang \
AR_aarch64_linux_android=llvm-ar \
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=aarch64-linux-android26-clang \
cargo build --release --target aarch64-linux-android

mkdir -p "$ROOT_DIR/android/app/src/main/jniLibs/armeabi-v7a" \
         "$ROOT_DIR/android/app/src/main/jniLibs/arm64-v8a"
cp target/armv7-linux-androideabi/release/libsway_rust.so "$ROOT_DIR/android/app/src/main/jniLibs/armeabi-v7a/"
cp target/aarch64-linux-android/release/libsway_rust.so "$ROOT_DIR/android/app/src/main/jniLibs/arm64-v8a/"

echo "==> Assembling signed Android Release APK..."
cd "$ROOT_DIR/android"
chmod +x ./gradlew
./gradlew assembleRelease

OUT_DIR="$HOME/Downloads/Android Development"
mkdir -p "$OUT_DIR"
OUT_APK="$OUT_DIR/Sway-Rust-v1.0.0.apk"
cp "$ROOT_DIR/android/app/build/outputs/apk/release/app-release.apk" "$OUT_APK"
ls -lh "$OUT_APK"
