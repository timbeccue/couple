// ActivityController.ts — the unified activity-strip controller (TODO-005); TS
// port of mac/Sources/Couple/ActivityStrip.swift + the presentation half of
// CoupleKit/ActivitySummary.swift. All the "what line, what fraction, is it
// honest?" DECISION math lives in the pure core (reached via
// Policies.activitySummary → cpl_policy_activity). This class owns only the three
// genuinely shell-side pieces: (1) it snapshots the live job signals into the
// policy's request, (2) it gates the first appearance on a ~250 ms debounce so
// quick work never flashes, (3) it carries a per-phase monotonic floor so a
// determinate bar never goes backwards — then it ASSEMBLES the localized line
// from the structured decision core returns (all copy stays shell-side).
import * as Policies from '../kit/Policies';
import type { ActivityDecision } from '../kit/Policies';
import type { AppModel } from './AppModel';
import { Observable } from './observable';

/** The single line the strip renders, or null for "show nothing". */
export interface ActivitySummaryLine {
  text: string;
  /** Trustworthy 0…1 for a determinate bar, or null for an indeterminate one. */
  fraction: number | null;
  isError: boolean;
  phaseKey: string | null;
}

/** Mirrors couple_core::policy::activity::DEBOUNCE_MS (the midpoint of the
 * spec's 200–300 ms window). */
const DEBOUNCE_MS = 250;

/** The filenames core does not carry (they are shell strings), threaded from the
 * snapshot into the rendered line. */
interface DisplayNames {
  importFilename?: string;
  exportFilename?: string;
  syncVideoName?: string;
}

export class ActivityController extends Observable {
  private readonly model: AppModel;

  /** The line the strip renders, or null. The single source of truth for the view. */
  summary: ActivitySummaryLine | null = null;

  private activeSince: number | null = null; // ms timestamp the current op became active
  private debounceTimer: ReturnType<typeof setTimeout> | null = null;
  private floorPhaseKey: string | null = null;
  private phaseFloor = 0;
  private failureMessage: string | null = null;
  private failureTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(model: AppModel) {
    super();
    this.model = model;
  }

  /** Subscribe to every feeding signal so a change recomputes the strip — the TS
   * analogue of the mac shell's explicit `activity.refresh()` calls. The model
   * bumps on pool import progress AND on ingest signals (IngestController calls
   * model.invalidate()); the batch export coordinator is its own Observable. */
  attach(): void {
    this.model.subscribe(() => this.refresh());
    this.model.batch.subscribe(() => this.refresh());
  }

  // MARK: - Driving

  /** Recompute from the current signals. Arms/cancels the debounce as work
   * starts/stops, advances a live determinate bar once visible, clears to null
   * when everything is idle. Cheap; safe to call often. */
  refresh(): void {
    const signals = this.buildRequest();
    if (!ActivityController.hasActiveWork(signals)) {
      this.activeSince = null;
      this.clearDebounce();
      this.floorPhaseKey = null;
      this.phaseFloor = 0;
      this.set(null);
      return;
    }
    if (this.activeSince === null) this.activeSince = Date.now();
    const elapsed = Date.now() - this.activeSince;
    if (elapsed >= DEBOUNCE_MS) {
      this.publish(signals, elapsed);
    } else if (this.debounceTimer === null) {
      this.debounceTimer = setTimeout(() => {
        this.debounceTimer = null;
        this.refresh();
      }, DEBOUNCE_MS - elapsed);
    }
  }

  /** Hard-reset to idle (Clear Session, TODO-001). */
  reset(): void {
    this.activeSince = null;
    this.clearDebounce();
    this.floorPhaseKey = null;
    this.phaseFloor = 0;
    this.failureMessage = null;
    if (this.failureTimer) {
      clearTimeout(this.failureTimer);
      this.failureTimer = null;
    }
    this.set(null);
  }

