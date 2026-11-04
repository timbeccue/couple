# Licensing — third-party code shipped with Couple

What Couple ships that it did not write, under which license, what that obliges
us to do, and where the build enforces it. Engineering record, not legal advice.
Code cites this file by name and section (`// docs/LICENSING §1`); do not rename.

## 1. ffmpeg / ffprobe — LGPL v2.1 or later

**How Couple uses ffmpeg.** As two separate executables (`ffmpeg`, `ffprobe`)
launched by the Rust core over command-line arguments and pipes
(`core/src/export.rs`, the decode path). Nothing in Couple links `libav*`.
Couple is therefore not a derivative work of ffmpeg, and the LGPL's
"user must be able to relink" clause (LGPL 2.1 §6) is satisfied trivially: a
user can replace the executable in `Contents/Helpers/`.

**Why an LGPL-only build is enough.** ffmpeg is LGPL unless configured with
`--enable-gpl` (x264, x265, some filters) or `--enable-nonfree` (Fraunhofer
AAC and friends; makes the binary undistributable). Couple needs nothing from
either group:

- matching decodes audio with ffmpeg's native decoders (AAC, MP3, FLAC, PCM,
  Opus, Vorbis, …) — all LGPL;
- export stream-copies video (`-c:v copy`) and encodes audio with Apple
  AudioToolbox (`aac_at`), ffmpeg's native `aac` encoder, or PCM — all LGPL.

**The rule.** A distribution build ships only an ffmpeg whose `ffmpeg -L`
reports the LGPL and whose configuration contains neither `--enable-gpl` nor
`--enable-nonfree`. `mac/Scripts/fetch-ffmpeg.sh` enforces this (`check_lgpl`)
on every build it hands out, and `make-app.sh --dist` is the only bundle
variant fit for distribution. A default (dev) `make-app.sh` copies whatever
ffmpeg is on `$PATH` — a Homebrew ffmpeg is GPL-configured — and labels the
bundle a development build in `NSHumanReadableCopyright`. Never distribute a
dev bundle.

**How the dist build is produced.** `fetch-ffmpeg.sh` downloads the pinned
ffmpeg.org release tarball, checks it against a pinned sha256 and (when `gpg`
is installed) the FFmpeg release signing key, and builds it with
`--disable-autodetect` and no external libraries. The result links only macOS
system frameworks and `/usr/lib`, which is what makes "corresponding source"
simple: it is exactly the unmodified tarball plus the configure line.

**Obligations and where each is met.**

1. *Ship the license text.* `Contents/Resources/Licenses/ffmpeg/` holds
   `COPYING.LGPLv2.1` and ffmpeg's `LICENSE.md` (its per-component license
   map), copied from the same tarball the binaries were built from.
2. *Tell the user.* `NSHumanReadableCopyright` (shown in the About panel)
   says the helpers are LGPL and where the licenses live; the Acknowledgements
   view (§3) shows the texts themselves.
3. *Make the corresponding source available.* `SOURCE.txt` in the same
   folder gives the exact tarball URL, its sha256, the signing key, and the
   configure line. The retained copy of the tarball is the
   `ffmpeg-source-<version>` prerelease in this repo (asset
   `ffmpeg-<version>.tar.xz` plus its `.asc`; marked prerelease and not
   latest so the app download links are unaffected). It is also what
   `fetch-ffmpeg.sh` downloads first, ffmpeg.org second, so a release build
   never depends on ffmpeg.org being up (it was down on 2026-11-04). **Keep
   that prerelease for three years past the last distribution of any build
   made from it**, and link it from each app release's notes. A local cache
   lives at `mac/vendor/ffmpeg/src/` (gitignored).
4. *Publish modifications.* There are none. If ffmpeg is ever patched, the
   patch must ship alongside and `SOURCE.txt` must say so.

**Bumping the pin.** Change `FFMPEG_VERSION` and `TARBALL_SHA256` in
`fetch-ffmpeg.sh` from a download whose GPG signature you verified (the
release signing key is committed as `mac/Scripts/ffmpeg-release-signing-key.asc`;
the script checks its fingerprint against the pin before trusting it), then
create the matching `ffmpeg-source-<version>` prerelease with the tarball and
`.asc` — in CI the script refuses to build without it. Delete
`mac/vendor/ffmpeg/<old>` if you want the space back. Nothing downstream
changes. Keep the previous prerelease until the three-year window on the last
build that shipped it has passed.

**Patents.** Separate from copyright. Stream-copying H.264/HEVC avoids
encoding entirely; the core AAC patents are widely regarded as expired;
AudioToolbox and VideoToolbox are covered by Apple's platform license.
Keep it that way: adding a software H.264/HEVC *encoder* reopens both the GPL
question (x264/x265) and the patent one.

## 2. Electron shell

