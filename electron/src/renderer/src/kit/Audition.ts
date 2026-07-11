// Audition.ts — audition stream handle (§4.8) + the output pump; TS twin of
// CoupleKit/Audition.swift. The DSP (mixing, gains, crossfades, drift
// rendering) is core-side behind cpl_audition_fill; the pump in couple-node
// pulls it straight to the device (no JS on the audio path).
import { native } from './native';
import type { Engine } from './Engine';
import type { SessionHandle } from './Session';

export type MonitorMode = 'pan' | 'overlay' | 'null';

const MODE_CODE: Record<MonitorMode, number> = { pan: 0, overlay: 1, null: 2 };

export class AuditionStream {
  private readonly engine: Engine;
  private id: bigint;

  constructor(engine: Engine, session: SessionHandle, mode: MonitorMode, tStart: number) {
    this.engine = engine;
    this.id = native.auditionOpen(engine.ctx, session.id, MODE_CODE[mode], tStart);
    if (this.id === 0n) throw engine.takeError();
  }

  get open(): boolean {
    return this.id !== 0n;
  }

  /** Start pulling to the audio device. */
  play(): void {
    if (this.id === 0n) return;
    native.audioStart(this.engine.ctx, this.id);
  }

  /** Stop the device pump; the stream stays open (clock holds). */
  pause(): void {
    native.audioStop();
  }

  get playing(): boolean {
    return native.audioRunning();
  }

  /** The stream's video-time clock (latency-compensated); NaN when stopped. */
  clock(): number {
    return native.audioClock();
  }

  underruns(): number {
    return native.audioUnderruns();
  }

  seek(t: number): void {
    if (this.id === 0n) return;
    native.auditionSeek(this.engine.ctx, this.id, t);
  }

  /** Click-free mode switch (240-sample crossfade, core-side). */
  setMode(mode: MonitorMode): void {
    if (this.id === 0n) return;
    native.auditionSetMode(this.engine.ctx, this.id, MODE_CODE[mode]);
  }

  /** Per-track monitor gains on top of the loudness-matching gains. */
  setGains(gainCam: number, gainExt: number): void {
    if (this.id === 0n) return;
    native.auditionSetGains(this.engine.ctx, this.id, gainCam, gainExt);
  }

  close(): void {
    if (this.id === 0n) return;
    native.audioStop();
    native.auditionClose(this.engine.ctx, this.id);
    this.id = 0n;
  }
}
