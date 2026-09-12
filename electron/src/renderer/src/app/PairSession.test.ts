import { afterEach, describe, expect, it, vi } from 'vitest';
import { Engine } from '../kit/Engine';
import type { SessionHandle } from '../kit/Session';
import type { TrackHandle } from '../kit/Track';
import { PairSession } from './PairSession';

const engines: Engine[] = [];

afterEach(() => {
  vi.restoreAllMocks();
  for (const engine of engines) engine.free();
  engines.length = 0;
});

function viewportPair() {
  const engine = new Engine();
  engines.push(engine);
  const pair = new PairSession(engine);
  const domain = vi.spyOn(pair, 'stageDomain').mockReturnValue([0, 100]);
  return { engine, pair, domain };
}

describe('PairSession viewport', () => {
  it('zooms about the cursor, clamps navigation, and notifies once per command', () => {
    const { pair } = viewportPair();
    const changed = vi.fn();
    pair.subscribe(changed);
    expect(pair.view()).toEqual([0, 100]);
    expect(pair.viewZoomed).toBe(false);

    pair.zoomView(0.5, 25);
    expect(pair.view()).toEqual([12.5, 62.5]);
    pair.panView(100);
    expect(pair.view()).toEqual([50, 100]);
    pair.zoomView(0.001, 75);
    expect(pair.view()).toEqual([74.75, 75.25]);
    pair.panView(-1000);
    expect(pair.view()).toEqual([0, 0.5]);
    expect(changed).toHaveBeenCalledTimes(4);

    pair.zoomView(1000, 0);
    expect(pair.view()).toEqual([0, 100]);
    expect(pair.viewZoomed).toBe(false);
    pair.fitView();
    expect(changed).toHaveBeenCalledTimes(6);
  });

  it('keeps the saved zoom when a changing track union temporarily fills the viewport', () => {
    const { pair, domain } = viewportPair();
    pair.zoomView(0.5, 50);
    expect(pair.view()).toEqual([25, 75]);
    domain.mockReturnValue([0, 40]);
    expect(pair.view()).toEqual([0, 40]);
    expect(pair.viewZoomed).toBe(true); // Fit stays available until explicitly requested.
    domain.mockReturnValue([0, 100]);
    expect(pair.view()).toEqual([25, 75]);

    pair.fitView();
    domain.mockReturnValue([-20, 120]);
    expect(pair.view()).toEqual([-20, 120]);
    expect(pair.viewZoomed).toBe(false);
  });

  it('starts fitted when a new pair is loaded without adding a reset notification', () => {
    const { engine, pair } = viewportPair();
    pair.zoomView(0.5, 50);
    // No media is needed to check shell viewport state across a session load.
    const session = { free() {} } as unknown as SessionHandle;
    const track = {} as TrackHandle;
    vi.spyOn(engine, 'sessionNew').mockReturnValue(session);
    const changed = vi.fn();
    pair.subscribe(changed);
    pair.load(track, track, null);
    expect(pair.view()).toEqual([0, 100]);
    expect(pair.viewZoomed).toBe(false);
    expect(changed).toHaveBeenCalledTimes(2); // teardown, then completed load
  });
});

function pivotPair(times = [10, 20, 30]) {
  const engine = new Engine();
  engines.push(engine);
  const pair = new PairSession(engine);
  const state = { offsetMs: 125, driftPpm: 40 };
  const calls: string[] = [];
  const session = {
    state: () => state,
    setDriftPpmLive: (v: number) => { calls.push('drift'); state.driftPpm = v; },
    setOffsetMs: (v: number) => { calls.push('offset'); state.offsetMs = v; },
    commitDrift: vi.fn(),
    measureLocal: vi.fn(),
  };
  pair.session = session as unknown as SessionHandle;
  pair.anchors = times.map(t => ({
    t, winMs: 500, auto: true, measurement: null, measurementCenter: null,
  }));
  pair.subscribe(() => calls.push('notify'));
  return { pair, state, calls, session };
}

describe('PairSession pivot drag', () => {
  it.each([
    { index: 0, reference: 2, delta: 8 },
    { index: 1, reference: 0, delta: -8 },
    { index: 2, reference: 0, delta: 0 },
  ])('uses the same pivot for anchor $index and a stage drag', ({ index, reference, delta }) => {
    const stage = pivotPair();
    const anchor = pivotPair();
    const atT = anchor.pair.anchors[index].t;
    const refT = anchor.pair.anchors[reference].t;
    const refLag = anchor.pair.mapping().localLagMs(refT);
    const atLag = anchor.pair.mapping().localLagMs(atT);
    const timer = vi.spyOn(globalThis, 'setTimeout');

    stage.pair.pivotDriftDrag(atT, refT, delta);
    anchor.pair.pivotAnchorDrag(index, delta);

    expect(anchor.state).toEqual(stage.state);
    expect(anchor.pair.mapping().localLagMs(refT)).toBeCloseTo(refLag, 10);
    expect(anchor.pair.mapping().localLagMs(atT)).toBeCloseTo(atLag + delta, 10);
    for (const result of [stage, anchor]) {
      expect(result.calls).toEqual(['drift', 'offset', 'notify']);
      expect(result.session.commitDrift).not.toHaveBeenCalled();
      expect(result.session.measureLocal).not.toHaveBeenCalled();
    }
    expect(timer).not.toHaveBeenCalled();
  });

  it.each([
    { span: 4.999, index: 0, moves: false },
    { span: 4.999, index: 1, moves: false },
    { span: 5, index: 0, moves: true },
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

  it.each(['no session', 'syncing'])('leaves both drag routes inert with %s', condition => {
    const { pair, calls } = pivotPair();
    if (condition === 'no session') pair.session = null;
    else pair.syncStatus = 'syncing';
    pair.pivotDriftDrag(20, 10, 8);
    pair.pivotAnchorDrag(1, 8);
    expect(calls).toEqual([]);
  });

  it('ignores an invalid anchor without writing or notifying', () => {
    const { pair, calls } = pivotPair();
    pair.pivotAnchorDrag(-1, 8);
    pair.pivotAnchorDrag(3, 8);
    expect(calls).toEqual([]);
  });
});
