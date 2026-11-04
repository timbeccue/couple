#!/usr/bin/env bash
# Shell drift check: warns when a change touches one shell's views (SwiftUI
# app or Electron renderer) without touching the other shell's views or
# SHELL-UI-PARITY.md. Non-blocking: it always exits 0 and only reports.
#
#   scripts/shell-drift-check.sh <base-ref> [<head-ref>]   (head defaults to HEAD)
#
# Example, local branch vs main:  scripts/shell-drift-check.sh main
# Runs in CI (the shell-drift job) and before each push once
# scripts/install-hooks.sh has been run.
#
# Why: decisions live in the Rust core and are shared, but the two UIs are
# separate code (PLAN-CORE-SHELL-BOUNDARY §1, SHELL-UI-PARITY.md). This turns
# "remember to match the other shell" into a visible reminder.
#
# Scope: only the view folders count. Replaying history since the Electron
# shell landed, wider groupings (models, controllers, the FFI kits) warned
# almost only on internal refactors, while user-facing affordances nearly
# always touch a view. A behavior added without any view change is missed.
#
# Quiet when any of these hold for the WHOLE range (not per pair/commit):
#   - both sides of a pair changed, or neither did
#   - SHELL-UI-PARITY.md changed (someone audited parity explicitly)
#   - a commit message carries the trailer "Shell-parity: n/a <reason>"
#     (an intentionally one-sided change, e.g. a shell-specific bug fix)
#
# Not counted: tests and CSS (chrome stays shell-idiomatic, SPEC §13.3; only
# behavior and presence must match). Renames are listed by destination path
# only, so moving a file out of a views folder does not count as a change.
set -euo pipefail

base="${1:?usage: shell-drift-check.sh <base-ref> [<head-ref>]}"
head="${2:-HEAD}"

for ref in "$base" "$head"; do
    if ! git rev-parse --verify --quiet "$ref^{commit}" >/dev/null; then
        echo "shell-drift-check: '$ref' not found; skipping."
        exit 0
    fi
done

if ! changed="$(git diff --name-only "$base...$head" 2>/dev/null)"; then
    echo "shell-drift-check: no merge base for $base...$head; skipping."
    exit 0
fi

# Pairs, as parallel arrays: display name, mac-side regex, electron-side regex.
pair_names=("Views")
pair_mac=('^mac/Sources/Couple/Views/')
pair_electron=('^electron/src/renderer/src/views/')
excluded='(\.test\.tsx?$|\.css$)'

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
for i in "${!pair_names[@]}"; do
    name="${pair_names[$i]}"
    mac_files="$(printf '%s\n' "$counted" | grep -E "${pair_mac[$i]}" || true)"
    el_files="$(printf '%s\n' "$counted" | grep -E "${pair_electron[$i]}" || true)"

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
    # shellcheck disable=SC2016 # literal backticks: markdown code spans
    report+="### Shell drift: $name"$'\n\n'"$msg"$'\n\n'"$(printf '%s\n' "$files" | sed 's/^/- `/; s/$/`/')"$'\n\n'
done

if [ "$warnings" -eq 0 ]; then
    echo "shell-drift-check: no one-sided shell changes."
elif [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf '%s' "$report" >> "$GITHUB_STEP_SUMMARY"
fi
exit 0
