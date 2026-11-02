#!/bin/sh
# Fetch the pinned static ffmpeg/ffprobe pair used by `make-app.sh --dist`.
#
# Why pinned + static: a dev build copies whatever ffmpeg is on $PATH, and a
# Homebrew ffmpeg is dynamically linked against versioned Cellar dylibs — the
# copy silently breaks on the next `brew upgrade` (seen 2026-11-01 as
# "unsupported format" on every import). The pinned build here links only OS
# frameworks and /usr/lib, so nothing on the user's machine can break it, and
# every dist bundle ships the exact binary the app was tested against.
#
# Source: https://ffmpeg.martin-riedl.de (build script is open source).
# LICENSE NOTE: this build bundles x264/x265, i.e. it is GPL-configured.
# Fine for personal use; before public distribution either meet GPL terms or
# swap the pin below for an LGPL-configured static build — any URL + sha256
# pair works, nothing else in the pipeline changes.
#
# Cached under mac/vendor/ffmpeg/<build-id>/ (gitignored). Verification:
# sha256 against the pins below (not the remote checksum files), otool -L
# must show only /System and /usr/lib, and both tools must run `-version`.
#
# stdout is exactly one line — the directory holding ffmpeg + ffprobe —
# so callers can do HELPER_DIR="$(Scripts/fetch-ffmpeg.sh)". Progress → stderr.
set -e
cd "$(dirname "$0")/.."

BUILD_ID="1789931890_9.0.2" # ffmpeg 9.0.2, macOS arm64, built 2026-09-20
BASE_URL="https://ffmpeg.martin-riedl.de/download/macos/arm64/$BUILD_ID"
FFMPEG_ZIP_SHA256="c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924"
FFPROBE_ZIP_SHA256="fcbe839537485eaee7a7a8bc5cbc0f90d53617e80943e8a5b2e31cb851197ea6"

DEST="vendor/ffmpeg/$BUILD_ID"

note() { echo "fetch-ffmpeg: $*" >&2; }

check_sha256() { # file expected
    actual="$(shasum -a 256 "$1" | awk '{print $1}')"
    if [ "$actual" != "$2" ]; then
        note "sha256 mismatch for $1"
        note "  expected: $2"
        note "  actual:   $actual"
        exit 1
    fi
}

check_static() { # binary — only OS frameworks and /usr/lib allowed
    bad="$(otool -L "$1" | tail -n +2 | awk '{print $1}' | grep -Ev '^(/usr/lib/|/System/)' || true)"
    if [ -n "$bad" ]; then
        note "$1 links non-system libraries (not a static build):"
        echo "$bad" >&2
        exit 1
    fi
}

check_runs() { # binary
    if ! "$1" -version >/dev/null 2>&1; then
        note "$1 failed to run -version"
        exit 1
    fi
}

if [ ! -x "$DEST/ffmpeg" ] || [ ! -x "$DEST/ffprobe" ]; then
    note "downloading pinned static ffmpeg $BUILD_ID"
    TMP="$(mktemp -d "${TMPDIR:-/tmp}/couple-ffmpeg.XXXXXX")"
    trap 'rm -rf "$TMP"' EXIT
    for tool in ffmpeg ffprobe; do
        curl -fsSL -o "$TMP/$tool.zip" "$BASE_URL/$tool.zip"
    done
    check_sha256 "$TMP/ffmpeg.zip" "$FFMPEG_ZIP_SHA256"
    check_sha256 "$TMP/ffprobe.zip" "$FFPROBE_ZIP_SHA256"
    unzip -o -q "$TMP/ffmpeg.zip" -d "$TMP"
    unzip -o -q "$TMP/ffprobe.zip" -d "$TMP"
    mkdir -p "$DEST"
    mv "$TMP/ffmpeg" "$TMP/ffprobe" "$DEST/"
else
    note "using cached $DEST"
fi

check_static "$DEST/ffmpeg"
check_static "$DEST/ffprobe"
check_runs "$DEST/ffmpeg"
check_runs "$DEST/ffprobe"
note "ok: static, pinned, launches"

echo "$DEST"
