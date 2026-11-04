#!/usr/bin/env node
// Provide the pinned LGPL static ffmpeg/ffprobe pair the Windows and Linux
// Electron installers bundle (the macOS leg uses mac/Scripts/fetch-ffmpeg.sh,
// which compiles its own; this script is the prebuilt counterpart for the two
// platforms where a public LGPL-configured static build exists).
//
// Source: BtbN/FFmpeg-Builds (https://github.com/BtbN/FFmpeg-Builds), the
// `-lgpl` (non-shared) variant for win64 and linux64 from one dated autobuild
// release. Pinned by exact release tag + asset name + sha256 computed from a
// download we inspected (never the remote checksum file). Pin a month-end
// autobuild: BtbN keeps those long-term while daily builds are pruned.
//
// LGPL rule (docs/LICENSING.md §1–2): the configure line embedded in the
// binary must contain neither --enable-gpl nor --enable-nonfree (checked on
// every platform, by reading the binary), and when the binary can run here
// `ffmpeg -L` must report the LGPL and both tools must run `-version`. These
// builds pass --enable-version3, so the license is LGPL v3 or later (the mac
// build is v2.1 or later); SOURCE.txt's first line says which.
//
// Output: electron/resources/ffmpeg/<platform>/ with ffmpeg(.exe),
// ffprobe(.exe), SOURCE.txt (release URL, asset, sha256, ffmpeg commit) and
// licenses/{COPYING.LGPLv3,LICENSE.md} — the layout
// src/renderer/src/app/Licenses.ts reads and electron-builder.yml copies
// (extraResources `resources/ffmpeg/${platform}` → `ffmpeg/`). Cached under
// electron/vendor/ffmpeg/<release-tag>/ (gitignored).
//
// Usage (from electron/):   node scripts/fetch-ffmpeg.mjs [--platform win32|linux]
// Default platform is the host's. A non-host platform still downloads, verifies
// and lays out the files (the run checks are skipped) — that is how the win32
// and linux pins are exercised from a Mac. Progress → stderr; stdout is exactly
// one line, the destination directory. Bumping the pin: change RELEASE_TAG,
// FFMPEG_REF and the per-platform asset/sha256 pairs below from a download you
// hashed yourself; nothing else in the pipeline changes.

import { createHash } from 'node:crypto';
import { createWriteStream } from 'node:fs';
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { fileURLToPath } from 'node:url';

// --- pins ------------------------------------------------------------------

const RELEASE_TAG = 'autobuild-2026-10-31-12-59'; // month-end autobuild, 2026-10-31
const RELEASE_URL = `https://github.com/BtbN/FFmpeg-Builds/releases/tag/${RELEASE_TAG}`;
const DOWNLOAD_BASE = `https://github.com/BtbN/FFmpeg-Builds/releases/download/${RELEASE_TAG}`;
// ffmpeg release/9.0 branch, 39 commits past the 9.0.2 tag, as BtbN names it.
const FFMPEG_REF = 'n9.0.2-39-g7adfbe40dc';
const FFMPEG_VERSION = '9.0.2';
const FFMPEG_COMMIT = '7adfbe40dcde598f93a6963d2f7b22cd75202ccc';
const FFMPEG_SOURCE_URL = `https://github.com/FFmpeg/FFmpeg/commit/${FFMPEG_COMMIT}`;

const PINS = {
  win32: {
    label: 'Windows x64',
    variant: 'win64-lgpl',
    asset: `ffmpeg-${FFMPEG_REF}-win64-lgpl-9.0.zip`,
    sha256: '9d37c564e0d01c18cde184a079fa6ef04d7d2783ada41da81d04165cc8f1a650',
    exe: '.exe',
  },
  linux: {
    label: 'Linux x64',
    variant: 'linux64-lgpl',
    asset: `ffmpeg-${FFMPEG_REF}-linux64-lgpl-9.0.tar.xz`,
    sha256: '6046d6731b26f58887e953561551d254a6eec9fbca4100dfeab56e19af8328d5',
    exe: '',
  },
};

// License texts. The archive's LICENSE.txt is ffmpeg's COPYING.LGPLv3 verbatim
// (pinned to that file's sha256 at FFMPEG_COMMIT, so a changed archive fails
// here); LICENSE.md (ffmpeg's per-component license map) is not in the
// archive and is fetched from the same commit. Both show in Acknowledgements.
const COPYING_LGPLV3_SHA256 = 'da7eabb7bafdf7d3ae5e9f223aa5bdc1eece45ac569dc21b3b037520b4464768';
const LICENSE_MD_SHA256 = '2e1d16c72fd74e12063776371da757322f8b77589386532f4fd8634bde7de1af';
const LICENSE_MD_URL = `https://raw.githubusercontent.com/FFmpeg/FFmpeg/${FFMPEG_COMMIT}/LICENSE.md`;

