import { afterEach, describe, expect, it, vi } from 'vitest';
import { Engine } from '../kit/Engine';
import type { SessionHandle } from '../kit/Session';
import { PairSession } from './PairSession';

const engines: Engine[] = [];

afterEach(() => {
  vi.restoreAllMocks();
  for (const engine of engines) engine.free();
  engines.length = 0;
});

function newPair() {
  const engine = new Engine();
  engines.push(engine);
  return new PairSession(engine);
}

describe('PairSession viewport', () => {
  it('keeps the saved zoom when a changing track union temporarily fills the viewport', () => {
    const pair = newPair();
    const domain = vi.spyOn(pair, 'stageDomain').mockReturnValue([0, 100]);
    const changed = vi.fn();
    pair.subscribe(changed);
    expect(pair.view()).toEqual([0, 100]);
    expect(pair.viewZoomed).toBe(false);

    pair.zoomView(0.5, 50);
    expect(pair.view()).toEqual([25, 75]);
    domain.mockReturnValue([0, 40]);
    expect(pair.view()).toEqual([0, 40]);
    expect(pair.viewZoomed).toBe(true); // Fit stays available until explicitly requested.
    domain.mockReturnValue([0, 100]);
    expect(pair.view()).toEqual([25, 75]);
    expect(changed).toHaveBeenCalledTimes(1);

    pair.fitView();
    domain.mockReturnValue([-20, 120]);
    expect(pair.view()).toEqual([-20, 120]);
    expect(pair.viewZoomed).toBe(false);
    expect(changed).toHaveBeenCalledTimes(2);
  });
});

function pivotPair(times = [10, 20, 30]) {
  const pair = newPair();
  const state = { offsetMs: 125, driftPpm: 40 };
  const calls: string[] = [];
  const session = {
    state: () => state,
    setDriftPpmLive: (v: number) => { calls.push('drift'); state.driftPpm = v; },
    setOffsetMs: (v: number) => { calls.push('offset'); state.offsetMs = v; },
    commitDrift: () => calls.push('commit'),
    measureLocal: () => calls.push('measure'),
  };
  pair.session = session as unknown as SessionHandle;
  pair.anchors = times.map(t => ({
    t, winMs: 500, auto: true, measurement: null, measurementCenter: null,
  }));
  pair.subscribe(() => calls.push('notify'));
  return { pair, state, calls };
}

describe('PairSession pivot drag', () => {
  it.each([
    { index: 0, reference: 2, delta: 8 },
    { index: 1, reference: 0, delta: -8 },
  ])('uses the same pivot for anchor $index and a stage drag', ({ index, reference, delta }) => {
    const stage = pivotPair();
    const anchor = pivotPair();
    const atT = anchor.pair.anchors[index].t;
    const refT = anchor.pair.anchors[reference].t;
    const refLag = anchor.pair.mapping().localLagMs(refT);
    const atLag = anchor.pair.mapping().localLagMs(atT);

    stage.pair.pivotDriftDrag(atT, refT, delta);
    anchor.pair.pivotAnchorDrag(index, delta);

    expect(anchor.state).toEqual(stage.state);
    expect(anchor.pair.mapping().localLagMs(refT)).toBeCloseTo(refLag, 10);
    expect(anchor.pair.mapping().localLagMs(atT)).toBeCloseTo(atLag + delta, 10);
    expect(stage.calls).toEqual(['drift', 'offset', 'notify']);
    expect(anchor.calls).toEqual(stage.calls);
  });

  it.each([
    { span: 4.999, index: 0, moves: false },
    { span: 5, index: 1, moves: true },
  ])('preserves the strict lever guard for span $span, anchor $index', ({ span, index, moves }) => {
    const stage = pivotPair([10, 10 + span]);
    const anchor = pivotPair([10, 10 + span]);
    const atT = anchor.pair.anchors[index].t;
    const refT = anchor.pair.anchors[1 - index].t;
    stage.pair.pivotDriftDrag(atT, refT, 8);
    anchor.pair.pivotAnchorDrag(index, 8);
    expect(anchor.state).toEqual(stage.state);
    expect(stage.calls).toEqual(moves ? ['drift', 'offset', 'notify'] : []);
    expect(anchor.calls).toEqual(stage.calls);
    if (!moves) expect(anchor.state).toEqual({ offsetMs: 125, driftPpm: 40 });
  });

  it('ignores an invalid anchor without writing or notifying', () => {
    const { pair, calls } = pivotPair();
    pair.pivotAnchorDrag(-1, 8);
    pair.pivotAnchorDrag(3, 8);
    expect(calls).toEqual([]);
  });
});
