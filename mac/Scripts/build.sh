#!/bin/sh
# Build couple-core's C ABI staticlib, then the Swift package.
# Usage: Scripts/build.sh [debug|release]   (default release)
set -e
cd "$(dirname "$0")/.."
CONFIG="${1:-release}"
(cd .. && cargo build --release -p couple-ffi)
swift build -c "$CONFIG"
echo "Run: COUPLE_OPEN_CAMERA=... COUPLE_OPEN_EXTERNAL=... .build/$CONFIG/Couple"
