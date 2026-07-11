// PoolPreview.ts — media-pool audio preview (SPEC-V2 §V2.7, TODO-002); TS port of
// mac/Sources/Couple/PoolPreviewController.swift. One shared, observable controller
// is the single source of truth for the pool's sticky preview player: exactly one
// entry is active at a time.
//
// The transport is split in two so the state machine is testable without an audio
// device: `PoolPreviewState` is a PURE value type carrying the whole observable
// surface (active key, playing/paused, currentTime, duration, scrubbing) and the
// transitions are pure functions returning a `PoolPreviewAction` describing what the
// player must then do (play / pause / seek / load). `PoolPreviewController` is the
// thin shell that drives a single HTMLAudioElement from those actions and feeds back
// its `timeupdate` ticks — the Electron twin of the mac shell's AVPlayer. Video
// entries preview their EMBEDDED audio through the same <audio> element (owner
// decision 2 — the container's audio track decodes), so `url` is fed verbatim.
import { Observable } from './observable';
import { mediaUrl } from './mediaUrl';

/** A file path → URL the <audio> element can load. Plain file:// is blocked on
 * the dev server's http origin, so media goes through couple-media://. */
function fileURL(path: string): string {
  return mediaUrl(path);
}

/** What the audio element should do after a state transition. The state machine
 * never touches the DOM; it only describes the side effect. */
export type PoolPreviewAction =
  | { kind: 'none' }
  // Load a different file and position at `at` seconds (the previous item is
  // released first — the controller owns that swap). `play` is the auto-start flag:
  // a toggle-driven load plays immediately (true); SELECT loads paused (false).
  | { kind: 'load'; url: string; at: number; play: boolean }
  | { kind: 'play' }
  | { kind: 'pause' }
  | { kind: 'seek'; t: number };

const END_EPSILON = 0.05;

/** The complete observable preview surface as a pure value. All transitions are
 * deterministic functions of the current value + the event, so the state machine is
 * unit-testable with no audio output. */
export class PoolPreviewState {
  activeKey: string | null = null;
  isPlaying = false;
  /** Playback position in seconds; preserved across pause, defines the next start,
   * and is the seek-strip value. Never overwritten by ticks while scrubbing. */
  currentTime = 0;
  /** Item duration in seconds (0 until the media reports it). */
  duration = 0;
  /** True only between scrubBegin/scrubEnd; suppresses tick overwrites so the
   * periodic update can't fight the user's drag. */
  isScrubbing = false;

  /** True when `key` is the active entry (drives that row's active styling). */
  isActive(key: string): boolean {
    return this.activeKey === key;
  }

  private get atEnd(): boolean {
    return this.duration > 0 && this.currentTime >= this.duration - END_EPSILON;
  }

  /** Toggle the play/pause button for `key`.
   * - A different (or no) active entry ⇒ switch to `key`, start at 0.
   * - The active entry while playing ⇒ pause, preserving position.
   * - The active entry while paused ⇒ resume (from 0 if it had ended). */
  toggle(key: string, url: string): PoolPreviewAction {
    if (this.activeKey !== key) return this.start(key, url);
    if (this.isPlaying) {
      this.isPlaying = false;
      return { kind: 'pause' };
    }
    if (this.atEnd) this.currentTime = 0;
    this.isPlaying = true;
    return { kind: 'play' };
  }

  private start(key: string, url: string): PoolPreviewAction {
    this.activeKey = key;
    this.currentTime = 0;
    this.duration = 0;
    this.isScrubbing = false;
    this.isPlaying = true;
    return { kind: 'load', url, at: 0, play: true };
  }

  /** SELECT = load-paused (owner decision 1): make `key` the active entry, loaded
   * and positioned at 0 but NOT playing, so the sticky bar shows its duration and
   * the user presses play to start. Re-selecting the already active key is a no-op
   * (a play in progress is not interrupted). */
  loadPaused(key: string, url: string): PoolPreviewAction {
    if (this.activeKey === key) return { kind: 'none' };
    this.activeKey = key;
    this.currentTime = 0;
    this.duration = 0;
    this.isScrubbing = false;
    this.isPlaying = false;
    return { kind: 'load', url, at: 0, play: false };
  }

