#!/bin/sh
# Builds libopenslot_mobile.so for Android with the NDK into the app's jniLibs (gradle runs this).
#   mobile-core/build_android.sh [ABI...]   (default: arm64-v8a x86_64)
# The NDK is $ANDROID_NDK_HOME, else the newest one in the Android SDK.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
export PATH="$HOME/.cargo/bin:$PATH"
api=29
sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}
ndk=${ANDROID_NDK_HOME:-$(ls -d "$sdk"/ndk/* 2>/dev/null | sort -V | tail -1)}
[ -d "$ndk" ] || { echo "no Android NDK found; install one with the SDK manager" >&2; exit 1; }
case $(uname -s) in
Darwin) host=darwin-x86_64 ;;
*) host=linux-x86_64 ;;
esac
bin="$ndk/toolchains/llvm/prebuilt/$host/bin"
[ $# -gt 0 ] || set -- arm64-v8a x86_64
for abi in "$@"; do
	case $abi in
	arm64-v8a) target=aarch64-linux-android ;;
	x86_64) target=x86_64-linux-android ;;
	*) echo "unknown ABI $abi" >&2; exit 2 ;;
	esac
	rustup target add "$target" >/dev/null 2>&1 || true
	var=$(echo "$target" | tr a-z- A-Z_)
	export "CARGO_TARGET_${var}_LINKER=$bin/$target$api-clang"
	export "CC_$(echo "$target" | tr - _)=$bin/$target$api-clang"
	export "AR_$(echo "$target" | tr - _)=$bin/llvm-ar"
	cargo build --manifest-path "$here/Cargo.toml" --release --lib --target "$target"
	out="$here/../android/app/src/main/jniLibs/$abi"
	mkdir -p "$out"
	cp "$here/target/$target/release/libopenslot_mobile.so" "$out/"
done
