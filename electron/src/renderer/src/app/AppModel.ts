// AppModel.ts — the shell's root model façade; TS port of AppModel.swift
// (M2 scope: pool + couples + selection + ingest + export settings; audition
// and persistence land with their milestones).
import { Engine } from '../kit/Engine';
import { env } from '../kit/native';
import * as Policies from '../kit/Policies';
import { AuditionController } from './AuditionController';
import { Observable } from './observable';
import { ActivityController } from './ActivityController';
import { BatchExport, type ExportFormat } from './BatchExport';
import { CoupleRow } from './CoupleRow';
import { IngestController } from './IngestController';
import { MediaPool } from './MediaPool';
import { PairSession } from './PairSession';
import { PoolController } from './PoolController';
import { QueueStore, type SavedQueue } from './QueueStore';
import { SessionStore } from './SessionStore';
import { ToastCenter } from './Toasts';
import { UndoStack } from './UndoStack';

export class AppModel extends Observable {
  readonly engine: Engine;
  readonly toasts: ToastCenter;
  readonly pool: MediaPool;
  readonly ingest: IngestController;
  /** The app's ONE shared audition (§5.3); follows the selection. */
  readonly audition: AuditionController;
  /** Serial batch export coordinator (§V2.8). */
  readonly batch: BatchExport;
  /** Media-pool selection + sticky preview + removal (owner decision 3; TODO-001/002). */
  readonly poolController: PoolController;
  /** Unified long-running activity strip (TODO-005) — debounced summary + floor. */
  readonly activity: ActivityController;
  /** Queue-mutation undo/redo stack (TODO-015): approve / re-pair / rename. */
  readonly undo: UndoStack;

  couples: CoupleRow[] = [];
  selectedID: string | null = null;

  // Export settings (§V2.8) — the rail owns format/destination.
  exportDestination: string | null = null;
  destinationAuto = true;
  globalFormat: ExportFormat = 'Auto';
  globalKeepCameraAudio = false;

  private cachedResolveParamsJSON: string | null = null;

  constructor() {
    super();
    this.engine = new Engine({
      ffmpegPath: env.COUPLE_FFMPEG_PATH || undefined,
      ffprobePath: env.COUPLE_FFPROBE_PATH || undefined,
      cacheDir: env.COUPLE_CACHE_DIR || undefined,
    });
    this.toasts = new ToastCenter();
    this.pool = new MediaPool(this.engine, this.toasts);
    this.ingest = new IngestController(this);
    this.audition = new AuditionController();
    this.batch = new BatchExport(this);
    this.poolController = new PoolController(this);
    this.undo = new UndoStack();
    this.activity = new ActivityController(this);
    this.activity.attach();
    this.pool.subscribe(() => this.bump());
    // A placed playhead drives the shared audition (audio clock is the master;
    // the video hero follows it) when the audition is on that pair.
    PairSession.onPlayheadSeek = (pair, t) => {
      if (this.audition.couple?.pair === pair) this.audition.seek(t);
    };
  }

  /** Number of approved couples ready to export (§V2.8). */
  get approvedCount(): number {
    return this.queued.length;
  }

  /** Pick an export destination folder; a manual choice pins it (§V2.8). */
  chooseExportDestination(dir: string): void {
    this.exportDestination = dir;
    this.destinationAuto = false;
    this.saveQueue();
    this.bump();
  }

  /** ResolveParams for the app: core defaults with the match classifier ON
   * (the app-side switch, mirroring the Swift shell; CLI stays opt-in). */
  resolveParamsJSON(): string | null {
    if (this.cachedResolveParamsJSON) return this.cachedResolveParamsJSON;
    const defaults = this.engine.resolveParamsDefaultJSON();
    if (!defaults) return JSON.stringify({ verifyActive: true });
    const params = JSON.parse(defaults) as Record<string, unknown>;
    params.verifyActive = true;
    this.cachedResolveParamsJSON = JSON.stringify(params);
    return this.cachedResolveParamsJSON;
  }

  // ---------- selection ----------

  // Three live workflow sections (§V2.2); visual order drives j/k.
  get queued(): CoupleRow[] {
    return this.couples.filter((c) => c.approved);
  }

