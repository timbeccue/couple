// PairSession.ts — one couple's core session + sync lifecycle; TS port of the
// session-lifecycle half of mac/Sources/Couple/PairSession.swift (the editor
// interaction surface lands with the waveform milestones). The session is
// created over (camera, external) tracks; auto-sync placement seeding is the
// core `sync_seed` policy's decision (§11.3) — never decided here.
import type { Engine } from '../kit/Engine';
import type { DriftMode, SessionHandle } from '../kit/Session';
import type { TrackHandle } from '../kit/Track';
import type { AcceptedWindow, RowCandidate, SyncResult } from '../kit/Wire';
import * as Policies from '../kit/Policies';
import { Mapping } from './Mapping';
import { Observable } from './observable';

export type SyncStatus = 'idle' | 'syncing' | 'synced' | 'failed';

export type LoupeLayout = 'grid' | 'stack';

export interface AnchorMeasurement {
  lagMs: number;
  ncc: number;
}

/** One anchor + everything measured at it (UX §2.2); the shell render shadow of
 * core's parallel-array wire shape. `measurementCenter` is L(tᵢ) at the time of
 * the measurement (drives the >30 ms staleness rule). Held as one struct so the
 * insert/remove/reorder sites can't drift the facts out of lockstep. */
export interface AnchorPoint {
  t: number;
  winMs: number;
  auto: boolean;
  measurement: AnchorMeasurement | null;
  measurementCenter: number | null;
}

/** Pure viewport math for the main waveform stage (mirrors WaveformStage.swift's
 * `OverviewViewport`): clamp / zoom / pan a visible time range within the track
 * union, enforcing a minimum span so the view can never collapse. Presentation
 * only — no alignment decision lives here (PLAN-CORE-SHELL-BOUNDARY §1). A range
 * covering the whole domain is represented as `null` (the fitted state). */
const MIN_SPAN_S = 0.5;

type Range = [number, number];

function clampView(range: Range, domain: Range): Range | null {
  const dSpan = domain[1] - domain[0];
  if (dSpan <= 0) return null;
  const span = Math.min(Math.max(range[1] - range[0], MIN_SPAN_S), dSpan);
  if (span >= dSpan) return null; // covers the whole domain ⇒ fitted
  let lo = Math.max(range[0], domain[0]);
  if (lo + span > domain[1]) lo = domain[1] - span;
  return [lo, lo + span];
}

function zoomView(visible: Range, factor: number, aroundT: number, domain: Range): Range | null {
  const span = visible[1] - visible[0];
  if (span <= 0 || factor <= 0) return clampView(visible, domain);
  const dSpan = domain[1] - domain[0];
  const newSpan = Math.min(Math.max(span * factor, MIN_SPAN_S), dSpan);
  const frac = Math.max(0, Math.min(1, (aroundT - visible[0]) / span));
  const lo = aroundT - frac * newSpan;
  return clampView([lo, lo + newSpan], domain);
}

function panView(visible: Range, dt: number, domain: Range): Range | null {
  return clampView([visible[0] + dt, visible[1] + dt], domain);
}

export class PairSession extends Observable {
  private readonly engine: Engine;

  session: SessionHandle | null = null;
  cameraTrack: TrackHandle | null = null;
  externalTrack: TrackHandle | null = null;
  /** Resolver evidence for the selected pair (seed input; may be null). */
  evidence: RowCandidate | null = null;

  syncStatus: SyncStatus = 'idle';
  lastSyncResult: SyncResult | null = null;
  lastSyncFailed = false;
  private syncJobPromise: Promise<void> | null = null;

  constructor(engine: Engine) {
    super();
    this.engine = engine;
  }

  get verified(): boolean {
    if (!this.session) return false;
    try {
      return this.session.state().verified;
    } catch {
      return false;
    }
  }

  setVerified(v: boolean): void {
    this.session?.setVerified(v);
    this.bump();
  }

  /** (Re)build the core session for the current selection. */
  load(cameraTrack: TrackHandle, externalTrack: TrackHandle, evidence: RowCandidate | null): void {
    this.teardown();
    this.cameraTrack = cameraTrack;
    this.externalTrack = externalTrack;
    this.evidence = evidence;
    this.session = this.engine.sessionNew(cameraTrack, externalTrack);
    this.syncStatus = 'idle';
    this.lastSyncFailed = false;
    this.playheadT = 0;
    this.viewport = null; // fresh fitted view per load
    this.anchors = [];
    this.focusedAnchor = null;
    this.bump();
  }