  /** Surface a job FAILURE as an actionable error that auto-clears after 6 s
   * (failures skip the debounce in the pure policy). */
  reportFailure(message: string): void {
    this.failureMessage = message;
    if (this.failureTimer) clearTimeout(this.failureTimer);
    this.refresh();
    this.failureTimer = setTimeout(() => {
      this.failureMessage = null;
      this.failureTimer = null;
      this.refresh();
    }, 6000);
  }

  private clearDebounce(): void {
    if (this.debounceTimer) {
      clearTimeout(this.debounceTimer);
      this.debounceTimer = null;
    }
  }

  private set(line: ActivitySummaryLine | null): void {
    if (!ActivityController.equal(this.summary, line)) {
      this.summary = line;
      this.bump();
    }
  }

  // MARK: - Snapshot + publish

  /** Run the pure policy with the current debounce elapsed + monotonic floor,
   * carry the floor forward, and store the rendered line. */
  private publish(signals: Record<string, unknown>, elapsed: number): void {
    // Which phase does the winning line belong to (past debounce, no floor)? The
    // floor tracks it and resets when the phase changes.
    const winning = Policies.activitySummary({ ...signals, elapsedMs: DEBOUNCE_MS, debounceMs: DEBOUNCE_MS });
    if (winning?.phaseKey !== this.floorPhaseKey) {
      this.floorPhaseKey = winning?.phaseKey ?? null;
      this.phaseFloor = 0;
    }
    const req: Record<string, unknown> = { ...signals, elapsedMs: elapsed, debounceMs: DEBOUNCE_MS };
    if (this.floorPhaseKey !== null) req.phaseFloor = this.phaseFloor;

    const decision = Policies.activitySummary(req);
    if (decision?.fraction != null && decision.phaseKey === this.floorPhaseKey) {
      this.phaseFloor = Math.max(this.phaseFloor, decision.fraction);
    }
    this.set(this.render(decision));
  }

  /** Project the live job signals into the policy request (each field maps 1:1 to
   * a real signal; nothing is fabricated). Filenames ride alongside for the render. */
  private names: DisplayNames = {};

  private buildRequest(): Record<string, unknown> {
    const m = this.model;
    const req: Record<string, unknown> = {};
    this.names = {};
    if (this.failureMessage !== null) req.failure = this.failureMessage;

    // Per-stage signals, each gated on its late-final-frame flag so a stale 100%
    // frame from a finished stage cannot stick.
    const imp = m.pool.importing ? m.pool.importActivity : null;
    const importing = imp
      ? { phase: imp.phase, fraction: imp.fraction, index: imp.index, total: imp.total }
      : null;
    if (imp) this.names.importFilename = imp.filename;

    const match = m.ingest.resolving
      ? (m.ingest.matchingActivity ?? { phase: 'fingerprinting' as const, fraction: 0 })
      : null;

    const sync =
      m.ingest.syncTotal > 0 && m.ingest.syncIndex > 0
        ? { index: m.ingest.syncIndex, total: m.ingest.syncTotal }
        : null;
    if (sync) this.names.syncVideoName = m.ingest.syncVideoName ?? undefined;

    if (m.ingest.ingestInFlight) {
      // UNIFIED ingest: import → match → sync is ONE 0→100% bar.
      const ingest: Record<string, unknown> = { stagesPlanned: m.ingest.ingestStages };
      if (importing) ingest.importing = importing;
      if (match) ingest.matching = { phase: match.phase, fraction: match.fraction };
      if (sync) ingest.syncing = sync;
      req.ingest = ingest;
    } else {
      if (importing) req.importing = importing;
      if (m.ingest.resolving) {
        req.pairing = {
          videoCount: m.pool.videos.length,
          audioCount: m.pool.audios.length,
          resolving: false,
          afterImport: false,
          importTail: false,
        };
      }
      if (sync) req.syncing = sync;
    }

    // Batch export: BatchExport phase + the running row's per-row fraction.
    const phase = m.batch.phase;
    if (phase.kind === 'preflight') {
      req.exporting = { index: 0, total: m.queued.length, rowFraction: 0, preflight: true };
    } else if (phase.kind === 'running') {
      let index = phase.done + 1;
      let rowFraction = 0;
      const queued = m.queued;
      for (let i = 0; i < queued.length; i++) {
        const s = queued[i].exportStatus;
        if (s.kind === 'running') {
          index = i + 1;
          rowFraction = s.fraction;
          this.names.exportFilename = queued[i].video.name;
          break;
        }
      }
      req.exporting = { index, total: phase.total, rowFraction, preflight: false };
    }
    return req;
  }

