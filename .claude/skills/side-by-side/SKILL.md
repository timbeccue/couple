---
name: side-by-side
description: Build and launch the mac and Electron shells of Couple next to each other from this session's checkout (or a given ref) so Tim can eyeball shell parity in the real apps. Use when asked to run, show, compare or verify the mac and Electron apps side by side, or to "run both shells".
---

# Side-by-side shells

Runs `scripts/side-by-side.sh`, which builds the mac dev-harness binary and the
Electron dev shell from one checkout, launches both, and tiles them next to each
other. Ctrl-C (or quitting either app) ends both.

## How to run it

1. Launch it in the user's Terminal panel with `run_in_terminal`, never through
   the Bash tool: apps started from the Bash tool get no real window.
   From this session's checkout (its own branch, uncommitted edits included):

   ```
   scripts/side-by-side.sh $ARGUMENTS
   ```

   Arguments pass straight through. Useful ones:
   - `<ref>` — any branch, tag or commit instead of this checkout. A ref already
     checked out in some worktree is used as-is; others get a reusable detached
     worktree under `~/.couple-compare/<slug>/` (first build there is a full
     cargo build plus `npm ci`, several minutes).
   - `--open <dir>` — preload the same session folder into both shells. Both
     then start in the FULL arrangement with layout persistence off (harness
     semantics of `COUPLE_PAIR_DIR` / `COUPLE_OPEN_DIR`). Without it both start
     compact, the real default, and the user drops a folder on each.
   - `--mac-only` / `--electron-only`, `--no-tile`, `--fresh` (temp Electron
     user data), `--dry-run` (only print which checkout would be used).
2. Read the tab with `read_terminal` until it prints `windows tiled` (or a
   `not tiled:` note) and `running — Ctrl-C quits both`. The first build in a
   fresh worktree takes minutes; later runs are incremental.
3. Tell the user both shells are up and which checkout they came from (the
   script's first `checkout:` line). Leave the tab running; the user drives the
   apps. Stop it with `stop_terminal_tab` only when they are done.

## If something goes wrong

- Logs: `$TMPDIR/couple-side-by-side/{mac,electron,tile}.log`.
- `not tiled: … Accessibility` — the terminal app lacks Accessibility
  permission (System Settings ▸ Privacy & Security ▸ Accessibility). The apps
  are still up, just not moved; say so and move on.
- `electron exited; quitting mac shell` right after launch — read
  `electron.log`; an `Electron uninstall` error means the Electron binary is
  missing (npm's install-script policy skipped electron's postinstall);
  `electron/scripts/build-native.mjs` fetches it, so re-run.
- The script is not on branches that predate it (added 2026-11-05 on
  `chore/side-by-side-script`); on such a branch run it from a checkout that
  has it, passing the branch as `<ref>`.

The dev mac binary shares `~/Library/Application Support/Couple` (queue and
per-pair autosaves) with the installed app, exactly like `mac/Scripts/dev.sh`.
