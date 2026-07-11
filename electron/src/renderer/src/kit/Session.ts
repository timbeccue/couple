// Session.ts — pair-session handle (§4.5); TS twin of CoupleKit/Session.swift.
// Setters are the core's lock-free hot path (gesture rate).
import { native } from './native';
import type { Engine } from './Engine';
import { Job } from './Job';
import type { AlignmentState, SyncResult } from './Wire';

export type DriftMode = 'resample' | 'none';

export class SessionHandle {
  readonly engine: Engine;
  readonly id: bigint;

  constructor(engine: Engine, id: bigint) {
    this.engine = engine;
    this.id = id;
  }

  free(): void {
    native.sessionFree(this.engine.ctx, this.id);
  }

  state(): AlignmentState {
    const s = native.stateJson(this.engine.ctx, this.id);
    if (s === null) throw this.engine.takeError();
    return JSON.parse(s) as AlignmentState;
  }

  setState(state: AlignmentState): void {
    if (native.setStateJson(this.engine.ctx, this.id, JSON.stringify(state)) !== 0) {
      throw this.engine.takeError();
    }
  }

  setOffsetMs(v: number): void {
    native.setOffsetMs(this.engine.ctx, this.id, v);
  }

  setDriftPpmLive(v: number): void {
    native.setDriftPpmLive(this.engine.ctx, this.id, v);
  }

  commitDrift(): void {
    native.commitDrift(this.engine.ctx, this.id);
  }

  setDriftMode(mode: DriftMode): void {
    native.setDriftMode(this.engine.ctx, this.id, mode === 'resample' ? 0 : 1);
  }

  setVerified(verified: boolean): void {
    native.setVerified(this.engine.ctx, this.id, verified);
  }

  measureLocal(tc: number, halfWinS: number, centerMs: number, spanMs: number) {
    const s = native.measureLocalJson(this.engine.ctx, this.id, tc, halfWinS, centerMs, spanMs);
    if (s === null) throw this.engine.takeError();
    return s === 'null' ? null : (JSON.parse(s) as { lagMs: number; ncc: number });
  }

  /** Unseeded full auto-sync (§4.4). Seed choice is `Policies.syncSeed`'s call. */
  autosyncStart(paramsJSON: string | null): Job<SyncResult> {
    return Job.start(
      this.engine,
      (cb) => native.autosyncStart(this.engine.ctx, this.id, paramsJSON, cb),
      (p) => JSON.parse(p) as SyncResult
    );
  }

  /** Seeded auto-sync (§11.3): begins from a resolver-proved / user-placed offset. */
  autosyncStartSeeded(paramsJSON: string | null, seedOffsetMs: number, seedDriftPpm: number): Job<SyncResult> {
    return Job.start(
      this.engine,
      (cb) =>
        native.autosyncStartSeeded(this.engine.ctx, this.id, paramsJSON, seedOffsetMs, seedDriftPpm, cb),
      (p) => JSON.parse(p) as SyncResult
    );
  }

  exportStart(opts: import('./Wire').ExportOptions): Job<import('./Wire').ExportReport> {
    return Job.start(
      this.engine,
      (cb) => native.exportStart(this.engine.ctx, this.id, JSON.stringify(opts), cb),
      (p) => JSON.parse(p) as import('./Wire').ExportReport
    );
  }
}