// --- helpers ---------------------------------------------------------------

const here = dirname(fileURLToPath(import.meta.url));
const electronDir = resolve(here, '..');

function note(msg) {
  process.stderr.write(`fetch-ffmpeg: ${msg}\n`);
}

function fail(msg) {
  note(msg);
  process.exit(1);
}

function parseArgs(argv) {
  const out = { platform: process.platform };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--platform') out.platform = argv[++i];
    else if (a.startsWith('--platform=')) out.platform = a.slice('--platform='.length);
    else fail(`unknown argument ${a}`);
  }
  return out;
}

function sha256File(file) {
  return createHash('sha256').update(readFileSync(file)).digest('hex');
}

function checkSha256(file, expected, what) {
  const actual = sha256File(file);
  if (actual !== expected) {
    note(`sha256 mismatch for ${what}`);
    note(`  expected: ${expected}`);
    note(`  actual:   ${actual}`);
    process.exit(1);
  }
}

async function download(url, dest) {
  note(`downloading ${url}`);
  const res = await fetch(url, { redirect: 'follow' });
  if (!res.ok || !res.body) fail(`download failed: HTTP ${res.status} for ${url}`);
  mkdirSync(dirname(dest), { recursive: true });
  const tmp = `${dest}.part`;
  await pipeline(Readable.fromWeb(res.body), createWriteStream(tmp));
  renameSync(tmp, dest);
}

function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { encoding: 'utf8', ...opts });
  if (r.error) fail(`${cmd} failed to start: ${r.error.message}`);
  return r;
}

function extract(archive, into) {
  mkdirSync(into, { recursive: true });
  // bsdtar (macOS, and Windows' built-in System32\tar.exe) extracts both zip
  // and tar.xz; GNU tar (Linux) handles tar.xz but not zip, so a zip there
  // goes through unzip. On Windows name the system tar explicitly: a GNU tar
  // from Git for Windows may shadow it on PATH and cannot read zips.
  let cmd = 'tar';
  let args = ['-xf', archive, '-C', into];
  if (process.platform === 'win32') {
    cmd = join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'tar.exe');
  } else if (process.platform === 'linux' && archive.endsWith('.zip')) {
    cmd = 'unzip';
    args = ['-q', archive, '-d', into];
  }
  const r = run(cmd, args, { stdio: ['ignore', 'ignore', 'inherit'] });
  if (r.status !== 0) fail(`extraction of ${archive} failed (exit ${r.status})`);
}

function onlyEntry(dir) {
  const entries = readdirSync(dir).filter((n) => !n.startsWith('.'));
  if (entries.length !== 1) fail(`expected one top-level entry in ${dir}, found: ${entries.join(', ') || '(none)'}`);
  return join(dir, entries[0]);
}

// The configure line is embedded in every ffmpeg binary as a NUL-terminated
// string (FFMPEG_CONFIGURATION, what `ffmpeg -version` prints as
// "configuration:"). Reading it works for a binary this host cannot run.
function checkLgplConfiguration(binary) {
  const bytes = readFileSync(binary);
  const start = bytes.indexOf('--prefix=');
  if (start < 0) fail(`${binary}: no embedded configure line found`);
  let end = bytes.indexOf(0, start);
  if (end < 0) end = bytes.length;
  const configuration = bytes.subarray(start, end).toString('latin1');
  for (const flag of ['--enable-gpl', '--enable-nonfree']) {
    if (configuration.split(' ').includes(flag)) {
      fail(`${binary}: configuration contains ${flag} (not an LGPL build)`);
    }
  }
}

function checkRuns(binary) {
  const v = run(binary, ['-version']);
  if (v.status !== 0) fail(`${binary} failed to run -version`);
  const l = run(binary, ['-L']);
  if (l.status !== 0 || !/GNU Lesser General Public License/.test(l.stdout)) {
    fail(`${binary}: -L does not report the LGPL`);
  }
}

// --- main ------------------------------------------------------------------

