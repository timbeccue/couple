#!/bin/sh
# Iteration loop: incremental build (cargo + swift) and launch.
# Usage:
#   Scripts/dev.sh            — build and launch (drag files in yourself)
#   Scripts/dev.sh example    — build and launch with the example pair preloaded
#   (the fine-verify match classifier is ON by default; COUPLE_VERIFY_ACTIVE=0 Scripts/dev.sh disables it for A/B)
set -e
cd "$(dirname "$0")/.."
PATH="$HOME/.cargo/bin:$PATH" # rustup install; sh doesn't read the zsh profile
(cd .. && cargo build --release -p couple-ffi)
swift build -c release -Xswiftc -DCOUPLE_DEVHARNESS # dev harness in; make-app.sh ships without it
if [ "$1" = "example" ]; then
  export COUPLE_OPEN_CAMERA="$PWD/../example_data/schumann-video.mp4"
  export COUPLE_OPEN_EXTERNAL="$PWD/../example_data/schumann-audio.wav"
fi
exec .build/release/Couple
