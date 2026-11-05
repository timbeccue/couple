// Hand-maintained typings for couple-node (electron/native/src/lib.rs).
// One export per cpl_* symbol; see ffi/couple.h for semantics. Handles are
// BigInt (0n = invalid/error); structured data is JSON strings; string-returning
// calls yield null on error (read takeErrorJson); status calls return the raw
// i32 (0 = ok).

/** Job progress callback: terminal phase is "done" | "error" | "cancelled". */
export type ProgressCallback = (
  phase: string,
  fraction: number,
  payloadJson: string | null
) => void;

export interface BandInfo {
  levelSpb: number;
  colT0: number;
  colDt: number;
}

export interface AuditionFillInfo {
  frames: number;
  streamT: number;
}

// lifecycle
export function abiVersion(): number;
export function version(): string | null;
export function ctxNew(configJson?: string | null): bigint;
export function ctxFree(ctx: bigint): void;
export function takeErrorJson(ctx: bigint): string | null;
export function jobCancel(ctx: bigint, job: bigint): void;
export function jobFree(ctx: bigint, job: bigint): void;

// media / tracks
export function probeJson(ctx: bigint, path: string): string | null;
export function probeJsonAsync(ctx: bigint, path: string): Promise<string>;
export function importStart(
  ctx: bigint,
  path: string,
  role: number,
  callback: ProgressCallback
): bigint;
export function trackInfoJson(ctx: bigint, track: bigint): string | null;
export function trackFree(ctx: bigint, track: bigint): void;
export function band(
  ctx: bigint,
  track: bigint,
  t0: number,
  t1: number,
  nCols: number,
  mins: Int8Array,
  maxs: Int8Array
): BandInfo | null;
export function rawWindow(
  ctx: bigint,
  track: bigint,
  t0: number,
  t1: number,
  out: Float32Array
): number | null;
export function rms(ctx: bigint, track: bigint, t0: number, t1: number): number | null;
export function onsetsJson(
  ctx: bigint,
  track: bigint,
  t0: number,
  t1: number,
  limit: number
): string | null;
export function bestOnsetInJson(
  ctx: bigint,
  track: bigint,
  t0: number,
  t1: number
): string | null;
export function bestOnsetNearJson(
  ctx: bigint,
  track: bigint,
  t: number,
  halfSpanS: number
): string | null;
export function trackStatsJson(ctx: bigint, track: bigint): string | null;

// session
export function sessionNew(ctx: bigint, camera: bigint, external: bigint): bigint;
export function sessionFree(ctx: bigint, session: bigint): void;
export function stateJson(ctx: bigint, session: bigint): string | null;
export function setStateJson(ctx: bigint, session: bigint, json: string): number;
export function setOffsetMs(ctx: bigint, session: bigint, v: number): number;
export function setDriftPpmLive(ctx: bigint, session: bigint, v: number): number;
export function commitDrift(ctx: bigint, session: bigint): number;
export function setDriftMode(ctx: bigint, session: bigint, mode: number): number;
export function setVerified(ctx: bigint, session: bigint, verified: boolean): number;

// sync
export function autosyncStart(
  ctx: bigint,
  session: bigint,
  paramsJson: string | null,
  callback: ProgressCallback
): bigint;
export function autosyncStartSeeded(
  ctx: bigint,
  session: bigint,
  paramsJson: string | null,
  seedOffsetMs: number,
  seedDriftPpm: number,
  callback: ProgressCallback
): bigint;
export function measureLocalJson(
  ctx: bigint,
  session: bigint,
  tc: number,
  halfWinS: number,
  centerMs: number,
  spanMs: number
): string | null;
export function fitLsJson(pointsJson: string): string | null;

// audition
export function auditionOpen(
  ctx: bigint,
  session: bigint,
  mode: number,
  tStart: number
): bigint;
export function auditionFill(
  ctx: bigint,
  stream: bigint,
  out: Float32Array
): AuditionFillInfo | null;
export function auditionSeek(ctx: bigint, stream: bigint, t: number): number;
export function auditionSetMode(ctx: bigint, stream: bigint, mode: number): number;
export function auditionSetGains(
  ctx: bigint,
  stream: bigint,
  gainCam: number,
  gainExt: number
): number;
export function auditionClose(ctx: bigint, stream: bigint): void;

// audio output pump (shell platform layer, PLAN-CORE-SHELL-BOUNDARY §4-G)
export function audioStart(ctx: bigint, stream: bigint): void;
export function audioStop(): void;
/** Audition video-time clock (latency-compensated); NaN when stopped. */
export function audioClock(): number;
export function audioUnderruns(): number;
export function audioRunning(): boolean;

// export
export function exportStart(
  ctx: bigint,
  session: bigint,
  optsJson: string,
  callback: ProgressCallback
): bigint;

// pairing / matching
export function pairingParamsDefaultJson(): string | null;
export function pairCorrelate(ctx: bigint, trackA: bigint, trackB: bigint): string | null;
export function pairCorrelateParams(
  ctx: bigint,
  trackA: bigint,
  trackB: bigint,
  paramsJson: string | null
): string | null;
export function pairCandidates(
  ctx: bigint,
  videos: bigint[],
  audios: bigint[]
): string | null;
export function pairCandidatesParams(
  ctx: bigint,
  videos: bigint[],
  audios: bigint[],
  paramsJson: string | null
): string | null;
export function matchPair(
  ctx: bigint,
  trackA: bigint,
  trackB: bigint,
  pairingParamsJson: string | null
): string | null;
export function matchCandidates(
  ctx: bigint,
  videos: bigint[],
  audios: bigint[],
  pairingParamsJson: string | null
): string | null;
export function matchCandidatesGraph(
  ctx: bigint,
  videos: bigint[],
  audios: bigint[],
  pairingParamsJson: string | null
): string | null;
export function matchCandidatesGraphStart(
  ctx: bigint,
  videos: bigint[],
  audios: bigint[],
  pairingParamsJson: string | null,
  callback: ProgressCallback
): bigint;
export function triangleCloses(requestJson: string): string | null;
export function pairFitBatch(ctx: bigint, pairsJson: string): string | null;
export function pairFitBatchParams(
  ctx: bigint,
  pairsJson: string,
  paramsJson: string | null
): string | null;
export function verifyBatch(ctx: bigint, pairsJson: string): string | null;
export function verifyBatchParams(
  ctx: bigint,
  pairsJson: string,
  paramsJson: string | null
): string | null;

// row resolution
export function resolveParamsDefaultJson(): string | null;
export function resolve(
  ctx: bigint,
  videos: bigint[],
  audios: bigint[],
  paramsJson: string | null
): string | null;
export function resolveStart(
  ctx: bigint,
  videos: bigint[],
  audios: bigint[],
  paramsJson: string | null,
  callback: ProgressCallback
): bigint;

// pure policies
export function policyMatchQuality(json: string): string | null;
export function policyRepairDrop(json: string): string | null;
export function policyRepairForced(json: string): string | null;
export function policyRowMerge(json: string): string | null;
export function policyMediaRemoval(json: string): string | null;
export function policyAnchorPropose(json: string): string | null;
export function policyShadowRows(json: string): string | null;
export function policyActivity(json: string): string | null;
export function policySyncSeed(json: string): string | null;
export function policyMonitor(json: string): string | null;
export function policyExportInclude(json: string): string | null;

// cache
export function cacheGc(ctx: bigint, maxBytes: bigint): bigint;
