// Wire.ts — TS mirror of the core's camelCase JSON wire schemas (SPEC §4),
// the Electron twin of CoupleKit/Wire.swift. Types are added as milestones
// need them; the JSON is decoded verbatim (core serializes camelCase already).

// ---------- §4.2 media ----------

export interface VideoStreamInfo {
  codec: string;
  width: number;
  height: number;
  nominalFps: number | null;
  avgFps: number | null;
  startTimeS: number;
  bitRate: number | null;
}

export interface AudioStreamInfo {
  codec: string;
  sampleRate: number;
  channels: number;
  bitsPerSample: number | null;
  startTimeS: number;
  durationS: number | null;
}

export interface MediaInfo {
  path: string;
  container: string;
  durationS: number;
  video: VideoStreamInfo | null;
  audio: AudioStreamInfo[];
  creationTimeUtc: string | null;
  tags: Record<string, string>;
}

export interface TrackStats {
  absPeak: number;
  activeP959600: number;
  activeRmsDbfs: number;
}

/** Import "done" payload (§4.3). `id` is the u64 track handle — decoded as
 * bigint via `decodeTrackMeta`, never plain JSON.parse. */
export interface TrackMeta {
  id: bigint;
  role: 'camera' | 'external';
  info: MediaInfo;
  epochS: number;
  durationS: number;
  sampleRate: number;
  normalization: number;
  stats: TrackStats;
  contentKey: string;
}

/** Parse a TrackMeta JSON payload, extracting the u64 `id` textually so
 * JSON.parse can't round it. */
export function decodeTrackMeta(json: string): TrackMeta {
  const m = /"id"\s*:\s*(\d+)/.exec(json);
  if (!m) throw new Error(`TrackMeta payload lacks id: ${json.slice(0, 200)}`);
  const parsed = JSON.parse(json) as Omit<TrackMeta, 'id'> & { id: number };
  return { ...parsed, id: BigInt(m[1]) };
}

// ---------- §4.4 sync ----------

export interface Fit {
  offMs: number;
  ppm: number;
  rmsMs: number;
}

export interface LocalMeasure {
  lagMs: number;
  ncc: number;
}

/** One attempted drift window in the last sync's telemetry (SyncResult.windows):
 * `t` = camera-time center, `lagMs`/`ncc` the measurement, `accepted` whether it
 * cleared the gate. The core JSON uses `t`/`accepted` (NOT tCenterS/used). */
export interface DriftWindow {
  t: number;
  lagMs: number;
  ncc: number;
  accepted: boolean;
}

/** An ACCEPTED per-window measurement persisted in the alignment state
 * (`driftWindows`) — the fit points the sparkline plots. */
export interface AcceptedWindow {
  t: number;
  lagMs: number;
  ncc: number;
}

export interface SyncResult {
  offsetMs: number;
  driftPpm: number;
  z: number;
  ncc: number;
  status: string;
  windows?: DriftWindow[];
  [k: string]: unknown;
}

// ---------- §4.5 alignment state ----------

export interface AlignmentState {
  offsetMs: number;
  driftPpm: number;
  driftMode: 'resample' | 'none';
  verified: boolean;
  /** Accepted drift windows (the fit points); the sparkline's authoritative source. */
  driftWindows?: AcceptedWindow[];
  [k: string]: unknown;
}

// ---------- §4.6 progress ----------

export interface ProgressUpdate {
  phase: string;
  fraction: number;
}

// ---------- pairing / evidence (SPEC-V2 §3, PLAN-SPECTRAL §14) ----------

export interface Coverage {
  start: number;
  end: number;
}

export type MatchClassification =
  | 'match'
  | 'noMatch'
  | 'ambiguous'
  | 'insufficientOverlap'
  | 'unavailable';

export interface PairCorrelation {
  z: number;
  lagMs: number;
  ncc: number;
  fineNcc: number | null;
  overlapBins: number;
  runnerUpZ: number | null;
  refined: boolean;
  insufficientOverlap: boolean;
  legacyCandidate: boolean;
  prefilterCandidate: boolean;
}

export interface PairEvidence {
  method: string;
  classification: MatchClassification;
  lagMs: number | null;
  driftPpm: number | null;
  score: number;
  runnerUpScore: number | null;
  scoreMargin: number | null;
  support: number | null;
  inlierFraction: number | null;
  temporalCoverage: number | null;
  residualRmsMs: number | null;
  residualP95Ms: number | null;
  overlapS: number | null;
  refinedLagMs: number | null;
  refinedDriftPpm: number | null;
  refinedRmsMs: number | null;
  legacy: PairCorrelation | null;
  [k: string]: unknown;
}

// ---------- row resolution (PLAN-CORE-SHELL-BOUNDARY §3.1) ----------

export type RowState =
  | 'confident'
  | 'partial'
  | 'split'
  | 'twoSources'
  | 'ambiguous'
  | 'noMatch';

export interface RowCandidate {
  audioKey: string;
  audioName: string;
  z: number;
  lagMs: number;
  coverage: Coverage;
}

export interface TriangleCheck {
  audioA: string;
  audioB: string;
  abZ: number;
  residualMs: number | null;
  simultaneous: boolean;
}

export interface CandidateOverflow {
  ceiling: number;
  candidates: number;
}

export interface ResolutionDetail {
  margin?: number;
  clusters: number;
  triangles?: TriangleCheck[];
  note?: string;
  candidateOverflow?: CandidateOverflow;
}

export interface ResolvedRow {
  videoKey: string;
  videoName: string;
  state: RowState;
  chip: string;
  selected: RowCandidate | null;
  alternates: RowCandidate[];
  resolution: ResolutionDetail;
}

// ---------- export (§4.9) ----------

export interface ExportOptions {
  kind: 'mux' | 'wav';
  outputPath: string;
  keepCameraAudio?: boolean;
  container?: string;
  faststart?: boolean;
  overwrite?: boolean;
}

export interface ExportReport {
  outputPath: string;
  [k: string]: unknown;
}
