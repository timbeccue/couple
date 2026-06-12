#!/bin/sh
# Assemble Couple.app (SPEC §11.5) and optionally a DMG.
#   Scripts/make-app.sh [--dmg] [--sign "Developer ID Application: …"]
# Default signing is ad-hoc (-). Notarization is an owner step (needs a
# Developer ID + notarytool credentials); the bundle layout is already
# hardened-runtime-compatible. ffmpeg/ffprobe are copied into Contents/Helpers/
# (core resolves ../Helpers first) — ship the pinned LGPL build there for
# distribution; the dev build copies whatever COUPLE_FFMPEG/$PATH provides.
set -e
cd "$(dirname "$0")/.."

SIGN_ID="-"
MAKE_DMG=0
while [ $# -gt 0 ]; do
    case "$1" in
        --dmg) MAKE_DMG=1 ;;
        --sign) shift; SIGN_ID="$1" ;;
    esac
    shift
done

VERSION="0.6.0"
(cd .. && cargo build --release -p couple-ffi)
swift build -c release

APP="dist/Couple.app"
rm -rf dist
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Helpers" "$APP/Contents/Resources"

cp .build/release/Couple "$APP/Contents/MacOS/Couple"

FFMPEG="${COUPLE_FFMPEG:-$(command -v ffmpeg)}"
FFPROBE="$(dirname "$FFMPEG")/ffprobe"
cp "$FFMPEG" "$APP/Contents/Helpers/ffmpeg"
cp "$FFPROBE" "$APP/Contents/Helpers/ffprobe"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Couple</string>
    <key>CFBundleDisplayName</key><string>Couple</string>
    <key>CFBundleIdentifier</key><string>com.timbeccue.couple</string>
    <key>CFBundleExecutable</key><string>Couple</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSHumanReadableCopyright</key><string>© 2026 Tim Beccue. ffmpeg/ffprobe are LGPL, shipped as separate executables in Contents/Helpers.</string>
</dict>
</plist>
PLIST

# The helpers need library validation disabled: a dev (homebrew) ffmpeg loads
# its own dylibs from /opt/homebrew, which hardened-runtime library validation
# otherwise rejects ("different Team IDs"). Harmless for a static LGPL build.
HELPER_ENTS="$(mktemp -t couple-helper-ents).plist"
cat > "$HELPER_ENTS" <<ENTS
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.cs.disable-library-validation</key><true/>
</dict>
</plist>
ENTS
codesign --force --options runtime --entitlements "$HELPER_ENTS" -s "$SIGN_ID" "$APP/Contents/Helpers/ffmpeg"
codesign --force --options runtime --entitlements "$HELPER_ENTS" -s "$SIGN_ID" "$APP/Contents/Helpers/ffprobe"
rm -f "$HELPER_ENTS"
codesign --force --options runtime -s "$SIGN_ID" "$APP"
codesign --verify --deep "$APP" && echo "codesign: ok ($SIGN_ID)"

if [ "$MAKE_DMG" = "1" ]; then
    hdiutil create -volname Couple -srcfolder dist -ov -format UDZO dist/Couple-$VERSION.dmg
    echo "dist/Couple-$VERSION.dmg"
fi
echo "built: $APP"
