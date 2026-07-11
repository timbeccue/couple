// Track.ts — imported-track handle (§4.2/§4.9); TS twin of CoupleKit/Track.swift.
// `band` fills caller-owned Int8Arrays (50µs budget — reuse buffers, no per-call
// allocation on the paint path).
import { native, type BandInfo } from './native';
import type { Engine } from './Engine';
import type { TrackMeta, TrackStats } from './Wire';

export interface OnsetPoint {
  t: number;
  score: number;
}

export class TrackHandle {
  private readonly engine: Engine;
  readonly meta: TrackMeta;

  constructor(engine: Engine, meta: TrackMeta) {
    this.engine = engine;
    this.meta = meta;
  }

  get id(): bigint {
    return this.meta.id;
  }

  free(): void {
    native.trackFree(this.engine.ctx, this.meta.id);
  }

  /** Fill `mins`/`maxs` (length >= nCols) with envelope columns for [t0, t1]. */
  band(t0: number, t1: number, nCols: number, mins: Int8Array, maxs: Int8Array): BandInfo {
    const info = native.band(this.engine.ctx, this.meta.id, t0, t1, nCols, mins, maxs);
    if (info === null) throw this.engine.takeError();
    return info;
  }

  /** Mono 48 kHz f32 window; returns frames written into `out`. */
  rawWindow(t0: number, t1: number, out: Float32Array): number {
    const frames = native.rawWindow(this.engine.ctx, this.meta.id, t0, t1, out);
    if (frames === null) throw this.engine.takeError();
    return frames;
  }

  rms(t0: number, t1: number): number {
    const v = native.rms(this.engine.ctx, this.meta.id, t0, t1);
    if (v === null) throw this.engine.takeError();
    return v;
  }

  onsets(t0: number, t1: number, limit: number): OnsetPoint[] {
    const s = native.onsetsJson(this.engine.ctx, this.meta.id, t0, t1, limit);
    if (s === null) throw this.engine.takeError();
    return JSON.parse(s) as OnsetPoint[];
  }

  bestOnsetIn(t0: number, t1: number): OnsetPoint | null {
    const s = native.bestOnsetInJson(this.engine.ctx, this.meta.id, t0, t1);
    if (s === null) throw this.engine.takeError();
    return s === 'null' ? null : (JSON.parse(s) as OnsetPoint);
  }

  bestOnsetNear(t: number, halfSpanS: number): OnsetPoint | null {
    const s = native.bestOnsetNearJson(this.engine.ctx, this.meta.id, t, halfSpanS);
    if (s === null) throw this.engine.takeError();
    return s === 'null' ? null : (JSON.parse(s) as OnsetPoint);
  }

  stats(): TrackStats {
    const s = native.trackStatsJson(this.engine.ctx, this.meta.id);
    if (s === null) throw this.engine.takeError();
    return JSON.parse(s) as TrackStats;
  }
}
