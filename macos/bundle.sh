#!/bin/sh
# Builds the Mac app: macos/build/Openslot.app (release, signed with Developer ID if available).
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
# Signed with this Mac's Developer ID certificate if it has one (OPENSLOT_SIGN_IDENTITY picks
# another, "-" for ad hoc). A stable identity lets macOS keep the microphone permission across
# updates: an ad hoc signature changes with every build, macOS asks again, and the modem's
# audio waits on that question.
identity=${OPENSLOT_SIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null | awk '/"Developer ID Application:/ { print $2; exit }')}
codesign --force --sign "${identity:--}" "$app"
echo "built $app"
