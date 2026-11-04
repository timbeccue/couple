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
2. *Tell the user.* `NSHumanReadableCopyright` (visible in the About panel)
   says the helpers are LGPL and where the licenses live. An in-app
   Acknowledgements view is an open item (§3).
3. *Make the corresponding source available.* `SOURCE.txt` in the same
   folder gives the exact tarball URL, its sha256, the signing key, and the
   configure line. The tarball itself is cached at `mac/vendor/ffmpeg/src/`
   (gitignored). **Before each public release, attach that tarball to the
   release** (or otherwise keep a copy under your control): the obligation
   runs three years past the last time that binary is distributed, and
   ffmpeg.org staying online is not something we control.
4. *Publish modifications.* There are none. If ffmpeg is ever patched, the
   patch must ship alongside and `SOURCE.txt` must say so.

**Bumping the pin.** Change `FFMPEG_VERSION` and `TARBALL_SHA256` in
`fetch-ffmpeg.sh` from a download whose GPG signature you verified; delete
`mac/vendor/ffmpeg/<old>` if you want the space back. Nothing downstream
changes. Keep the previous tarball around until the three-year window on the
last build that shipped it has passed.

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
- Linux `.deb` alternative: declare `Depends: ffmpeg` instead of bundling.
  The distribution is then the distributor of ffmpeg, not us.

Electron itself ships Chromium's own (LGPL) ffmpeg library; electron-builder
includes its notices (`LICENSE.electron.txt`, `LICENSES.chromium.html`)
automatically. Unrelated to the helper above.

## 3. Open items

- Dependency notices for the Rust, Swift and npm trees are not generated yet
  (almost all MIT/Apache-2.0; Apache requires carrying any `NOTICE` forward).
  `cargo about` can produce the Rust side as one file.
- An Acknowledgements view in both shells listing the above
  (`SHELL-UI-PARITY.md` applies).