  teardown(): void {
    this.session?.free();
    this.session = null;
    this.cameraTrack = null;
    this.externalTrack = null;
    this.evidence = null;
    this.syncStatus = 'idle';
    this.lastSyncResult = null;
    this.anchors = [];
    this.focusedAnchor = null;
    this.bump();
  }

  /** Ambient auto-sync (§V2.4): dots carry the result, no announcement.
   * Seed choice is the PURE core policy (`cpl_policy_sync_seed`): resolver
   * evidence ⇒ seeded twin, none ⇒ the unchanged unseeded sync. */
  runAutoSync(): void {
    const session = this.session;
    if (!session) return;

    let currentOffsetMs = 0;
    try {
      currentOffsetMs = session.state().offsetMs;
    } catch {
      /* fresh session */
    }
    const seed = Policies.syncSeed({
      evidenceLagMs: this.evidence?.lagMs ?? null,
      evidenceZ: this.evidence?.z ?? null,
      evidenceDriftPpm: null,
      explicit: false,
      lastSyncFailed: this.lastSyncFailed,
      currentOffsetMs,
    });

    const job = seed
      ? session.autosyncStartSeeded(null, seed.offsetMs, seed.driftPpm)
      : session.autosyncStart(null);

    this.syncStatus = 'syncing';
    this.bump();
    this.syncJobPromise = job.value
      .then((result) => {
        this.lastSyncResult = result;
        this.lastSyncFailed = false;
        this.syncStatus = 'synced';
        // Core wrote the mapping; propose/measure/frame anchors (UX §2.2).
        this.onSyncSettled();
        this.bump();
      })
      .catch(() => {
        this.lastSyncFailed = true;
        this.syncStatus = 'failed';
        this.bump();
      })
      .finally(() => {
        this.syncJobPromise = null;
      });
  }

  async awaitSync(): Promise<void> {
    await this.syncJobPromise;
  }

  // ---- Editor view state (M3): read-only waveform display + navigation ----
  // The canonical alignment lives in core (SPEC §4.5); these are shell-owned
  // presentation state for the waveform stage. Alignment EDITING is M5.

  /** Playhead position in video (camera) time, seconds. Audio wiring is M4. */
  playheadT = 0;
  /** Main-stage visible window; `null` ⇒ fitted to the full union (`stageDomain`). */
  private viewport: Range | null = null;

  // ---- Anchor + loupe state (M8 parity): shell render shadow; core owns
  // alignment (SPEC §4.5). Anchors stay in chronological order by `t` (UX §2.2). ----
  anchors: AnchorPoint[] = [];
  lockZoom = true;
  loupeLayout: LoupeLayout = 'grid';
  /** The loupe/anchor the audition playhead is focused on, or null. */
  focusedAnchor: number | null = null;
  static readonly defaultWinMs = 500;
  /** RMS floor (dBFS) below which a lane window is silence for anchor usability. */
  static readonly anchorSilenceFloorDb = -55;
  private remeasureTimer: ReturnType<typeof setTimeout> | null = null;

  /** Current alignment mapping (offset/drift) read from core state. */
  mapping(): Mapping {
    let offsetMs = 0;
    let driftPpm = 0;
    try {
      const s = this.session?.state();
      if (s) {
        offsetMs = s.offsetMs;
        driftPpm = s.driftPpm;
      }
    } catch {
      /* fresh session — identity mapping */
    }
    return new Mapping(offsetMs, driftPpm);
  }

  /** Union of both tracks' extents in video time (UX §1.6) — the external lane
   * slides through the mapping, so this tracks the live offset/drift. */
  stageDomain(): Range {
    const cam = this.cameraTrack;
    const ext = this.externalTrack;
    if (!cam || !ext) return [0, 1];
    const m = this.mapping();
    const e0 = m.tForU(0);
    const e1 = m.tForU(ext.meta.durationS);
    const lo = Math.min(0, e0);
    const hi = Math.max(cam.meta.durationS, e1);
    return hi > lo ? [lo, hi] : [0, 1];
  }

