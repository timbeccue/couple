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
