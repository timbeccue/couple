// Policies.ts — thin wrappers over the 11 pure cpl_policy_* decisions +
// triangleCloses (PLAN-CORE-SHELL-BOUNDARY §3.2). TS twin of the CoupleKit
// policy mirror files. NO decision logic here: each wrapper builds the request
// JSON, decodes the uniform envelope, and maps a failure to the CONSERVATIVE
// fallback documented in couple.h — never a silent optimistic default.
import { native } from './native';

type Envelope<T> = { ok: true; value: T } | { ok: false; error: { kind: string; message: string } };

function call<T>(fn: (json: string) => string | null, request: unknown, fallback: T): T {
  const raw = fn(JSON.stringify(request));
  if (raw === null) return fallback;
  try {
    const env = JSON.parse(raw) as Envelope<T>;
    return env.ok ? env.value : fallback;
  } catch {
    return fallback;
  }
}

// ---------- match quality (verdict-dot color) ----------

export type MatchQuality = 'confident' | 'questionable' | 'none';
export type SelectionProvenance = 'none' | 'automatic' | 'manual';

export function matchQuality(req: {
  forced: boolean;
  selection: SelectionProvenance;
  autoVerdict: 'confident' | 'questionable' | 'none';
  manualMatchConfident: boolean | null;
}): MatchQuality {
  // Conservative fallback: questionable — never silently green.
  return call<{ quality: MatchQuality }>(native.policyMatchQuality, req, { quality: 'questionable' })
    .quality;
}

// ---------- manual drag-to-pair ----------

export type RepairDropOutcome = 'noOp' | 'accept' | 'needsConfirmation' | 'reject';

export function repairDrop(req: {
  hasSelectedCouple: boolean;
  payloadIsAudio: boolean;
  videoAvailable: boolean;
  currentSelectedKey: string | null;
  droppedKey: string;
  /** Carries PINNED (a manual pick) since PLAN-COMPACT-MODE D9; key name kept until Phase 6 (D12). */
  approved: boolean;
}): RepairDropOutcome {
  // Conservative fallback: reject — mutates NO state.
  return call<{ outcome: RepairDropOutcome }>(native.policyRepairDrop, req, { outcome: 'reject' })
    .outcome;
}

export function repairForced(req: { candidateZ: number; zMin: number; syncFailed: boolean }): boolean {
  // Conservative fallback: forced=true — Questionable, never a silent green.
  return call<{ forced: boolean }>(native.policyRepairForced, req, { forced: true }).forced;
}

// ---------- re-resolution merge ----------

export interface RowMergePlan {
  selectedAudioKey: string | null;
  candidateKeys: string[];
  adoptFreshDisplay: boolean;
  rebuildSession: boolean;
  clearForced: boolean;
}

export function rowMergePlan(req: {
  /** Pinned ONLY since PLAN-COMPACT-MODE D6 (was approved || pinned); the caller composes it. */
  protected: boolean;
  currentSelectedKey: string | null;
  currentCandidateKeys: string[];
  freshSelectedKey: string | null;
  freshCandidateKeys: string[];
}): RowMergePlan {
  // Conservative fallback: the PROTECTED-row plan — keep the existing
  // selection/candidates/display, never silently swap a source.
  return call<RowMergePlan>(native.policyRowMerge, req, {
    selectedAudioKey: req.currentSelectedKey,
    candidateKeys: req.currentCandidateKeys,
    adoptFreshDisplay: false,
    rebuildSession: false,
    clearForced: false,
  });
}

// ---------- media removal / clear session ----------

export interface RemovalPlan {
  removableKeys: string[];
  needsConfirmation: boolean;
  affectedCouples: number;
  /** Affected couples INCLUDED for export (PLAN-COMPACT-MODE D7; key name kept until Phase 6, D12). */
  affectedApproved: number;
}

export interface RemovalCouple {
  videoKey: string;
  selectedAudioKey: string | null;
  /** Included for export (PLAN-COMPACT-MODE D7; was approved ≡ queued; key name kept per D12). */
  approved: boolean;
}

export function mediaRemovalPlan(req: {
  selectedKeys: string[];
  poolKeys: string[];
  couples: RemovalCouple[];
}): RemovalPlan {
  // Conservative fallback: the EMPTY plan — a failure removes nothing.
  return call<RemovalPlan>(native.policyMediaRemoval, { op: 'plan', ...req }, {
    removableKeys: [],
    needsConfirmation: false,
    affectedCouples: 0,
    affectedApproved: 0,
  });
}

export function clearSessionCounts(req: { mediaCount: number; couples: RemovalCouple[] }): {
  media: number;
  couples: number;
  /** Couples included for export (D7; key name kept per D12). */
  approved: number;
} {
  return call(native.policyMediaRemoval, { op: 'clearCounts', ...req }, { media: 0, couples: 0, approved: 0 });
}

// ---------- anchors ----------

export interface AnchorCandidate {
  t: number;
  score: number;
  usable: boolean;
}

export function anchorPropose(req: {
  overlap: { start: number; end: number } | null;
  candidates: AnchorCandidate[];
}): number[] {
  // Conservative fallback: EMPTY proposal (the manual-rescue state).
  return call<number[]>(native.policyAnchorPropose, { op: 'propose', ...req }, []);
}

export function anchorAdd(req: {
  overlap: { start: number; end: number } | null;
  existing: number[];
  candidates: AnchorCandidate[];
}): number | null {
  return call<number | null>(native.policyAnchorPropose, { op: 'add', ...req }, null);
}

// ---------- shadow telemetry ----------

export function shadowRows(req: {
  evidence: unknown[];
  audioKeys: string[];
  audioNames: string[];
}): Array<{ audioKey: string; audioName: string; evidence: Record<string, unknown> }> {
  // Conservative fallback: EMPTY projection — telemetry simply does not display.
  return call(native.policyShadowRows, req, []);
}

