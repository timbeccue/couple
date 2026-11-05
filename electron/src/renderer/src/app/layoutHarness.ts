// layoutHarness.ts — env-gated verification of the compact arrangement and the
// arrangement switch (PLAN-COMPACT-MODE Phase 4/6): the Electron twins of the mac
// DebugDrive COUPLE_COMPACT_TEST / COUPLE_LAYOUT_TOGGLE_TEST cases, same env keys
// and the same "<CASE> PASS|FAIL <check>" + "summary: n passed, m failed" lines a
// headless run greps. Main forces the arrangement for a harness run (compact for
// these two, full for every other COUPLE_*_TEST) and never persists it. With
// COUPLE_SHOT_DIR set, shots land there (main's capturePage). Media via
// COUPLE_OPEN_DIR; quit via COUPLE_AUTOQUIT_MS. Never runs in production.
import { env } from '../kit/native';
import type { AppModel } from './AppModel';
import type { CoupleRow } from './CoupleRow';
import { WindowLayoutBridge, type Frame } from './WindowLayout';
import { compactSegment, COMPACT_MONITOR_CASES } from '../views/VideoHero';

const nodeRequire: NodeRequire = (window as unknown as { require: NodeRequire }).require;
const pathMod = nodeRequire('node:path') as typeof import('node:path');
const fs = nodeRequire('node:fs') as typeof import('node:fs');
const os = nodeRequire('node:os') as typeof import('node:os');

const wait = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

function settled(c: CoupleRow): boolean {
  return c.pair.session === null || c.pair.syncStatus === 'synced' || c.pair.syncStatus === 'failed';
}

async function settleQueue(model: AppModel): Promise<void> {
  for (let i = 0; i < 3000; i++) {
    if (!model.ingest.ingestInFlight && model.couples.length > 0 && model.couples.every(settled)) return;
    await wait(100);
  }
}

async function shot(tag: string, name: string): Promise<void> {
  const dir = env.COUPLE_SHOT_DIR;
  if (!dir) return;
  const { ipcRenderer } = nodeRequire('electron') as typeof import('electron');
  fs.mkdirSync(dir, { recursive: true });
  const path = pathMod.join(dir, `${name}.png`);
  try {
    await ipcRenderer.invoke('couple:capture', path);
    console.log(`${tag}: screenshot ${path}`);
  } catch (e) {
    console.log(`${tag}: screenshot failed ${String(e)}`);
  }
}

function counter(tag: string): { check: (name: string, ok: boolean) => void; summary: () => void } {
  let pass = 0;
  let fail = 0;
  return {
    check(name, ok) {
      if (ok) pass += 1;
      else fail += 1;
      console.log(`${tag} ${ok ? 'PASS' : 'FAIL'} ${name}`);
    },
    summary() {
      console.log(`${tag} summary: ${pass} passed, ${fail} failed`);
    },
  };
}

export function runLayoutHarnesses(model: AppModel): void {
  if (env.COUPLE_COMPACT_TEST === '1') void compactTest(model);
  if (env.COUPLE_LAYOUT_TOGGLE_TEST === '1') void layoutToggleTest(model);
}

/** The compact arrangement over a real session: the arrangement and the window
 * clamps (width 340–420, height ≥ 560), then shots of the empty window, a Ready row
 * ("compact-synced"), the no-audio row, a row with its candidate list inline
 * ("compact-pick") and a batch export in flight ("compact-exporting"). */
