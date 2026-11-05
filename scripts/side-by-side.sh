#!/bin/bash
# Run the mac (SwiftUI) and Electron shells side by side from one branch, for
# eyeballing parity (SHELL-UI-PARITY.md) in the real apps.
#
#   scripts/side-by-side.sh                      # this checkout, as it is on disk
#   scripts/side-by-side.sh main                 # any branch / tag / commit
#   scripts/side-by-side.sh feature/x --open ~/sessions/recital-3
#   scripts/side-by-side.sh --mac-only | --electron-only
#   scripts/side-by-side.sh --no-tile            # don't move the windows
#   scripts/side-by-side.sh <ref> --dry-run      # only say which checkout would be used
#
# Branch resolution: no argument = the checkout this script lives in (uncommitted
# changes included). A ref that is already checked out in some worktree (the main
# checkout or a .claude/worktrees/* one) uses that worktree as-is. Anything else
# gets a detached worktree under ~/.couple-compare/<slug>/ that is reused and
# re-pointed at the ref on every run (first build there is a full cargo build;
# its target/ is a symlink into ~/.couple-compare/target so dependency builds
# are shared across refs).
#
# What runs: mac = the dev-harness binary (mac/.build/release/Couple, same as
# Scripts/dev.sh); Electron = `electron-vite dev` (HMR on renderer edits). Both
# use the ffmpeg on $PATH / $COUPLE_FFMPEG. Ctrl-C quits both.
#
# --open <dir> preloads the same folder into both (COUPLE_PAIR_DIR / COUPLE_OPEN_DIR).
#   Caveat: both shells treat a preload as a harness run — they start in the FULL
#   arrangement and don't persist layout. Without --open each starts compact
#   (the real default) and you drop the folder on both.
# The dev mac binary shares ~/Library/Application Support/Couple with the installed
# app (queue + per-pair autosaves), exactly like Scripts/dev.sh. Electron dev state
# lives under ~/Library/Application Support/couple-electron (--fresh = a temp dir).
set -euo pipefail

usage() { sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }

REF=""
OPEN_DIR=""
RUN_MAC=1
RUN_ELECTRON=1
TILE=1
FRESH=0
DRY=0
while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help) usage ;;
    --open) shift; OPEN_DIR="${1:?--open needs a directory}" ;;
    --mac-only) RUN_ELECTRON=0 ;;
    --electron-only) RUN_MAC=0 ;;
    --no-tile) TILE=0 ;;
    --fresh) FRESH=1 ;;
    --dry-run) DRY=1 ;;
    -*) echo "unknown option: $1" >&2; usage 2 ;;
    *) [ -z "$REF" ] || { echo "only one ref allowed" >&2; usage 2; }; REF="$1" ;;
  esac
  shift
done
[ -z "$OPEN_DIR" ] || OPEN_DIR="$(cd "$OPEN_DIR" && pwd)"

export PATH="$HOME/.cargo/bin:$PATH" # rustup install; a bare sh doesn't read the zsh profile
HERE_REPO="$(cd "$(dirname "$0")/.." && pwd)"
COMPARE_HOME="${COUPLE_COMPARE_HOME:-$HOME/.couple-compare}"

say() { printf '\033[1m[side-by-side]\033[0m %s\n' "$*"; }

# ---- 1. resolve the checkout to build ----------------------------------------
resolve_repo() {
  if [ -z "$REF" ]; then
    echo "$HERE_REPO"; return
  fi
  git -C "$HERE_REPO" rev-parse --verify --quiet "$REF^{commit}" >/dev/null \
    || { echo "unknown ref: $REF" >&2; exit 2; }
  # Already checked out somewhere? Use that worktree (its working tree, not just the ref).
  local path=""; local branch=""
  while IFS= read -r line; do
    case "$line" in
      worktree\ *) path="${line#worktree }" ;;
      branch\ *) branch="${line#branch refs/heads/}"
                 if [ "$branch" = "$REF" ]; then echo "$path"; return; fi ;;
    esac
  done < <(git -C "$HERE_REPO" worktree list --porcelain)
  # Otherwise a detached compare worktree, re-pointed at the ref each run.
  local slug; slug="$(printf '%s' "$REF" | tr '/' '-' | tr -c 'A-Za-z0-9._\n-' '_')"
  local dir="$COMPARE_HOME/$slug"
  if [ ! -d "$dir/.git" ] && [ ! -f "$dir/.git" ]; then
    say "creating compare worktree $dir @ $REF" >&2
    mkdir -p "$COMPARE_HOME"
    git -C "$HERE_REPO" worktree add --detach "$dir" "$REF" >&2
  else
    # Tracked-file changes only: the target symlink and build output are untracked.
    if [ -n "$(git -C "$dir" status --porcelain -uno)" ]; then
      echo "compare worktree $dir has local changes; clean it or remove it (git worktree remove --force $dir)" >&2
      exit 2
    fi
    say "updating compare worktree $dir -> $REF" >&2
    git -C "$dir" checkout --quiet --detach "$REF" >&2
  fi
  # Share the cargo dependency builds across compare worktrees. Package.swift and
  # build-native.mjs both resolve <repo>/target by path, so a symlink is enough.
  if [ ! -e "$dir/target" ]; then
    mkdir -p "$COMPARE_HOME/target"
    ln -s "$COMPARE_HOME/target" "$dir/target"
  fi
  echo "$dir"
}