  /** The media reported its duration (or it changed). Clamp the position in case a
   * scrub ran ahead of a not-yet-known duration. */
  setDuration(d: number): void {
    if (!Number.isFinite(d) || d <= 0) return;
    this.duration = d;
    if (this.currentTime > d) this.currentTime = d;
  }

  /** Periodic time update. Ignored while scrubbing or when paused/idle, so it never
   * fights the user. */
  tick(t: number): void {
    if (!this.isPlaying || this.isScrubbing || this.activeKey === null || !Number.isFinite(t)) return;
    this.currentTime = Math.max(0, t);
  }

  /** User grabbed the strip: freeze tick updates until release. */
  scrubBegin(): void {
    if (this.activeKey !== null) this.isScrubbing = true;
  }

  /** User dragged the strip to `t`. Works while playing or paused; seeks the player
   * immediately so audio tracks the thumb. */
  scrubTo(t: number): PoolPreviewAction {
    if (this.activeKey === null) return { kind: 'none' };
    const clamped = this.clampToTrack(t);
    this.currentTime = clamped;
    return { kind: 'seek', t: clamped };
  }

  /** User released the strip. Position is already set; just re-enable ticks. */
  scrubEnd(): void {
    this.isScrubbing = false;
  }

  /** The item played to its natural end: stop and rewind so the next play starts
   * from the beginning. */
  reachedEnd(): PoolPreviewAction {
    if (this.activeKey === null) return { kind: 'none' };
    this.isPlaying = false;
    this.isScrubbing = false;
    this.currentTime = 0;
    return { kind: 'pause' };
  }

  /** Explicit stop/cleanup (teardown, or removing the active file — TODO-001).
   * Clears the active entry entirely. */
  stop(): PoolPreviewAction {
    const wasActive = this.activeKey !== null;
    this.activeKey = null;
    this.isPlaying = false;
    this.isScrubbing = false;
    this.currentTime = 0;
    this.duration = 0;
    return wasActive ? { kind: 'pause' } : { kind: 'none' };
  }

  private clampToTrack(t: number): number {
    if (!Number.isFinite(t)) return 0;
    if (this.duration > 0) return Math.min(Math.max(0, t), this.duration);
    return Math.max(0, t);
  }
}

/** The thin shell: owns one HTMLAudioElement, applies `PoolPreviewAction`s, and
 * feeds `timeupdate`/`durationchange`/`ended` back into the pure state. Views read
 * `state` (via useModel) and call the transition wrappers; nothing else mutates it. */
export class PoolPreviewController extends Observable {
  readonly state = new PoolPreviewState();
  private audio: HTMLAudioElement | null = null;
  /** The AUTHORITATIVE track duration from the probe (`entry.durationS`), the mac
   * shell's model (AVPlayer + known duration). The <audio> element's own `duration`
   * is UNRELIABLE with chunked 206 serving — Chromium extrapolates it from the first
   * buffered chunk (e.g. 58.25 s for a 120 s / 17 MB WAV served in 8 MB chunks) — so
   * it must never drive the strip domain, scrub clamp, or end detection. */
  private authoritativeDuration = 0;
  /** A seek requested before the element could accept it (readyState < HAVE_METADATA);
   * Chromium silently drops such assignments, so we replay it once metadata loads. */
  private pendingSeek: number | null = null;

  // ---------- view-facing transport ----------

  toggle(key: string, path: string, durationS = 0): void {
    if (durationS > 0) this.authoritativeDuration = durationS;
    this.apply(this.state.toggle(key, fileURL(path)));
    this.bump();
  }

  /** SELECT = load-paused (owner decision 1): load the item into the sticky bar
   * without auto-playing. Drives the primary follow in PoolController.selectPool. */
  select(key: string, path: string, durationS = 0): void {
    if (durationS > 0) this.authoritativeDuration = durationS;
    this.apply(this.state.loadPaused(key, fileURL(path)));
    this.bump();
  }

