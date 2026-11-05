// AppModel.test.ts (F5a) — the root model's manual-pick evidence flow, re-pair
// undo/redo, rename undo, and queue persistence, driven against the REAL binding
// (only media-bound calls — matchPair, session load/sync, ensureSession — are
// stubbed at the instance). A2 is the 87136e4 regression shape: a manual pick is
// questionable until the CORE classifier says 'match', a core fault never greens,
// and a stale selection is a no-op.
import { describe, it, expect, beforeEach } from 'vitest';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { AppModel } from './AppModel';
import { CoupleRow } from './CoupleRow';
import { PoolEntry } from './MediaPool';
import { QueueStore, isoNow } from './QueueStore';
import type { TrackHandle } from '../kit/Track';
import type { PairEvidence, ResolvedRow, RowCandidate, RowState } from '../kit/Wire';

const fakeHandle = { id: 1n, free() {} } as unknown as TrackHandle;

function entry(id: string, role: 'video' | 'audio', path: string): PoolEntry {
  return new PoolEntry({
    id,
    path,
    name: path.split('/').pop() ?? path,
    role,
    durationS: 10,
    creationTimeUtc: null,
    info: null,
    fileSizeBytes: null,
    handle: fakeHandle,
  });
}

function cand(audioKey: string): RowCandidate {
  return { audioKey, audioName: `${audioKey}.wav`, z: 12, lagMs: 0, coverage: { t0: 0, t1: 1, seconds: 1, fraction: 0.1 } };
}

function makeRow(videoKey: string, selectedKey: string | null, state: RowState, altKeys: string[]): ResolvedRow {
  return {
    videoKey,
    videoName: `${videoKey}.mp4`,
    state,
    chip: state,
    selected: selectedKey ? cand(selectedKey) : null,
    alternates: altKeys.map(cand),
    resolution: { clusters: 1 },
  };
}

function newModel(): AppModel {
  const model = new AppModel();
  model.ingest.ensureSession = () => {};
  return model;
}

function pushEntry(model: AppModel, id: string, role: 'video' | 'audio', path: string): PoolEntry {
  const e = entry(id, role, path);
  model.pool.entries.push(e);
  return e;
}

function addCouple(
  model: AppModel,
  videoEntry: PoolEntry,
  selectedKey: string | null,
  state: RowState,
  altKeys: string[]
): CoupleRow {
  const couple = new CoupleRow(videoEntry, makeRow(videoEntry.id, selectedKey, state, altKeys), model.engine);
  couple.pair.load = () => {};
  couple.pair.runAutoSync = () => {};
  model.couples = [couple];
  return couple;
}

const flush = (): Promise<void> => new Promise((r) => setImmediate(r));

beforeEach(() => QueueStore.clear());

