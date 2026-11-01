#!/bin/sh
# Builds the Mac app: macos/build/Openslot.app (release, signed ad hoc for this Mac).
#   macos/bundle.sh            build for this Mac's architecture
#   macos/bundle.sh universal  build for Apple silicon and Intel
# Needs the XARA checkout next to this one (../xara relative to the repository).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
export PATH="$HOME/.cargo/bin:$PATH"
export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-12.0}
[ -d "$here/../../xara/crates/xara-ffi" ] || { echo "XARA not found at $here/../../xara" >&2; exit 1; }
app="$here/build/Openslot.app"
if [ "${1:-}" = universal ]; then
	rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
	for t in aarch64-apple-darwin x86_64-apple-darwin; do
		cargo build --manifest-path "$here/Cargo.toml" --release --target "$t"
	done
	bin="$here/build/openslot"
	mkdir -p "$here/build"
	lipo -create -output "$bin" \
		"$here/target/aarch64-apple-darwin/release/openslot" \
		"$here/target/x86_64-apple-darwin/release/openslot"
else
	cargo build --manifest-path "$here/Cargo.toml" --release
	bin="$here/target/release/openslot"
fi
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$here/Info.plist" "$app/Contents/"
cp "$here/../assets/icon/openslot.icns" "$app/Contents/Resources/"
cp "$bin" "$app/Contents/MacOS/openslot"
# Ad hoc signature: enough to run on this Mac and to ask for microphone access.
codesign --force --sign - "$app"
echo "built $app"
