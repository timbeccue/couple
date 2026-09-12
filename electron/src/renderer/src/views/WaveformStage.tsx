// WaveformStage.tsx — the dual-lane waveform editor (SPEC §11.2 / UX §1.6),
// a port of mac/Sources/Couple/Views/WaveformStage.swift: camera lane on top
// (teal), external lane below (amber) drawn through the live mapping, with one
// zoomable/pannable strip for navigation, alignment editing, and anchors. Envelope
// columns come from TrackHandle.band into caller-owned Int8Arrays reused across
// frames (the call has a 50µs budget — never allocate per frame). Presentation
// only; no decision logic lives here (PLAN-CORE-SHELL-BOUNDARY §1).
import React, { useEffect, useRef } from 'react';
import { appModel } from '../app/AppModel';
import type { CoupleRow } from '../app/CoupleRow';
import type { PairSession } from '../app/PairSession';
import type { TrackHandle } from '../kit/Track';
import { useModel } from '../app/observable';
import { env } from '../kit/native';
import { useCanvas } from './useCanvas';
import { SparklineView } from './SparklineView';
import './editor.css';

// Semantic colors are fixed (SPEC §13.3), identical hues to the mac Theme.
const CAMERA = '#2ec8c2'; // teal
const EXTERNAL = '#e0a63c'; // amber
const CANVAS_BG = '#0f1113';
const TICK_COLOR = 'rgba(216, 218, 222, 0.32)';

const TICK_ROW_H = 14;
// A badge gutter above the lanes keeps the anchor pin heads and the playhead knob
// off the waveform pixels; the tick row sits below (WaveformStage.swift badgeGutterH).
const BADGE_GUTTER_H = 16;
const MAX_COLS = 2048;

interface BandBuffers {
  mins: Int8Array;
  maxs: Int8Array;
}

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