  /** Overlap on the video timeline through the current mapping, null when empty. */
  overlap(): Range | null {
    const cam = this.cameraTrack;
    const ext = this.externalTrack;
    if (!cam || !ext) return null;
    return this.mapping().overlap(cam.meta.durationS, ext.meta.durationS);
  }

  /** The main stage's displayed range: the stored viewport clamped to the live
   * union, or the full union when fitted. */
  view(): Range {
    const domain = this.stageDomain();
    if (this.viewport === null) return domain;
    return clampView(this.viewport, domain) ?? domain;
  }

  /** True when the main stage is zoomed/panned in from the full union. */
  get viewZoomed(): boolean {
    return this.viewport !== null;
  }

  private storeView(r: Range | null): void {
    this.viewport = r;
    this.bump();
  }

  /** Restore the full-union fit. */
  fitView(): void {
    this.storeView(null);
  }

  /** Scroll-zoom the main stage about a pointer time (`factor` < 1 zooms in). */
  zoomView(factor: number, aroundT: number): void {
    this.storeView(zoomView(this.view(), factor, aroundT, this.stageDomain()));
  }

  /** Scroll-pan the main stage by `dt` seconds. */
  panView(dt: number): void {
    this.storeView(panView(this.view(), dt, this.stageDomain()));
  }

  /** Injected by AppModel: a placed playhead also seeks the shared audition
   * when it is attached to this pair (audio clock is the master — seeking the
   * <video> directly would just get snapped back by the slaving loop). */
  static onPlayheadSeek: ((pair: PairSession, t: number) => void) | null = null;

  /** Place the playhead (click / scrub); clamped to the union. */
  setPlayhead(t: number): void {
    const [lo, hi] = this.stageDomain();
    this.playheadT = Math.max(lo, Math.min(hi, t));
    PairSession.onPlayheadSeek?.(this, this.playheadT);
    this.bump();
  }

  // ---- Alignment editing (M5): stage gestures + keyboard nudges ----
  // Gated by `inputsEnabled` exactly like the Swift stage; editing never touches
  // the verified/approved flag (the mac shell doesn't un-verify on an edit).
  // Core owns the canonical values (SPEC §4.5) — every write goes to the session
  // hot setter and the readouts/sparkline re-read state after a bump().

  /** Alignment inputs are live only with a session and no sync running (UX §3). */
  get inputsEnabled(): boolean {
    return this.session !== null && this.syncStatus !== 'syncing';
  }

  /** Live offset write (gesture-rate hot setter, `cpl_set_offset_ms`). */
  setOffsetLive(v: number): void {
    this.session?.setOffsetMs(v);
    this.mappingChanged();
    this.bump();
  }

  /** ← / → nudge: shift the external content by `deltaMs` (InputRouter §2.9). */
  nudgeOffset(deltaMs: number): void {
    if (!this.inputsEnabled) return;
    this.setOffsetLive(this.mapping().offsetMs + deltaMs);
  }

  /** Live drift write; a drift write while the mode is `none` flips it to
   * `resample` (core-enforced UX §1.5) — we just surface the result via bump. */
  setDriftLive(v: number): void {
    this.session?.setDriftPpmLive(v);
    this.mappingChanged();
    this.bump();
  }

  commitDrift(): void {
    this.session?.commitDrift();
    this.bump();
  }

  /** Drift-mode toggle (resample/none). Core enforces the UX §1.5 semantics
   * (None zeroes live drift; Resample restores the saved fit). */
  setDriftMode(mode: DriftMode): void {
    this.session?.setDriftMode(mode);
    this.mappingChanged();
    this.bump();
  }

  /** [ / ] nudge: adjust drift by `deltaPpm`, committing the resample plan. */
  nudgeDrift(deltaPpm: number): void {
    if (!this.inputsEnabled) return;
    this.setDriftLive(this.mapping().driftPpm + deltaPpm);
    this.commitDrift();
  }

