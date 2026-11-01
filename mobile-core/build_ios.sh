#!/bin/sh
# Builds the core as a static library for iOS (Xcode's "Build Rust core" phase runs this).
#   mobile-core/build_ios.sh TARGET   aarch64-apple-ios or aarch64-apple-ios-sim
set -eu
here=$(cd "$(dirname "$0")" && pwd)
export PATH="$HOME/.cargo/bin:$PATH"
export IPHONEOS_DEPLOYMENT_TARGET=${IPHONEOS_DEPLOYMENT_TARGET:-17.0}
# Xcode's environment points cargo's C builds (SQLite, ring) at the Mac SDK; let cc pick
unset SDKROOT
target=${1:-aarch64-apple-ios-sim}
rustup target add "$target" >/dev/null 2>&1 || true
exec cargo build --manifest-path "$here/Cargo.toml" --release --lib --target "$target"
