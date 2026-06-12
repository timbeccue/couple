#!/bin/sh
# Iteration loop: incremental build (cargo + swift) and launch.
# Usage:
#   Scripts/dev.sh            — build and launch (drag files in yourself)
#   Scripts/dev.sh example    — build and launch with the example pair preloaded
set -e
cd "$(dirname "$0")/.."
PATH="$HOME/.cargo/bin:$PATH" # rustup install; sh doesn't read the zsh profile
(cd .. && cargo build --release -p couple-ffi)
swift build -c release
if [ "$1" = "example" ]; then
  export COUPLE_OPEN_CAMERA="$PWD/../example_data/schumann-video.mp4"
  export COUPLE_OPEN_EXTERNAL="$PWD/../example_data/schumann-audio.wav"
fi
exec .build/release/Couple
