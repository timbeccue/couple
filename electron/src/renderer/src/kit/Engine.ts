// Engine.ts — the kit handle over cpl_ctx (SPEC §4.9); TS twin of
// CoupleKit/Engine.swift. Thread-safety is the core's concern; jobs report
// from worker threads and surface here via the binding's ThreadsafeFunction.
import { native } from './native';
import { Job } from './Job';
import { TrackHandle } from './Track';
import { SessionHandle } from './Session';
import {
  decodeTrackMeta,
  type Fit,
  type MediaInfo,
  type PairEvidence,
  type ResolvedRow,
  type TrackMeta,
} from './Wire';

export class CoreError extends Error {
  readonly kind: string;
  constructor(kind: string, message: string) {
    super(message);
    this.name = 'CoreError';
    this.kind = kind;
  }
}

export type TrackRole = 'camera' | 'external';

export interface EngineConfig {
  ffmpegPath?: string;
  ffprobePath?: string;
  cacheDir?: string;
}

export class Engine {
  readonly ctx: bigint;

  constructor(config: EngineConfig = {}) {
    this.ctx = native.ctxNew(JSON.stringify(config));
    if (this.ctx === 0n) throw Engine.staticTakeError();
  }

  free(): void {
    native.ctxFree(this.ctx);
  }

  static get coreVersion(): string {
    return native.version() ?? '?';
  }

  static get abiVersion(): number {
    return native.abiVersion();
  }

  // ---------- error plumbing ----------

  takeError(): CoreError {
    return Engine.staticTakeError(this.ctx);
  }

  static staticTakeError(ctx: bigint = 0n): CoreError {
    const json = native.takeErrorJson(ctx);
    if (!json) return new CoreError('internal', 'unknown core error');
    try {
      const obj = JSON.parse(json) as { error?: unknown; message?: string };
      let kind = 'internal';
      if (typeof obj.error === 'string') kind = obj.error;
      else if (obj.error && typeof obj.error === 'object') {
        kind = Object.keys(obj.error)[0] ?? 'internal';
      }
      return new CoreError(kind, obj.message ?? json);
    } catch {
      return new CoreError('internal', json);
    }
  }

  /** Decode a core JSON payload; null pointer ⇒ throw the thread-local error. */
  private decode<T>(s: string | null): T {
    if (s === null) throw this.takeError();
    return JSON.parse(s) as T;
  }

  /** As `decode`, but the payload may be the literal `null` (e.g. measure misses). */
  private decodeOptional<T>(s: string | null): T | null {
    if (s === null) throw this.takeError();
    if (s === 'null') return null;
    return JSON.parse(s) as T;
  }

  // ---------- media ----------

  probe(path: string): MediaInfo {
    return this.decode<MediaInfo>(native.probeJson(this.ctx, path));
  }

  /** Start an import job (§4.3); the job's value is the imported TrackMeta. */
  importStart(path: string, role: TrackRole): Job<TrackMeta> {
    return Job.start(
      this,
      (cb) => native.importStart(this.ctx, path, role === 'camera' ? 0 : 1, cb),
      decodeTrackMeta
    );
  }

  track(meta: TrackMeta): TrackHandle {
    return new TrackHandle(this, meta);
  }

  sessionNew(camera: TrackHandle, external: TrackHandle): SessionHandle {
    const id = native.sessionNew(this.ctx, camera.id, external.id);
    if (id === 0n) throw this.takeError();
    return new SessionHandle(this, id);
  }

  /** fit_ls over (tS, lagMs) points; null when < 2 points (§4.4). */
  fitLS(points: Array<[number, number]>): Fit | null {
    const s = native.fitLsJson(JSON.stringify(points));
    if (s === null) return null;
    return s === 'null' ? null : (JSON.parse(s) as Fit);
  }

  cacheGC(maxBytes: bigint): bigint {
    return native.cacheGc(this.ctx, maxBytes);
  }

  // ---------- row resolution (PLAN-CORE-SHELL-BOUNDARY §3.1) ----------

  resolveParamsDefaultJSON(): string | null {
    return native.resolveParamsDefaultJson();
  }

  pairingParamsDefaultJSON(): string | null {
    return native.pairingParamsDefaultJson();
  }

  /** Synchronous row resolution (`cpl_resolve`); the app path is `resolveStart`. */
  resolve(videos: bigint[], audios: bigint[], paramsJSON: string | null): ResolvedRow[] | null {
    if (videos.length === 0) return [];
    const s = native.resolve(this.ctx, videos, audios, paramsJSON);
    if (s === null) return null;
    return JSON.parse(s) as ResolvedRow[];
  }

  /** JOB form of `resolve` (`cpl_resolve_start`): same `[ResolvedRow]` value,
   * with determinate MatchFingerprint/MatchCorrelate/MatchGraph/Verify/Resolve
   * progress. Callers short-circuit empty inputs (no work ⇒ no job). */
  resolveStart(videos: bigint[], audios: bigint[], paramsJSON: string | null): Job<ResolvedRow[]> {
    return Job.start(
      this,
      (cb) => native.resolveStart(this.ctx, videos, audios, paramsJSON, cb),
      (payload) => JSON.parse(payload) as ResolvedRow[]
    );
  }

  /** Batch method-neutral evidence (display-only decoration; never re-decides rows). */
  matchCandidates(
    videos: bigint[],
    audios: bigint[],
    paramsJSON: string | null
  ): PairEvidence[][] | null {
    if (videos.length === 0) return [];
    if (audios.length === 0) return videos.map(() => []);
    const s = native.matchCandidates(this.ctx, videos, audios, paramsJSON);
    if (s === null) return null;
    return JSON.parse(s) as PairEvidence[][];
  }
}
