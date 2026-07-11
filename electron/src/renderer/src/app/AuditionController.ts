// AuditionController.ts — the app's ONE shared audition (§5.3: one audition
// handle at a time); TS port of the session/stream half of
// mac/Sources/Couple/AuditionController.swift. The muted <video> element is
// slaved to this clock by VideoHero (playbackRate micro-slew), the Electron
// stand-in for the mac shell's AVPlayer-slaved-to-audio-clock.
import { AuditionStream, type MonitorMode } from '../kit/Audition';
import { Observable } from './observable';
import type { CoupleRow } from './CoupleRow';

export class AuditionController extends Observable {
  private stream: AuditionStream | null = null;
  couple: CoupleRow | null = null;
  mode: MonitorMode = 'overlay'; // default matches Swift AuditionController.swift monitorMode = .overlay
  playing = false;
  // Per-lane monitor gain (0–2) + mute (WaveformStage.swift LaneAudioControls).
  // Mute is a separate flag so the pre-mute slider level is remembered and
  // restored on unmute; the effective gain sent to core is 0 while muted.
  camGain = 1;
  extGain = 1;
  camMuted = false;
  extMuted = false;
  /** Last seek/pause position (video time) — the paused-frame anchor the hero
   * shows while the clock is stopped (clock() is NaN when not pumping). */
  lastSeekT: number | null = null;

  /** Attach the audition to a couple's session (tears down any previous). */
  attach(couple: CoupleRow, tStart = 0): void {
    this.teardown();
    if (!couple.pair.session) return;
    try {
      this.stream = new AuditionStream(couple.pair.session.engine, couple.pair.session, this.mode, tStart);
      this.couple = couple;
      this.applyGains(); // a fresh stream adopts the current lane gains/mutes
    } catch {
      this.stream = null;
      this.couple = null;
    }
    this.bump();
  }

  teardown(): void {
    this.stream?.close();
    this.stream = null;
    this.couple = null;
    this.playing = false;
    this.bump();
  }

  get attached(): boolean {
    return this.stream?.open ?? false;
  }

  play(): void {
    if (!this.stream) return;
    this.stream.play();
    this.playing = true;
    this.bump();
  }

  pause(): void {
    const c = this.clock();
    if (Number.isFinite(c)) this.lastSeekT = c; // hold the frame where we stopped
    this.stream?.pause();
    this.playing = false;
    this.bump();
  }

  toggle(): void {
    if (this.playing) this.pause();
    else this.play();
  }

  seek(t: number): void {
    this.stream?.seek(t);
    this.lastSeekT = t;
    this.bump();
  }

  /** Video-time clock for slaving; NaN when not running. */
  clock(): number {
    return this.stream?.clock() ?? NaN;
  }

  underruns(): number {
    return this.stream?.underruns() ?? 0;
  }

  setMode(mode: MonitorMode): void {
    this.mode = mode;
    this.stream?.setMode(mode);
    this.bump();
  }

  /** Push the effective (mute-applied) gains to the core mixer. */
  private applyGains(): void {
    this.stream?.setGains(this.camMuted ? 0 : this.camGain, this.extMuted ? 0 : this.extGain);
  }

  setCamGain(v: number): void {
    this.camGain = v;
    this.applyGains();
    this.bump();
  }

  setExtGain(v: number): void {
    this.extGain = v;
    this.applyGains();
    this.bump();
  }

  toggleCamMute(): void {
    this.camMuted = !this.camMuted;
    this.applyGains();
    this.bump();
  }

  toggleExtMute(): void {
    this.extMuted = !this.extMuted;
    this.applyGains();
    this.bump();
  }
}
