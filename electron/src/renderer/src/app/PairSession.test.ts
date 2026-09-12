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
