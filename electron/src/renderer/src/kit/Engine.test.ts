// Engine.test.ts (F5a) — the kit handle over the REAL couple-node binding:
// the frozen resolver defaults, the sync/async probe error surface (F5c), and
// the empty-input short-circuit. No media is touched (a missing path is enough
// to exercise the CoreError path).
import { describe, it, expect } from 'vitest';
import { Engine, CoreError } from './Engine';

describe('Engine', () => {
  it('E1 resolveParamsDefaultJSON carries the frozen resolver defaults', () => {
    const engine = new Engine({});
    const json = engine.resolveParamsDefaultJSON();
    expect(json).not.toBeNull();
    const obj = JSON.parse(json!) as { zMin: number; zMargin: number };
    expect(obj.zMin).toBe(8.5);
    expect(obj.zMargin).toBe(6.0);
    engine.free();
  });

  it('E2 probeAsync rejects with a CoreError for an unreadable path', async () => {
    const engine = new Engine({});
    await expect(engine.probeAsync('/nonexistent/definitely-missing.wav')).rejects.toBeInstanceOf(
      CoreError
    );
    engine.free();
  });

  it('E3 sync probe throws the same CoreError surface', () => {
    const engine = new Engine({});
    expect(() => engine.probe('/nonexistent/definitely-missing.wav')).toThrow(CoreError);
    engine.free();
  });

  it('E4 resolve short-circuits empty video input', () => {
    const engine = new Engine({});
    expect(engine.resolve([], [], null)).toEqual([]);
    engine.free();
  });
});