  private static hasActiveWork(req: Record<string, unknown>): boolean {
    return (
      req.importing != null ||
      req.pairing != null ||
      req.syncing != null ||
      req.exporting != null ||
      req.ingest != null ||
      req.failure != null
    );
  }

  // MARK: - Presentation (all copy + pluralization stays shell-side)

  /** Map a structured decision to the localized line, or null (show nothing). */
  private render(decision: ActivityDecision | null): ActivitySummaryLine | null {
    if (!decision) return null;
    const text = this.text(decision.line);
    if (text === null) return null;
    return { text, fraction: decision.fraction, isError: decision.isError, phaseKey: decision.phaseKey };
  }

  private text(line: Record<string, unknown> & { kind: string }): string | null {
    const n = (key: string): number => {
      const v = line[key];
      return typeof v === 'number' ? v : 0;
    };
    const names = this.names;
    switch (line.kind) {
      case 'failure':
        return typeof line.message === 'string' ? line.message : '';
      case 'import':
      case 'ingestImport': {
        const pct = n('percent');
        const idx = line.index;
        const total = line.total;
        if (typeof idx === 'number' && typeof total === 'number' && total > 1 && names.importFilename) {
          return `Importing ${idx} of ${total}: ${names.importFilename} — ${pct}%`;
        }
        if (names.importFilename) return `Importing ${names.importFilename} — ${pct}%`;
        return `Importing — ${pct}%`;
      }
      case 'export': {
        const pct = n('percent');
        const idx = n('index');
        const total = n('total');
        if (names.exportFilename) return `Exporting ${idx} of ${total}: ${names.exportFilename} — ${pct}%`;
        return `Exporting ${idx} of ${total} — ${pct}%`;
      }
      case 'exportPreflight':
        return 'Preparing export…';
      case 'exportFinishing':
        return 'Finishing export…';
      case 'pairingMatchingTail':
      case 'ingestMatching':
        return 'Matching couples…';
      case 'pairingResolving':
        return 'Resolving matches…';
      case 'pairingAfterImport':
        return 'Updating matches after import…';
      case 'pairingComparing': {
        const v = n('videoCount');
        const a = n('audioCount');
        return `Comparing ${v} ${plural('video', v)} with ${a} audio ${plural('file', a)}…`;
      }
      case 'pairingAnalyzing': {
        const c = n('count');
        return `Analyzing ${c} ${plural('file', c)} for matches…`;
      }
      case 'syncing':
      case 'ingestSyncing': {
        const idx = n('index');
        const total = n('total');
        let text = `Syncing couple ${idx} of ${total}`;
        if (names.syncVideoName) text += `: ${names.syncVideoName}`;
        return text;
      }
      case 'ingestWorking':
        return 'Working…';
      default:
        return null; // unknown kind ⇒ conservative fallback (show nothing)
    }
  }

  private static equal(a: ActivitySummaryLine | null, b: ActivitySummaryLine | null): boolean {
    if (a === null || b === null) return a === b;
    return (
      a.text === b.text && a.fraction === b.fraction && a.isError === b.isError && a.phaseKey === b.phaseKey
    );
  }
}

function plural(word: string, n: number): string {
  return n === 1 ? word : `${word}s`;
}
