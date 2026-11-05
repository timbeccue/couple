// CoupleRow.ts — one queue row: a video + its selected audio + alternates;
// TS port of mac/Sources/Couple/Couple.swift. The verdict is the PURE core
// `match_quality` policy — evidence-based, never derived from the act of
// selecting (TODO-009).
//
// Include-for-export (UX §V2.12, PLAN-COMPACT-MODE D1/D8): there is no approve
// flag. A couple is INCLUDED in the next export when its verdict is good and it
// has not been exported yet, unless the user toggled its row checkbox; the row
// stores only the two inputs of that rule (`explicit`, `exportedAt`) and
// `included` is DERIVED through the core `export_include` policy on every read.
import type { Engine } from '../kit/Engine';
import * as Policies from '../kit/Policies';
import type { ResolutionDetail, ResolvedRow, RowCandidate, RowState } from '../kit/Wire';
import { Observable } from './observable';
import type { PoolEntry } from './MediaPool';
import { PairSession } from './PairSession';

export type Verdict = 'good' | 'questionable' | 'none';

export type ExportStatus =
  | { kind: 'idle' }
  | { kind: 'running'; fraction: number }
  | { kind: 'done'; outputPath: string }
  | { kind: 'failed'; message: string };

/** RowState → resolver auto-verdict (UI mapping, mirrors app-side Resolver.swift). */
function autoVerdict(state: RowState): 'confident' | 'questionable' | 'none' {
  switch (state) {
    case 'confident':
    case 'partial':
    case 'split':
    case 'twoSources':
      return 'confident';
    case 'ambiguous':
      return 'questionable';
    case 'noMatch':
      return 'none';
  }
}

function matchQualityOf(v: Verdict): Policies.MatchQuality {
  return v === 'good' ? 'confident' : v;
}

export class CoupleRow extends Observable {
  readonly id: string; // video content key
  readonly video: PoolEntry;

  state: RowState;
  chip: string;
  detail: ResolutionDetail;
  candidates: RowCandidate[];
  selectedAudioKey: string | null;
  pinned = false; // manual choice; re-runs never override (§3.6)
  forced = false; // kept below-floor manual pairing (honest, §3.6/§V2.6)
  selectionProvenance: Policies.SelectionProvenance;
  manualMatchConfident: boolean | null = null;

  // ----- include-for-export (D1/D8): the ONLY stored inclusion state -----
  /** The user's row checkbox: true/false once toggled, null while untouched (⇒ the
   * policy default). Persisted (optional field in queue.json); cleared by a manual
   * re-pair, which makes the default rule apply again. */
  explicit: boolean | null = null;
  /** When this couple last left the app as a file (ISO 8601, stamped by
   * BatchExport on each success; D8). Persisted. Drives the ✓ time and drops the
   * couple out of the default include set until it is ticked again. */
  exportedAt: string | null = null;

  readonly pair: PairSession;
  stemOverride: string | null = null;
  exportStatus: ExportStatus = { kind: 'idle' };

  /** z of the last manual pick (0 for a fresh pool drag) + the resolver z
   * floor — the inputs the forced-verdict policy re-checks when a sync lands. */
  private lastPickZ = 0;
  private lastPickZMin = 8.5;

  constructor(video: PoolEntry, row: ResolvedRow, engine: Engine) {
    super();
    this.id = row.videoKey;
    this.video = video;
    this.state = row.state;
    this.chip = row.chip;
    this.detail = row.resolution;
    this.candidates = (row.selected ? [row.selected] : []).concat(row.alternates);
    this.selectedAudioKey = row.selected?.audioKey ?? null;
    this.selectionProvenance = row.selected ? 'automatic' : 'none';
    this.pair = new PairSession(engine);
    // Row-level UI reads (dots, sections) subscribe to the row; sync status
    // changes on the pair must re-render them too. A settled sync also
    // re-checks the forced verdict for a manual pairing (§3.6: the policy runs
    // at pick time AND when the sync completes).
    this.pair.subscribe(() => {
      if (
        this.selectionProvenance === 'manual' &&
        (this.pair.syncStatus === 'synced' || this.pair.syncStatus === 'failed')
      ) {
        this.forced = Policies.repairForced({
          candidateZ: this.lastPickZ,
          zMin: this.lastPickZMin,
          syncFailed: this.pair.syncStatus === 'failed',
        });
      }
      this.invalidate();
    });
  }