async function main() {
  const { platform } = parseArgs(process.argv.slice(2));
  const pin = PINS[platform];
  if (!pin) {
    fail(
      platform === 'darwin'
        ? 'macOS is handled by mac/Scripts/fetch-ffmpeg.sh (see .github/workflows/release.yml)'
        : `no pin for platform '${platform}' (have: ${Object.keys(PINS).join(', ')})`,
    );
  }
  const hostMatches = platform === process.platform;

  const cacheRoot = join(electronDir, 'vendor', 'ffmpeg', RELEASE_TAG);
  const archive = join(cacheRoot, pin.asset);
  const stage = join(cacheRoot, platform); // final layout, built once
  const dest = join(electronDir, 'resources', 'ffmpeg', platform);
  const tools = [`ffmpeg${pin.exe}`, `ffprobe${pin.exe}`];

  const staged = tools.every((t) => existsSync(join(stage, t))) && existsSync(join(stage, 'SOURCE.txt'));
  if (!staged) {
    if (!existsSync(archive)) await download(`${DOWNLOAD_BASE}/${pin.asset}`, archive);
    else note(`using cached ${archive}`);
    checkSha256(archive, pin.sha256, pin.asset);

    const tmp = mkdtempSync(join(tmpdir(), 'couple-ffmpeg-'));
    try {
      extract(archive, tmp);
      const root = onlyEntry(tmp);
      const bin = join(root, 'bin');
      for (const t of tools) {
        if (!existsSync(join(bin, t))) fail(`${pin.asset} has no bin/${t}`);
      }
      const archiveLicense = join(root, 'LICENSE.txt');
      if (!existsSync(archiveLicense)) fail(`${pin.asset} has no LICENSE.txt`);
      checkSha256(archiveLicense, COPYING_LGPLV3_SHA256, `${pin.asset}: LICENSE.txt (expected ffmpeg's COPYING.LGPLv3)`);
      const licenseMd = join(cacheRoot, 'LICENSE.md');
      if (!existsSync(licenseMd)) await download(LICENSE_MD_URL, licenseMd);
      checkSha256(licenseMd, LICENSE_MD_SHA256, 'LICENSE.md');

      rmSync(stage, { recursive: true, force: true });
      mkdirSync(join(stage, 'licenses'), { recursive: true });
      for (const t of tools) cpSync(join(bin, t), join(stage, t));
      cpSync(archiveLicense, join(stage, 'licenses', 'COPYING.LGPLv3'));
      cpSync(licenseMd, join(stage, 'licenses', 'LICENSE.md'));
      writeFileSync(
        join(stage, 'SOURCE.txt'),
        [
          // First line is parsed by Licenses.ts: version = first word starting
          // with a digit; "LGPL v3" selects the license name it shows.
          `ffmpeg/ffprobe ${FFMPEG_VERSION} (git ${FFMPEG_REF}) — LGPL v3 or later build for ${pin.label}`,
          '',
          `Prebuilt, unmodified, by BtbN/FFmpeg-Builds (variant ${pin.variant}; configured with`,
          '--enable-version3 and without --enable-gpl / --enable-nonfree):',
          `  release:  ${RELEASE_URL}`,
          `  asset:    ${pin.asset}`,
          `  sha256:   ${pin.sha256}`,
          `  builder:  https://github.com/BtbN/FFmpeg-Builds/tree/${RELEASE_TAG}`,
          '            (build scripts; the external libraries it links and their sources)',
          '',
          `ffmpeg source: ${FFMPEG_SOURCE_URL} (branch release/9.0)`,
          '',
          "licenses/COPYING.LGPLv3 is the archive's LICENSE.txt (identical to that file in",
          'the ffmpeg source); licenses/LICENSE.md is from the same ffmpeg commit.',
          'Fetched by electron/scripts/fetch-ffmpeg.mjs; obligations in docs/LICENSING.md.',
          '',
        ].join('\n'),
      );
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  } else {
    note(`using cached ${stage}`);
  }

  for (const t of tools) checkLgplConfiguration(join(stage, t));
  if (hostMatches) {
    for (const t of tools) checkRuns(join(stage, t));
  } else {
    note(`host is ${process.platform}; skipping -version/-L run checks for ${platform}`);
  }

  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dirname(dest), { recursive: true });
  cpSync(stage, dest, { recursive: true });
  for (const t of tools) {
    const size = statSync(join(dest, t)).size;
    note(`${join('resources', 'ffmpeg', platform, t)}  ${size} bytes`);
  }
  process.stdout.write(`${dest}\n`);
}

main().catch((e) => fail(e?.stack ?? String(e)));
