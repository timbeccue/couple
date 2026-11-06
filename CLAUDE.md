# Couple — session orientation

macOS app (SwiftUI, macOS 14+) over a shared Rust core. It pairs and syncs two
independent video recordings of the same event by matching their audio: fingerprint
the audio, find the time offset, verify the match, resolve clips into synced pairs,
export. The same engine ships as a CLI (`couple`) for batch runs and testing.

## Doc map — which file answers what

Read the relevant doc before re-deriving behavior from code. Code comments cite these
by name and section (e.g. `// SPEC §5.6`, `// PLAN-MATCH-CLASSIFIER §13.4`) — do not
rename them.

- `SPEC.md` — **authority for the engine and v1 product**: sync algorithm (envelope
  NCC, drift fit), core↔shell typed API + C ABI, file formats/caches, import/export,
  test tolerances and perf budgets, CLI, v1 editor semantics. Not superseded.
- `SPEC-V2.md` — **delta only**: the queue/pairing redesign (candidate matrix,
  coverage, triangle closure, media pool, batch export). Where it conflicts with
  SPEC.md it wins for v2 scope; it supersedes only SPEC.md §12 and the
  "single-pair-editor is the product" framing. Everything else in SPEC.md stands.
- `UX-DECISIONS.md` — append-only UX decision ledger; standing authority for shell
  behavior. Latest entries win (`REVISED` markers).
- `PROGRESS.md` — session log since 2026-06-15 **and the binding `SPEC-CHANGE:`
  ledger**. Earlier entries: `docs/archive/PROGRESS-2026-06-11-to-14.md` (its
  SPEC-CHANGE entries remain binding — search both).
- `TODO.md` — active work items (TODO-nnn) plus agent instructions.
- `SHELL-UI-PARITY.md` — the per-surface checklist of REQUIRED shell
  affordances (created 2026-07-04 after Electron-port drift); each line cites
  the authoritative Swift view. Any shell change that adds/removes a
  user-facing affordance updates it in the same change.
- `PLAN-CORE-SHELL-BOUNDARY.md` — core/shell boundary rule ("decision lives in Rust
  core; presentation lives in the shell"). Phases A–F shipped; Phase G (new
  shells) started 2026-07-04 via the `electron/` shell. Stays in root while
  Phase G is in progress.
- `docs/LICENSING.md` — third-party licensing record: the ffmpeg LGPL rule
  (what a distributable build may contain, how `fetch-ffmpeg.sh` enforces it),
  the obligations each release must meet, Electron bundling rules, and the
  dependency-notice pipeline (`scripts/third-party-notices.sh` →
  `third-party/` → bundle `Licenses/` → Acknowledgements view).
- `docs/archive/` — shipped plans, historical only; kept because code cites them
  (latest: `PLAN-COMPACT-MODE.md`, 2026-11-05 — compact arrangement +
  include-for-export model; its decisions are the SPEC-CHANGE entries of that
  date and UX-DECISIONS §V2.11/§V2.12).
- `fixtures/matcher-corpus/CALIBRATION.md` — derivation of frozen matcher
  thresholds. §1–5 are self-marked superseded; §6–7 (profile 2) is the live
  reference. Gate *enforcement* is in code (`matching_evidence.rs`).

## Layout

- `core/` — Rust `couple-core`: all domain logic — `fingerprint/`, `matching.rs`,
  `sync.rs`, `verify.rs`, `resolve.rs`, and `policy/` (pure decision layer,
  proptest-covered).
- `ffi/` — `couple-ffi`: C ABI staticlib (`libcouple.a` + cbindgen `couple.h`);
  also crate-type rlib so `electron/native` consumes the same `cpl_*` surface as
  a typed dependency.