// ---------- activity strip ----------

/** Wire request DTOs mirror core/src/policy/activity.rs. Optional fields stay
 * omitted unless the caller supplies them; nullable fields mirror Rust Option. */
export interface ActivityImporting {
  phase: 'probing' | 'decoding' | 'waveform' | 'finishing';
  fraction: number;
  index?: number | null;
  total?: number | null;
}

export interface ActivityPairing {
  videoCount: number;
  audioCount: number;
  resolving?: boolean;
  afterImport?: boolean;
  importTail?: boolean;
}

export interface ActivitySyncing {
  index: number;
  total: number;
}

export interface ActivityExporting {
  index: number;
  total: number;
  rowFraction: number;
  preflight?: boolean;
}

export interface ActivityIngest {
  importing?: ActivityImporting | null;
  matching?: {
    phase: 'fingerprinting' | 'comparing' | 'crosschecking';
    fraction: number;
  } | null;
  syncing?: ActivitySyncing | null;
  stagesPlanned: {
    importStage?: boolean;
    matchStage?: boolean;
    syncStage?: boolean;
  };
}

/** Live signals before the controller adds elapsed time and its phase floor. */
export interface ActivitySignals {
  importing?: ActivityImporting | null;
  pairing?: ActivityPairing | null;
  syncing?: ActivitySyncing | null;
  exporting?: ActivityExporting | null;
  ingest?: ActivityIngest | null;
  failure?: string | null;
}

export interface ActivityRequest extends ActivitySignals {
  elapsedMs: number;
  debounceMs?: number;
  phaseFloor?: number | null;
}

export interface ActivityDecision {
  line: Record<string, unknown> & { kind: string };
  fraction: number | null;
  isError: boolean;
  phaseKey: string | null;
}

export function activitySummary(req: ActivityRequest): ActivityDecision | null {
  // Conservative fallback: no line (idle) — never a wrong-but-confident line.
  return call<{ summary: ActivityDecision | null }>(native.policyActivity, req, { summary: null })
    .summary;
}

// ---------- auto-sync seed ----------

export interface SyncSeed {
  offsetMs: number;
  driftPpm: number;
  source: 'evidence' | 'manual';
}

export function syncSeed(req: {
  evidenceLagMs: number | null;
  evidenceZ: number | null;
  evidenceDriftPpm: number | null;
  explicit: boolean;
  lastSyncFailed: boolean;
  currentOffsetMs: number;
}): SyncSeed | null {
  // Conservative fallback: null ⇒ the unseeded sync (unchanged behavior).
  return call<{ seed: SyncSeed | null }>(native.policySyncSeed, req, { seed: null }).seed;
}

// ---------- monitor A/B (flip / both / picker) ----------

export type MonitorModeWire = 'pan' | 'overlay' | 'null' | 'camera' | 'external';

export interface MonitorPlan {
  next: MonitorModeWire;
  lastSide: MonitorModeWire;
  lastBoth: MonitorModeWire;
}

/** `a` flips camera ⇄ external, `b` restores the both-mode used last, the picker
 * selects; core returns the next mode plus the memory to hand back next time
 * (CoupleKit MonitorPolicy twin). */
export function monitorPlan(req: {
  action: 'flip' | 'both' | 'select';
  current: MonitorModeWire;
  chosen?: MonitorModeWire | null;
  lastSide?: MonitorModeWire | null;
  lastBoth?: MonitorModeWire | null;
}): MonitorPlan {
  // Conservative fallback: keep the current mode and memory (never changes what is heard).
  return call<MonitorPlan>(native.policyMonitor, req, {
    next: req.current,
    lastSide: req.lastSide ?? 'camera',
    lastBoth: req.lastBoth ?? 'overlay',
  });
}

// ---------- include for export (PLAN-COMPACT-MODE §3, D1/D8; UX-DECISIONS V2.12) ----------

/** One row's inclusion inputs: the verdict, the user's checkbox (null = untouched)
 * and whether the couple has been exported already (`exportedAt != null`, D8). */
export interface IncludeInput {
  verdict: MatchQuality;
  explicit: boolean | null;
  exported: boolean;
}

export interface IncludeDecision {
  /** The row goes out on the next export (`verdict good && (explicit ?? !exported)`). */
  included: boolean;
  /** The row is listed in the export rail's Files list (`included || exported`). */
  inFilesList: boolean;
}

/** Per-row inclusion decisions in input order plus the N in "Export N files"
 * (ExportIncludePolicy twin; decision in core policy::export_include). */
export function exportInclude(rows: IncludeInput[]): { rows: IncludeDecision[]; count: number } {
  // Conservative fallback: nothing included — a fault never lets a file leave the app;
  // already-exported couples stay listed.
  return call<{ rows: IncludeDecision[]; count: number }>(native.policyExportInclude, { rows }, {
    rows: rows.map((r) => ({ included: false, inFilesList: r.exported })),
    count: 0,
  });
}

// ---------- §13.4 simultaneity predicate ----------

export function triangleCloses(req: {
  va: { lagMs: number; driftPpm: number };
  ab: { lagMs: number; driftPpm: number };
  vb: { lagMs: number; driftPpm: number };
  abClassification: string;
  coverageS: number;
  offsetTolMs: number;
  driftTolPpm: number;
}): { closes: boolean; residualMaxMs: number } {
  // A fault never fabricates a simultaneity.
  const raw = native.triangleCloses(JSON.stringify(req));
  if (raw === null) return { closes: false, residualMaxMs: 0 };
  try {
    return JSON.parse(raw) as { closes: boolean; residualMaxMs: number };
  } catch {
    return { closes: false, residualMaxMs: 0 };
  }
}