  get toApprove(): CoupleRow[] {
    return this.couples.filter((c) => !c.approved && c.verdict === 'good');
  }

  get toFix(): CoupleRow[] {
    return this.couples.filter((c) => !c.approved && c.verdict !== 'good');
  }

  get orderedCouples(): CoupleRow[] {
    return [...this.queued, ...this.toApprove, ...this.toFix];
  }

  get selected(): CoupleRow | null {
    return this.couples.find((c) => c.id === this.selectedID) ?? null;
  }

  select(id: string | null): void {
    if (this.selectedID === id) return;
    this.selectedID = id;
    const c = this.selected;
    if (c && c.hasSession) this.attachAudition(c);
    else this.audition.teardown();
    this.bump();
  }

  /** (Re)attach the shared audition to a couple's live session. */
  attachAudition(couple: CoupleRow): void {
    if (!couple.hasSession) return;
    this.audition.attach(couple);
  }

  /** The resolver z floor (core default) — the forced-verdict policy input. */
  get zMin(): number {
    const json = this.resolveParamsJSON();
    if (!json) return 8.5;
    return (JSON.parse(json) as { zMin?: number }).zMin ?? 8.5;
  }

  /** Manual drag-to-pair (§3.6): what a dropped pool audio does to a couple is
   * the PURE core repair-drop policy's decision; this only applies it. */
  repairDrop(couple: CoupleRow, droppedKey: string): void {
    const outcome = Policies.repairDrop({
      hasSelectedCouple: true,
      payloadIsAudio: this.pool.entry(droppedKey)?.role === 'audio',
      videoAvailable: couple.video.handle !== null,
      currentSelectedKey: couple.selectedAudioKey,
      droppedKey,
      approved: couple.approved,
    });
    if (outcome === 'reject' || outcome === 'noOp') return;
    if (outcome === 'needsConfirmation') {
      const name = this.pool.entry(droppedKey)?.name ?? droppedKey;
      if (!window.confirm(`Replace the approved pairing with ${name}? The sync will re-run.`)) {
        return;
      }
    }
    // Capture the pre-repair state BEFORE mutating so ⌘Z restores the whole row
    // (selection, pins, approval, candidates) — mirrors AppModel.repair (§V2.6).
    const snapshot = this.captureRepair(couple);
    if (couple.approved) couple.pair.setVerified(false); // un-queue the accepted replacement
    couple.exportStatus = { kind: 'idle' };
    const z = couple.candidates.find((c) => c.audioKey === droppedKey)?.z ?? 0;
    couple.applyManualPick(droppedKey, z, this.zMin);
    this.ingest.ensureSession(couple);
    void this.reevaluateManualMatch(couple); // TODO-009: green only if this pair clears the classifier
    this.registerRepairUndo(couple, snapshot);
    this.saveQueue();
    this.invalidate();
  }

  /** TODO-009: (re)evaluate a MANUAL pick's match evidence and update its dot,
   * reusing the SAME confident classifier the resolver uses — core's
   * `PairEvidence.classification === 'match'` (`matchPair`), no re-derived threshold
   * (invariant 8). The Electron shell runs the fingerprint-primary default, so the
   * cell's classification is the whole answer (mirror of the fingerprint branch of
   * Resolver.swift `manualPairIsConfident`). Run on a manual pick / re-pair and on a
   * restore whose evidence wasn't persisted, so a questionable-looking manual pick
   * that DOES match greens without a full re-sync. A core fault (null) is NOT
   * confident — a manual pick never greens on absent evidence. Deferred a microtask
   * so the pick paints (questionable) before the dot may flip (the Swift shell runs
   * it off-main); no-op if the pair's handles are gone or the selection changed
   * underneath it. */
  async reevaluateManualMatch(couple: CoupleRow): Promise<void> {
    const cam = couple.video.handle;
    const key = couple.selectedAudioKey;
    const audio = key ? (this.pool.entry(key)?.handle ?? null) : null;
    if (!cam || !key || !audio) return;
    await Promise.resolve(); // let the pick paint first (Swift shell runs this off-main)
    const confident = this.engine.matchPair(cam, audio)?.classification === 'match';
    if (couple.selectionProvenance !== 'manual' || couple.selectedAudioKey !== key) return;
    couple.manualMatchConfident = confident;
    couple.invalidate();
    this.saveQueue();
  }

