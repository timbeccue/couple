#!/usr/bin/env bash
# realdata-sweep.sh — F4 one-command owner calibration sweep (PLAN-REALDATA-CALIBRATION.md).
#
# Given COUPLE_REALDATA_DIR (a parent directory of real session folders), this:
#   1. discovers session dirs (any dir directly holding >= 2 media files),
#   2. runs `couple pair --json` per session (candidate matrix + spectral evidence + rows),
#   3. runs `couple sync --json` per resolved couple,
#   4. runs the frozen SPEC §9.4 regression suites when the canonical schumann pair
#      is present in the dir root,
#   5. writes realdata-report.md / realdata-report.json — measured offset/RMS/ppm/
#      z/margin/coverage distributions vs the CURRENT [TUNE] verdict lines, with
#      cross-cell Matches flagged as the SPEC-V2 §3.5 different-take hazard.
#
# It MEASURES AND REPORTS ONLY — it never changes a threshold. Threshold changes
# require owner sign-off via the SPEC-CHANGE protocol (SPEC §0).
#
# Usage:
#   COUPLE_REALDATA_DIR=/path/to/sessions scripts/realdata-sweep.sh [--out FILE.md]
#   scripts/realdata-sweep.sh --self-check   # synthetic end-to-end dry run (no real data)
#
# Prerequisite: ffmpeg on PATH (or COUPLE_FFMPEG). Re-runs are warm: real mode
# uses the default platform cache, so multi-GB imports happen once.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(pwd)"

SELF_CHECK=0
OUT_MD="$ROOT/realdata-report.md"
while [ $# -gt 0 ]; do
  case "$1" in
    --self-check) SELF_CHECK=1 ;;
    --out) OUT_MD="${2:?--out needs a path}"; shift ;;
    -h|--help) sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "realdata-sweep: unknown argument $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

cargo build -q --release -p couple-cli
BIN="$ROOT/target/release/couple"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PY_ARGS=()
if [ "$SELF_CHECK" = 1 ]; then
  # Synthetic stand-in session (SessionSpec, SPEC-V2 §7): two videos, two externals;
  # each external covers exactly one video, so the cross cells are honest negatives
  # (the different-take stand-in) and both couples must sync.
  cat > "$WORK/spec.json" <<'EOF'
{
  "seed": 41,
  "cameras": [
    { "name": "video1", "masterT0": 30.0,  "durS": 40.0 },
    { "name": "video2", "masterT0": 150.0, "durS": 40.0 }
  ],
  "externals": [
    { "name": "externalA", "masterT0": 0.0,   "durS": 100.0 },
    { "name": "externalB", "masterT0": 120.0, "durS": 90.0 }
  ]
}
EOF
  mkdir -p "$WORK/data"
  "$BIN" --quiet fixture session --out "$WORK/data/selfcheck-session" --spec "$WORK/spec.json" >/dev/null
  DATA_DIR="$WORK/data"
  OUT_MD="$WORK/realdata-report.md"
  PY_ARGS+=(--cache-dir "$WORK/cache" --self-check)
else
  DATA_DIR="${COUPLE_REALDATA_DIR:-}"
  if [ -z "$DATA_DIR" ] || [ ! -d "$DATA_DIR" ]; then
    {
      echo "realdata-sweep: COUPLE_REALDATA_DIR is not set (or is not a directory)."
      echo "Point it at the parent directory of your real sessions and re-run:"
      echo "  COUPLE_REALDATA_DIR=/path/to/sessions scripts/realdata-sweep.sh"
      echo "Or verify the tool without real data:  scripts/realdata-sweep.sh --self-check"
    } >&2
    exit 2
  fi
fi

# Frozen §9.4 regression suites — only meaningful against the canonical schumann pair;
# recorded as NOT RUN otherwise. Suite FAILURES are reported, they do not abort the sweep.
SUITES="$WORK/suites.txt"
: > "$SUITES"
if [ "$SELF_CHECK" = 0 ] && [ -f "$DATA_DIR/schumann-video.mp4" ] && [ -f "$DATA_DIR/schumann-audio.wav" ]; then
  export COUPLE_REALDATA_DIR="$DATA_DIR"
  run_suite() {
    local name="$1"; shift
    if cargo test --release "$@" > "$WORK/$name.log" 2>&1; then
      echo "$name PASS" >> "$SUITES"
    else
      echo "$name FAIL — log tail:" >> "$SUITES"
      tail -n 12 "$WORK/$name.log" | sed 's/^/    /' >> "$SUITES"
    fi
  }
  if [ -f "$DATA_DIR/peaks/external.json" ] && [ -f "$DATA_DIR/peaks/camera.json" ]; then
    run_suite realdata_import -p couple-core --test realdata_import -- --include-ignored
  else
    echo "realdata_import NOT-RUN (peaks/*.json absent under COUPLE_REALDATA_DIR)" >> "$SUITES"
  fi
  run_suite realdata_sync -p couple-core --test realdata_sync -- --include-ignored
  run_suite resolve_realdata -p couple-core --test resolve -- --include-ignored --exact realdata_pair_confident_row
  run_suite pair_gates_realdata -p couple-cli --test pair_gates -- --include-ignored --exact realdata_pair_confident_row
else
  echo "NOT-RUN (schumann pair not present in this directory)" >> "$SUITES"
fi

python3 "$ROOT/scripts/realdata_sweep.py" \
  --bin "$BIN" --data-dir "$DATA_DIR" --out-md "$OUT_MD" --suites "$SUITES" \
  ${PY_ARGS[@]+"${PY_ARGS[@]}"}
