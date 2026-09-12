// Real native-job ownership checks. Run in a child process with a timeout:
// natural exit proves that rejected and completed jobs released their TSFNs.
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const native = require('../native/index.cjs');
const cacheDir = mkdtempSync(join(tmpdir(), 'couple-jobs-'));
const ctx = native.ctxNew(JSON.stringify({ cacheDir }));
assert.notEqual(ctx, 0n);

function terminal(start) {
  return new Promise((resolve, reject) => {
    let id = 0n;
    let ended = false;
    id = start((phase, fraction, payload) => {
      assert.equal(ended, false, 'no callbacks after the terminal event');
      if (phase !== 'done' && phase !== 'error' && phase !== 'cancelled') return;
      ended = true;
      native.jobFree(ctx, id);
      resolve({ phase, fraction, payload });
    });
    if (id === 0n) reject(new Error(`unexpected start rejection: ${native.takeErrorJson(ctx)}`));
  });
}

try {
  const rejected = [
    ['importStart', cb => native.importStart(0n, '/missing.wav', 0, cb)],
    ['autosyncStart', cb => native.autosyncStart(ctx, 0n, '{broken', cb)],
    ['autosyncStartSeeded', cb => native.autosyncStartSeeded(ctx, 0n, '{broken', 0, 0, cb)],
    ['exportStart', cb => native.exportStart(ctx, 0n, '{broken', cb)],
    ['matchCandidatesGraphStart', cb => native.matchCandidatesGraphStart(ctx, [], [], '{broken', cb)],
    ['resolveStart', cb => native.resolveStart(ctx, [], [], '{broken', cb)],
  ];
  for (const [name, start] of rejected) {
    const id = start(() => assert.fail(`${name}: rejected starts must not call back`));
    assert.equal(id, 0n, name);
    const error = native.takeErrorJson(ctx);
    assert.ok(error && JSON.parse(error).error, `${name}: caller can drain the start error`);
  }

  // Null and absent params still select core defaults and produce successful
  // jobs; the kit remains responsible for releasing the returned job handle.
  for (const params of [null, undefined]) {
    for (const start of [
      cb => native.resolveStart(ctx, [], [], params, cb),
      cb => native.matchCandidatesGraphStart(ctx, [], [], params, cb),
    ]) {
      const result = await terminal(start);
      assert.equal(result.phase, 'done');
      assert.equal(result.fraction, 1);
      assert.deepEqual(JSON.parse(result.payload), []);
    }
  }

  // Invalid sessions and missing media fail on the worker, after ownership has
  // transferred. These must deliver one error event and release the callback.
  const exportOptions = JSON.stringify({ kind: 'wav', outputPath: join(cacheDir, 'unused.wav') });
  for (const start of [
    cb => native.importStart(ctx, join(cacheDir, 'missing.wav'), 0, cb),
    cb => native.autosyncStart(ctx, 0n, null, cb),
    cb => native.autosyncStartSeeded(ctx, 0n, null, 0, 0, cb),
    cb => native.exportStart(ctx, 0n, exportOptions, cb),
  ]) {
    const result = await terminal(start);
    assert.equal(result.phase, 'error');
    assert.ok(JSON.parse(result.payload).error);
  }
  await new Promise(resolve => setImmediate(resolve));
} finally {
  native.ctxFree(ctx);
  rmSync(cacheDir, { recursive: true, force: true });
}

console.log('native job lifecycle: passed');