async function compactTest(model: AppModel): Promise<void> {
  const TAG = 'COMPACT-TEST';
  const { check, summary } = counter(TAG);
  await wait(300);
  if (!model.hasContent) await shot(TAG, 'compact-empty');
  await settleQueue(model);
  check('arrangement is compact', model.layoutMode === 'compact');
  const st = await WindowLayoutBridge.state();
  if (st) {
    const b = st.bounds;
    console.log(
      `${TAG}: window frame ${b.width}×${b.height} · min ${st.minSize[0]}×${st.minSize[1]} · max width ${st.maxSize[0] || '∞'}`
    );
    check('window width within 340–420', b.width >= 340 && b.width <= 420);
    check('window height ≥ 560', b.height >= 560);
    check(
      'window resize limits: width 340–420, min height 560',
      st.minSize[0] === 340 && st.maxSize[0] === 420 && st.minSize[1] >= 560
    );
  } else {
    console.log(`${TAG}: SKIPPED frame checks (no window bridge)`);
  }

  const ready = model.ready.find((c) => c.pair.session) ?? model.ready[0];
  if (ready) {
    model.select(ready.id);
    await wait(1200);
    check(
      `Ready row selected and previewed (${ready.video.name})`,
      model.selectedID === ready.id && model.audition.couple === ready && model.audition.attached
    );
    check(
      'compact picker shows the mode as one of Camera · Synced · External',
      COMPACT_MONITOR_CASES.includes(compactSegment(model.audition.mode))
    );
    check('no Files list / pool / editor tools in compact', document.querySelector('.full-only') === null);
    check('preview block shows the PairStrip', document.querySelector('.blk-pv .pair-strip') !== null);
    await shot(TAG, 'compact-synced');
  } else {
    check('a Ready row exists', false);
  }

  const none = model.couples.find((c) => c.state === 'noMatch');
  if (none) {
    model.select(none.id);
    await wait(800);
    check(
      `no-audio row selected (${none.video.name}); one-line variant applies`,
      model.selectedID === none.id && none.candidates.length === 0 && document.querySelector('.no-audio-line') !== null
    );
    await shot(TAG, 'compact-noaudio');
  } else {
    console.log(`${TAG}: SKIPPED no-audio shot (no no-match couple)`);
  }

  const two =
    model.couples.find((c) => c.selectedAudioKey !== null && c.alternates.length > 0) ??
    model.couples.find((c) => c.candidates.length >= 2);
  if (two) {
    model.select(two.id);
    if (two.pinned && two.selectedAudioKey !== null) {
      model.compareCoupleID = two.id;
      model.invalidate();
    }
    await wait(1000);
    check(
      `row with alternates shows its candidate list inline (${two.video.name}, ${two.candidates.length} candidates)`,
      (two.needsResolve || model.compareCoupleID === two.id) === (document.querySelector('.blk-pv .resolve-banner') !== null)
    );
    await shot(TAG, 'compact-pick');
  } else {
    console.log(`${TAG}: SKIPPED pick shot (no couple with alternates)`);
  }

  if (model.includedCount > 0) {
    const outDir = fs.mkdtempSync(pathMod.join(os.tmpdir(), 'couple-compact-test-'));
    model.exportDestination = outDir;
    model.destinationAuto = false;
    if (ready) model.select(ready.id);
    const n = model.includedCount;
    const run = model.batch.run();
    let sawRunning = false;
    for (let i = 0; i < 100; i++) {
      if (model.batch.phase.kind === 'running') {
        sawRunning = true;
        break;
      }
      await wait(50);
    }
    await wait(250);
    const stillRunning = model.batch.isRunning;
    await shot(TAG, 'compact-exporting');
    console.log(`${TAG}: export shot taken ${stillRunning ? 'mid-run' : 'after the batch finished'}`);
    check(`batch export ran (${n} included)`, sawRunning || !stillRunning);
    await run;
    const ph = model.batch.phase;
    check(
      ph.kind === 'finished' ? `batch finished: ${ph.exported} exported, ${ph.failed} failed` : 'batch finished',
      ph.kind === 'finished' && ph.exported === n && ph.failed === 0
    );
    check('exported couples are stamped and left the default include set', model.couples.filter((c) => c.exportedAt !== null).length === n);
    check('arrangement still compact after export', model.layoutMode === 'compact');
  }
  summary();
}

/** Play a Ready row, then toggle compact → full → compact → full → compact and
 * assert at each step: selection unchanged, playhead advanced (never reset),
 * inclusion unchanged, the <video> element not remounted, the window restored to
 * each arrangement's last frame, and the monitor mode round-trips Pan (full) →
 * Synced (compact) → Pan (full) (D10). */
