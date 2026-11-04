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
- `docs/archive/` — shipped plans, historical only; kept because code cites them.
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
  POOL_PREVIEW, POOL_INFO, ACK = open Acknowledgements at startup) mirroring
  the mac harness idiom.
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
  live; v2 is queue-scope, not a replacement).
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
  `Scripts/fetch-ffmpeg.sh` (compiled from the sha256+GPG-verified ffmpeg.org
  tarball with no external libs; otool-, launch- and LGPL-verified; license
  texts + `SOURCE.txt` land in `Contents/Resources/Licenses/ffmpeg/`, the
  generated `third-party/` notices next to them in `Licenses/`; cache
  `mac/vendor/`, gitignored; first run compiles for a few minutes). The only
  variant fit for distribution — obligations in `docs/LICENSING.md` (attach
  the source tarball to each public release). Default dev build copies
  `COUPLE_FFMPEG`/`$PATH` ffmpeg (a GPL-configured Homebrew build), labels the
  bundle a dev build, and warns if it links non-system dylibs (a Homebrew copy
  breaks on `brew upgrade` — the 2026-11-01 "unsupported format" failure).
- `cd mac && swift test`
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
