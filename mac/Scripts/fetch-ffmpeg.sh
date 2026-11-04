#!/bin/sh
# Provide the pinned LGPL static ffmpeg/ffprobe pair used by `make-app.sh --dist`.
#
# Built here from the pinned ffmpeg.org release tarball rather than downloaded
# as a prebuilt: every public static macOS build we found is GPL-configured
# (x264/x265), and Couple needs nothing GPL — export stream-copies video and
# encodes audio with AudioToolbox / ffmpeg's native AAC / PCM, matching decodes
# audio with native decoders. Licensing rationale and obligations: docs/LICENSING.md.
#
# Why static + pinned: a dev build copies whatever ffmpeg is on $PATH, and a
# Homebrew ffmpeg is dynamically linked against versioned Cellar dylibs — the
# copy silently breaks on the next `brew upgrade` (seen 2026-11-01 as
# "unsupported format" on every import). This build links only OS frameworks
# and /usr/lib, so nothing on the user's machine can break it, and every dist
# bundle ships the exact binary the app was tested against.
#
# LGPL rule (enforced by check_lgpl below): the binary must report the LGPL in
# `ffmpeg -L`, and its configuration must not contain --enable-gpl or
# --enable-nonfree. --disable-autodetect keeps Homebrew libraries out, so the
# corresponding source is exactly the unmodified tarball pinned here plus
# CONFIGURE_FLAGS; both are recorded in SOURCE.txt next to the binaries, and
# the tarball is kept in vendor/ffmpeg/src/ (gitignored) for the
# source-availability obligation.
#
# Verification: tarball sha256 against the pin below, plus the FFmpeg release
# GPG signature when gpg is installed and the .asc is reachable (key committed
# next to this script, fingerprint pinned); otool -L must show only /System
# and /usr/lib; both tools must run `-version` and pass check_lgpl.
#
# Output: vendor/ffmpeg/<version>-lgpl-<arch>/ holding ffmpeg, ffprobe,
# SOURCE.txt and licenses/{COPYING.LGPLv2.1,LICENSE.md}. stdout is exactly one
# line — that directory — so callers can do HELPER_DIR="$(Scripts/fetch-ffmpeg.sh)".
# Progress → stderr. Builds for the host architecture (a few minutes, once;
# cached afterwards). Bumping the pin: change FFMPEG_VERSION and TARBALL_SHA256
# from a signature-verified download and upload that tarball + .asc to a new
# `ffmpeg-source-<version>` prerelease (see MIRROR_URL); nothing else changes.
set -e
cd "$(dirname "$0")/.."

FFMPEG_VERSION="9.0.2" # released 2026-09-17
TARBALL="ffmpeg-$FFMPEG_VERSION.tar.xz"
UPSTREAM_URL="https://ffmpeg.org/releases/$TARBALL"
# Fetched from the mirror first: the same tarball (and its .asc) kept as an
# asset of the `ffmpeg-source-<version>` prerelease in this repo — the LGPL
# source-retention copy (docs/LICENSING.md §1) doubling as the build input, so
# a release build does not depend on ffmpeg.org being reachable (it was not on
# 2026-11-04, which failed the first release dry run). The sha256 pin applies
# whichever source answered. In CI the mirror is required: a pin bump must
# upload the new tarball there before it can ship (the error says how).
MIRROR_URL="https://github.com/timbeccue/couple/releases/download/ffmpeg-source-$FFMPEG_VERSION/$TARBALL"
TARBALL_SHA256="8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e"
# Committed copy of the "FFmpeg release signing key" (upstream:
# https://ffmpeg.org/ffmpeg-devel.asc); its fingerprint is checked against the
# pin below before use, so the file is a convenience, not a trust root.
SIGNING_KEY_FILE="Scripts/ffmpeg-release-signing-key.asc"
SIGNING_KEY_FPR="FCF986EA15E6E293A5644F10B4322F04D67658D8"

# No --enable-gpl / --enable-nonfree / --enable-version3. Everything enabled
# here is a macOS system framework or a /usr/lib dylib; no network protocols
# (the core only reads local files and pipes).
CONFIGURE_FLAGS="--disable-autodetect --disable-gpl --disable-nonfree --disable-version3 \
--enable-static --disable-shared --disable-doc --disable-ffplay --disable-network --disable-debug \
--enable-videotoolbox --enable-audiotoolbox --enable-zlib --enable-bzlib"

ARCH="$(uname -m)"
SRC_DIR="vendor/ffmpeg/src"
DEST="vendor/ffmpeg/$FFMPEG_VERSION-lgpl-$ARCH"

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

check_lgpl() { # binary — reports LGPL and was not configured with GPL/nonfree code
    if ! "$1" -L 2>/dev/null | grep -q "GNU Lesser General Public"; then  # text wraps before "License"
        note "$1 does not report the LGPL in -L output"
        exit 1
    fi
    cfg="$("$1" -version 2>/dev/null | grep '^configuration:' || true)"
    case "$cfg" in
        *--enable-gpl*|*--enable-nonfree*)
            note "$1 was configured with GPL or nonfree code: $cfg"
            exit 1 ;;
    esac
}

