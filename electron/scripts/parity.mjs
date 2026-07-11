// Phase G §9 parity gate (PLAN-CORE-SHELL-BOUNDARY): the Electron kit and the
// `couple` CLI must produce IDENTICAL resolution decisions on shared fixtures.
//
// Mirrors scripts/golden-pair-rows.sh scenario-for-scenario: generates the same
// deterministic fixture sessions (`couple fixture session`), resolves them twice —
// once via `couple pair --json` (the CLI resolver) and once via the couple-node
// binding (`resolveStart`, the same core job the Electron app will call) — and
// compares the machine-stable decision projection (state / selection / alternates /
// clusters / triangle simultaneity / overflow / note / unused audio). Raw matcher
// floats are excluded for the same reason the golden script excludes them
// (AAC/ffmpeg-version dependent); decisions are what must not drift between shells.
//
// Usage: node scripts/parity.mjs   (builds couple-cli + couple-node first; needs ffmpeg)
import { execSync, execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync, readdirSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..');
const specsDir = join(repoRoot, 'fixtures', 'golden', 'specs');

// Same scenario table as scripts/golden-pair-rows.sh. `kitParams` mutates the
// core defaults JSON exactly as the CLI flags mutate the CLI-side params.
const SCENARIOS = [
  { name: 'continuous', spec: 'continuous.json', cliArgs: [], kitParams: null },
  { name: 'simultaneous', spec: 'simultaneous.json', cliArgs: [], kitParams: null },
  {
    name: 'restart-legacy',
    spec: 'restart.json',
    cliArgs: ['--matcher', 'legacy'],
    kitParams: (p) => {
      p.matcher = 'legacy';
    },
  },
  {
    name: 'ambiguous-legacy',
    spec: 'ambiguous.json',
    cliArgs: ['--matcher', 'legacy', '--params', 'triangle-tol-ms=0.000001', '--params', 'z-margin=1000'],
    kitParams: (p) => {
      p.matcher = 'legacy';
      p.triangleTolMs = 0.000001;
      p.zMargin = 1000;
    },
  },
  {
    name: 'overflow-legacy',
    spec: 'overflow.json',
    cliArgs: ['--matcher', 'legacy', '--params', 'verify-active=1', '--params', 'max-verify-pairs-per-video=4'],
    kitParams: (p) => {
      p.matcher = 'legacy';
      p.verifyActive = true;
      p.pairing.maxVerifyPairsPerVideo = 4;
    },
  },
];

// ---------- build both consumers ----------
console.log('building couple-cli + couple-node (release)…');
execSync('cargo build --release -q -p couple-cli -p couple-node', { cwd: repoRoot, stdio: 'inherit' });
execSync('node scripts/build-native.mjs', { cwd: join(here, '..'), stdio: 'inherit' });

const BIN = join(repoRoot, 'target', 'release', 'couple');
const require = createRequire(import.meta.url);
const native = require('../native/index.cjs');

// ---------- the machine-stable decision projection (golden-pair-rows.sh `project`) ----------
// Both sides are reduced to this shape before comparison. CLI rows carry file
// names as `video`/`selected.audio`; core wire rows carry videoName/audioName —
// the projector accepts either.
function projectRows(rows, unusedAudio) {
  const out = rows.map((r) => {
    const res = r.resolution ?? {};
    const selected = r.selected ? (r.selected.audio ?? r.selected.audioName ?? null) : null;
    return {
      video: r.video ?? r.videoName,
      state: r.state,
      chip: r.chip,
      selected,
      alternates: (r.alternates ?? []).map((a) => a.audio ?? a.audioName),
      clusters: res.clusters ?? null,
      note: res.note ?? null,
      candidateOverflow: res.candidateOverflow ?? null,
      triangles: (res.triangles ?? []).map((t) => ({
        audioA: t.audioA,
        audioB: t.audioB,
        simultaneous: t.simultaneous,
      })),
    };
  });
  return JSON.stringify({ rows: out, unusedAudio }, null, 2);
}

// ---------- kit-side resolution (the CLI's run_pair enumeration, ported) ----------
function takeError(ctx) {
  const e = native.takeErrorJson(ctx);
  return e ? JSON.parse(e).message ?? e : 'unknown core error';
}

/// Run a job to its terminal phase; resolves with the "done" payload JSON and
/// frees the job handle. `startFn(cb)` must return the job id.
function awaitJob(ctx, what, startFn) {
  return new Promise((resolveP, rejectP) => {
    let id = 0n;
    const settle = (fn, v) => {
      if (id !== 0n) native.jobFree(ctx, id);
      fn(v);
    };
    id = startFn((phase, _fraction, payload) => {
      if (phase === 'done') settle(resolveP, payload);
      else if (phase === 'error') settle(rejectP, new Error(`${what}: ${payload}`));
      else if (phase === 'cancelled') settle(rejectP, new Error(`${what}: cancelled`));
    });
    if (id === 0n) rejectP(new Error(`${what} start failed: ${takeError(ctx)}`));
  });
}

async function kitResolve(sessDir, cacheDir, mutateParams) {
  const ctx = native.ctxNew(JSON.stringify({ cacheDir }));
  if (ctx === 0n) throw new Error('ctxNew failed');
  try {
    // Deterministic listing: plain files sorted by name, dotfiles ignored (run_pair step 0).
    const names = readdirSync(sessDir)
      .filter((n) => !n.startsWith('.'))
      .filter((n) => statSync(join(sessDir, n)).isFile())
      .sort();

    // Probe + classify (run_pair step 1): video stream ⇒ video; audio-only ⇒ audio.
    const videoFiles = [];
    const audioFiles = [];
    for (const n of names) {
      const raw = native.probeJson(ctx, join(sessDir, n));
      if (raw === null) {
        native.takeErrorJson(ctx); // skipped, like probe_lenient failures
        continue;
      }
      const info = JSON.parse(raw);
      if (info.video != null) videoFiles.push({ name: n, hasAudio: (info.audio ?? []).length > 0 });
      else if ((info.audio ?? []).length > 0) audioFiles.push({ name: n });
    }

    // Import (run_pair step 2): videos as camera (0), audios as external (1);
    // audio-less videos are not imported (woven as inert no-match rows below).
    async function importOne(path, role) {
      const payload = await awaitJob(ctx, `import ${path}`, (cb) =>
        native.importStart(ctx, path, role, cb)
      );
      // The done payload is TrackMeta; its `id` (first key) is the u64 track
      // handle — extract it textually so JSON.parse can't round it.
      const m = /"id"\s*:\s*(\d+)/.exec(payload);
      if (!m) throw new Error(`import done payload lacks id: ${payload}`);
      return BigInt(m[1]);
    }

    const videoIds = []; // aligned to videoFiles; null for audio-less
    for (const v of videoFiles) {
      videoIds.push(v.hasAudio ? await importOne(join(sessDir, v.name), 0) : null);
    }
    const audioIds = [];
    for (const a of audioFiles) {
      audioIds.push(await importOne(join(sessDir, a.name), 1));
    }

    // Params: mutate the CORE defaults JSON (the single source, like the CLI does).
    let paramsJson = null;
    if (mutateParams) {
      const defaults = JSON.parse(native.resolveParamsDefaultJson());
      mutateParams(defaults);
      paramsJson = JSON.stringify(defaults);
    }

    // Resolve via the JOB twin (the Electron app's path); byte-identical to cpl_resolve.
    const importable = videoIds.filter((id) => id !== null);
    let resolvedRows = [];
    if (importable.length > 0) {
      const payload = await awaitJob(ctx, 'resolve', (cb) =>
        native.resolveStart(ctx, importable, audioIds, paramsJson, cb)
      );
      resolvedRows = JSON.parse(payload);
    }

    // Weave audio-less videos as inert no-match rows (run_pair step 4).
    const rows = [];
    let ri = 0;
    for (const v of videoFiles) {
      if (!v.hasAudio) {
        rows.push({
          video: v.name,
          state: 'noMatch',
          chip: 'no match',
          selected: null,
          alternates: [],
          resolution: { clusters: 0, note: 'no audio stream' },
        });
      } else {
        rows.push(resolvedRows[ri++]);
      }
    }

    // Unused-audio tray (§3.6): audios no row selects, in audio order.
    const used = new Set(
      rows.map((r) => (r.selected ? r.selected.audio ?? r.selected.audioName : null)).filter(Boolean)
    );
    const unusedAudio = audioFiles.map((a) => a.name).filter((n) => !used.has(n));

    return projectRows(rows, unusedAudio);
  } finally {
    native.ctxFree(ctx);
  }
}

// ---------- run all scenarios ----------
const work = mkdtempSync(join(tmpdir(), 'couple-parity-'));
let fail = 0;
try {
  for (const s of SCENARIOS) {
    const sess = join(work, s.name);
    const cliCache = join(work, `${s.name}-cache-cli`);
    const kitCache = join(work, `${s.name}-cache-kit`);

    execFileSync(BIN, ['--quiet', 'fixture', 'session', '--out', sess, '--spec', join(specsDir, s.spec)], {
      stdio: ['ignore', 'ignore', 'inherit'],
    });

    const cliOut = JSON.parse(
      execFileSync(BIN, ['--json', '--quiet', '--cache-dir', cliCache, 'pair', sess, ...s.cliArgs], {
        encoding: 'utf8',
        maxBuffer: 64 * 1024 * 1024,
      })
    );
    const cliProjection = projectRows(cliOut.rows ?? [], cliOut.unusedAudio ?? []);

    const kitProjection = await kitResolve(sess, kitCache, s.kitParams);

    if (cliProjection === kitProjection) {
      console.log(`ok    ${s.name}`);
    } else {
      fail = 1;
      console.error(`DRIFT ${s.name}: CLI and kit resolved differently`);
      console.error('--- CLI ---');
      console.error(cliProjection);
      console.error('--- kit ---');
      console.error(kitProjection);
    }
  }
} finally {
  rmSync(work, { recursive: true, force: true });
}

if (fail) {
  console.error('parity: DRIFT detected — the shells disagree on resolution decisions');
  process.exit(1);
}
console.log('parity: kit rows identical to CLI rows on all scenarios (Phase G §9 gate)');