describe('AppModel', () => {
  it('A1 zMin reads the core resolver default', () => {
    const model = newModel();
    expect(model.zMin).toBe(8.5);
    expect(model.zMin).toBe((JSON.parse(model.engine.resolveParamsDefaultJSON()!) as { zMin: number }).zMin);
  });

  it('A2 REGRESSION (87136e4): a manual pick greens only once the core classifier says match', async () => {
    const model = newModel();
    model.engine.matchPair = () => ({ classification: 'match' } as PairEvidence);
    const v = pushEntry(model, 'v1', 'video', '/m/v1.mp4');
    pushEntry(model, 'a1', 'audio', '/m/a1.wav');
    const couple = addCouple(model, v, 'a1', 'confident', []);

    couple.applyManualPick('a1', 12, model.zMin);
    expect(couple.manualMatchConfident).toBeNull();
    expect(couple.verdict).toBe('questionable');

    await model.reevaluateManualMatch(couple);
    expect(couple.manualMatchConfident).toBe(true);
    expect(couple.verdict).toBe('good');
    expect(model.ready).toContain(couple);
  });

  it('A3 a core-fault (null evidence) manual pick never greens', async () => {
    const model = newModel();
    model.engine.matchPair = () => null;
    const v = pushEntry(model, 'v1', 'video', '/m/v1.mp4');
    pushEntry(model, 'a1', 'audio', '/m/a1.wav');
    const couple = addCouple(model, v, 'a1', 'confident', []);

    couple.applyManualPick('a1', 12, model.zMin);
    await model.reevaluateManualMatch(couple);
    expect(couple.manualMatchConfident).toBe(false);
    expect(couple.verdict).toBe('questionable');
  });

  it('A4 a selection that changed underneath a re-evaluation is a no-op', async () => {
    const model = newModel();
    model.engine.matchPair = () => null;
    const v = pushEntry(model, 'v1', 'video', '/m/v1.mp4');
    pushEntry(model, 'a1', 'audio', '/m/a1.wav');
    const couple = addCouple(model, v, 'a1', 'confident', []);

    couple.applyManualPick('a1', 12, model.zMin);
    const p = model.reevaluateManualMatch(couple);
    couple.selectedAudioKey = 'b1'; // the selection moved before the microtask ran
    await p;
    expect(couple.manualMatchConfident).toBeNull();
  });

  it('A5 repairDrop re-pairs, re-evaluates, and is undoable / redoable', async () => {
    const model = newModel();
    model.engine.matchPair = () => ({ classification: 'match' } as PairEvidence);
    const v = pushEntry(model, 'v1', 'video', '/m/v1.mp4');
    pushEntry(model, 'a1', 'audio', '/m/a1.wav');
    pushEntry(model, 'b1', 'audio', '/m/b1.wav');
    const couple = addCouple(model, v, 'a1', 'confident', []);

    model.repairDrop(couple, 'b1');
    expect(couple.selectedAudioKey).toBe('b1');
    expect(couple.selectionProvenance).toBe('manual');
    expect(couple.pinned).toBe(true);
    await flush();
    expect(couple.manualMatchConfident).toBe(true);
    expect(model.undo.canUndo).toBe(true);

    model.performUndo();
    expect(couple.selectedAudioKey).toBe('a1');
    expect(couple.selectionProvenance).toBe('automatic');
    expect(couple.manualMatchConfident).toBeNull();
    expect(model.undo.canRedo).toBe(true);

    model.performRedo();
    expect(couple.selectedAudioKey).toBe('b1');
    expect(couple.selectionProvenance).toBe('manual');
  });

  it('A6 repairDrop of the already-selected key is a no-op', () => {
    const model = newModel();
    const v = pushEntry(model, 'v1', 'video', '/m/v1.mp4');
    pushEntry(model, 'a1', 'audio', '/m/a1.wav');
    const couple = addCouple(model, v, 'a1', 'confident', []);

    model.repairDrop(couple, 'a1');
    expect(model.undo.canUndo).toBe(false);
  });

  it('A7 setStemOverride is undoable / redoable', () => {
    const model = newModel();
    const v = pushEntry(model, 'v1', 'video', '/m/v1.mp4');
    const couple = addCouple(model, v, 'a1', 'confident', []);

    model.setStemOverride(couple, 'custom');
    expect(couple.outputStem).toBe('custom');
    model.performUndo();
    expect(couple.outputStem).toBe('v1_synced');
    model.performRedo();
    expect(couple.outputStem).toBe('custom');
  });

  it('A8 a manual pick round-trips through queue persistence (TODO-009)', () => {
    const dir = mkdtempSync(join(tmpdir(), 'couple-queue-'));
    const vPath = join(dir, 'v1.mp4');
    const aPath = join(dir, 'a1.wav');
    writeFileSync(vPath, 'v');
    writeFileSync(aPath, 'a');

    const model = newModel();
    const v = pushEntry(model, 'v1', 'video', vPath);
    pushEntry(model, 'a1', 'audio', aPath);
    const couple = addCouple(model, v, 'a1', 'confident', []);
    couple.applyManualPick('a1', 12, model.zMin);
    couple.manualMatchConfident = true;
    model.saveQueue();

    const saved = QueueStore.load();
    expect(saved).not.toBeNull();
    expect(saved!.couples[0]).toMatchObject({
      selectionProvenance: 'manual',
      manualMatchConfident: true,
      pinned: true,
    });
  });

  it('A9 clearSession empties the queue and removes the restore record', () => {
    const dir = mkdtempSync(join(tmpdir(), 'couple-queue-'));
    const vPath = join(dir, 'v1.mp4');
    const aPath = join(dir, 'a1.wav');
    writeFileSync(vPath, 'v');
    writeFileSync(aPath, 'a');

    const model = newModel();
    const v = pushEntry(model, 'v1', 'video', vPath);
    pushEntry(model, 'a1', 'audio', aPath);
    addCouple(model, v, 'a1', 'confident', []);
    model.saveQueue();
    expect(QueueStore.load()).not.toBeNull();

    model.clearSession();
    expect(model.couples).toEqual([]);
    expect(model.selectedID).toBeNull();
    expect(model.pool.isEmpty).toBe(true);
    expect(model.undo.canUndo).toBe(false);
    expect(QueueStore.load()).toBeNull();
  });

  it('A10 include-for-export: a good couple is included by default; the checkbox opts out, undoably', () => {
    const model = newModel();
    const v = pushEntry(model, 'v1', 'video', '/m/v1.mp4');
    pushEntry(model, 'a1', 'audio', '/m/a1.wav');
    const couple = addCouple(model, v, 'a1', 'confident', []);
    expect(couple.verdict).toBe('good');
    expect(couple.canInclude).toBe(true);
    expect(couple.included).toBe(true);
    expect(model.ready).toContain(couple);
    expect(model.includedCount).toBe(1);

    model.toggleInclude(couple);
    expect(couple.explicit).toBe(false);
    expect(couple.included).toBe(false);
    expect(model.ready).toContain(couple); // inclusion never moves a row
    expect(model.includedCount).toBe(0);
    expect(model.filesList).toEqual([]);

    model.performUndo();
    expect(couple.explicit).toBeNull();
    expect(couple.included).toBe(true);
    model.performRedo();
    expect(couple.explicit).toBe(false);
  });

  it('A11 an exported couple leaves the default include set but stays in the Files list', () => {
    const model = newModel();
    const v = pushEntry(model, 'v1', 'video', '/m/v1.mp4');
    pushEntry(model, 'a1', 'audio', '/m/a1.wav');
    const couple = addCouple(model, v, 'a1', 'confident', []);
    couple.exportedAt = '2026-11-05T08:14:03Z';
    expect(couple.included).toBe(false);
    expect(couple.inFilesList).toBe(true);
    model.toggleInclude(couple); // tick it back on
    expect(couple.explicit).toBe(true);
    expect(couple.included).toBe(true);
  });

  it('A12 flagged couples are in To fix, cannot be included, and sort above Ready', () => {
    const model = newModel();
    const v1 = pushEntry(model, 'v1', 'video', '/m/v1.mp4');
    const v2 = pushEntry(model, 'v2', 'video', '/m/v2.mp4');
    pushEntry(model, 'a1', 'audio', '/m/a1.wav');
    const good = new CoupleRow(v1, makeRow('v1', 'a1', 'confident', []), model.engine);
    const pick = new CoupleRow(v2, makeRow('v2', null, 'ambiguous', ['a1', 'a2']), model.engine);
    model.couples = [good, pick];
    expect(pick.verdict).not.toBe('good');
    expect(pick.canInclude).toBe(false);
    expect(pick.included).toBe(false);
    model.toggleInclude(pick); // no checkbox ⇒ no-op
    expect(pick.explicit).toBeNull();
    expect(model.toFix).toEqual([pick]);
    expect(model.orderedCouples).toEqual([pick, good]);
  });

  it('A13 a manual re-pair clears explicit/exportedAt; undo restores them', () => {
    const model = newModel();
    model.engine.matchPair = () => null;
    const v = pushEntry(model, 'v1', 'video', '/m/v1.mp4');
    pushEntry(model, 'a1', 'audio', '/m/a1.wav');
    pushEntry(model, 'b1', 'audio', '/m/b1.wav');
    const couple = addCouple(model, v, 'a1', 'confident', ['b1']);
    couple.explicit = true;
    couple.exportedAt = '2026-11-05T08:14:03Z';

    model.repair(couple, 'b1');
    expect(couple.selectedAudioKey).toBe('b1');
    expect(couple.pinned).toBe(true);
    expect(couple.explicit).toBeNull();
    expect(couple.exportedAt).toBeNull();
    expect(model.canUndoManualPick(couple)).toBe(true);

    model.undoManualPick(couple);
    expect(couple.selectedAudioKey).toBe('a1');
    expect(couple.pinned).toBe(false);
    expect(couple.explicit).toBe(true);
    expect(couple.exportedAt).toBe('2026-11-05T08:14:03Z');
    expect(model.canUndoManualPick(couple)).toBe(false);
  });

  it('A14 the replace confirmation is asked only for a PINNED couple (D9)', () => {
    const model = newModel();
    const v = pushEntry(model, 'v1', 'video', '/m/v1.mp4');
    pushEntry(model, 'a1', 'audio', '/m/a1.wav');
    pushEntry(model, 'b1', 'audio', '/m/b1.wav');
    pushEntry(model, 'c1', 'audio', '/m/c1.wav');
    const couple = addCouple(model, v, 'a1', 'confident', []);
    const asked: string[] = [];
    const w = window as unknown as { confirm: (m: string) => boolean };
    const original = w.confirm;
    w.confirm = (m: string) => {
      asked.push(m);
      return true;
    };
    try {
      model.repairDrop(couple, 'b1'); // included but unpinned: replaces without asking
      expect(asked).toHaveLength(0);
      expect(couple.selectedAudioKey).toBe('b1');
      model.repairDrop(couple, 'c1'); // now pinned: asks first
      expect(asked).toHaveLength(1);
      expect(asked[0]).toContain('Replace the audio you chose?');
      expect(couple.selectedAudioKey).toBe('c1');
    } finally {
      w.confirm = original;
    }
  });

  it('A15 explicit/exportedAt round-trip through queue.json; an approve-era file loads with defaults', () => {
    const dir = mkdtempSync(join(tmpdir(), 'couple-queue-'));
    const vPath = join(dir, 'v1.mp4');
    const aPath = join(dir, 'a1.wav');
    writeFileSync(vPath, 'v');
    writeFileSync(aPath, 'a');
    const model = newModel();
    const v = pushEntry(model, 'v1', 'video', vPath);
    pushEntry(model, 'a1', 'audio', aPath);
    const couple = addCouple(model, v, 'a1', 'confident', []);
    couple.explicit = false;
    couple.exportedAt = isoNow(new Date('2026-11-05T08:14:03.456Z'));
    model.saveQueue();
    const saved = QueueStore.load()!;
    expect(saved.schema).toBe('couple-queue/1');
    expect(saved.couples[0].explicit).toBe(false);
    expect(saved.couples[0].exportedAt).toBe('2026-11-05T08:14:03Z'); // mac .iso8601 form
    expect('approved' in saved.couples[0]).toBe(false);

    // A file written under the approve model: `approved` present, no include fields.
    const legacy = { ...saved.couples[0], approved: true } as Record<string, unknown>;
    delete legacy.explicit;
    delete legacy.exportedAt;
    const restored = new CoupleRow(v, makeRow('v1', 'a1', 'confident', []), model.engine);
    restored.explicit = (legacy.explicit as boolean | undefined) ?? null;
    restored.exportedAt = (legacy.exportedAt as string | undefined) ?? null;
    expect(restored.included).toBe(true);
  });

  it('A16 clearSessionCounts counts INCLUDED couples (D7)', () => {
    const model = newModel();
    const v = pushEntry(model, 'v1', 'video', '/m/v1.mp4');
    pushEntry(model, 'a1', 'audio', '/m/a1.wav');
    const couple = addCouple(model, v, 'a1', 'confident', []);
    expect(model.clearSessionCounts()).toEqual({ media: 2, couples: 1, included: 1 });
    model.toggleInclude(couple);
    expect(model.clearSessionCounts()).toEqual({ media: 2, couples: 1, included: 0 });
  });

  it('A17 the layout switch maps a Pan monitor to Synced while compact and back (D10)', async () => {
    const model = newModel();
    model.layoutMode = 'full';
    model.audition.selectMode('pan');
    expect(model.audition.mode).toBe('pan');
    await model.requestLayoutMode('compact');
    expect(model.layoutMode).toBe('compact');
    expect(model.audition.mode).toBe('overlay');
    await model.requestLayoutMode('full');
    expect(model.audition.mode).toBe('pan');
  });
});