REPO="$(resolve_repo)"
DESCR="$(git -C "$REPO" describe --always --dirty 2>/dev/null || true)"
BRANCH="$(git -C "$REPO" branch --show-current 2>/dev/null || true)"
say "checkout: $REPO (${BRANCH:-detached} $DESCR)"
[ "$DRY" = 0 ] || exit 0

# ---- 2. build (sequentially: both shells' cargo steps share target/) ----------
cd "$REPO"
PKGS="-p couple-ffi"
[ "$RUN_ELECTRON" = 1 ] && PKGS="$PKGS -p couple-node"
say "cargo build --release $PKGS"
# shellcheck disable=SC2086
cargo build --release $PKGS

if [ "$RUN_MAC" = 1 ]; then
  say "swift build (dev harness)"
  (cd mac && swift build -c release -Xswiftc -DCOUPLE_DEVHARNESS)
fi
if [ "$RUN_ELECTRON" = 1 ]; then
  if [ ! -d electron/node_modules ]; then
    say "npm ci (electron/node_modules missing)"
    (cd electron && npm ci --no-audit --no-fund)
  fi
  say "electron native binding"
  (cd electron && node scripts/build-native.mjs)
fi

# ---- 3. launch ----------------------------------------------------------------
LOG_DIR="${TMPDIR:-/tmp}/couple-side-by-side"
mkdir -p "$LOG_DIR"
MAC_PID=0
EL_PID=0 # the `npx electron-vite dev` process; the Electron app itself is its child
ELECTRON_BIN_RE="$REPO/electron/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron( |\$)"
cleanup() {
  trap - INT TERM EXIT
  [ "$MAC_PID" = 0 ] || kill "$MAC_PID" 2>/dev/null || true
  [ "$EL_PID" = 0 ] || kill "$EL_PID" 2>/dev/null || true
  # electron-vite dev spawns Electron as a child; make sure it goes too.
  pkill -f "$ELECTRON_BIN_RE" 2>/dev/null || true
  wait 2>/dev/null || true
}
trap cleanup INT TERM EXIT

if [ "$RUN_MAC" = 1 ]; then
  say "launching mac shell (log: $LOG_DIR/mac.log)"
  (
    export COUPLE_DEVHARNESS=1
    [ -z "$OPEN_DIR" ] || export COUPLE_PAIR_DIR="$OPEN_DIR"
    exec "$REPO/mac/.build/release/Couple"
  ) >"$LOG_DIR/mac.log" 2>&1 &
  MAC_PID=$!
  disown "$MAC_PID" # no "Terminated" job notice when cleanup kills it
fi

if [ "$RUN_ELECTRON" = 1 ]; then
  say "launching electron shell (log: $LOG_DIR/electron.log)"
  (
    cd "$REPO/electron"
    [ -z "$OPEN_DIR" ] || export COUPLE_OPEN_DIR="$OPEN_DIR"
    if [ "$FRESH" = 1 ]; then
      COUPLE_USER_DATA="$(mktemp -d "$LOG_DIR/electron-userdata.XXXXXX")"
      export COUPLE_USER_DATA
    fi
    exec npx electron-vite dev
  ) >"$LOG_DIR/electron.log" 2>&1 &
  EL_PID=$!
  disown "$EL_PID"
fi

alive() { kill -0 "$1" 2>/dev/null; }

# ---- 4. tile: mac on the left, Electron to its right (best effort) -------------
# Needs Accessibility permission for the terminal app (System Settings ▸ Privacy
# & Security ▸ Accessibility). Windows are found by process id, so an installed
# Couple.app or another worktree's build is never touched. Position only — each
# app owns its own size.
electron_app_pid() { pgrep -f "$ELECTRON_BIN_RE" 2>/dev/null | head -n1; }