  /** Pivot-drift drag (Shift-drag, UX §2.4 pivot math): rotate the mapping line
   * so L(atT) changes by `deltaLMs` while L(refT) stays fixed. `refT`/`atT` are
   * captured at gesture start so the lever arm stays stable through the drag. */
  pivotDriftDrag(atT: number, refT: number, deltaLMs: number): void {
    if (!this.inputsEnabled) return;
    const dt = atT - refT;
    if (Math.abs(dt) < 5) return; // guard |tᵢ − t_r| < 5 s (too short to rotate)
    const slope = deltaLMs / dt; // ms per second
    const m = this.mapping();
    this.session?.setDriftPpmLive(m.driftPpm + slope * 1000);
    this.session?.setOffsetMs(m.offsetMs - refT * slope);
    this.bump();
  }

  /** Restore a captured (offset, drift) — used to cancel a gesture on Esc. */
  restoreAlignment(offsetMs: number, driftPpm: number): void {
    this.session?.setOffsetMs(offsetMs);
    this.session?.setDriftPpmLive(driftPpm);
    this.session?.commitDrift();
    this.bump();
  }

  // ---- Anchors + loupes (M8 parity, port of PairSession.swift §2.2/§2.3/§2.4) ----
  // Anchor placement/count is the PURE core policy (`anchorPropose`/`anchorAdd`);
  // this layer only feeds it onset candidates and renders/measures the result.

  get bothLoaded(): boolean {
    return this.cameraTrack !== null && this.externalTrack !== null;
  }

  /** True when automatic proposal placed no anchor (no usable overlap) — the
   * manual-rescue state (UX §2.2 / TODO-011). */
  get anchorsUnavailable(): boolean {
    return this.bothLoaded && this.anchors.length === 0;
  }

  /** Displayed residual Δᵢ = λ(tᵢ) − L(tᵢ) (UX §2.1), against the CURRENT mapping. */
  displayedDelta(i: number): number | null {
    const a = this.anchors[i];
    if (!a || !a.measurement) return null;
    return a.measurement.lagMs - this.mapping().localLagMs(a.t);
  }

  /** Accepted per-window measurements from core state — the fit points the
   * sparkline plots (mirrors the mac shell's `pair.driftWindows`, not the last
   * sync's raw telemetry). */
  driftWindows(): AcceptedWindow[] {
    try {
      return this.session?.state().driftWindows ?? [];
    } catch {
      return [];
    }
  }

  /** Global fit status vs the current mapping (UX §2.4: one global RMS). */
  fitStatus(): { rmsMs: number; windows: number } | null {
    const acc = this.driftWindows();
    if (acc.length === 0) return null;
    const m = this.mapping();
    const sq = acc.reduce((s, w) => {
      const r = w.lagMs - m.localLagMs(w.t);
      return s + r * r;
    }, 0);
    return { rmsMs: Math.sqrt(sq / acc.length), windows: acc.length };
  }

  /** Build ranked onset candidates, marking each usable iff BOTH mapped lanes
   * have non-silent structure (TODO-011). Reuses the engine's onset list; no
   * shell-side scoring (PLAN-CORE-SHELL-BOUNDARY §1). */
  private anchorCandidates(): Policies.AnchorCandidate[] {
    const cam = this.cameraTrack;
    const ext = this.externalTrack;
    const ov = this.overlap();
    if (!cam || !ov) return [];
    const m = this.mapping();
    let onsets: { t: number; score: number }[] = [];
    try {
      onsets = cam.onsets(ov[0], ov[1], 400);
    } catch {
      return [];
    }
    return onsets.map((o) => ({
      t: o.t,
      score: o.score,
      usable: this.laneHasContent(cam, o.t) && (ext ? this.laneHasContent(ext, m.u(o.t)) : false),
    }));
  }

  private laneHasContent(track: TrackHandle, t: number): boolean {
    const dur = track.meta.durationS;
    if (t < 0 || t > dur) return false;
    const half = 0.25;
    try {
      return track.rms(Math.max(0, t - half), Math.min(dur, t + half)) > PairSession.anchorSilenceFloorDb;
    } catch {
      return false;
    }
  }

  /** Automatic anchor proposal (TODO-011): count decided by the pure policy on
   * usable-overlap duration. Anchors always lie inside the overlap. */
  proposeAnchors(): void {
    const ov = this.overlap();
    const picks = Policies.anchorPropose({
      overlap: ov ? { start: ov[0], end: ov[1] } : null,
      candidates: this.anchorCandidates(),
    });
    this.anchors = picks.map((t) => ({
      t,
      winMs: PairSession.defaultWinMs,
      auto: true,
      measurement: null,
      measurementCenter: null,
    }));
    this.bump();
  }

