#!/bin/sh
# Regenerate third-party/ — the license notices for every dependency tree Couple
# ships (Rust crates, npm packages, Swift packages); docs/LICENSING.md §3.
# Fails on an unknown or copyleft license. Needs `cargo` on PATH and
# electron/node_modules installed (`cd electron && npm ci`).
#
#   scripts/third-party-notices.sh          regenerate third-party/ (commit the result)
#   scripts/third-party-notices.sh --check  fail if third-party/ is stale
#
# The build scripts copy third-party/ into the bundles: make-app.sh ->
# Contents/Resources/Licenses/, electron-builder.yml -> resources/Licenses/.
set -e
cd "$(dirname "$0")/.."
PATH="$HOME/.cargo/bin:$PATH"
exec python3 -I scripts/third_party_notices.py "$@"
