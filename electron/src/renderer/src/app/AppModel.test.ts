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
import { QueueStore } from './QueueStore';
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
  return { audioKey, audioName: `${audioKey}.wav`, z: 12, lagMs: 0, coverage: { start: 0, end: 1 } };
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
    expect(model.toApprove).toContain(couple);
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
});