  // ---------- approve / rename (undoable, TODO-015) ----------

  /** Approve ≡ queue for export (§V2.5). Only a couple with a session can be
   * approved. Symmetric, so undo re-toggles (and redo comes free from the
   * re-registration on undo). */
  toggleApprove(couple: CoupleRow): void {
    if (!couple.pair.session) return;
    couple.pair.setVerified(!couple.approved);
    this.undo.register(couple.approved ? 'Approve' : 'Unapprove', () => this.toggleApprove(couple));
    this.autosaveCouple(couple);
    this.saveQueue();
    this.invalidate();
  }

  /** Set (or clear) a couple's in-place export-name override (§V2.8), undoably.
   * Symmetric via the old value, so redo is the re-registration on undo. */
  setStemOverride(couple: CoupleRow, newValue: string | null): void {
    const old = couple.stemOverride;
    if (old === newValue) return;
    couple.stemOverride = newValue;
    couple.invalidate();
    this.undo.register('Rename Export', () => this.setStemOverride(couple, old));
    this.saveQueue();
    this.invalidate();
  }

  performUndo(): void {
    this.undo.undo();
    this.invalidate();
  }

  performRedo(): void {
    this.undo.redo();
    this.invalidate();
  }

  // ---------- re-pair undo snapshot (TODO-015 / §V2.6) ----------

  private captureRepair(c: CoupleRow): RepairSnapshot {
    return {
      selectedAudioKey: c.selectedAudioKey,
      pinned: c.pinned,
      forced: c.forced,
      chip: c.chip,
      provenance: c.selectionProvenance,
      manualMatchConfident: c.manualMatchConfident,
      approved: c.approved,
      exportStatus: c.exportStatus,
      candidates: [...c.candidates],
    };
  }

  private registerRepairUndo(couple: CoupleRow, snapshot: RepairSnapshot): void {
    this.undo.register('Change Audio', () => this.applyRepairSnapshot(snapshot, couple));
  }

  /** Put a row back to a pre-repair snapshot (undo), re-registering the current
   * state as redo. The session rebuilds through the same guarded path repair uses;
   * a snapshot whose audio was since removed declines with a toast. */
  private applyRepairSnapshot(snapshot: RepairSnapshot, couple: CoupleRow): void {
    const cam = couple.video.handle;
    if (!cam) return;
    const audioHandle = snapshot.selectedAudioKey
      ? (this.pool.entry(snapshot.selectedAudioKey)?.handle ?? null)
      : null;
    if (snapshot.selectedAudioKey !== null && audioHandle === null) {
      this.toasts.error("Can't undo", 'That audio is no longer in the session.');
      return;
    }
    this.registerRepairUndo(couple, this.captureRepair(couple)); // redo
    couple.selectedAudioKey = snapshot.selectedAudioKey;
    couple.pinned = snapshot.pinned;
    couple.forced = snapshot.forced;
    couple.chip = snapshot.chip;
    couple.selectionProvenance = snapshot.provenance;
    couple.manualMatchConfident = snapshot.manualMatchConfident;
    couple.exportStatus = snapshot.exportStatus;
    couple.candidates = snapshot.candidates;
    if (couple.id === this.selectedID) this.audition.teardown();
    if (audioHandle) {
      couple.pair.load(
        cam,
        audioHandle,
        couple.candidates.find((c) => c.audioKey === snapshot.selectedAudioKey) ?? null
      );
      couple.pair.runAutoSync();
      if (snapshot.approved) couple.pair.setVerified(true); // load() reset it
    } else {
      couple.pair.teardown();
    }
    if (couple.id === this.selectedID) this.attachAudition(couple);
    couple.invalidate();
    this.saveQueue();
    this.invalidate();
  }

  // ---------- persistence (SPEC-V2 §5.4) ----------

  saveQueue(): void {
    QueueStore.save(this);
  }

