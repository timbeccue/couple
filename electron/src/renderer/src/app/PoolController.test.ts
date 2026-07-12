// PoolController.test.ts (F5a) — the pure Finder-style pool-selection transition
// (poolSelect) and the pool-list height clamp (poolListHeight). Framework-free
// ports of PoolSelection.swift / PoolRailLayout.swift; no binding needed.
import { describe, it, expect } from 'vitest';
import { poolSelect, poolListHeight, POOL_MIN_LIST_HEIGHT, type PoolSelectModifier } from './PoolController';

const ORDER = ['a', 'b', 'c', 'd', 'e'];

function sel(
  clicked: string,
  modifier: PoolSelectModifier,
  current: string[],
  primary: string | null,
  anchor: string | null,
  ordered: string[] = ORDER
) {
  return poolSelect({ current: new Set(current), primary, anchor, clicked, ordered, modifier });
}

describe('poolSelect', () => {
  it('S1 plain click replaces selection/primary/anchor with the clicked key', () => {
    const r = sel('c', 'plain', ['a', 'b'], 'a', 'a');
    expect([...r.selection]).toEqual(['c']);
    expect(r.primary).toBe('c');
    expect(r.anchor).toBe('c');
  });

  it('S2 ⌘ toggle-on adds the key and it leads (primary + anchor)', () => {
    const r = sel('c', 'command', ['a', 'b'], 'a', 'a');
    expect(r.selection.has('c')).toBe(true);
    expect(r.primary).toBe('c');
    expect(r.anchor).toBe('c');
  });

  it('S3 ⌘ toggle-off a non-primary keeps the primary; anchor falls back to primary', () => {
    const r = sel('b', 'command', ['a', 'b', 'c'], 'a', 'b');
    expect([...r.selection].sort()).toEqual(['a', 'c']);
    expect(r.primary).toBe('a');
    expect(r.anchor).toBe('a');
  });

  it('S4 ⌘ removing the primary re-homes to the nearest remaining (tie ⇒ earlier)', () => {
    const r = sel('c', 'command', ['a', 'c', 'e'], 'c', 'c');
    expect([...r.selection].sort()).toEqual(['a', 'e']);
    expect(r.primary).toBe('a'); // a and e both dist 2 from c ⇒ earlier in order
    expect(r.anchor).toBe('a');
  });

  it('S5 ⌘ removing the last selected clears all three', () => {
    const r = sel('a', 'command', ['a'], 'a', 'a');
    expect(r.selection.size).toBe(0);
    expect(r.primary).toBeNull();
    expect(r.anchor).toBeNull();
  });

  it('S6 ⇧ inclusive range from anchor→clicked, both directions; primary=clicked, anchor preserved', () => {
    const fwd = sel('d', 'shift', ['b'], 'b', 'b');
    expect([...fwd.selection]).toEqual(['b', 'c', 'd']);
    expect(fwd.primary).toBe('d');
    expect(fwd.anchor).toBe('b');

    const rev = sel('b', 'shift', ['d'], 'd', 'd');
    expect([...rev.selection]).toEqual(['b', 'c', 'd']);
    expect(rev.primary).toBe('b');
    expect(rev.anchor).toBe('d');
  });

  it('S7 ⇧ without a usable anchor degrades to a plain click', () => {
    const r = sel('c', 'shift', ['a'], 'a', null);
    expect([...r.selection]).toEqual(['c']);
    expect(r.primary).toBe('c');
    expect(r.anchor).toBe('c');
  });

  it('S8 a click on an unknown key is inert', () => {
    const r = sel('z', 'plain', ['a', 'b'], 'a', 'b');
    expect([...r.selection].sort()).toEqual(['a', 'b']);
    expect(r.primary).toBe('a');
    expect(r.anchor).toBe('b');
  });
});

describe('poolListHeight', () => {
  it('S9 floors at 168, bounds by poolContent, and grows into spare between', () => {
    expect(POOL_MIN_LIST_HEIGHT).toBe(168);
    // spare small ⇒ the floor wins
    expect(poolListHeight(200, 180, 500)).toBe(168);
    // ample spare ⇒ bounded by the pool's own content
    expect(poolListHeight(1000, 0, 300)).toBe(300);
    // spare between floor and content ⇒ grows into the spare
    expect(poolListHeight(600, 100, 1000)).toBe(500);
  });
});
