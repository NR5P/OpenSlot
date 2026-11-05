#!/usr/bin/env bash
# Archive the iPhone/iPad app in Release and upload it to App Store Connect, where it shows
# under TestFlight a few minutes later (internal testers get it at once; external testers
# after the first Beta App Review).
#
#   ios/scripts/release.sh            # archive and upload
#   ios/scripts/release.sh archive    # archive only (ios/build/Openslot.xcarchive)
#
# One-time setup: Xcode signed in to team G4GK5372G8 (Settings > Accounts) and an app record
# for io.github.nr5p.openslot in App Store Connect. Each upload needs a build number
# (CURRENT_PROJECT_VERSION) higher than the last one uploaded.
set -euo pipefail

MODE=${1:-upload}
DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
BUILD="$DIR/build"
ARCHIVE="$BUILD/Openslot.xcarchive"
mkdir -p "$BUILD"
rm -rf "$ARCHIVE"

echo "==> Archiving Openslot (iOS)"
xcodebuild -project "$DIR/Openslot.xcodeproj" -scheme Openslot -configuration Release \
  -destination "generic/platform=iOS" -archivePath "$ARCHIVE" \
  -allowProvisioningUpdates archive | grep -E "error:|warning: .*signing|\*\* ARCHIVE" || true
[[ -d "$ARCHIVE" ]] || { echo "Archive failed - see above." >&2; exit 1; }

if [[ "$MODE" == archive ]]; then
  echo "==> Archived to $ARCHIVE"
  exit 0
fi

echo "==> Uploading to App Store Connect"
LOG="$BUILD/upload.log"
xcodebuild -exportArchive -archivePath "$ARCHIVE" \
  -exportOptionsPlist "$DIR/scripts/ExportOptions.plist" -exportPath "$BUILD/export" \
  -allowProvisioningUpdates >"$LOG" 2>&1 || true
grep -E -A2 "error:|Upload|EXPORT" "$LOG" || true
grep -q "EXPORT SUCCEEDED" "$LOG" || { echo "Upload failed - see above or $LOG." >&2; exit 1; }
echo "==> Done. Check App Store Connect > TestFlight; processing takes a few minutes."
