// IngestController.ts — import → resolve → couples orchestration (SPEC-V2 §5.2);
// TS port of mac/Sources/Couple/IngestController.swift. Owns the unified
// import→match→sync activity window and the serialized per-couple sync queue.
// The §3.3 resolution DECISION runs in core via one determinate job
// (Engine.resolveStart, the cpl_resolve_start twin) — the SAME resolver the CLI
// and the Swift shell use; audio-less videos are wrapped here as inert
// no-match rows (the header contract).
import type { Engine } from '../kit/Engine';
import * as Policies from '../kit/Policies';
import type { ProgressUpdate, ResolvedRow } from '../kit/Wire';
import type { AppModel } from './AppModel';
import { CoupleRow } from './CoupleRow';

export type MatchPhase = 'fingerprinting' | 'comparing' | 'crosschecking';

export interface MatchingSignal {
  phase: MatchPhase;
  fraction: number;
}

export class IngestController {
  private readonly model: AppModel;

  /** True while the resolution job runs. */
  resolving = false;
  matchingActivity: MatchingSignal | null = null;
  /** True from a drop's start until its post-import sync queue drains — the
   * window in which import→match→sync is ONE unified 0→100% bar. */
  ingestInFlight = false;
  /** Which ingest stages the current operation will run, so the unified activity
   * bar renormalizes over exactly the present stages (TODO-005). A dropped batch
   * runs all three; a standalone re-resolve leaves import off. */
  ingestStages = { importStage: true, matchStage: true, syncStage: true };

  // Serialized per-couple sync queue (§5.2).
  private syncPending: CoupleRow[] = [];
  private syncRunning = false;
  syncIndex = 0;
  syncTotal = 0;
  syncVideoName: string | null = null;
  /** Restore path: saved alignment instead of a fresh auto-sync (§5.4). */
  suppressAutoSync = false;

  constructor(model: AppModel) {
    this.model = model;
  }

  importDropped(paths: string[]): void {
    void this.ingest(paths);
  }

  async ingest(paths: string[]): Promise<void> {
    this.ingestInFlight = true;
    this.model.invalidate();
    const added = await this.model.pool.importDropped(paths);
    if (added.length === 0) {
      // All-duplicates drop: no match/sync to run, end the window cleanly.
      this.ingestInFlight = false;
      this.model.invalidate();
      return;
    }
    // Default the export folder to where the most recent import came from,
    // until the user picks one explicitly.
    if (this.model.destinationAuto && added.length > 0) {
      const last = added[added.length - 1];
      this.model.exportDestination = last.path.slice(0, last.path.lastIndexOf('/')) || null;
    }
    await this.resolveQueue();
    if (!this.syncRunning && this.syncPending.length === 0) {
      this.ingestInFlight = false;
      this.model.invalidate();
    }
  }

  /** Run the §3.3 row resolution over the pool, then build/update couples —
   * pinned/approved selections survive (§3.6, via the pure row-merge policy). */
  async resolveQueue(): Promise<void> {
    const { pool, engine } = this.model;
    if (pool.videos.length === 0 && pool.audios.length === 0) return;
    this.resolving = true;
    this.model.invalidate();

    const importableHandles = pool.videos.filter((v) => v.handle).map((v) => v.handle!.id);
    const audioHandles = pool.audios.map((a) => a.handle!.id);

    let coreRows: ResolvedRow[] = [];
    if (importableHandles.length > 0) {
      try {
        const job = engine.resolveStart(importableHandles, audioHandles, this.model.resolveParamsJSON());
        job.onProgress((u) => {
          if (!this.resolving) return;
          this.matchingActivity = IngestController.matchPhase(u);
          this.model.invalidate();
        });
        coreRows = await job.value;
      } catch {
        coreRows = [];
      }
    }

    // Splice core's importable-video rows back into full pool order; wrap each
    // audio-less video as an inert no-match row (the resolver's own shape).
    const byVideo = new Map(coreRows.map((r) => [r.videoKey, r]));
    const assembled: ResolvedRow[] = pool.videos.map((v) => {
      const r = v.handle ? byVideo.get(v.id) : undefined;
      return (
        r ?? {
          videoKey: v.id,
          videoName: v.name,
          state: 'noMatch',
          chip: 'no match',
          selected: null,
          alternates: [],
          resolution: { clusters: 0, note: 'no audio stream' },
        }
      );
    });

    this.applyResolution(assembled);
    // Late-final-frame gate: clear the matching signal synchronously with
    // `resolving`, so a stale 100% frame cannot stick on the strip.
    this.matchingActivity = null;
    this.resolving = false;
    this.model.invalidate();
  }