Bundled ffmpeg is optional there (`electron/electron-builder.yml`,
`resources/ffmpeg/<platform>/`); when the directory is empty the core falls
back to `COUPLE_FFMPEG` / `$PATH`. When bundling:

- macOS: use the output of `mac/Scripts/fetch-ffmpeg.sh` (binaries,
  `licenses/`, `SOURCE.txt`) — electron-builder copies the whole directory.
- Windows / Linux: use an **lgpl** variant only (BtbN's FFmpeg-Builds publishes
  separate `gpl` and `lgpl` builds for win64 and linux64 from a public CI
  workflow); put the license texts and a `SOURCE.txt` with the build's source
  pointer alongside. The same `ffmpeg -L` / configuration rule from §1 applies.
  `electron/scripts/fetch-ffmpeg.mjs` does this (the release workflow runs it
  on the Windows and Linux runners): one pinned month-end BtbN release, asset
  + sha256 per platform, configure-line check for `--enable-gpl` /
  `--enable-nonfree`, `-version` and `-L` when the host can run the binary,
  then `resources/ffmpeg/<platform>/{ffmpeg,ffprobe,SOURCE.txt,licenses/}`.
  Note these builds pass `--enable-version3`, so they are **LGPL v3 or later**
  (`licenses/COPYING.LGPLv3`), not v2.1 like the mac build; the first line of
  `SOURCE.txt` says so and the Acknowledgements view reads it from there. The
  corresponding source is the ffmpeg commit named in `SOURCE.txt` plus BtbN's
  build scripts at the pinned tag; keep a copy of the pinned archives
  (`electron/vendor/ffmpeg/`, gitignored) for the three-year window, as with
  the mac tarball.
- Linux `.deb` alternative: declare `Depends: ffmpeg` instead of bundling.
  The distribution is then the distributor of ffmpeg, not us.

Electron itself ships Chromium's own (LGPL) ffmpeg library; electron-builder
includes its notices (`LICENSE.electron.txt`, `LICENSES.chromium.html`)
automatically. Unrelated to the helper above.

## 3. Dependency notices and the Acknowledgements view (done 2026-11-04)

**Notices.** `scripts/third-party-notices.sh` generates `third-party/`
(committed, deterministic) from the three dependency trees a build ships:

- `rust-crates.txt` — every crate reachable over *normal* dependency edges
  from the workspace members (dev/build dependencies such as proptest,
  cbindgen and bindgen are not shipped and not listed), read from
  `cargo metadata` and the crate sources in the cargo registry: the declared
  SPDX expression, the alternative Couple uses for an `OR`, and the crate's
  own license files. A crate that ships none gets the canonical text from
  `scripts/license-texts/` with its authors on the MIT copyright line. Any
  `NOTICE` file is carried in verbatim (Apache-2.0 §4(d); none today).
- `npm-packages.txt` — the Electron shell's production packages
  (`npm ls --omit=dev`) plus the Electron runtime. Chromium/Node notices are
  electron-builder's `LICENSE.electron.txt` / `LICENSES.chromium.html`.
- `swift-packages.txt` — records that `mac/Package.swift` has no external
  packages; the script fails if one is added, so the tree is handled on
  purpose.
- `components.json` — the component list (name, version, license, tree) the
  views render.

The script **fails** on any license outside its allowed set (permissive only:
MIT, Apache-2.0, BSD, ISC, Zlib, Unicode, CC0, Unlicense, BSL, 0BSD) —
unknown, missing, or copyleft including weak copyleft (MPL). Adding such a
dependency is a licensing decision: record it here first, then extend
`ALLOWED` in `scripts/third_party_notices.py`. `--check` regenerates to a
temp dir and fails if `third-party/` is stale; `make-app.sh --dist` runs it.
Rerun the script after any `Cargo.lock` / `package-lock.json` change and
commit the result.

**Where they ship.** `make-app.sh` copies `third-party/` into
`Contents/Resources/Licenses/` (next to `ffmpeg/` from §1) in both the dev
and dist variants; `electron/electron-builder.yml` copies it to
`resources/Licenses/` via `extraResources`.

**Acknowledgements view.** Both shells (`mac/Sources/Couple/Views/AboutView.swift`
+ `AcknowledgementsView.swift`; `electron/src/renderer/src/views/AboutView.tsx`
+ `AcknowledgementsView.tsx`) replace the stock About panel with one that
shows the version and the copyright/ffmpeg line and opens Acknowledgements
(also under Help). Acknowledgements lists every bundled component with its
license name and shows the bundled texts: ffmpeg's `COPYING.LGPLv2.1`,
`LICENSE.md` and `SOURCE.txt` when ffmpeg is bundled, and the per-tree
notices files, scrolled to the selected component. The loaders
(`Licenses.swift`, `app/Licenses.ts`) read the same files the same way;
`COUPLE_LICENSES_DIR` overrides the folder for runs outside a bundle.
`SHELL-UI-PARITY.md` has the line.

No open items.