- `cli/` — `couple-cli`, binary `couple`.
- `electron/` — cross-platform Electron shell (Phase G, started 2026-07-04):
  `native/` = `couple-node` napi binding (thin Engine/Wire port + cpal audition
  pump; the only Rust in the shell), `src/renderer/src/{kit,app,views}` mirror
  the CoupleKit/app split, `scripts/parity.mjs` = the PLAN-CORE-SHELL-BOUNDARY
  §9 gate (kit rows byte-identical to CLI rows; runs in CI on all 3 OSes).
  Engine lives in the renderer (nodeIntegration; local content only). Commands
  from `electron/`: `npm run build:native | smoke | parity | test | dev | typecheck`;
  package via `npx electron-builder`. Harness env: `COUPLE_OPEN_DIR`,
  `COUPLE_SHOT_PATH`/`COUPLE_SHOT_DELAY_MS`, `COUPLE_AUTOQUIT_MS`, `COUPLE_HUD`,
  `COUPLE_USER_DATA` (persistence root override), plus a `COUPLE_*_TEST` family
  (PLAY, EXPORT, NUDGE, ANCHOR, PERSIST, RESTORE, UNDO, QUALITY, REMOVE,
  POOL_PREVIEW, POOL_INFO, ACK = open Acknowledgements at startup, COMPACT,
  LAYOUT_TOGGLE = the compact-arrangement cases; `COUPLE_SHOT_DIR` for their
  shots) mirroring the mac harness idiom. Any `COUPLE_*_TEST` forces the full
  arrangement and disables layout persistence; COMPACT/LAYOUT_TOGGLE force compact.
- `mac/` — Swift package: `CoupleKit` (FFI binding + policy mirrors), `Couple`
  (SwiftUI app), `CCouple` (C shim), `Tests/`. Swift mirrors Rust policies
  file-for-file (`RowMergePolicy.swift` ↔ `policy/row_merge.rs`, etc.); the Rust
  side is the authority.
- `third-party/` — generated dependency license notices (`components.json`,
  `rust-crates.txt`, `npm-packages.txt`, `swift-packages.txt`), committed;
  regenerate with `scripts/third-party-notices.sh` after a `Cargo.lock` or
  `package-lock.json` change. Both bundles copy it to their `Licenses/` folder.
- `scripts/`, `fixtures/` (test corpora; media gitignored, manifests tracked),
  `example_data/` (owner-local media, optional), `design/` (mockups v1+v2 — both
  live; v2 is queue-scope, not a replacement; `mockups-compact/` = the 2026-11-04
  compact-mode prototypes, direction D chosen — serve with `.claude/launch.json`
  "mockups" or `python3 -m http.server 8742` from the repo root; `logo/` = final logo SVGs + `Couple.icns`
  + the wordmark font (Nunito, OFL, in `logo/fonts/`), regen per
  `design/logo/README.md` — `outline-uple.py` writes both wordmark SVGs and
  `mac/.../CoupleWordmarkGlyphs.swift` (generated; the app draws the lockup natively,
  no font bundled); the app icon ships from `mac/Resources/AppIcon.icns`, wired in
  `make-app.sh`, and for Electron from `electron/build/icon.{icns,ico,png}`
  (electron-builder's default icon dir; regen commands in the logo README);
  the README header uses the wordmark SVGs via `<picture>`).
- `README.md` — public landing for the GitHub repo: wordmark header, Electron
  download links (`releases/latest/download/...`), first-launch caveats,
  requirements, release how-to.
- `website/` — product marketing page (`index.html`, static; only external dep is
  Google Fonts). Kept from the 2026-07-04 prototype round (the "product frame"
  variant; other prototypes discarded). Preview: `python3 -m http.server 8642`
  from `website/`.

## Build & test (CI is source of truth: `.github/workflows/ci.yml`; needs ffmpeg)

- `cargo test --release --workspace` — integration suites require `--release`.
- `scripts/golden-pair-rows.sh` — resolver-decision golden diff vs `fixtures/golden/rows/` (CI runs it in the test job). Intended decision change: `COUPLE_UPDATE_GOLDEN=1 scripts/golden-pair-rows.sh`, review diff, commit.
- `scripts/third-party-notices.sh [--check]` — regenerate `third-party/` from
  `cargo metadata` + `npm ls` (needs `electron/node_modules`); fails on an
  unknown or copyleft license (allowed set in `scripts/third_party_notices.py`;
  rule in `docs/LICENSING.md` §3). `--check` fails if the committed output is
  stale; `make-app.sh --dist` runs it.
- `scripts/shell-drift-check.sh <base-ref>` — warn-only (CI `shell-drift` job, and
  pre-push once `scripts/install-hooks.sh` has run): flags a change that touches
  one shell's views (`mac/Sources/Couple/Views/` vs
  `electron/src/renderer/src/views/`) without the other or `SHELL-UI-PARITY.md`.
  Opt out for an intentionally one-sided change with a
  `Shell-parity: n/a <reason>` line in a commit message.