function hmmss(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  if (s >= 3600) {
    return `${Math.floor(s / 3600)}:${String(Math.floor((s % 3600) / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
  }
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

/** Fill and draw one lane's min/max envelope columns (WaveformStage.swift
 * drawLane). `t0`/`t1` are in the LANE's own time (the external lane passes its
 * mapped u-range so one band query stays column-aligned). */
function drawLane(
  ctx: CanvasRenderingContext2D,
  track: TrackHandle,
  buf: BandBuffers,
  t0: number,
  t1: number,
  cols: number,
  colW: number,
  midY: number,
  halfH: number,
  color: string,
  gainMul: number,
  dimmed: boolean
): void {
  try {
    track.band(t0, t1, cols, buf.mins, buf.maxs);
  } catch {
    return; // core zero-fills out-of-track columns; a hard error just skips paint
  }
  // The monitor gain scales the lane so what you see tracks what you'd hear
  // (WaveformStage.swift drawLane); mute dims it instead of hiding it.
  const gain = (1.44 / Math.max(track.meta.stats.activeP959600, 1e-6)) * gainMul;
  // UX §4.1: the gain applies to envelope spans; min/max are half a span, hence
  // the 0.5 factor (typical loud content ≈ 72% of lane height).
  const scale = (0.5 * gain * track.meta.normalization) / 127.0;
  ctx.beginPath();
  for (let i = 0; i < cols; i++) {
    const vMin = clamp(buf.mins[i] * scale, -1, 1);
    const vMax = clamp(buf.maxs[i] * scale, -1, 1);
    if (vMax - vMin <= 1e-4) continue;
    const yTop = midY - vMax * halfH;
    const yBot = midY - vMin * halfH;
    ctx.rect(i * colW, yTop, colW, Math.max(yBot - yTop, 0.7));
  }
  ctx.globalAlpha = dimmed ? 0.25 : 0.85;
  ctx.fillStyle = color;
  ctx.fill();
  ctx.globalAlpha = 1;
}

/** Per-lane monitor state driving the waveform scale/dim (mirror of the audio). */
interface LaneAudio {
  camGain: number;
  extGain: number;
  camMuted: boolean;
  extMuted: boolean;
}

/** Time tick row with h:mm:ss labels (WaveformStage.swift drawTicks). */
function drawTicks(
  ctx: CanvasRenderingContext2D,
  width: number,
  d0: number,
  d1: number,
  y: number
): void {
  const span = d1 - d0;
  if (span <= 0) return;
  const pxPerSec = width / span;
  const candidates = [15, 30, 60, 120, 300, 600, 1200];
  const step = candidates.find((c) => c * pxPerSec >= 56) ?? 1800;
  ctx.strokeStyle = 'rgba(216, 218, 222, 0.19)';
  ctx.lineWidth = 1;
  ctx.fillStyle = TICK_COLOR;
  ctx.font = '8px ui-monospace, monospace';
  ctx.textBaseline = 'top';
  ctx.beginPath();
  let t = Math.floor(d0 / step) * step;
  while (t <= d1) {
    if (t >= d0) {
      const x = ((t - d0) / span) * width;
      ctx.moveTo(x, y);
      ctx.lineTo(x, y + 4);
      ctx.fillText(hmmss(t), x + 3, y + 4);
    }
    t += step;
  }
  ctx.stroke();
}

/** Paint the dual-lane stage over [d0, d1] (video time). */
function paintStage(
  ctx: CanvasRenderingContext2D,
  cssW: number,
  cssH: number,
  pair: PairSession,
  d0: number,
  d1: number,
  buf: BandBuffers,
  laneAudio: LaneAudio,
  playing: boolean
): void {
  ctx.fillStyle = CANVAS_BG;
  ctx.fillRect(0, 0, cssW, cssH);

  const cam = pair.cameraTrack;
  const ext = pair.externalTrack;
  const span = d1 - d0;
  if (!cam || !ext || span <= 0) return;

  // A badge gutter above the lanes, the tick row below (WaveformStage.swift).
  const waveH = cssH - TICK_ROW_H - BADGE_GUTTER_H;
  const laneH = waveH / 2;
  const cols = Math.min(Math.max(Math.floor(cssW), 1), MAX_COLS);
  const colW = cssW / cols;
  const m = pair.mapping();

  // Overlap highlight (subtle, behind the lanes).
  const ov = pair.overlap();
  if (ov) {
    const x0 = ((ov[0] - d0) / span) * cssW;
    const x1 = ((ov[1] - d0) / span) * cssW;
    ctx.fillStyle = 'rgba(255, 255, 255, 0.045)';
    ctx.fillRect(x0, BADGE_GUTTER_H, x1 - x0, waveH);
  }

  // Camera lane (top); external lane (bottom) drawn through the live mapping.
  // Each scaled by its monitor gain, dimmed when muted.
  drawLane(ctx, cam, buf, d0, d1, cols, colW, BADGE_GUTTER_H + laneH / 2, laneH / 2 - 3, CAMERA, laneAudio.camGain, laneAudio.camMuted);
  drawLane(ctx, ext, buf, m.u(d0), m.u(d1), cols, colW, BADGE_GUTTER_H + laneH + laneH / 2, laneH / 2 - 3, EXTERNAL, laneAudio.extGain, laneAudio.extMuted);

  drawTicks(ctx, cssW, d0, d1, BADGE_GUTTER_H + waveH);

  // Lane labels.
  ctx.font = '8px ui-monospace, monospace';
  ctx.textBaseline = 'top';
  ctx.fillStyle = 'rgba(46, 200, 194, 0.7)';
  ctx.fillText('V · CAMERA', 4, BADGE_GUTTER_H + 4);
  ctx.fillStyle = 'rgba(224, 166, 60, 0.7)';
  ctx.fillText('A · EXTERNAL', 4, BADGE_GUTTER_H + laneH + 4);

  // Playhead (UX §1.6): always visible once a session exists — bright while
  // playing, grey when parked — with a grab-handle triangle at the gutter edge
  // (WaveformStage.swift playhead + knob).
  const px = ((pair.playheadT - d0) / span) * cssW;
  if (px >= 0 && px <= cssW) {
    ctx.strokeStyle = `rgba(255, 255, 255, ${playing ? 0.85 : 0.5})`;
    ctx.lineWidth = playing ? 1 : 1.5;
    ctx.beginPath();
    ctx.moveTo(px, BADGE_GUTTER_H);
    ctx.lineTo(px, BADGE_GUTTER_H + waveH);
    ctx.stroke();
    ctx.beginPath();
    ctx.moveTo(px - 4, BADGE_GUTTER_H - 6);
    ctx.lineTo(px + 4, BADGE_GUTTER_H - 6);
    ctx.lineTo(px, BADGE_GUTTER_H + 2);
    ctx.closePath();
    ctx.fillStyle = `rgba(255, 255, 255, ${playing ? 0.85 : 0.55})`;
    ctx.fill();
  }
}

function useBandBuffers(): BandBuffers {
  const ref = useRef<BandBuffers | null>(null);
  if (ref.current === null) {
    ref.current = { mins: new Int8Array(MAX_COLS), maxs: new Int8Array(MAX_COLS) };
  }
  return ref.current;
}

function timeAtX(e: React.PointerEvent | React.WheelEvent, d0: number, d1: number): number {
  const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
  const x = e.clientX - rect.left;
  return d0 + (x / Math.max(1, rect.width)) * (d1 - d0);
}

const ANCHOR_COLORS = ['#a68cfa', '#fa7085', '#61a6fa', '#a3e636'];

// A live drag on the stage. `offset`/`drift` edit alignment (external lane,
// InputRouter/WaveformStage.swift); `pan` navigates (camera lane). Esc restores
// the (offset, drift) captured at drag start (UX §3, "no half-applied guess").
interface DragState {
  mode: 'pan' | 'offset' | 'drift';
  lastX: number;
  moved: number;
  startOffsetMs: number;
  startDriftPpm: number;
  refT: number; // drift pivot reference (video time), fixed for the gesture
  grabT: number; // grabbed content time (video time), fixed for the gesture
  esc: (e: KeyboardEvent) => void;
}

/** The single dual-lane waveform surface (WaveformStage.swift `OverviewStrip`):
 * ONE zoomable/pannable strip that owns the anchor pins and the playhead — the
 * mac shell has no separate minimap, so we don't either (the `Fit` chip restores
 * the full union). Wheel zooms about the cursor (or pans horizontally). A drag on
 * the external lane edits alignment (plain = live offset, Shift = pivot drift); a
 * drag on the camera lane pans; a click seeks the playhead (and the audition when
 * attached). Anchor pins ride on top: draggable (live moveAnchor, releaseAnchor
 * on drop, no snap), a click focuses the loupe. Editing gated by `inputsEnabled`;
 * Esc cancels the gesture. */
function Stage({
  couple,
  pair,
  onShowLoupes,
}: {
  couple: CoupleRow;
  pair: PairSession;
  onShowLoupes: () => void;
}): React.JSX.Element {
  const audition = useModel(appModel.audition);
  const buf = useBandBuffers();
  const containerRef = useRef<HTMLDivElement>(null);
  const drag = useRef<DragState | null>(null);
  const pinDrag = useRef<{ i: number; moved: number } | null>(null);
  const canvasRef = useCanvas((ctx, w, h) => {
    const [d0, d1] = pair.view();
    paintStage(
      ctx,
      w,
      h,
      pair,
      d0,
      d1,
      buf,
      {
        camGain: audition.camGain,
        extGain: audition.extGain,
        camMuted: audition.camMuted,
        extMuted: audition.extMuted,
      },
      audition.playing
    );
  });

  const endDrag = (): void => {
    const d = drag.current;
    if (!d) return;
    window.removeEventListener('keydown', d.esc);
    drag.current = null;
  };

  const timeFromClientX = (clientX: number): number => {
    const rect = containerRef.current?.getBoundingClientRect();
    const [d0, d1] = pair.view();
    if (!rect || rect.width <= 0) return d0;
    return d0 + ((clientX - rect.left) / rect.width) * (d1 - d0);
  };

  const seekTo = (t: number): void => {
    pair.setPlayhead(t);
    if (appModel.audition.couple === couple) appModel.audition.seek(t);
  };

  const [d0, d1] = pair.view();
  const span = d1 - d0 || 1;

  return (
    <div className="wave-main">
      <div className="wave-lanes" ref={containerRef}>
      <canvas
        ref={canvasRef}
        onWheel={(e) => {
          const [vd0, vd1] = pair.view();
          const rect = e.currentTarget.getBoundingClientRect();
          if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
            pair.panView((e.deltaX / Math.max(1, rect.width)) * (vd1 - vd0));
          } else {
            pair.zoomView(Math.exp(e.deltaY * 0.0015), timeAtX(e, vd0, vd1));
          }
        }}
        onPointerDown={(e) => {
          e.currentTarget.setPointerCapture(e.pointerId);
          const rect = e.currentTarget.getBoundingClientRect();
          const y = e.clientY - rect.top;
          const waveH = rect.height - TICK_ROW_H - BADGE_GUTTER_H;
          const onExternal = y > BADGE_GUTTER_H + waveH / 2 && y <= BADGE_GUTTER_H + waveH;
          const [vd0, vd1] = pair.view();
          const m = pair.mapping();
          const ov = pair.overlap();
          const mode: DragState['mode'] =
            onExternal && pair.inputsEnabled ? (e.shiftKey ? 'drift' : 'offset') : 'pan';
          const esc = (ke: KeyboardEvent): void => {
            if (ke.key !== 'Escape' || !drag.current) return;
            pair.restoreAlignment(drag.current.startOffsetMs, drag.current.startDriftPpm);
            endDrag();
          };
          window.addEventListener('keydown', esc);
          drag.current = {
            mode,
            lastX: e.clientX,
            moved: 0,
            startOffsetMs: m.offsetMs,
            startDriftPpm: m.driftPpm,
            refT: ov ? ov[0] : vd0,
            grabT: timeAtX(e, vd0, vd1),
            esc,
          };
        }}
        onPointerMove={(e) => {
          const d = drag.current;
          if (!d) return;
          const rect = e.currentTarget.getBoundingClientRect();
          const dx = e.clientX - d.lastX;
          d.lastX = e.clientX;
          d.moved += Math.abs(dx);
          const [vd0, vd1] = pair.view();
          const vspan = vd1 - vd0;
          const msPerPx = (vspan / Math.max(1, rect.width)) * 1000;
          switch (d.mode) {
            case 'pan':
              pair.panView(-(dx / Math.max(1, rect.width)) * vspan);
              break;
            case 'offset':
              pair.setOffsetLive(pair.mapping().offsetMs - dx * msPerPx);
              break;
            case 'drift':
              pair.pivotDriftDrag(d.grabT, d.refT, -dx * msPerPx);
              break;
          }
        }}
        onPointerUp={(e) => {
          const d = drag.current;
          e.currentTarget.releasePointerCapture(e.pointerId);
          endDrag();
          if (!d) return;
          if (d.mode === 'drift') pair.commitDrift();
          if (d.moved < 3) {
            const [vd0, vd1] = pair.view();
            seekTo(timeAtX(e, vd0, vd1));
          }
        }}
      />
      {pair.anchors.map((a, i) => {
        const frac = (a.t - d0) / span;
        if (frac < 0 || frac > 1) return null; // off the zoomed view
        return (
          <div
            key={i}
            className="anchor-pin"
            style={{ left: `${frac * 100}%`, ['--pin' as string]: ANCHOR_COLORS[i % 4] }}
            title={`Anchor ${i + 1}`}
            onPointerDown={(e) => {
              e.stopPropagation();
              e.currentTarget.setPointerCapture(e.pointerId);
              pinDrag.current = { i, moved: 0 };
            }}
            onPointerMove={(e) => {
              const d = pinDrag.current;
              if (!d) return;
              d.moved += Math.abs(e.movementX);
              if (pair.inputsEnabled) pair.moveAnchor(d.i, timeFromClientX(e.clientX));
            }}
            onPointerUp={(e) => {
              const d = pinDrag.current;
              pinDrag.current = null;
              e.currentTarget.releasePointerCapture(e.pointerId);
              if (!d) return;
              if (d.moved < 3) {
                pair.focusedAnchor = d.i;
                pair.invalidate();
                onShowLoupes();
              } else {
                pair.releaseAnchor(d.i);
              }
            }}
          >
            <span className="anchor-badge">{i + 1}</span>
          </div>
        );
      })}
      </div>
      {/* Per-lane monitor mute + volume in a static column beside the lanes, one
          cluster per lane (WaveformStage.swift laneControlsColumn). */}
      {pair.session && (
        <div className="lane-controls">
          <LaneAudioControls
            tint={CAMERA}
            muted={audition.camMuted}
            gain={audition.camGain}
            onMute={() => audition.toggleCamMute()}
            onGain={(v) => audition.setCamGain(v)}
          />
          <LaneAudioControls
            tint={EXTERNAL}
            muted={audition.extMuted}
            gain={audition.extGain}
            onMute={() => audition.toggleExtMute()}
            onGain={(v) => audition.setExtGain(v)}
          />
        </div>
      )}
    </div>
  );
}

/** Speaker glyph (SF Symbols speaker.wave.2.fill / speaker.slash.fill twin) as a
 * crisp inline SVG — sound waves when live, a slash when muted. */
function SpeakerIcon({ muted }: { muted: boolean }): React.JSX.Element {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path d="M2.2 6 H5 L8.4 3.2 V12.8 L5 10 H2.2 Z" fill="currentColor" />
      {muted ? (
        <path d="M11 5.6 L14.4 10.4 M14.4 5.6 L11 10.4" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" />
      ) : (
        <>
          <path d="M10.7 5.5 a3.3 3.3 0 0 1 0 5" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
          <path d="M12.5 3.9 a5.8 5.8 0 0 1 0 8.2" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
        </>
      )}
    </svg>
  );
}

/** One lane's monitor controls: a mute toggle + a 0–2 volume slider tinted to the
 * lane color (WaveformStage.swift LaneAudioControls, 507-545). The slider scales
 * both the audio (core gains) and the drawn waveform; mute remembers the level
 * and disables the slider. */
function LaneAudioControls({
  tint,
  muted,
  gain,
  onMute,
  onGain,
}: {
  tint: string;
  muted: boolean;
  gain: number;
  onMute: () => void;
  onGain: (v: number) => void;
}): React.JSX.Element {
  return (
    <div className="lane-ctl">
      <button
        className={`lane-mute${muted ? ' muted' : ''}`}
        style={{ color: muted ? undefined : tint }}
        onClick={onMute}
        title={muted ? 'Unmute' : 'Mute'}
      >
        <SpeakerIcon muted={muted} />
      </button>
      <input
        type="range"
        min={0}
        max={2}
        step={0.05}
        value={gain}
        disabled={muted}
        style={{ '--lane-tint': tint } as React.CSSProperties}
        title="Monitor volume (waveform scales to match)"
        onChange={(e) => onGain(Number(e.target.value))}
      />
    </div>
  );
}

/** Stage-local keyboard nudges (InputRouter.swift keyCodes 123/124 + `[`/`]`):
 * ← / → nudge offset (step 1 ms, Shift 10, Alt 0.1; ← moves external later),
 * `[` / `]` nudge drift (step 1 ppm, Alt 0.1), ⌘0 fits. Active only while the
 * stage holds focus, so they never fight the app-level queue keys in App.tsx. */
function onStageKey(pair: PairSession, e: React.KeyboardEvent): void {
  switch (e.key) {
    case 'ArrowLeft':
      pair.nudgeOffset(e.shiftKey ? 10 : e.altKey ? 0.1 : 1);
      e.preventDefault();
      break;
    case 'ArrowRight':
      pair.nudgeOffset(e.shiftKey ? -10 : e.altKey ? -0.1 : -1);
      e.preventDefault();
      break;
    case '[':
      pair.nudgeDrift(e.altKey ? -0.1 : -1);
      e.preventDefault();
      break;
    case ']':
      pair.nudgeDrift(e.altKey ? 0.1 : 1);
      e.preventDefault();
      break;
    case '0':
      if (e.metaKey) {
        pair.fitView();
        e.preventDefault();
      }
      break;
  }
}

/** Overlap chip text + tone (WaveformStage.swift overlapChip). */
function overlapChip(pair: PairSession): { text: string; tone: 'good' | 'warn' } {
  const cam = pair.cameraTrack;
  if (!cam || !pair.externalTrack) return { text: 'no alignment', tone: 'warn' };
  const m = pair.mapping();
  if (pair.syncStatus === 'idle' && m.offsetMs === 0 && m.driftPpm === 0) {
    return { text: 'no alignment', tone: 'warn' };
  }
  const ov = pair.overlap();
  if (!ov) return { text: 'no overlap', tone: 'warn' };
  const secs = ov[1] - ov[0];
  const pct = Math.round((secs / cam.meta.durationS) * 100);
  const mm = Math.floor(secs / 60);
  const ss = Math.round(secs % 60);
  return { text: `overlap ${pct}% · ${mm}:${String(ss).padStart(2, '0')}`, tone: 'good' };
}

/** Fit-quality caption above the sparkline (CenterColumn.swift fitStatusLine,
 * 232-258): `fit RMS 2.52 ms · 5/12 win · offset-only`, derived from the last
 * sync's accepted/attempted windows and drift-measurability. */
function fitCaption(pair: PairSession): { text: string; tone: 'good' | 'bad' | 'dim' } {
  if (pair.syncStatus === 'syncing') return { text: 'measuring…', tone: 'dim' };
  if (pair.syncStatus === 'failed') return { text: 'sync failed', tone: 'bad' };
  const fit = pair.fitStatus();
  if (!fit) return { text: 'not measured', tone: 'dim' };
  const ls = pair.lastSyncResult;
  const attempted = ls?.windows?.length;
  const attStr = attempted != null ? `/${attempted}` : '';
  // §5.5 slope guard: a slope needs ≥2 accepted windows; core may also supply an
  // explicit driftMeasurable flag.
  const dmRaw: unknown = ls ? ls.driftMeasurable : undefined;
  const dm = typeof dmRaw === 'boolean' ? dmRaw : fit.windows >= 2;
  const drift = ls ? (dm ? '' : ' · offset-only') : '';
  return {
    text: `fit RMS ${fit.rmsMs.toFixed(2)} ms · ${fit.windows}${attStr} win${drift}`,
    tone: 'good',
  };
}

export function WaveformStage({
  couple,
  pair,
  onShowLoupes,
}: {
  couple: CoupleRow;
  pair: PairSession;
  onShowLoupes: () => void;
}): React.JSX.Element {
  useModel(pair);

  // COUPLE_NUDGE_TEST (headless verification): once the pair syncs, nudge the
  // offset +500 ms through the same setter the ← / → keys use and log the
  // before/after so a harness run can confirm the edit reached core state.
  const nudged = useRef(false);
  useEffect(() => {
    if (nudged.current || !env.COUPLE_NUDGE_TEST || pair.syncStatus !== 'synced') return;
    nudged.current = true;
    const before = pair.mapping().offsetMs;
    pair.nudgeOffset(500);
    const after = pair.mapping().offsetMs;
    console.log(
      `[nudge-test] offsetMs before=${before.toFixed(1)} after=${after.toFixed(1)} delta=${(after - before).toFixed(1)}`
    );
  }, [pair, pair.syncStatus]);

  // COUPLE_ANCHOR_TEST: after sync, exercise "+ anchor" and confirm every anchor
  // (auto-proposed + the added one) lands inside the overlap.
  const anchorTested = useRef(false);
  useEffect(() => {
    if (anchorTested.current || !env.COUPLE_ANCHOR_TEST || pair.syncStatus !== 'synced') return;
    anchorTested.current = true;
    const before = pair.anchors.length;
    pair.addAnchor();
    const ov = pair.overlap();
    const inside = ov ? pair.anchors.every((a) => a.t >= ov[0] && a.t <= ov[1]) : false;
    console.log(
      `[anchor-test] overlap=[${ov ? ov[0].toFixed(2) : '-'},${ov ? ov[1].toFixed(2) : '-'}] ` +
        `count=${before}->${pair.anchors.length} ts=${pair.anchors.map((a) => a.t.toFixed(2)).join(',')} ` +
        `allInsideOverlap=${inside}`
    );
    const fit = pair.fitStatus();
    console.log(`[fit-test] fitStatus=${fit ? `rms=${fit.rmsMs.toFixed(2)} win=${fit.windows}` : 'null'}`);
  }, [pair, pair.syncStatus]);

  if (!pair.cameraTrack || !pair.externalTrack) {
    return <div className="waveform-stage empty">Waveform appears once both tracks load.</div>;
  }
  const chip = overlapChip(pair);
  return (
    <div
      className="waveform-stage"
      tabIndex={0}
      onKeyDown={(e) => onStageKey(pair, e)}
      onPointerDown={(e) => e.currentTarget.focus()}
    >
      <div className="wave-toolbar">
        <span className={`overlap-chip ${chip.tone}`}>{chip.text}</span>
        {pair.anchorsUnavailable && (
          <span className="overlap-chip warn" title="Not enough overlap to propose anchors automatically.">
            place anchors manually
          </span>
        )}
        <span className="wave-spacer" />
        <div className="seg">
          <button
            className={pair.loupeLayout === 'grid' ? 'on' : undefined}
            onClick={() => pair.setLoupeLayout('grid')}
          >
            Side-by-side
          </button>
          <button
            className={pair.loupeLayout === 'stack' ? 'on' : undefined}
            onClick={() => pair.setLoupeLayout('stack')}
          >
            Stacked
          </button>
        </div>
        <button className="wave-chip" disabled={!pair.viewZoomed} onClick={() => pair.fitView()}>
          ⇔ Fit
        </button>
        <button
          className={`wave-chip${pair.lockZoom ? ' on' : ''}`}
          onClick={() => pair.toggleLockZoom()}
          title="Lock all loupe zooms together"
        >
          lock zoom
        </button>
        <button
          className="wave-chip"
          disabled={pair.anchors.length >= 4 || !pair.inputsEnabled}
          onClick={() => pair.addAnchor()}
          title="Add an anchor at the strongest onset in the overlap"
        >
          + anchor
        </button>
      </div>
      {/* CenterColumn.swift WaveformRow: the strip fills the row, with a narrow
          always-visible fit column (caption + drift sparkline) to its right. */}
      <div className="wave-row">
        <Stage couple={couple} pair={pair} onShowLoupes={onShowLoupes} />
        <div className="fit-col">
          <div className={`fit-caption ${fitCaption(pair).tone}`}>{fitCaption(pair).text}</div>
          <SparklineView pair={pair} />
          <button
            className="fit-resync"
            disabled={pair.syncStatus === 'syncing' || !pair.inputsEnabled}
            onClick={() => pair.runAutoSync()}
            title="Re-run auto-sync — never re-proposes anchors or re-frames a view you've seen"
          >
            ↻ re-sync
          </button>
        </div>
      </div>
    </div>
  );
}