download() { # dest url — retries transient failures; 1 if the URL is unavailable
    if curl -fsSL --retry 2 --retry-delay 5 --connect-timeout 20 -o "$1.part" "$2"; then
        mv "$1.part" "$1"
        note "fetched $2"
        return 0
    fi
    rm -f "$1.part"
    note "unavailable: $2"
    return 1
}

fetch_source() {
    mkdir -p "$SRC_DIR"
    if [ ! -f "$SRC_DIR/$TARBALL" ]; then
        if ! download "$SRC_DIR/$TARBALL" "$MIRROR_URL"; then
            if [ -n "${CI:-}" ]; then
                note "the mirror has no $TARBALL; a release build requires it. Upload the"
                note "sha256-verified upstream tarball (and its .asc) first:"
                note "  gh release create ffmpeg-source-$FFMPEG_VERSION $TARBALL $TARBALL.asc --prerelease --latest=false"
                exit 1
            fi
            note "falling back to upstream (remember to upload it to the mirror before a release)"
            download "$SRC_DIR/$TARBALL" "$UPSTREAM_URL" || { note "could not download $TARBALL"; exit 1; }
        fi
    fi
    check_sha256 "$SRC_DIR/$TARBALL" "$TARBALL_SHA256"
    if ! command -v gpg >/dev/null 2>&1; then
        note "gpg not installed: skipping signature check (sha256 pin still enforced)"
        return
    fi
    if ! download "$TMP/$TARBALL.asc" "$MIRROR_URL.asc" && ! download "$TMP/$TARBALL.asc" "$UPSTREAM_URL.asc"; then
        note "signature file not available from the mirror or upstream: skipping signature check (sha256 pin still enforced)"
        return
    fi
    note "verifying release signature"
    gpg --batch --quiet --no-default-keyring --keyring "$TMP/keyring.gpg" --import "$SIGNING_KEY_FILE" 2>/dev/null
    imported="$(gpg --batch --no-default-keyring --keyring "$TMP/keyring.gpg" --with-colons --fingerprint 2>/dev/null \
        | awk -F: '/^fpr/ { print $10; exit }')"
    if [ "$imported" != "$SIGNING_KEY_FPR" ]; then
        note "$SIGNING_KEY_FILE has fingerprint '$imported', expected $SIGNING_KEY_FPR"
        exit 1
    fi
    if ! gpg --batch --status-fd 1 --no-default-keyring --keyring "$TMP/keyring.gpg" \
            --verify "$TMP/$TARBALL.asc" "$SRC_DIR/$TARBALL" 2>/dev/null \
            | grep -q "VALIDSIG $SIGNING_KEY_FPR"; then
        note "GPG signature on $TARBALL is not from key $SIGNING_KEY_FPR"
        exit 1
    fi
}

build() {
    note "building ffmpeg $FFMPEG_VERSION for $ARCH (LGPL, no external libraries) — a few minutes"
    tar -xJf "$SRC_DIR/$TARBALL" -C "$TMP"
    SRC="$TMP/ffmpeg-$FFMPEG_VERSION"
    if ! (cd "$SRC" && ./configure $CONFIGURE_FLAGS >"$TMP/configure.log" 2>&1); then
        note "configure failed; see $TMP/configure.log and $SRC/ffbuild/config.log"
        trap - EXIT
        exit 1
    fi
    if ! (cd "$SRC" && make -j"$(sysctl -n hw.ncpu)" >"$TMP/make.log" 2>&1); then
        note "make failed; see $TMP/make.log"
        trap - EXIT
        exit 1
    fi
    rm -rf "$DEST"
    mkdir -p "$DEST/licenses"
    cp "$SRC/ffmpeg" "$SRC/ffprobe" "$DEST/"
    cp "$SRC/COPYING.LGPLv2.1" "$SRC/LICENSE.md" "$DEST/licenses/"
    cat > "$DEST/SOURCE.txt" <<SRCTXT
ffmpeg/ffprobe $FFMPEG_VERSION — LGPL v2.1 or later build for macOS $ARCH
Built $(date -u +%Y-%m-%dT%H:%M:%SZ) on macOS $(sw_vers -productVersion) by couple/mac/Scripts/fetch-ffmpeg.sh

Corresponding source (unmodified):
  $UPSTREAM_URL
  retained copy: $MIRROR_URL
  sha256 $TARBALL_SHA256
  signed by FFmpeg release signing key $SIGNING_KEY_FPR

Configure line:
  ./configure $CONFIGURE_FLAGS

No external libraries are linked; the binaries use only macOS system
frameworks and /usr/lib. Licenses: COPYING.LGPLv2.1 and LICENSE.md alongside.
SRCTXT
}

if [ ! -x "$DEST/ffmpeg" ] || [ ! -x "$DEST/ffprobe" ] || [ ! -f "$DEST/SOURCE.txt" ]; then
    TMP="$(mktemp -d "${TMPDIR:-/tmp}/couple-ffmpeg.XXXXXX")"
    trap 'rm -rf "$TMP"' EXIT
    fetch_source
    build
else
    note "using cached $DEST"
fi

check_static "$DEST/ffmpeg"
check_static "$DEST/ffprobe"
check_runs "$DEST/ffmpeg"
check_runs "$DEST/ffprobe"
check_lgpl "$DEST/ffmpeg"
check_lgpl "$DEST/ffprobe"
note "ok: static, LGPL, pinned, launches"

echo "$DEST"
