// Media-free smoke test of the couple-node binding: ABI gate, error plumbing,
// pure policies, defaults providers, and the job/TSFN path (via an import job
// on a nonexistent file, which must fail loudly either at start or terminally).
// Run after `npm run build:native`. Exits nonzero on any failure.
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const native = require('../native/index.cjs');

let failures = 0;
function check(name, cond, detail = '') {
  if (cond) {
    console.log(`ok    ${name}`);
  } else {
    failures += 1;
    console.error(`FAIL  ${name}${detail ? ` — ${detail}` : ''}`);
  }
}

// --- ABI + version ---
check('abiVersion == 1', native.abiVersion() === 1, `got ${native.abiVersion()}`);
const version = native.version();
check('version() non-empty', typeof version === 'string' && version.length > 0);

// --- ctx lifecycle + error plumbing ---
const cacheDir = mkdtempSync(join(tmpdir(), 'couple-smoke-'));
const ctx = native.ctxNew(JSON.stringify({ cacheDir }));
check('ctxNew != 0n', ctx !== 0n);

const badProbe = native.probeJson(ctx, '/nonexistent/file.mp4');
check('probe of missing file yields null', badProbe === null);
const err = native.takeErrorJson(ctx);
check('takeErrorJson after failed probe', err !== null && JSON.parse(err).message?.length > 0, err ?? 'null');

// --- pure policies (no media needed) ---
const mq = native.policyMatchQuality(
  JSON.stringify({
    forced: false,
    selection: 'automatic',
    autoVerdict: 'confident',
    manualMatchConfident: null,
  })
);
const mqParsed = mq ? JSON.parse(mq) : null;
check(
  'policyMatchQuality ok envelope',
  mqParsed?.ok === true && mqParsed?.value?.quality === 'confident',
  mq ?? 'null'
);

const mqBad = native.policyMatchQuality('{"not":"a valid request"');
const mqBadParsed = mqBad ? JSON.parse(mqBad) : null;
check('policyMatchQuality error envelope on malformed JSON', mqBadParsed?.ok === false, mqBad ?? 'null');

const seed = native.policySyncSeed(
  JSON.stringify({ evidenceLagMs: 1234.5, evidenceZ: 20.0, explicit: false, lastSyncFailed: false, currentOffsetMs: 0 })
);
const seedParsed = seed ? JSON.parse(seed) : null;
check('policySyncSeed decides', seedParsed?.ok === true && 'seed' in (seedParsed?.value ?? {}), seed ?? 'null');

// --- defaults providers ---
const pairingDefaults = native.pairingParamsDefaultJson();
check('pairingParamsDefaultJson parses', pairingDefaults !== null && !!JSON.parse(pairingDefaults));
const resolveDefaults = native.resolveParamsDefaultJson();
const rd = resolveDefaults ? JSON.parse(resolveDefaults) : null;
check(
  'resolveParamsDefaultJson has matcher/zMin/pairing',
  rd !== null && 'matcher' in rd && 'zMin' in rd && 'pairing' in rd,
  resolveDefaults ?? 'null'
);

// --- triangle predicate (pure) ---
const tri = native.triangleCloses(
  JSON.stringify({
    va: { lagMs: 0, driftPpm: 0 },
    ab: { lagMs: 0, driftPpm: 0 },
    vb: { lagMs: 0, driftPpm: 0 },
    abClassification: 'match',
    coverageS: 100,
    offsetTolMs: 250,
    driftTolPpm: 50,
  })
);
const triParsed = tri ? JSON.parse(tri) : null;
check('triangleCloses on identical legs', triParsed?.closes === true, tri ?? 'null');

// --- job path: import of a nonexistent file must fail loudly ---
const jobResult = await new Promise((resolveP) => {
  let sawTerminal = false;
  const id = native.importStart(ctx, '/nonexistent/file.mp4', 0, (phase, _fraction, payload) => {
    if (phase === 'done' || phase === 'error' || phase === 'cancelled') {
      sawTerminal = true;
      resolveP({ started: true, phase, payload });
    }
  });
  if (id === 0n) {
    // Start-time rejection is also a valid loud failure; error is on the thread-local.
    resolveP({ started: false, error: native.takeErrorJson(ctx) });
  } else {
    setTimeout(() => {
      if (!sawTerminal) resolveP({ started: true, phase: 'TIMEOUT', payload: null });
    }, 15000);
  }
});
check(
  'import job on missing file fails loudly',
  (jobResult.started && jobResult.phase === 'error') || (!jobResult.started && jobResult.error !== null),
  JSON.stringify(jobResult)
);

native.ctxFree(ctx);
rmSync(cacheDir, { recursive: true, force: true });

if (failures > 0) {
  console.error(`smoke: ${failures} failure(s)`);
  process.exit(1);
}
console.log('smoke: all checks passed');