async function layoutToggleTest(model: AppModel): Promise<void> {
  const TAG = 'LAYOUT-TOGGLE-TEST';
  const { check, summary } = counter(TAG);
  const near = (a: Frame | undefined, b: Frame | undefined): boolean =>
    !!a &&
    !!b &&
    Math.abs(a.x - b.x) <= 1 &&
    Math.abs(a.y - b.y) <= 1 &&
    Math.abs(a.width - b.width) <= 1 &&
    Math.abs(a.height - b.height) <= 1;
  const fs_ = (r: Frame | undefined): string => (r ? `${r.x},${r.y} ${r.width}×${r.height}` : '—');
  await settleQueue(model);
  const ready = model.ready.find((c) => c.pair.session);
  if (!ready) {
    console.log(`${TAG}: SKIPPED (no Ready couple with a session)`);
    summary();
    return;
  }
  check('starts compact', model.layoutMode === 'compact');
  model.select(ready.id);
  await wait(600);
  const snapshot = (): string => model.couples.map((c) => `${c.id}:${c.included}/${String(c.explicit)}`).join(',');
  const inclusion = snapshot();
  const a = model.audition;
  if (a.couple !== ready) model.attachAudition(ready);
  a.seek(2);
  a.play();
  await wait(1200);
  const playing = a.playing;
  console.log(`${TAG}: playing=${playing}`);
  let lastT = a.playheadFor(ready);
  const playheadOK = (): boolean => {
    const t = a.playheadFor(ready);
    const ok = playing ? t > lastT : t >= lastT - 1e-6 && t > 0;
    lastT = t;
    return ok;
  };
  const video0 = document.querySelector('.blk-pv video');
  const c0 = (await WindowLayoutBridge.state())?.bounds;

  await model.requestLayoutMode('full');
  await wait(900);
  check('→ full: arrangement full', model.layoutMode === 'full');
  check('→ full: selection unchanged', model.selectedID === ready.id);
  check(`→ full: playhead advanced (${a.playheadFor(ready).toFixed(2)} s)`, playheadOK());
  check('→ full: still playing', a.playing === playing);
  check('→ full: inclusion unchanged', snapshot() === inclusion);
  check('→ full: the <video> element was not remounted', document.querySelector('.blk-pv video') === video0);
  let st = await WindowLayoutBridge.state();
  check(`→ full: window ≥ 640 wide (${fs_(st?.bounds)})`, (st?.bounds.width ?? 0) >= 640);
  if (st) {
    // A distinctive full frame so the restore is meaningful.
    await WindowLayoutBridge.setBounds({ x: st.bounds.x, y: st.bounds.y, width: 1100, height: 760 });
    await wait(300);
  }
  const f1 = (await WindowLayoutBridge.state())?.bounds;
  a.selectMode('pan');
  check('full: monitor set to Pan L/R', a.mode === 'pan');

  await model.requestLayoutMode('compact');
  await wait(900);
  check('→ compact: arrangement compact', model.layoutMode === 'compact');
  check('→ compact: selection unchanged', model.selectedID === ready.id);
  check(`→ compact: playhead advanced (${a.playheadFor(ready).toFixed(2)} s)`, playheadOK());
  check('→ compact: inclusion unchanged', snapshot() === inclusion);
  check('→ compact: Pan heard and shown as Synced (overlay)', a.mode === 'overlay' && compactSegment(a.mode) === 'overlay');
  check('→ compact: the <video> element was not remounted', document.querySelector('.blk-pv video') === video0);
  st = await WindowLayoutBridge.state();
  if (st) {
    check(`→ compact: last compact frame restored (${fs_(st.bounds)} vs ${fs_(c0)})`, near(st.bounds, c0));
    check('→ compact: width within 340–420', st.bounds.width >= 340 && st.bounds.width <= 420);
  } else {
    console.log(`${TAG}: SKIPPED frame checks (no window bridge)`);
  }

  await model.requestLayoutMode('full');
  await wait(900);
  check('→ full again: monitor back to Pan L/R', a.mode === 'pan');
  check('→ full again: selection unchanged', model.selectedID === ready.id);
  check(`→ full again: playhead advanced (${a.playheadFor(ready).toFixed(2)} s)`, playheadOK());
  check('→ full again: inclusion unchanged', snapshot() === inclusion);
  st = await WindowLayoutBridge.state();
  if (st) check(`→ full again: last full frame restored (${fs_(st.bounds)} vs ${fs_(f1)})`, near(st.bounds, f1));

  await model.requestLayoutMode('compact');
  await wait(900);
  st = await WindowLayoutBridge.state();
  if (st) check(`→ compact again: last compact frame restored (${fs_(st.bounds)})`, near(st.bounds, c0));
  check('→ compact again: selection + inclusion unchanged', model.selectedID === ready.id && snapshot() === inclusion);
  a.pause();
  summary();
}