  /** Persist every live pair's alignment + the queue (quit / clean save). */
  saveAll(): void {
    for (const c of this.couples) this.autosaveCouple(c);
    this.saveQueue();
  }

  /** Write a couple's per-pair alignment (SessionStore) if it has a live session. */
  autosaveCouple(couple: CoupleRow): void {
    const session = couple.pair.session;
    const ext = couple.selectedAudioKey ? this.pool.entry(couple.selectedAudioKey) : null;
    if (!session || !ext) return;
    let alignment;
    try {
      alignment = session.state();
    } catch {
      return;
    }
    SessionStore.save(
      { path: couple.video.path, contentKey: couple.video.id, durationS: couple.video.durationS },
      { path: ext.path, contentKey: ext.id, durationS: ext.durationS },
      alignment
    );
  }

  /** Apply this pair's saved per-pair alignment (§5.4) if present, so a reopened
   * couple recovers its offset WITHOUT a fresh sync. Returns false when none. */
  restoreCoupleAlignment(couple: CoupleRow): boolean {
    const session = couple.pair.session;
    const ext = couple.selectedAudioKey ? this.pool.entry(couple.selectedAudioKey) : null;
    if (!session || !ext) return false;
    const saved = SessionStore.load(couple.video.id, ext.id);
    if (!saved) return false;
    try {
      session.setState(saved.alignment);
    } catch {
      return false;
    }
    // Reflect the restored state so dots read synced and no fresh sync is queued.
    couple.pair.syncStatus = 'synced';
    couple.pair.lastSyncFailed = false;
    couple.pair.invalidate();
    return true;
  }

  /** On launch, offer to restore the previous queue (§5.4) — unless a debug drive
   * is loading content explicitly. The offer is a non-blocking toast action. */
  offerRestoreOnLaunch(): void {
    if (env.COUPLE_OPEN_DIR) return;
    const saved = QueueStore.load();
    if (!saved) return;
    const when = saved.savedAt.slice(0, 16).replace('T', ' ');
    const n = saved.couples.length;
    this.toasts.show('info', 'Restore previous session?', `${n} couple${n === 1 ? '' : 's'} · ${when}`, {
      label: 'Restore',
      run: () => void this.applyRestore(saved),
    });
  }

  /** Re-import the saved files, re-resolve, then overlay the saved per-couple
   * selection (pins), approved flags, per-couple alignment (from the unchanged
   * per-pair autosave), and export settings (§5.4). suppressAutoSync keeps the
   * re-ingest from firing a fresh sync where a saved alignment applies instead. */
  async applyRestore(saved: SavedQueue): Promise<void> {
    this.ingest.suppressAutoSync = true;
    await this.ingest.ingest(saved.files.map((f) => f.path));
    for (const sc of saved.couples) {
      const c = this.couples.find((x) => x.id === sc.videoKey);
      if (!c) continue;
      c.pinned = sc.pinned;
      c.forced = sc.forced;
      c.stemOverride = sc.stemOverride;
      if (sc.selectedAudioKey !== c.selectedAudioKey) {
        c.selectedAudioKey = sc.selectedAudioKey;
        const e = sc.selectedAudioKey ? this.pool.entry(sc.selectedAudioKey) : null;
        if (sc.selectedAudioKey && e && !c.candidates.some((x) => x.audioKey === sc.selectedAudioKey)) {
          c.candidates.push({
            audioKey: sc.selectedAudioKey,
            audioName: e.name,
            z: 0,
            lagMs: 0,
            coverage: { start: 0, end: 0 },
          });
        }
        this.ingest.ensureSession(c);
      }
      // TODO-009: restore provenance + evidence so the dot color round-trips; a
      // pre-009 file lacks provenance → infer from `pinned` (a pinned pick is manual).
      const provenance =
        sc.selectionProvenance ?? (c.selectedAudioKey === null ? 'none' : c.pinned ? 'manual' : 'automatic');
      c.selectionProvenance = provenance;
      c.manualMatchConfident = provenance === 'manual' ? (sc.manualMatchConfident ?? null) : null;
      // Prefer the user's saved per-pair alignment; else sync this couple.
      if (!this.restoreCoupleAlignment(c) && c.pair.session) this.ingest.enqueueSync(c);
      if (sc.approved && c.pair.session && !c.approved) c.pair.setVerified(true);
      // TODO-009: recompute a manual pick's evidence verdict only when it wasn't
      // saved (a back-compat restore), reusing the SAME classifier as the pick path.
      if (provenance === 'manual' && c.manualMatchConfident === null) void this.reevaluateManualMatch(c);
    }
    if (saved.export.destination) {
      this.exportDestination = saved.export.destination;
      this.destinationAuto = false; // a saved destination is treated as pinned
    }
    this.globalFormat = (saved.export.globalFormat as ExportFormat) || 'Auto';
    this.globalKeepCameraAudio = saved.export.keepCameraAudio;
    this.ingest.suppressAutoSync = false;
    const first = this.orderedCouples[0];
    if (first) this.select(first.id);
    this.toasts.success('Session restored', `${this.couples.length} couples`);
    this.invalidate();
  }