  /** Apply a MANUAL audio pick (§3.6): candidate re-pick or pool repair drop.
   * The forced verdict is the PURE core policy; a fresh pool drag carries
   * candidateZ 0. The caller must run ensureSession afterwards. */
  applyManualPick(audioKey: string, candidateZ: number, zMin: number): void {
    this.selectedAudioKey = audioKey;
    this.selectionProvenance = 'manual';
    this.manualMatchConfident = null; // questionable until the caller re-evaluates this pair's match evidence (TODO-009)
    this.pinned = true; // a manual choice; re-runs never override (§3.6)
    this.lastPickZ = candidateZ;
    this.lastPickZMin = zMin;
    this.forced = Policies.repairForced({ candidateZ, zMin, syncFailed: false });
    this.invalidate();
  }

  /** The core include decision for this row (`policy::export_include`, D1):
   * `included = verdict good ∧ (explicit ?? exportedAt == null)`,
   * `inFilesList = included ∪ exported`. Derived on every read — a verdict change,
   * undo, restore or sync completion needs no recompute hook. */
  private get includeDecision(): Policies.IncludeDecision {
    const exported = this.exportedAt !== null;
    return (
      Policies.exportInclude([{ verdict: matchQualityOf(this.verdict), explicit: this.explicit, exported }])
        .rows[0] ?? { included: false, inFilesList: exported }
    );
  }

  /** Goes out on the next export (the row checkbox; counts toward "Export N files"). */
  get included(): boolean {
    return this.includeDecision.included;
  }

  /** Listed in the export rail's Files list: included, or already exported. */
  get inFilesList(): boolean {
    return this.includeDecision.inFilesList;
  }

  /** Whether the checkbox is offered at all (D1: a good verdict only) — the core
   * rule with an explicit tick on a never-exported row (ExportIncludePolicy.canInclude
   * twin). A "pick one" / "no audio" / forced couple cannot be included until fixed. */
  get canInclude(): boolean {
    return (
      Policies.exportInclude([{ verdict: matchQualityOf(this.verdict), explicit: true, exported: false }])
        .rows[0]?.included ?? false
    );
  }

  /** This couple's auto-sync is running (the row's "syncing…" line). */
  get isSyncRunning(): boolean {
    return this.pair.session !== null && this.pair.syncStatus === 'syncing';
  }

  get selectedCandidate(): RowCandidate | null {
    if (!this.selectedAudioKey) return null;
    return this.candidates.find((c) => c.audioKey === this.selectedAudioKey) ?? null;
  }

  get alternates(): RowCandidate[] {
    return this.candidates.filter((c) => c.audioKey !== this.selectedAudioKey);
  }

  get hasSession(): boolean {
    return this.pair.session !== null;
  }

  /** Evidence-based three-category verdict (§V2.3) via the pure core policy. */
  get verdict(): Verdict {
    const q = Policies.matchQuality({
      forced: this.forced,
      selection: this.selectedAudioKey === null ? 'none' : this.selectionProvenance,
      autoVerdict: autoVerdict(this.state),
      manualMatchConfident: this.manualMatchConfident,
    });
    return q === 'confident' ? 'good' : q === 'questionable' ? 'questionable' : 'none';
  }

  /** Short word that rides along only when action is needed (§V2.3). */
  get actionWord(): string | null {
    if (this.forced) return 'forced';
    if (this.selectedAudioKey === null) {
      if (this.state === 'ambiguous') return 'pick one';
      if (this.state === 'noMatch') return 'no audio';
    }
    return null;
  }

  /** True when the couple needs the resolve banner (§V2.6). */
  get needsResolve(): boolean {
    switch (this.state) {
      case 'ambiguous':
      case 'noMatch':
      case 'split':
      case 'twoSources':
        return true;
      default:
        return this.selectedAudioKey === null;
    }
  }

  get resolveTitle(): string {
    if (this.selectedAudioKey !== null) return 'Audio source — alternates available';
    if (this.state === 'noMatch') return 'No matching audio found';
    if (this.state === 'ambiguous') return 'Choose the audio for this take';
    return 'Audio source — alternates available';
  }

  /** Default output stem (§V2.8): `<video-stem>_synced`; extension is the rail's. */
  get defaultStem(): string {
    const base = this.video.name.replace(/\.[^.]+$/, '');
    return `${base}_synced`;
  }

  get outputStem(): string {
    return this.stemOverride && this.stemOverride.length > 0 ? this.stemOverride : this.defaultStem;
  }
}
