// Engine.test.ts (F5a) — the kit handle over the REAL couple-node binding:
// the frozen resolver defaults, the sync/async probe error surface (F5c), and
// the empty-input short-circuit. No media is touched (a missing path is enough
// to exercise the CoreError path).
import { describe, it, expect, vi } from 'vitest';
import { Engine, CoreError } from './Engine';
import { CoreError as SharedCoreError } from './CoreError';
import { native } from './native';

describe('Engine', () => {
  it.each([null, ''])('keeps the engine fallback for absent error payload %s', (payload) => {
    const take = vi.spyOn(native, 'takeErrorJson').mockReturnValue(payload);
    try {
      expect(Engine.staticTakeError()).toMatchObject({
        name: 'CoreError', kind: 'internal', message: 'unknown core error',
      });
    } finally {
      take.mockRestore();
    }
  });

  it.each([
    ['string kind', '{"error":"decode","message":"cannot decode"}', 'decode', 'cannot decode'],
    ['object kind', '{"error":{"io":"missing"},"message":"file missing"}', 'io', 'file missing'],
    ['first object key', '{"error":{"io":{},"decode":{}}}', 'io', '{"error":{"io":{},"decode":{}}}'],
    ['empty message', '{"error":"invalid","message":""}', 'invalid', ''],
    ['missing message', '{"error":"invalid"}', 'invalid', '{"error":"invalid"}'],
    ['missing kind', '{"message":"failure"}', 'internal', 'failure'],
    ['empty kind object', '{"error":{},"message":"failure"}', 'internal', 'failure'],
    ['null kind', '{"error":null,"message":"failure"}', 'internal', 'failure'],
    ['malformed JSON', '{broken', 'internal', '{broken'],
    ['null JSON', 'null', 'internal', 'null'],
    ['string JSON', '"failure"', 'internal', '"failure"'],
    ['empty payload', '', 'internal', ''],
  ])('decodes %s without changing error details', (_name, payload, kind, message) => {
    const error = Engine.coreErrorFromJSON(payload);
    expect(error).toBeInstanceOf(SharedCoreError);
    expect(error).toMatchObject({ name: 'CoreError', kind, message });
  });

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