  /** Automatic "+ anchor": strongest usable onset in the overlap, cap 4 (the
   * pure policy picks it; nothing when none qualifies). */
  addAnchor(): void {
    if (this.anchors.length >= 4) return;
    const ov = this.overlap();
    const t = Policies.anchorAdd({
      overlap: ov ? { start: ov[0], end: ov[1] } : null,
      existing: this.anchors.map((a) => a.t),
      candidates: this.anchorCandidates(),
    });
    if (t === null) return;
    this.insertAnchor(t);
  }

  private insertAnchor(t: number): void {
    const idx = this.anchors.findIndex((a) => a.t > t);
    const at = idx < 0 ? this.anchors.length : idx;
    this.anchors.splice(at, 0, {
      t,
      winMs: this.currentLockedWindow ?? PairSession.defaultWinMs,
      auto: true,
      measurement: null,
      measurementCenter: null,
    });
    this.measureAnchor(at);
    this.autoFrame();
    this.bump();
  }

  removeAnchor(i: number): void {
    if (this.anchors.length <= 2 || !this.anchors[i]) return;
    this.anchors.splice(i, 1);
    if (this.focusedAnchor !== null && this.focusedAnchor >= this.anchors.length) {
      this.focusedAnchor = null;
    }
    this.bump();
  }

  /** Live drag: move without snapping (loupes update live). */
  moveAnchor(i: number, t: number): void {
    const cam = this.cameraTrack;
    if (!this.anchors[i] || !cam) return;
    this.anchors[i].t = Math.max(0, Math.min(t, cam.meta.durationS));
    this.bump();
  }

  /** Drag release: keep the placement (no snap — owner 2026-06-12); re-sort,
   * invalidate the moved measurement, re-measure and re-frame. */
  releaseAnchor(i: number): void {
    if (!this.anchors[i]) return;
    this.anchors[i].measurement = null;
    this.anchors[i].measurementCenter = null;
    this.anchors.sort((a, b) => a.t - b.t);
    this.anchors.forEach((a, idx) => {
      if (!a.measurement) this.measureAnchor(idx);
    });
    this.autoFrame();
    this.bump();
  }

  /** Explicit ⌖ snap: jump to the strongest transient within ±4 s (UX §2.2). */
  snapAnchor(i: number): void {
    const cam = this.cameraTrack;
    if (!cam || !this.anchors[i]) return;
    let o: { t: number } | null = null;
    try {
      o = cam.bestOnsetNear(this.anchors[i].t, 4);
    } catch {
      o = null;
    }
    if (o) this.anchors[i].t = o.t;
    this.releaseAnchor(i);
  }

  private measureAnchor(i: number): void {
    const a = this.anchors[i];
    if (!this.session || !a) return;
    const center = this.mapping().localLagMs(a.t);
    a.measurement = this.session.measureLocal(a.t, 9, center, 80);
    a.measurementCenter = center;
  }

  measureAllAnchors(): void {
    this.anchors.forEach((_, i) => this.measureAnchor(i));
  }

  /** Re-measure stale anchors on a 450 ms debounce once the mapping moved > 30 ms
   * from a measurement's center (UX §2.4). Called after every mapping mutation. */
  private mappingChanged(immediate = false): void {
    if (this.remeasureTimer) clearTimeout(this.remeasureTimer);
    if (!this.session || this.anchors.length === 0) return;
    const work = (): void => {
      let any = false;
      this.anchors.forEach((a, i) => {
        const lNow = this.mapping().localLagMs(a.t);
        if (a.measurementCenter === null || Math.abs(lNow - a.measurementCenter) > 30) {
          this.measureAnchor(i);
          any = true;
        }
      });
      if (any) this.bump();
    };
    if (immediate) work();
    else this.remeasureTimer = setTimeout(work, 450);
  }

  // MARK: loupe windows — auto-framing + zoom lock (UX §2.3)

  get currentLockedWindow(): number | null {
    if (!this.lockZoom || this.anchors.length === 0) return null;
    return Math.max(...this.anchors.map((a) => a.winMs));
  }