tile() {
  local el_app=0 i
  for i in $(seq 1 180); do # up to 90 s for both windows
    if [ "$RUN_ELECTRON" = 1 ]; then
      alive "$EL_PID" || { echo "electron exited"; return; }
      [ "$el_app" != 0 ] || el_app="$(electron_app_pid || true)"; el_app="${el_app:-0}"
    fi
    if [ "$RUN_MAC" = 1 ] && ! alive "$MAC_PID"; then echo "mac exited"; return; fi
    # Only once every requested app process exists (electron-vite spawns the
    # Electron app a few seconds after it starts).
    if [ "$RUN_ELECTRON" = 0 ] || [ "$el_app" != 0 ]; then
      local r
      r="$(osascript - "$MAC_PID" "$el_app" <<'EOS' 2>&1
on mainWindow(proc)
  -- Electron and AppKit both own invisible helper windows (~43 px tall);
  -- the real one is the first with a plausible height.
  tell application "System Events"
    repeat with w in windows of proc
      set sz to size of w
      if (item 2 of sz) > 100 then return w
    end repeat
  end tell
  return missing value
end mainWindow

on run argv
  set macPid to (item 1 of argv) as integer
  set elPid to (item 2 of argv) as integer
  tell application "System Events"
    if macPid is not 0 then
      set macProcs to (processes whose unix id is macPid)
      if (count of macProcs) is 0 then return "waiting"
    end if
    if elPid is not 0 then
      set elProcs to (processes whose unix id is elPid)
      if (count of elProcs) is 0 then return "waiting"
    end if
    -- Re-place until both windows hold position: each shell applies its own
    -- saved frame some time after its window appears (undoing an early move),
    -- and System Events only sees windows on the current Space. Stops once both
    -- have sat at the target for two consecutive checks (~3 s), or after ~20 s.
    set macDone to macPid is 0
    set elDone to elPid is 0
    set macW to 360
    set quiet to 0
    set trace to ""
    repeat with pass from 1 to 14
      delay 1.5
      set moved to false
      if macPid is not 0 then
        try
          set w to my mainWindow(item 1 of macProcs)
          if w is missing value then error "no window yet"
          set sz to size of w
          set macW to item 1 of sz
          set pos to position of w
          set trace to trace & pass & ": mac " & (item 1 of pos) & "," & (item 2 of pos)
          if (item 1 of pos) is not 12 or (item 2 of pos) > 48 then
            set position of w to {12, 40}
            set moved to true
            set trace to trace & " -> 12,40"
          end if
          set macDone to true
        end try
      end if
      if elPid is not 0 then
        try
          set w to my mainWindow(item 1 of elProcs)
          if w is missing value then error "no window yet"
          set pos to position of w
          set targetX to 12 + macW + 12
          set trace to trace & "  el " & (item 1 of pos) & "," & (item 2 of pos)
          if (item 1 of pos) is not targetX or (item 2 of pos) > 48 then
            set position of w to {targetX, 40}
            set moved to true
            set trace to trace & " -> " & targetX & ",40"
          end if
          set elDone to true
        end try
      end if
      set trace to trace & linefeed
      if macDone and elDone and not moved then
        set quiet to quiet + 1
        if quiet >= 2 then exit repeat
      else
        set quiet to 0
      end if
    end repeat
    if not (macDone and elDone) then return "a window never showed up on the current Space" & linefeed & trace
  end tell
  return "tiled" & linefeed & trace
end run
EOS
)"
      case "$r" in
        waiting) ;;
        *) printf '%s\n' "$r" | tail -n +2 >"$LOG_DIR/tile.log"
           printf '%s\n' "$r" | head -n1; return ;;
      esac
    fi
    sleep 0.5
  done
  echo "windows not up after 90 s"
}
if [ "$TILE" = 1 ]; then
  result="$(tile)"
  case "$result" in
    tiled) say "windows tiled" ;;
    *exited) say "$result before both windows were up — see $LOG_DIR/" ;;
    *) say "not tiled: ${result:-osascript failed} (grant Accessibility to your terminal, or use --no-tile)" ;;
  esac
fi

say "running — Ctrl-C quits both. Logs in $LOG_DIR/"
# Exit when the first shell exits (quitting one app ends the comparison).
# (macOS bash 3.2 has no `wait -n`, hence the poll.)
while :; do
  if [ "$MAC_PID" != 0 ] && ! alive "$MAC_PID"; then say "mac shell exited; quitting electron"; exit 0; fi
  if [ "$EL_PID" != 0 ] && ! alive "$EL_PID"; then say "electron exited; quitting mac shell"; exit 0; fi
  sleep 1
done