- `cargo build --release -p couple-ffi` — must run **before** any swift build.
- `cd mac && swift build -c release` — ship variant (DebugDrive stubbed).
- `cd mac && swift build -c release -Xswiftc -DCOUPLE_DEVHARNESS` — dev harness.
- `cd mac && Scripts/make-app.sh [--dist] [--dmg]` — assemble `mac/dist/Couple.app`.
  `--dist` builds and bundles the pinned **LGPL** static ffmpeg via
  `Scripts/fetch-ffmpeg.sh` (compiled from the sha256+GPG-verified ffmpeg
  source tarball — fetched from this repo's `ffmpeg-source-<ver>` prerelease
  first, ffmpeg.org second; signing key committed next to the script; in CI
  the mirror is required — with no external libs; otool-, launch- and LGPL-verified; license
  texts + `SOURCE.txt` land in `Contents/Resources/Licenses/ffmpeg/`, the
  generated `third-party/` notices next to them in `Licenses/`; cache
  `mac/vendor/`, gitignored; first run compiles for a few minutes). The only
  variant fit for distribution — obligations in `docs/LICENSING.md` (attach
  the source tarball to each public release). Default dev build copies
  `COUPLE_FFMPEG`/`$PATH` ffmpeg (a GPL-configured Homebrew build), labels the
  bundle a dev build, and warns if it links non-system dylibs (a Homebrew copy
  breaks on `brew upgrade` — the 2026-11-01 "unsupported format" failure).
- `cd mac && swift test`
- `scripts/side-by-side.sh [ref] [--open <dir>] [--mac-only|--electron-only] [--no-tile] [--fresh]`
  — build and launch the mac dev-harness binary and `electron-vite dev` from one
  checkout, tiled side by side, for manual parity checks. No ref = this checkout
  as on disk; a ref checked out in any worktree uses that worktree; any other
  ref gets a reusable detached worktree under `~/.couple-compare/<slug>/`
  (shared cargo `target/` there). `--open` preloads the same folder into both
  (`COUPLE_PAIR_DIR` / `COUPLE_OPEN_DIR`; both shells then start in the full
  arrangement and skip layout persistence — without it, both start compact).
  Ctrl-C quits both; logs in `$TMPDIR/couple-side-by-side/`. From a Claude
  session: `/side-by-side [args]` (project skill, `.claude/skills/side-by-side/`).
- Release (Electron installers → DRAFT GitHub Release; `.github/workflows/release.yml`):
  fires ONLY on a pushed `v*` tag (`git tag v0.1.0 && git push origin v0.1.0`),
  never on ordinary pushes; the draft is published by hand. The tag is the
  version (stamped into `electron/package.json` at build time). Installer names
  are versionless (`Couple-mac-arm64.dmg` etc., `artifactName` in
  `electron/electron-builder.yml`) so `releases/latest/download/<name>` links in
  `README.md` stay valid. Every installer bundles ffmpeg + ffprobe: macOS via
  `mac/Scripts/fetch-ffmpeg.sh` (whole helper dir incl. `SOURCE.txt` +
  `licenses/`); Windows/Linux via
  `electron/scripts/fetch-ffmpeg.mjs [--platform win32|linux]` — one pinned
  month-end BtbN FFmpeg-Builds `-lgpl` release (asset + sha256 per platform,
  LGPL v3 via `--enable-version3`; configure-line, `-version` and `-L` checks)
  laid out as `electron/resources/ffmpeg/<platform>/{ffmpeg,ffprobe,SOURCE.txt,licenses/}`;
  cache `electron/vendor/`, gitignored; a non-host `--platform` downloads and
  verifies without the run checks, which is how to exercise both pins from a Mac.
  `workflow_dispatch` from a branch = dry run (artifacts only, no release).
