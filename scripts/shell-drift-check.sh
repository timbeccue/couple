#!/usr/bin/env bash
# Shell drift check: warns when a change touches one shell's UI (SwiftUI app
# or Electron renderer) without touching the other shell's counterpart or
# SHELL-UI-PARITY.md. Non-blocking: it always exits 0 and only reports.
#
#   scripts/shell-drift-check.sh <base-ref> [<head-ref>]   (head defaults to HEAD)
#
# Example, local branch vs main:  scripts/shell-drift-check.sh main
#
# Why: decisions live in the Rust core and are shared, but the two UIs are
# separate code (PLAN-CORE-SHELL-BOUNDARY §1, SHELL-UI-PARITY.md). This turns
# "remember to match the other shell" into a visible reminder.
#
# Quiet when any of these hold for the range:
#   - both sides of a pair changed, or neither did
#   - SHELL-UI-PARITY.md changed (someone audited parity explicitly)
#   - a commit message carries the trailer "Shell-parity: n/a <reason>"
#     (an intentionally one-sided change, e.g. a shell-specific bug fix)
#
# Not counted: tests, dev-harness files, and CSS (chrome stays
# shell-idiomatic, SPEC §13.3; only behavior and presence must match).
set -euo pipefail

base="${1:?usage: shell-drift-check.sh <base-ref> [<head-ref>]}"
head="${2:-HEAD}"

if ! git rev-parse --verify --quiet "$base^{commit}" >/dev/null; then
    echo "shell-drift-check: base '$base' not found; skipping."
    exit 0
fi

changed="$(git diff --name-only "$base...$head")"

# Pair definitions: name | mac-side regex | electron-side regex
pairs=(
    "UI|^mac/Sources/Couple/|^electron/src/(main|renderer/src/(app|views))/|^electron/src/renderer/src/[^/]+\.(ts|tsx)$"
    "Kit (FFI binding + policy mirrors)|^mac/Sources/CoupleKit/|^electron/src/renderer/src/kit/"
)
excluded='(\.test\.tsx?$|\.css$|^mac/Sources/Couple/DebugDrive\.swift$|^electron/src/renderer/src/app/harness\.ts$)'

counted="$(printf '%s\n' "$changed" | grep -Ev "$excluded" || true)"

if printf '%s\n' "$changed" | grep -qx 'SHELL-UI-PARITY.md'; then
    echo "shell-drift-check: SHELL-UI-PARITY.md changed in range; ok."
    exit 0
fi
if git log --format=%B "$base..$head" | grep -qiE '^Shell-parity:[[:space:]]*n/?a'; then
    echo "shell-drift-check: 'Shell-parity: n/a' trailer present; ok."
    exit 0
fi

warnings=0
report=""
for pair in "${pairs[@]}"; do
    name="${pair%%|*}"
    rest="${pair#*|}"
    mac_re="${rest%%|*}"
    el_re="${rest#*|}"
    mac_files="$(printf '%s\n' "$counted" | grep -E "$mac_re" || true)"
    el_files="$(printf '%s\n' "$counted" | grep -E "$el_re" || true)"

    if [ -n "$mac_files" ] && [ -z "$el_files" ]; then
        only="Swift"; other="Electron"; files="$mac_files"
    elif [ -z "$mac_files" ] && [ -n "$el_files" ]; then
        only="Electron"; other="Swift"; files="$el_files"
    else
        continue
    fi

    warnings=$((warnings + 1))
    msg="$name changed in the $only shell only. Mirror it in the $other shell, update SHELL-UI-PARITY.md, or add 'Shell-parity: n/a <reason>' to a commit message."
    if [ -n "${GITHUB_ACTIONS:-}" ]; then
        echo "::warning title=Shell drift ($name)::$msg"
    fi
    echo "WARNING: $msg"
    printf '%s\n' "$files" | sed 's/^/    /'
    report+="### Shell drift: $name"$'\n\n'"$msg"$'\n\n'
    # shellcheck disable=SC2016 # literal backticks: markdown code spans
    report+="$(printf '%s\n' "$files" | sed 's/^/- `/; s/$/`/')"$'\n\n'
done

if [ "$warnings" -eq 0 ]; then
    echo "shell-drift-check: no one-sided shell changes."
elif [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf '%s' "$report" >> "$GITHUB_STEP_SUMMARY"
fi
exit 0
