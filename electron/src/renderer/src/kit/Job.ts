// Job.ts — typed wrapper over a core job (§4.6); TS twin of CoupleKit/Job.swift.
// Progress arrives on the JS task queue (via the binding's TSFN); the terminal
// phase settles `value` and frees the job handle.
import { native, type ProgressCallback } from './native';
import type { Engine } from './Engine';
import { CoreError } from './Engine';
import type { ProgressUpdate } from './Wire';

export class JobCancelledError extends Error {
  constructor() {
    super('job cancelled');
    this.name = 'JobCancelledError';
  }
}

export class Job<T> {
  private readonly engine: Engine;
  private id: bigint;
  private progressListeners: Array<(u: ProgressUpdate) => void> = [];

  /** Settles on the terminal callback: "done" ⇒ decoded value, "error" ⇒
   * CoreError, "cancelled" ⇒ JobCancelledError. */
  readonly value: Promise<T>;

  private constructor(engine: Engine, start: (cb: ProgressCallback) => bigint, decode: (payload: string) => T) {
    this.engine = engine;
    this.id = 0n;
    this.value = new Promise<T>((resolve, reject) => {
      const settle = (fn: () => void) => {
        if (this.id !== 0n) {
          native.jobFree(engine.ctx, this.id);
          this.id = 0n;
        }
        this.progressListeners = [];
        fn();
      };
      const cb: ProgressCallback = (phase, fraction, payloadJson) => {
        switch (phase) {
          case 'done':
            settle(() => {
              try {
                resolve(decode(payloadJson ?? ''));
              } catch (e) {
                reject(e);
              }
            });
            break;
          case 'error':
            settle(() => reject(Job.decodeError(payloadJson)));
            break;
          case 'cancelled':
            settle(() => reject(new JobCancelledError()));
            break;
          default:
            for (const l of this.progressListeners) l({ phase, fraction });
        }
      };
      const id = start(cb);
      if (id === 0n) {
        reject(engine.takeError());
      } else {
        this.id = id;
      }
    });
    // Unhandled-rejection hygiene: callers that only use onProgress still get
    // a settled promise; swallow here, surface via explicit awaits.
    this.value.catch(() => {});
  }

  static start<T>(engine: Engine, startFn: (cb: ProgressCallback) => bigint, decode: (payload: string) => T): Job<T> {
    return new Job<T>(engine, startFn, decode);
  }

  onProgress(listener: (u: ProgressUpdate) => void): this {
    this.progressListeners.push(listener);
    return this;
  }

  /** Cancellation is observed by the core within 100ms (§4.6); the terminal
   * "cancelled" callback settles `value`. */
  cancel(): void {
    if (this.id !== 0n) native.jobCancel(this.engine.ctx, this.id);
  }

  private static decodeError(payloadJson: string | null): CoreError {
    if (!payloadJson) return new CoreError('internal', 'job failed');
    try {
      const obj = JSON.parse(payloadJson) as { error?: unknown; message?: string };
      let kind = 'internal';
      if (typeof obj.error === 'string') kind = obj.error;
      else if (obj.error && typeof obj.error === 'object') {
        kind = Object.keys(obj.error)[0] ?? 'internal';
      }
      return new CoreError(kind, obj.message ?? payloadJson);
    } catch {
      return new CoreError('internal', payloadJson);
    }
  }
}