- ROC gate (weekly/on-demand, plus auto on push/PR touching matcher paths — .github/workflows/roc-gate.yml):
  `cargo test --release -p couple-core --test matching_evidence -- --ignored synthetic_corpus_roc_gates_hold`
- Real data (owner-only): `COUPLE_REALDATA_DIR=<sessions> scripts/realdata-sweep.sh`
  — one-command calibration sweep + report (`--self-check` = synthetic dry run).
  The realdata suites are `#[ignore]`d ("SKIPPED (no realdata)" in default
  output); the sweep runs them via `--include-ignored`.
- Known wart: after adding Swift test seams, `swift test` can throw a spurious link
  error while `swift build` passes — fix with `rm -rf mac/.build/*/debug`.

## Flags & env

- Match classifier ("verify"): Rust `verify_active` defaults **off** in
  `resolve.rs`; the app turns it on, CLI opt-in via `--params verify-active=1` or
  `COUPLE_VERIFY_ACTIVE`.
- Seeded auto-sync (§11.3): the app seeds placement from resolver evidence / manual
  rescue automatically (`policy/sync_seed.rs` decides); CLI opt-in via
  `couple sync --seed-lag-ms=<ms>`. Unseeded behavior is byte-identical.
- `COUPLE_DEVHARNESS` (env) + `-DCOUPLE_DEVHARNESS` (compile flag) — dev harness.
- Mac window arrangement (UX §V2.11, PLAN-COMPACT-MODE Phase 4): **compact** or
  **full**, toggled by the ⤢ Full / ⤡ Compact button in the window's top-right corner
  (`Views/ArrangementToggle.swift`, a RootView overlay in both arrangements;
  UX §V2.14) or View ▸ Compact Window / Full Window (⌘⇧C). Persisted in UserDefaults
  `com.timbeccue.couple.layoutMode` (absent ⇒ **compact**) plus each
  arrangement's last frame (`com.timbeccue.couple.windowFrame.{compact,full}`);
  compact width is clamped 340–420, height min 560. Every dev-harness case forces
  **full** (and never writes these defaults) except `COUPLE_COMPACT_TEST` (with
  `COUPLE_PAIR_DIR` [+ `COUPLE_SHOT_DIR`]: clamps + compact-{empty,synced,noaudio,
  pick,exporting} shots) and `COUPLE_LAYOUT_TOGGLE_TEST` (with `COUPLE_PAIR_DIR`:
  toggles while playing; selection, playhead, inclusion, per-arrangement frames
  and the Pan→Synced→Pan monitor round-trip), which start compact. Other Phase
  1–3 keys: `COUPLE_INCLUDE_TEST`, `COUPLE_CAND_AUDITION_TEST`.
- `COUPLE_LICENSES_DIR` — overrides where the Acknowledgements view looks for
  the Licenses folder (both shells; a bare `swift build` run has no bundle —
  point it at `third-party/`). Electron main also sets it, plus
  `COUPLE_FFMPEG_LICENSE_DIR` and `COUPLE_APP_VERSION`, for the renderer.
- `COUPLE_FFMPEG` (ffmpeg path), `COUPLE_MATCHER`/`COUPLE_SHADOW_MATCHER`,
  `COUPLE_REALDATA_DIR`, `COUPLE_UPDATE_GOLDEN`, plus a `COUPLE_*_TEST` /
  `COUPLE_*_SHOT_DIR` family driving the UI screenshot harness.

## Conventions

- New behavior decisions go to the Rust `core/src/policy/` layer, never Swift-only;
  shells present, they don't decide (PLAN-CORE-SHELL-BOUNDARY §1).
- Spec deviations are recorded as `SPEC-CHANGE:` entries in PROGRESS.md.
- Shipped plans move to `docs/archive/` (git mv; name-based code citations survive).
- **Keep this file current.** Before ending a session that changed any of the
  following, update the matching section here in the same commit: build/test
  commands, feature flags or env vars, workspace layout, doc roles (a doc created,
  archived, or superseded), or a shipped plan phase. This file is the only
  orientation new sessions get — a stale line here costs every future session.
