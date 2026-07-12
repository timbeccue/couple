// CoupleRow.test.ts (F5a) — one queue row's verdict / actionWord / forced
// lifecycle. The verdict dot is the PURE core match_quality policy through the
// REAL binding (never derived from the act of selecting); this pins the
// manual-pick flow the 87136e4 parity bug lived in (pick ⇒ questionable until
// evidence greens it; a forced or sub-floor pick never greens).
import { describe, it, expect } from 'vitest';
import { Engine } from '../kit/Engine';
import type { ResolvedRow, RowCandidate, RowState } from '../kit/Wire';
import { CoupleRow } from './CoupleRow';
import { PoolEntry } from './MediaPool';
import type { TrackHandle } from '../kit/Track';

const engine = new Engine({});
const fakeHandle = { id: 1n, free() {} } as unknown as TrackHandle;

function makeEntry(id: string, role: 'video' | 'audio', path: string): PoolEntry {
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

function makeRow(
  videoKey: string,
  selectedKey: string | null,
  state: RowState,
  alternateKeys: string[]
): ResolvedRow {
  return {
    videoKey,
    videoName: `${videoKey}.mp4`,
    state,
    chip: state,
    selected: selectedKey ? cand(selectedKey) : null,
    alternates: alternateKeys.map(cand),
    resolution: { clusters: 1 },
  };
}

function video(): PoolEntry {
  return makeEntry('v1', 'video', '/m/v1.mp4');
}

describe('CoupleRow', () => {
  it('C1 adopts a confident automatic selection', () => {
    const row = new CoupleRow(video(), makeRow('v1', 'a1', 'confident', []), engine);
    expect(row.candidates.map((c) => c.audioKey)).toEqual(['a1']);
    expect(row.selectedAudioKey).toBe('a1');
    expect(row.selectionProvenance).toBe('automatic');
    expect(row.verdict).toBe('good');
    expect(row.actionWord).toBeNull();
  });

  it('C2 an ambiguous unselected row needs a pick', () => {
    const row = new CoupleRow(video(), makeRow('v1', null, 'ambiguous', ['a1', 'a2']), engine);
    expect(row.verdict).toBe('questionable');
    expect(row.actionWord).toBe('pick one');
    expect(row.needsResolve).toBe(true);
    expect(row.selectionProvenance).toBe('none');
  });

  it('C3 a no-match row has no audio', () => {
    const row = new CoupleRow(video(), makeRow('v1', null, 'noMatch', []), engine);
    expect(row.verdict).toBe('none');
    expect(row.actionWord).toBe('no audio');
  });

  it('C4 a manual pick is questionable until its evidence is confident', () => {
    const row = new CoupleRow(video(), makeRow('v1', 'a1', 'confident', []), engine);
    row.applyManualPick('a2', 12, 8.5);
    expect(row.selectionProvenance).toBe('manual');
    expect(row.pinned).toBe(true);
    expect(row.manualMatchConfident).toBeNull();
    expect(row.forced).toBe(false);
    expect(row.verdict).toBe('questionable');
    row.manualMatchConfident = true;
    expect(row.verdict).toBe('good');
  });

  it('C5 a sub-floor manual pick is forced and never greens', () => {
    const row = new CoupleRow(video(), makeRow('v1', 'a1', 'confident', []), engine);
    row.applyManualPick('a2', 3, 8.5);
    expect(row.forced).toBe(true);
    expect(row.actionWord).toBe('forced');
    expect(row.verdict).toBe('questionable');
    row.manualMatchConfident = true;
    expect(row.verdict).toBe('questionable'); // forced overrides confident evidence
  });

  it('C6 a settled sync re-checks the forced verdict for a manual pairing', () => {
    const row = new CoupleRow(video(), makeRow('v1', 'a1', 'confident', []), engine);
    row.applyManualPick('a2', 0, 8.5); // fresh pool drag carries no z
    expect(row.forced).toBe(false);
    row.pair.syncStatus = 'failed';
    row.pair.invalidate();
    expect(row.forced).toBe(true);
    row.pair.syncStatus = 'synced';
    row.pair.invalidate();
    expect(row.forced).toBe(false);
  });
});
