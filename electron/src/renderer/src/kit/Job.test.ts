// Job terminal errors have their own empty-payload fallback, but otherwise
// expose the same CoreError contract as synchronous and async engine errors.
import { describe, it, expect, vi } from 'vitest';
import { Engine, CoreError } from './Engine';
import { Job } from './Job';
import { native, type ProgressCallback } from './native';

describe('Job terminal errors', () => {
  it.each([
    ['string kind', '{"error":"decode","message":"cannot decode"}', 'decode', 'cannot decode'],
    ['null payload', null, 'internal', 'job failed'],
    ['empty payload', '', 'internal', 'job failed'],
  ])('frees the handle and rejects with %s', async (_name, payload, kind, message) => {
    const engine = new Engine();
    // Inject the job source to exercise the wrapper's fallback and ownership.
    const free = vi.spyOn(native, 'jobFree').mockImplementation(() => {});
    const decode = vi.fn((value: string) => value);
    let callback!: ProgressCallback;
    try {
      const job = Job.start(engine, (cb) => {
        callback = cb;
        return 123n;
      }, decode);
      callback('error', 1, payload);
      expect(free).toHaveBeenCalledExactlyOnceWith(engine.ctx, 123n);
      await expect(job.value).rejects.toBeInstanceOf(CoreError);
      await expect(job.value).rejects.toMatchObject({ name: 'CoreError', kind, message });
      expect(decode).not.toHaveBeenCalled();
    } finally {
      free.mockRestore();
      engine.free();
    }
  });
});