  /** Auto-frame: winᵢ = clamp(max(default, 5·|Δᵢ|) rounded to 10 ms, cap 2 s). */
  autoFrame(): void {
    if (this.anchors.length === 0) return;
    const desired = (i: number): number => {
      const d = Math.abs(this.displayedDelta(i) ?? 0);
      return Math.min(Math.round(Math.max(PairSession.defaultWinMs, 5 * d) / 10) * 10, 2000);
    };
    if (this.lockZoom) {
      const autos = this.anchors.map((a, i) => ({ a, i })).filter((x) => x.a.auto);
      if (autos.length === 0) return;
      const w = Math.max(...autos.map((x) => desired(x.i)));
      for (const a of this.anchors) a.winMs = w;
    } else {
      this.anchors.forEach((a, i) => {
        if (a.auto) a.winMs = desired(i);
      });
    }
    this.bump();
  }

  /** Manual scroll-zoom: exits auto mode (all loupes under lock; one unlocked). */
  manualZoom(i: number, factor: number): void {
    if (!this.anchors[i]) return;
    const clamp = (w: number): number => Math.min(Math.max(w, 1), 8000);
    if (this.lockZoom) {
      const w = clamp(this.anchors[i].winMs * factor);
      for (const a of this.anchors) {
        a.winMs = w;
        a.auto = false;
      }
    } else {
      this.anchors[i].winMs = clamp(this.anchors[i].winMs * factor);
      this.anchors[i].auto = false;
    }
    this.bump();
  }

  /** Clicking the auto chip re-arms (all loupes under lock). */
  armAuto(i: number): void {
    if (!this.anchors[i]) return;
    if (this.lockZoom) for (const a of this.anchors) a.auto = true;
    else this.anchors[i].auto = true;
    this.autoFrame();
  }

  toggleLockZoom(): void {
    this.lockZoom = !this.lockZoom;
    if (this.lockZoom && this.anchors.length > 0) {
      const w = Math.max(...this.anchors.map((a) => a.winMs));
      for (const a of this.anchors) a.winMs = w;
    }
    this.bump();
  }

  setLoupeLayout(layout: LoupeLayout): void {
    this.loupeLayout = layout;
    this.bump();
  }

  // MARK: pivot-about-anchor + ⇥ align (UX §2.4, DECIDED math)

  /** Reference anchor: anchor 1 (leftmost), or the rightmost when dragging 1. */
  pivotReference(i: number): number {
    return i === 0 ? this.anchors.length - 1 : 0;
  }

  /** δ = desired change of L(tᵢ) in ms; rotates the mapping about the reference. */
  pivotAnchorDrag(i: number, deltaLMs: number): void {
    if (!this.inputsEnabled || !this.anchors[i]) return;
    const r = this.pivotReference(i);
    const dt = this.anchors[i].t - this.anchors[r].t;
    if (Math.abs(dt) < 5) return;
    const slope = deltaLMs / dt;
    const m = this.mapping();
    this.session?.setDriftPpmLive(m.driftPpm + slope * 1000);
    this.session?.setOffsetMs(m.offsetMs - this.anchors[r].t * slope);
    this.bump();
  }

  /** ⇥ align: zero this anchor's measured Δ (pivot math; plain offset when it IS
   * the reference). Re-applies auto-framing (UX §2.3). */
  alignAnchor(i: number): void {
    if (!this.inputsEnabled) return;
    const delta = this.displayedDelta(i);
    if (delta === null) return;
    const r = this.pivotReference(i);
    if (i === r || this.anchors.length < 2 || Math.abs(this.anchors[i].t - this.anchors[r].t) < 5) {
      this.setOffsetLive(this.mapping().offsetMs + delta);
    } else {
      this.pivotAnchorDrag(i, delta);
      this.commitDrift();
    }
    this.mappingChanged(true);
    this.measureAllAnchors();
    this.autoFrame();
  }

  /** After a sync settles: propose anchors the first time, then measure/frame. */
  onSyncSettled(): void {
    const firstProposal = this.anchors.length === 0;
    if (firstProposal) this.proposeAnchors();
    this.measureAllAnchors();
    if (firstProposal) this.autoFrame();
    this.bump();
  }
}
