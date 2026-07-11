// Mapping.ts — alignment mapping math (UX §2.1, single source of truth);
// the sanctioned per-shell affine, a straight port of mac/Sources/Couple/Mapping.swift.
//   u(t) = t·(1 + driftPpm·1e−6) + offsetMs/1000
//   L(t) = offsetMs + t·driftPpm·1e−3   (ms)
export class Mapping {
  readonly offsetMs: number;
  readonly driftPpm: number;

  constructor(offsetMs: number, driftPpm: number) {
    this.offsetMs = offsetMs;
    this.driftPpm = driftPpm;
  }

  get rate(): number {
    return 1 + this.driftPpm * 1e-6;
  }

  /** Video time → external-file time (seconds). */
  u(t: number): number {
    return t * this.rate + this.offsetMs / 1000;
  }

  /** Local lag in ms at video time t. */
  localLagMs(t: number): number {
    return this.offsetMs + t * this.driftPpm * 1e-3;
  }

  /** External-file time → video time (seconds). */
  tForU(u: number): number {
    return (u - this.offsetMs / 1000) / this.rate;
  }

  /** Overlap on the video timeline, null when empty. */
  overlap(camDur: number, extDur: number): [number, number] | null {
    const t0 = Math.max(0, this.tForU(0));
    const t1 = Math.min(camDur, this.tForU(extDur));
    return t1 > t0 ? [t0, t1] : null;
  }
}