  // ---------- clear session (TODO-001) ----------

  get hasContent(): boolean {
    return !this.pool.isEmpty;
  }

  resetExportSettings(): void {
    this.exportDestination = null;
    this.destinationAuto = true;
    this.globalFormat = 'Auto';
    this.globalKeepCameraAudio = false;
  }

  /** The media/couple/approved counts for the Clear Session dialog copy (pure core). */
  clearSessionCounts(): { media: number; couples: number; approved: number } {
    return Policies.clearSessionCounts({
      mediaCount: this.pool.entries.length,
      couples: this.couples.map((c) => ({
        videoKey: c.id,
        selectedAudioKey: c.selectedAudioKey,
        approved: c.approved,
      })),
    });
  }

  /** Confirm + clear (TODO-001). Blocked while a batch export runs (it owns the
   * couples' files). The dialog states the consequence-focused counts. */
  requestClearSession(): void {
    if (!this.hasContent || this.batch.isRunning) return;
    const c = this.clearSessionCounts();
    const msg =
      `This discards ${c.media} media file${c.media === 1 ? '' : 's'}, ` +
      `${c.couples} couple${c.couples === 1 ? '' : 's'} (${c.approved} approved) and returns to the ` +
      `import screen. Your source files and any files you have already exported are not deleted.`;
    if (window.confirm(msg)) this.clearSession();
  }

  /** Clear the whole session and return to the import screen. Stops work, releases
   * every pooled track, clears queue/export/undo state, REMOVES the persisted
   * restore record, and resets to empty. Source/exported files are untouched; the
   * cleared state never reappears on relaunch (the record is gone and the save
   * guard refuses to re-persist an empty session). */
  clearSession(): void {
    this.poolController.teardown(); // stop the pool preview player before freeing handles
    this.audition.teardown();
    for (const couple of this.couples) couple.pair.teardown();
    this.couples = [];
    this.selectedID = null;
    this.resetExportSettings();
    this.pool.teardown();
    this.pool.skipped = [];
    this.poolController.prunePoolSelection(); // pool now empty ⇒ selection clears
    this.undo.clear();
    QueueStore.clear();
    SessionStore.clearAll();
    this.activity.reset();
    this.invalidate();
  }

  // ---------- lifecycle ----------

  teardown(): void {
    this.saveAll(); // persist queue + per-pair alignment on quit (§5.4) before freeing
    this.audition.teardown(); // stream closes before its session frees
    for (const c of this.couples) c.pair.teardown();
    this.couples = [];
    this.pool.teardown();
    this.engine.free();
  }
}

/** Everything a manual re-pair mutates on a row, captured BEFORE the mutation so
 * ⌘Z can put the row back (including approval — pair.load resets verified). */
interface RepairSnapshot {
  selectedAudioKey: string | null;
  pinned: boolean;
  forced: boolean;
  chip: string;
  provenance: Policies.SelectionProvenance;
  manualMatchConfident: boolean | null;
  approved: boolean;
  exportStatus: CoupleRow['exportStatus'];
  candidates: CoupleRow['candidates'];
}

/** The renderer's single AppModel (the window owns exactly one, like the mac
 * shell's @State model). */
export const appModel = new AppModel();
