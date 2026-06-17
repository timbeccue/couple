#!/usr/bin/env bash
# Golden capture/compare for `couple pair` resolution DECISIONS — Phase 0 of
# PLAN-CORE-SHELL-BOUNDARY.md. Freezes the machine-stable decision projection of the resolved
# rows (state / selection / alternates / clusters / triangle simultaneity / overflow / note /
# unused audio) over deterministic fixed-seed sessions, so the core↔shell relocation can be
# proven to change NO resolution decision.
#
# Raw matcher floats (z / score / lagMs / coverage) are intentionally EXCLUDED: they are AAC /
# ffmpeg-version dependent (the repo's cli/tests/pair_gates.rs uses lag tolerances for exactly
# this reason). This golden freezes WHAT the resolver decides, not the measurement bytes. If a
# future ffmpeg/codec change shifts a decision, recapture and review the diff.
#
# Usage:
#   scripts/golden-pair-rows.sh                        # compare live output to committed goldens
#   COUPLE_UPDATE_GOLDEN=1 scripts/golden-pair-rows.sh # (re)capture the committed goldens
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(pwd)"
SPECS="$ROOT/fixtures/golden/specs"
GOLDEN="$ROOT/fixtures/golden/rows"
mkdir -p "$GOLDEN"

cargo build -q -p couple-cli
BIN="$ROOT/target/debug/couple"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# name | spec file | extra `couple pair` args
# Covers every row state: confident, twoSources, split, partial, ambiguous, noMatch (and the
# candidateOverflow honesty path). The ambiguous + overflow scenarios drive the legacy z-margin /
# resource-ceiling paths via legacy-only constants (mirrors cli/tests/pair_gates.rs).
SCENARIOS=(
  "continuous|continuous.json|"
  "simultaneous|simultaneous.json|"
  "restart-legacy|restart.json|--matcher legacy"
  "ambiguous-legacy|ambiguous.json|--matcher legacy --params triangle-tol-ms=0.000001 --params z-margin=1000"
  "overflow-legacy|overflow.json|--matcher legacy --params verify-active=1 --params max-verify-pairs-per-video=4"
)

# Reads `couple pair --json` on stdin, prints the machine-stable decision projection. Uses
# `python3 -c` so the DATA arrives on stdin (a `python3 - <<HEREDOC` would feed the program there).
project() {
  python3 -c '
import sys, json
d = json.load(sys.stdin)
rows = []
for r in d.get("rows", []):
    sel = r.get("selected") or {}
    res = r.get("resolution", {})
    rows.append({
        "video": r["video"],
        "state": r["state"],
        "chip": r["chip"],
        "selected": sel.get("audio"),
        "alternates": [a["audio"] for a in r.get("alternates", [])],
        "clusters": res.get("clusters"),
        "note": res.get("note"),
        "candidateOverflow": res.get("candidateOverflow"),
        "triangles": [
            {"audioA": t["audioA"], "audioB": t["audioB"], "simultaneous": t["simultaneous"]}
            for t in res.get("triangles", [])
        ],
    })
print(json.dumps({"rows": rows, "unusedAudio": d.get("unusedAudio", [])},
                 indent=2, sort_keys=True))
'
}

fail=0
for entry in "${SCENARIOS[@]}"; do
  IFS='|' read -r name spec extra <<< "$entry"
  sess="$WORK/$name"; cache="$WORK/$name-cache"
  "$BIN" --quiet fixture session --out "$sess" --spec "$SPECS/$spec" >/dev/null
  # shellcheck disable=SC2086
  live="$("$BIN" --json --quiet --cache-dir "$cache" pair "$sess" $extra | project)"
  gfile="$GOLDEN/$name.json"
  if [ "${COUPLE_UPDATE_GOLDEN:-0}" = "1" ]; then
    printf '%s\n' "$live" > "$gfile"
    echo "captured $name -> ${gfile#"$ROOT"/}"
  elif [ ! -f "$gfile" ]; then
    echo "MISSING golden for $name — run: COUPLE_UPDATE_GOLDEN=1 $0"; fail=1
  elif diff -u "$gfile" <(printf '%s\n' "$live") >/dev/null; then
    echo "ok    $name"
  else
    echo "DRIFT $name:"; diff -u "$gfile" <(printf '%s\n' "$live") || true; fail=1
  fi
done
[ "$fail" = 0 ] && echo "golden-pair-rows: all scenarios match" || echo "golden-pair-rows: DRIFT detected"
exit $fail
