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
import { WindowLayoutBridge, type LayoutMode } from './WindowLayout';
import type { MonitorMode } from '../kit/Audition';
import type { SessionHandle } from '../kit/Session';
import type { TrackHandle } from '../kit/Track';
import type { RowCandidate } from '../kit/Wire';

const nodeRequire: NodeRequire = (window as unknown as { require: NodeRequire }).require;
const pathMod = nodeRequire('node:path') as typeof import('node:path');

/** A candidate being auditioned at its best-fit lag in a SCRATCH core session (row
 * camera + that candidate's audio, offset = the candidate's lag; PLAN-COMPACT-MODE
 * Phase 3 step 2). Nothing is pinned or re-synced; the couple's own session is
 * untouched. Core reads the session offset on every audition fill. */
export interface CandidateAudition {
  coupleID: string;
  candidate: RowCandidate;
  track: TrackHandle;
  session: SessionHandle;
  /** What to put back when the audition ends. */
  previousMode: MonitorMode;
  previousPlayhead: number;
}

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
  /** Queue-mutation undo/redo stack (TODO-015): include toggle / re-pair / rename. */
  readonly undo: UndoStack;

  couples: CoupleRow[] = [];
  selectedID: string | null = null;

  // Export settings (§V2.8) — the rail owns format/destination.
  exportDestination: string | null = null;
  destinationAuto = true;
  globalFormat: ExportFormat = 'Auto';
  globalKeepCameraAudio = false;

  private cachedResolveParamsJSON: string | null = null;

  // ---------- arrangement (UX §V2.11; PLAN-COMPACT-MODE Phase 6) ----------

  /** Compact (session line, fixed preview, couple list, export panel) or full
   * (three columns). Main owns the persisted value and the window frame; COMPACT
   * when nothing is stored (first launch after the update), then last-used. */
  layoutMode: LayoutMode = WindowLayoutBridge.initialMode();
  /** The couple whose candidate list is reopened inline under the player by
   * "Compare others" / "Change…" after a manual pick (§6.3). Model-owned so it
   * survives an arrangement switch; selecting a row clears it. */
  compareCoupleID: string | null = null;
  /** The full editor's ⚙ sync tools are expanded (Return in compact opens them). */
  syncToolsExpanded = false;
  /** D10: a Pan L/R both-mode is heard as Synced (Overlay) while compact; this
   * remembers that the user's mode was Pan, restored on returning to full. */
  private panHeldForCompact = false;
  /** Installed by the root view: switches arrangements WITH the window move and
   * the FLIP of the four blocks. Absent (tests), the switch is model-only. */
  layoutSwitcher: ((mode: LayoutMode) => Promise<void>) | null = null;

  /** The candidate being auditioned, if any (one at a time). */
  candidateAudition: CandidateAudition | null = null;
  /** Per couple, the row as it stood before its first manual pick this run —
   * drives the preview's "Audio chosen by you · Undo" link. */
  private prePickSnapshots = new Map<string, RepairSnapshot>();

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

  /** The export batch (D1): every couple the core include rule says goes out next. */
  get included(): CoupleRow[] {
    return this.couples.filter((c) => c.included);
  }

  /** N in "Export N files" / "N to export". */
  get includedCount(): number {
    return this.included.length;
  }

  /** The export rail's Files list: included ∪ already exported (rule in core). */
  get filesList(): CoupleRow[] {
    return this.couples.filter((c) => c.inFilesList);
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

  // Two live workflow sections (UX §V2.12 / D2): To fix above Ready; visual order
  // drives j/k. Membership is the VERDICT alone — inclusion never moves a row.

  /** Verdict good: includable, in the Ready section. */
  get ready(): CoupleRow[] {
    return this.couples.filter((c) => c.verdict === 'good');
  }

  /** Verdict questionable / none ("pick one", "no audio", forced): To fix. */
  get toFix(): CoupleRow[] {
    return this.couples.filter((c) => c.verdict !== 'good');
  }

  get orderedCouples(): CoupleRow[] {
    return [...this.toFix, ...this.ready];
  }

  get selected(): CoupleRow | null {
    return this.couples.find((c) => c.id === this.selectedID) ?? null;
  }

  select(id: string | null): void {
    if (this.selectedID === id) return;
    this.endCandidateAudition(); // choosing another row tears the scratch session down
    this.compareCoupleID = null; // a reopened "Compare others" list closes with the row
    this.selectedID = id;
    const c = this.selected;
    if (c && c.hasSession) this.attachAudition(c);
    else this.audition.teardown();
    this.bump();
  }

  /** (Re)attach the shared audition to a couple's live session. */
  attachAudition(couple: CoupleRow): void {
    this.endCandidateAudition();
    if (!couple.hasSession) return;
    this.audition.attach(couple);
  }

  /** Seek the selected couple's preview (PairStrip click/drag — scrub, never snap):
   * the shared audition when it streams for this couple (incl. a candidate
   * audition), and the pair's own playhead either way. */
  seekPreview(couple: CoupleRow, t: number): void {
    const dur = couple.video.durationS > 0 ? couple.video.durationS : (couple.pair.cameraTrack?.meta.durationS ?? 0);
    const clamped = Math.max(0, dur > 0 ? Math.min(dur, t) : t);
    couple.pair.playheadT = clamped;
    if (this.audition.couple === couple) this.audition.seek(clamped);
    couple.pair.invalidate();
  }

  /** The resolver z floor (core default) — the forced-verdict policy input. */
  get zMin(): number {
    const json = this.resolveParamsJSON();
    if (!json) return 8.5;
    return (JSON.parse(json) as { zMin?: number }).zMin ?? 8.5;
  }

  /** Manual drag-to-pair (§3.6): what a dropped pool audio does to a couple is
   * the PURE core repair-drop policy's decision; this only applies it. The replace
   * confirmation is asked only for a PINNED couple (a manual pick, D9) — an
   * included-but-unpinned couple replaces without asking. */
  repairDrop(couple: CoupleRow, droppedKey: string): void {
    const outcome = Policies.repairDrop({
      hasSelectedCouple: true,
      payloadIsAudio: this.pool.entry(droppedKey)?.role === 'audio',
      videoAvailable: couple.video.handle !== null,
      currentSelectedKey: couple.selectedAudioKey,
      droppedKey,
      pinned: couple.pinned,
    });
    if (outcome === 'reject' || outcome === 'noOp') return;
    if (outcome === 'needsConfirmation') {
      const name = this.pool.entry(droppedKey)?.name ?? droppedKey;
      // Same copy as the mac shell's pinned-replacement dialog (RootView.swift).
      const msg =
        `Replace the audio you chose?\n\nYou chose the audio for ${couple.video.name} yourself. ` +
        `Pairing it with ${name} replaces that choice, clears its export status, and rebuilds the ` +
        `current pair session.`;
      if (!window.confirm(msg)) return;
    }
    // Capture the pre-repair state BEFORE mutating so ⌘Z restores the whole row
    // (selection, pins, include inputs, candidates) — mirrors performRepairReplace.
    const snapshot = this.captureRepair(couple);
    couple.exportStatus = { kind: 'idle' }; // drop any stale per-couple export result
    this.repair(couple, droppedKey, snapshot);
  }

  /** Manual re-pair (§V2.6): point a couple at a chosen audio (candidate "use" or
   * pool drag). Pins it and re-syncs; below-floor keeps the pairing but marks it
   * `forced`. Clears the include-for-export inputs (`explicit`, `exportedAt`;
   * PLAN-COMPACT-MODE §3): the old export is stale and the default rule applies to
   * the new pairing once its verdict lands. Undoable (⌘Z / the preview's Undo). */
  repair(couple: CoupleRow, audioKey: string, undoSnapshot?: RepairSnapshot): void {
    const audioEntry = this.pool.entry(audioKey);
    if (!audioEntry || !audioEntry.handle || !couple.video.handle) return;
    if (couple.selectedAudioKey === audioKey && couple.pinned) return;
    const before = undoSnapshot ?? this.captureRepair(couple);
    // The first manual pick of an unpinned row remembers the pre-pick state so the
    // preview's "Audio chosen by you · Undo" can return the couple to To fix; a
    // later re-pick keeps that origin.
    if (!before.pinned) this.prePickSnapshots.set(couple.id, before);
    this.endCandidateAudition(); // "use" ends any audition of this couple's candidates
    this.compareCoupleID = null;
    couple.explicit = null; // D1/D8: a new pairing starts from the default rule
    couple.exportedAt = null; // the previous export (if any) is of the OLD pairing
    const z = couple.candidates.find((c) => c.audioKey === audioKey)?.z ?? 0;
    if (!couple.candidates.some((c) => c.audioKey === audioKey)) {
      // A brand-new audio (pool drag) is appended — candidates are never destroyed.
      couple.candidates = [
        ...couple.candidates,
        { audioKey, audioName: audioEntry.name, z: 0, lagMs: 0, coverage: { t0: 0, t1: 0, seconds: 0, fraction: 0 } },
      ];
    }
    couple.applyManualPick(audioKey, z, this.zMin);
    this.ingest.ensureSession(couple);
    void this.reevaluateManualMatch(couple); // TODO-009: green only if this pair clears the classifier
    this.registerRepairUndo(couple, before);
    this.saveQueue();
    this.invalidate();
  }

  // ---------- manual-pick Undo (PLAN-COMPACT-MODE Phase 3 step 3) ----------

  /** True when the preview's "Undo" link has a pre-pick state to return to (a pick
   * made in this run; a pin restored from a saved session has no in-memory origin). */
  canUndoManualPick(couple: CoupleRow): boolean {
    return couple.pinned && this.prePickSnapshots.has(couple.id);
  }

  /** The preview's Undo link: the SAME inverse ⌘Z applies to a repair, restoring the
   * row to its pre-pick state — unpinned, the couple back in To fix. */
  undoManualPick(couple: CoupleRow): void {
    const snap = this.prePickSnapshots.get(couple.id);
    if (!snap) return;
    this.prePickSnapshots.delete(couple.id);
    this.applyRepairSnapshot(snap, couple);
  }

  // ---------- candidate audition (PLAN-COMPACT-MODE Phase 3 step 2; UX §V2.11) ----------

  /** ▶ on a candidate: open a scratch session (row camera + that audio) at the
   * candidate's lag, stream the shared audition from it (the video and playhead
   * stay), monitor Overlay, and play from the playhead. ▶ on another candidate
   * swaps scratch sessions; ■ is `stopCandidateAudition`. */
  auditionCandidate(couple: CoupleRow, cand: RowCandidate): void {
    if (couple.id !== this.selectedID) return;
    const cam = couple.video.handle;
    const audio = this.pool.entry(cand.audioKey)?.handle ?? null;
    if (!cam || !audio) return;
    const previous = this.candidateAudition;
    const t = this.audition.playheadFor(couple);
    let scratch: SessionHandle;
    try {
      scratch = this.engine.sessionNew(cam, audio);
    } catch (e) {
      this.toasts.error("Can't play this candidate", e instanceof Error ? e.message : String(e));
      return;
    }
    scratch.setOffsetMs(cand.lagMs);
    const previousMode = previous?.previousMode ?? this.audition.mode;
    // A temporary override, not a user choice: set directly so the A/B memory in
    // policy::monitor is left exactly as the user had it.
    this.audition.setMode('overlay');
    // Swapping closes the open stream (the previous scratch's, if any) first.
    this.audition.attachSession(couple, scratch, t);
    previous?.session.free();
    this.candidateAudition = {
      coupleID: couple.id,
      candidate: cand,
      track: audio,
      session: scratch,
      previousMode,
      previousPlayhead: previous?.previousPlayhead ?? t,
    };
    this.audition.play();
    this.invalidate();
  }

  /** ■ / closing the banner: end the audition and put the couple's own session,
   * monitor mode and playhead back. */
  stopCandidateAudition(): void {
    const a = this.candidateAudition;
    if (!a) return;
    this.candidateAudition = null;
    const couple = this.couples.find((c) => c.id === a.coupleID) ?? null;
    this.audition.teardown(); // closes the scratch stream before its session frees
    this.audition.setMode(a.previousMode);
    if (couple && couple.id === this.selectedID) {
      couple.pair.playheadT = a.previousPlayhead;
      if (couple.hasSession) {
        this.audition.attach(couple, a.previousPlayhead);
        this.audition.seek(a.previousPlayhead);
      }
      couple.pair.invalidate();
    }
    a.session.free();
    this.invalidate();
  }

  /** End without restoring the session (the caller re-attaches or tears down the
   * audition itself): close the scratch stream, free the scratch session. */
  endCandidateAudition(): void {
    const a = this.candidateAudition;
    if (!a) return;
    this.candidateAudition = null;
    this.audition.teardown();
    this.audition.setMode(a.previousMode);
    a.session.free();
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

  // ---------- include for export / rename (undoable, TODO-015) ----------

  /** The row checkbox (D1/D4): flip this couple's inclusion in the next export by
   * recording an EXPLICIT choice — `explicit = !included` — so the choice survives
   * a re-sync or re-resolve while the verdict stays good (the rule in core derives
   * `included` from it). Only an includable (verdict good) couple can be toggled;
   * the checkbox is not shown otherwise. */
  toggleInclude(couple: CoupleRow): void {
    if (!couple.canInclude) return;
    this.setExplicitInclude(couple, !couple.included);
  }

  /** The single undoable write path for `CoupleRow.explicit` (null = back to the
   * policy default). Symmetric via the old value, so redo is the re-registration on
   * undo; ⌘Z after ticking an exported couple back on returns it to "exported,
   * excluded". */
  setExplicitInclude(couple: CoupleRow, newValue: boolean | null): void {
    const old = couple.explicit;
    if (old === newValue) return;
    couple.explicit = newValue;
    couple.invalidate();
    this.undo.register(couple.included ? 'Include in Export' : 'Exclude from Export', () =>
      this.setExplicitInclude(couple, old)
    );
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

  captureRepair(c: CoupleRow): RepairSnapshot {
    return {
      selectedAudioKey: c.selectedAudioKey,
      pinned: c.pinned,
      forced: c.forced,
      chip: c.chip,
      provenance: c.selectionProvenance,
      manualMatchConfident: c.manualMatchConfident,
      explicit: c.explicit,
      exportedAt: c.exportedAt,
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
    this.endCandidateAudition();
    // Back to an unpinned row ⇒ the "Audio chosen by you · Undo" line has nothing
    // left to undo.
    if (!snapshot.pinned) this.prePickSnapshots.delete(couple.id);
    couple.selectedAudioKey = snapshot.selectedAudioKey;
    couple.pinned = snapshot.pinned;
    couple.forced = snapshot.forced;
    couple.chip = snapshot.chip;
    couple.selectionProvenance = snapshot.provenance;
    couple.manualMatchConfident = snapshot.manualMatchConfident;
    couple.explicit = snapshot.explicit;
    couple.exportedAt = snapshot.exportedAt;
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
    } else {
      couple.pair.teardown();
    }
    if (couple.id === this.selectedID) this.attachAudition(couple);
    couple.invalidate();
    this.saveQueue();
    this.invalidate();
  }

  // ---------- arrangement (UX §V2.11) ----------

  /** Most common parent-folder name among the imported files ("bach session"), or
   * null for an empty pool. Presentation only (the session line's identity). */
  get sessionName(): string | null {
    const counts = new Map<string, number>();
    for (const e of this.pool.entries) {
      const dir = pathMod.basename(pathMod.dirname(e.path));
      counts.set(dir, (counts.get(dir) ?? 0) + 1);
    }
    let best: string | null = null;
    let bestN = 0;
    for (const [k, n] of counts) {
      if (n > bestN || (n === bestN && best !== null && k < best)) {
        best = k;
        bestN = n;
      }
    }
    return best;
  }

  /** Switch arrangements: through the root view's switcher (window move + FLIP)
   * when installed, else model-only plus the window move. */
  async requestLayoutMode(mode: LayoutMode): Promise<void> {
    if (mode === this.layoutMode) return;
    if (this.layoutSwitcher) {
      await this.layoutSwitcher(mode);
      return;
    }
    this.setLayoutMode(mode);
    WindowLayoutBridge.apply(mode, false);
  }

  toggleLayoutMode(): Promise<void> {
    return this.requestLayoutMode(this.layoutMode === 'compact' ? 'full' : 'compact');
  }

  /** Return in compact: switch to full with the selected couple's editor open. */
  openFullEditor(): Promise<void> {
    if (!this.selected) return Promise.resolve();
    this.syncToolsExpanded = true;
    return this.requestLayoutMode('full');
  }

  /** The model side of a switch (the window move is the caller's): map the
   * monitor mode (D10) and flip the mode. Selection, playhead, playback and
   * inclusion are untouched — the audition is owned by its controller. */
  setLayoutMode(mode: LayoutMode): void {
    if (mode === this.layoutMode) return;
    if (mode === 'compact') {
      // D10: Pan L/R has no compact segment — heard (and shown) as Synced while
      // compact; the policy still decides via selectMode.
      if (this.audition.mode === 'pan') {
        this.audition.selectMode('overlay');
        this.panHeldForCompact = true;
      } else {
        this.panHeldForCompact = false;
      }
    } else if (this.panHeldForCompact) {
      // Back to the user's Pan, unless they picked something else meanwhile.
      if (this.audition.mode === 'overlay') this.audition.selectMode('pan');
      this.panHeldForCompact = false;
    }
    this.layoutMode = mode;
    this.invalidate();
  }

  /** "No audio" → Choose file… (compact): pair `path` with `couple` as a manual
   * pick. A file already in the pool repairs at once (the same path a drop uses);
   * a new file is imported first and paired once the pool has it. */
  pairWithChosenFile(couple: CoupleRow, path: string): void {
    const target = pathMod.resolve(path);
    const entryKey = (): string | null =>
      this.pool.entries.find((e) => pathMod.resolve(e.path) === target && e.handle !== null)?.id ?? null;
    const key = entryKey();
    if (key) {
      this.repairDrop(couple, key);
      return;
    }
    this.ingest.importDropped([path]);
    const id = couple.id;
    void (async () => {
      for (let i = 0; i < 1200; i++) {
        await new Promise((r) => setTimeout(r, 100));
        const k = entryKey();
        if (!this.pool.importing && !this.ingest.ingestInFlight && k) {
          const c = this.couples.find((x) => x.id === id);
          if (c) this.repairDrop(c, k);
          return;
        }
      }
    })();
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
   * selection (pins), include-for-export inputs (`explicit`/`exportedAt`, D8 —
   * absent in a pre-D1 file ⇒ null ⇒ every good couple included), per-couple
   * alignment (from the unchanged per-pair autosave), and export settings (§5.4). suppressAutoSync keeps the
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
      c.explicit = sc.explicit ?? null; // absent in a pre-D1 file ⇒ policy default
      c.exportedAt = sc.exportedAt ?? null;
      if (sc.selectedAudioKey !== c.selectedAudioKey) {
        c.selectedAudioKey = sc.selectedAudioKey;
        const e = sc.selectedAudioKey ? this.pool.entry(sc.selectedAudioKey) : null;
        if (sc.selectedAudioKey && e && !c.candidates.some((x) => x.audioKey === sc.selectedAudioKey)) {
          c.candidates.push({
            audioKey: sc.selectedAudioKey,
            audioName: e.name,
            z: 0,
            lagMs: 0,
            coverage: { t0: 0, t1: 0, seconds: 0, fraction: 0 },
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

  /** The media/couple/included counts for the Clear Session dialog copy (pure core). */
  clearSessionCounts(): { media: number; couples: number; included: number } {
    return Policies.clearSessionCounts({
      mediaCount: this.pool.entries.length,
      couples: this.couples.map((c) => ({
        videoKey: c.id,
        selectedAudioKey: c.selectedAudioKey,
        included: c.included,
      })),
    });
  }

  /** Confirm + clear (TODO-001). Blocked while a batch export runs (it owns the
   * couples' files). The dialog states the consequence-focused counts. */
  requestClearSession(): void {
    if (!this.hasContent || this.batch.isRunning) return;
    const c = this.clearSessionCounts();
    const msg =
      `Clear this session?\n\nThis discards ${c.media} media file${c.media === 1 ? '' : 's'}, ` +
      `${c.couples} couple${c.couples === 1 ? '' : 's'} (${c.included} included in the export) and returns ` +
      `to the import screen. Your source files and any files you have already exported are not deleted.`;
    if (window.confirm(msg)) this.clearSession();
  }

  /** Clear the whole session and return to the import screen. Stops work, releases
   * every pooled track, clears queue/export/undo state, REMOVES the persisted
   * restore record, and resets to empty. Source/exported files are untouched; the
   * cleared state never reappears on relaunch (the record is gone and the save
   * guard refuses to re-persist an empty session). */
  clearSession(): void {
    this.poolController.teardown(); // stop the pool preview player before freeing handles
    this.endCandidateAudition();
    this.audition.teardown();
    for (const couple of this.couples) couple.pair.teardown();
    this.couples = [];
    this.selectedID = null;
    this.compareCoupleID = null;
    this.prePickSnapshots.clear();
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
    this.endCandidateAudition();
    this.audition.teardown(); // stream closes before its session frees
    for (const c of this.couples) c.pair.teardown();
    this.couples = [];
    this.pool.teardown();
    this.engine.free();
  }
}

/** Everything a manual re-pair mutates on a row, captured BEFORE the mutation so
 * ⌘Z can put the row back (including the include-for-export inputs — `repair`
 * clears `explicit` and `exportedAt` because the old export is stale). */
export interface RepairSnapshot {
  selectedAudioKey: string | null;
  pinned: boolean;
  forced: boolean;
  chip: string;
  provenance: Policies.SelectionProvenance;
  manualMatchConfident: boolean | null;
  explicit: boolean | null;
  exportedAt: string | null;
  exportStatus: CoupleRow['exportStatus'];
  candidates: CoupleRow['candidates'];
}

/** The renderer's single AppModel (the window owns exactly one, like the mac
 * shell's @State model). */
export const appModel = new AppModel();