  scrubBegin(): void {
    this.state.scrubBegin();
    this.bump();
  }

  scrubTo(t: number): void {
    this.apply(this.state.scrubTo(t));
    this.bump();
  }

  scrubEnd(): void {
    this.state.scrubEnd();
    this.bump();
  }

  /** Explicit cleanup/stop path (app/pool teardown; the seam per-file removal
   * (TODO-001) calls before releasing a pool entry's handle). Idempotent. */
  stop(): void {
    this.apply(this.state.stop());
    this.release();
    this.bump();
  }

  // ---------- action application (the only place the audio element is touched) ----

  private apply(action: PoolPreviewAction): void {
    switch (action.kind) {
      case 'none':
        break;
      case 'load':
        this.loadAudio(action.url, action.at, action.play);
        break;
      case 'play':
        void this.audio?.play();
        break;
      case 'pause':
        this.audio?.pause();
        break;
      case 'seek':
        this.seekElement(action.t);
        break;
    }
  }

  /** Set the element's position, deferring until metadata loads if it can't yet
   * accept a seek (Chromium drops `currentTime` writes while readyState < 1). */
  private seekElement(t: number): void {
    const a = this.audio;
    if (!a) return;
    if (a.readyState < 1 /* HAVE_METADATA */) {
      this.pendingSeek = t;
      return;
    }
    a.currentTime = t;
  }

  private loadAudio(url: string, start: number, play: boolean): void {
    this.release();
    const a = new Audio(url);
    a.preload = 'auto';
    // The PROBE duration is authoritative from the moment we load (the mac shell's
    // model). Seed the pure state with it so the strip domain, scrub clamp, and end
    // detection are correct immediately — never waiting on, or trusting, the
    // element's under-reported `duration` (see `authoritativeDuration`).
    if (this.authoritativeDuration > 0) this.state.setDuration(this.authoritativeDuration);
    // Only adopt the element's duration when the probe gave us nothing (a defensive
    // fallback); a good probe value always wins.
    const adoptElementDuration = (): void => {
      if (this.authoritativeDuration <= 0) this.state.setDuration(a.duration);
      this.bump();
    };
    a.addEventListener('durationchange', adoptElementDuration);
    a.addEventListener('loadedmetadata', () => {
      if (this.pendingSeek !== null) {
        a.currentTime = this.pendingSeek;
        this.pendingSeek = null;
      } else if (start > 0) {
        a.currentTime = start;
      }
      adoptElementDuration();
    });
    a.addEventListener('timeupdate', () => {
      this.state.tick(a.currentTime);
      this.bump();
    });
    a.addEventListener('ended', () => {
      // A chunked-served element fires `ended` at ITS under-reported duration, well
      // before the true end. Only honor an end that lands near the authoritative
      // duration; otherwise it is a serving artifact and the position is preserved.
      if (this.authoritativeDuration > 0 && a.currentTime < this.authoritativeDuration - 0.5) return;
      this.apply(this.state.reachedEnd());
      this.bump();
    });
    this.audio = a;
    if (start > 0) this.seekElement(start);
    // SELECT loads the item ready but paused (owner decision 1); only the
    // toggle/play path auto-starts.
    if (play) void a.play();
  }

  private release(): void {
    const a = this.audio;
    if (!a) return;
    a.pause();
    a.removeAttribute('src');
    a.load();
    this.audio = null;
  }

  /** Test-only snapshot of the live element vs the pure state (COUPLE_POOL_SEEK_TEST). */
  debugSnapshot(): { elDuration: number; elCurrent: number; elReady: number; stDuration: number; stCurrent: number } {
    return {
      elDuration: this.audio?.duration ?? NaN,
      elCurrent: this.audio?.currentTime ?? NaN,
      elReady: this.audio?.readyState ?? -1,
      stDuration: this.state.duration,
      stCurrent: this.state.currentTime,
    };
  }
}