  private static matchPhase(u: ProgressUpdate): MatchingSignal {
    let phase: MatchPhase;
    switch (u.phase) {
      case 'MatchFingerprint':
        phase = 'fingerprinting';
        break;
      case 'MatchCorrelate':
      case 'Verify':
        phase = 'comparing';
        break;
      case 'MatchGraph':
      case 'Resolve':
        phase = 'crosschecking';
        break;
      default:
        phase = 'fingerprinting';
    }
    return { phase, fraction: u.fraction };
  }

  private applyResolution(rows: ResolvedRow[]): void {
    const model = this.model;
    const order: CoupleRow[] = [];
    for (const row of rows) {
      const videoEntry = model.pool.entry(row.videoKey);
      if (!videoEntry) continue;
      const existing = model.couples.find((c) => c.id === row.videoKey);
      if (existing) {
        this.mergeResolution(existing, row);
        order.push(existing);
      } else {
        const couple = new CoupleRow(videoEntry, row, model.engine);
        order.push(couple);
        this.ensureSession(couple);
      }
    }
    model.couples = order;
    if (model.selectedID === null) model.select(model.orderedCouples[0]?.id ?? null);
    model.invalidate();
  }

  /** §3.6 stability: fold a fresh resolution into an existing couple. The WHAT
   * is the PURE core row-merge policy; this only applies the mutations together. */
  private mergeResolution(couple: CoupleRow, row: ResolvedRow): void {
    const fresh = (row.selected ? [row.selected] : []).concat(row.alternates);
    const plan = Policies.rowMergePlan({
      protected: couple.pinned || couple.approved,
      currentSelectedKey: couple.selectedAudioKey,
      currentCandidateKeys: couple.candidates.map((c) => c.audioKey),
      freshSelectedKey: row.selected?.audioKey ?? null,
      freshCandidateKeys: fresh.map((c) => c.audioKey),
    });

    if (plan.adoptFreshDisplay) {
      couple.state = row.state;
      couple.chip = row.chip;
      couple.detail = row.resolution;
      couple.candidates = fresh;
      if (plan.clearForced) couple.forced = false;
      couple.selectionProvenance = row.selected ? 'automatic' : 'none';
      couple.manualMatchConfident = null;
    } else {
      const known = new Set(couple.candidates.map((c) => c.audioKey));
      couple.candidates.push(...fresh.filter((c) => !known.has(c.audioKey)));
    }
    if (plan.rebuildSession) {
      couple.selectedAudioKey = plan.selectedAudioKey;
      this.ensureSession(couple);
    }
    couple.invalidate();
  }

  /** Ensure the couple's PairSession reflects its selection; queue auto-sync.
   * No selection ⇒ tear the session down (ambiguous/no-match carry no session). */
  ensureSession(couple: CoupleRow): void {
    const model = this.model;
    // Close the shared audition's stream before freeing this couple's old core
    // session (audio-graph teardown ordering — the stream was opened from it).
    if (couple.id === model.selectedID) model.audition.teardown();
    const key = couple.selectedAudioKey;
    const cam = couple.video.handle;
    const audio = key ? model.pool.entry(key)?.handle : null;
    if (!key || !cam || !audio) {
      couple.pair.teardown();
      return;
    }
    couple.pair.load(cam, audio, couple.candidates.find((c) => c.audioKey === key) ?? null);
    if (!this.suppressAutoSync) this.enqueueSync(couple);
    if (couple.id === model.selectedID) model.attachAudition(couple);
  }

  // ---------- serialized per-couple sync (§5.2) ----------

  enqueueSync(couple: CoupleRow): void {
    if (!this.syncPending.includes(couple)) {
      this.syncPending.push(couple);
      this.syncTotal += 1; // high-water mark across the batch
    }
    this.pumpSync();
  }

  private pumpSync(): void {
    if (this.syncRunning || this.syncPending.length === 0) return;
    this.syncRunning = true;
    this.syncIndex = 0;
    this.model.invalidate();
    void (async () => {
      let next: CoupleRow | undefined;
      while ((next = this.syncPending.shift())) {
        if (!next.pair.session) continue;
        this.syncIndex += 1;
        this.syncVideoName = next.video.name;
        this.model.invalidate();
        next.pair.runAutoSync(); // ambient (§V2.4); dots carry the result
        await next.pair.awaitSync();
        // Persist this pair's alignment (§5.4) so a reopen recovers the offset
        // without re-syncing (the per-pair autosave the restore path reads).
        this.model.autosaveCouple(next);
      }
      this.syncRunning = false;
      this.syncIndex = 0;
      this.syncTotal = 0;
      this.syncVideoName = null;
      this.ingestInFlight = false;
      this.model.saveQueue(); // persist the queue once the sync batch drains
      this.model.invalidate();
    })();
  }
}
