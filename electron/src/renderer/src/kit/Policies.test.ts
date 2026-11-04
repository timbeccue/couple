// Policies.test.ts (F5a) — the pure cpl_policy_* decisions through the REAL
// binding, so every assertion is CORE's own decision (no re-encoded table).
// Also pins the CONSERVATIVE fallbacks: a malformed request must degrade to the
// protected / empty plan, never a silent optimistic default.
import { describe, it, expect } from 'vitest';
import * as Policies from './Policies';

describe('matchQuality', () => {
  it('P1 automatic selection carries the resolver auto-verdict', () => {
    expect(
      Policies.matchQuality({
        forced: false,
        selection: 'automatic',
        autoVerdict: 'confident',
        manualMatchConfident: null,
      })
    ).toBe('confident');
    expect(
      Policies.matchQuality({
        forced: false,
        selection: 'automatic',
        autoVerdict: 'questionable',
        manualMatchConfident: null,
      })
    ).toBe('questionable');
  });

  it('P2 a forced pairing is never confident', () => {
    expect(
      Policies.matchQuality({
        forced: true,
        selection: 'automatic',
        autoVerdict: 'confident',
        manualMatchConfident: null,
      })
    ).toBe('questionable');
  });

  it('P3 a manual pick greens only once its evidence is confident', () => {
    const base = { forced: false, selection: 'manual' as const, autoVerdict: 'confident' as const };
    expect(Policies.matchQuality({ ...base, manualMatchConfident: null })).toBe('questionable');
    expect(Policies.matchQuality({ ...base, manualMatchConfident: true })).toBe('confident');
    expect(Policies.matchQuality({ ...base, manualMatchConfident: false })).toBe('questionable');
  });

  it('P4 an unknown selection degrades to the conservative fallback', () => {
    expect(
      Policies.matchQuality({
        forced: false,
        selection: 'bogus' as never,
        autoVerdict: 'confident',
        manualMatchConfident: null,
      })
    ).toBe('questionable');
  });
});

describe('repairForced', () => {
  it('P5 sub-floor or failed pairings are forced; strong / no-z pairings are not', () => {
    expect(Policies.repairForced({ candidateZ: 3, zMin: 8.5, syncFailed: false })).toBe(true);
    expect(Policies.repairForced({ candidateZ: 0, zMin: 8.5, syncFailed: false })).toBe(false);
    expect(Policies.repairForced({ candidateZ: 12, zMin: 8.5, syncFailed: false })).toBe(false);
    expect(Policies.repairForced({ candidateZ: 0, zMin: 8.5, syncFailed: true })).toBe(true);
  });
});

describe('repairDrop', () => {
  const base = {
    hasSelectedCouple: true,
    payloadIsAudio: true,
    videoAvailable: true,
    currentSelectedKey: 'a1',
    droppedKey: 'a1',
    approved: false,
  };

  it('P6 outcomes: same-key no-op, unapproved accept, approved confirm, non-audio reject', () => {
    expect(Policies.repairDrop({ ...base, droppedKey: 'a1' })).toBe('noOp');
    expect(Policies.repairDrop({ ...base, droppedKey: 'a2', approved: false })).toBe('accept');
    expect(Policies.repairDrop({ ...base, droppedKey: 'a2', approved: true })).toBe('needsConfirmation');
    expect(Policies.repairDrop({ ...base, droppedKey: 'a2', payloadIsAudio: false })).toBe('reject');
  });
});

describe('conservative fallbacks', () => {
  it('P7 a malformed rowMergePlan request keeps the protected-row plan', () => {
    const plan = Policies.rowMergePlan({
      protected: true,
      currentSelectedKey: 'a1',
      currentCandidateKeys: ['a1', 'a2'],
      freshSelectedKey: 'a1',
      freshCandidateKeys: 'nope' as never,
    });
    expect(plan).toEqual({
      selectedAudioKey: 'a1',
      candidateKeys: ['a1', 'a2'],
      adoptFreshDisplay: false,
      rebuildSession: false,
      clearForced: false,
    });
  });

  it('P8 a malformed mediaRemovalPlan request yields the empty plan', () => {
    const plan = Policies.mediaRemovalPlan({
      selectedKeys: ['a1'],
      poolKeys: ['a1'],
      couples: 'nope' as never,
    });
    expect(plan).toEqual({
      removableKeys: [],
      needsConfirmation: false,
      affectedCouples: 0,
      affectedApproved: 0,
    });
  });
});

describe('monitorPlan (A/B solos, UX-DECISIONS 2026-11-03)', () => {
  it('first flip from both goes to camera and remembers the mode left', () => {
    expect(Policies.monitorPlan({ action: 'flip', current: 'overlay' })).toEqual({
      next: 'camera',
      lastSide: 'camera',
      lastBoth: 'overlay',
    });
  });

  it('flip alternates sides; both restores the mode left; flip resumes the last side', () => {
    const p1 = Policies.monitorPlan({ action: 'flip', current: 'pan' });
    const p2 = Policies.monitorPlan({ action: 'flip', current: p1.next, lastSide: p1.lastSide, lastBoth: p1.lastBoth });
    expect(p2.next).toBe('external');
    const p3 = Policies.monitorPlan({ action: 'both', current: p2.next, lastSide: p2.lastSide, lastBoth: p2.lastBoth });
    expect(p3.next).toBe('pan');
    const p4 = Policies.monitorPlan({ action: 'flip', current: p3.next, lastSide: p3.lastSide, lastBoth: p3.lastBoth });
    expect(p4.next).toBe('external');
  });

  it('select adopts the choice', () => {
    expect(Policies.monitorPlan({ action: 'select', current: 'overlay', chosen: 'external' })).toEqual({
      next: 'external',
      lastSide: 'external',
      lastBoth: 'overlay',
    });
  });

  it('malformed request keeps the current mode (conservative fallback)', () => {
    // @ts-expect-error — deliberately malformed action
    expect(Policies.monitorPlan({ action: 'junk', current: 'pan', lastBoth: 'pan' }).next).toBe('pan');
  });
});
