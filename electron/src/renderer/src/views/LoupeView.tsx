// LoupeView.tsx — the per-anchor loupe (UX §1.6): both tracks overlaid at one
// center line with additive "screen"/lighter blending, per-loupe local
// normalization to 86% half-height, a residual guidance arrow, and a header with
// the local Δ + snap/align/remove actions. Below 0.5 s windows the canvas
// switches to raw-sample rendering (SPEC §11.2). A port of LoupeView.swift.
// Presentation only — anchor placement/align is decided in PairSession's pure
// policy calls (PLAN-CORE-SHELL-BOUNDARY §1).
import React, { useRef } from 'react';
import { appModel } from '../app/AppModel';
import type { CoupleRow } from '../app/CoupleRow';
import type { PairSession } from '../app/PairSession';
import type { TrackHandle } from '../kit/Track';
import { useModel } from '../app/observable';
import { useCanvas } from './useCanvas';
import './editor.css';

const CAMERA = '#2ec8c2';
const EXTERNAL = '#e0a63c';
const CANVAS_BG = '#0f1113';
const WARN = '#f2b840';
// 4-hue anchor palette (Theme.anchorColors): violet / rose / sky / lime.
const ANCHOR_COLORS = ['#a68cfa', '#fa7085', '#61a6fa', '#a3e636'];

const RAW_THRESHOLD_S = 0.5;
const MAX_COLS = 2048;

function hmmss(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  if (s >= 3600) {
    return `${Math.floor(s / 3600)}:${String(Math.floor((s % 3600) / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
  }
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

function deltaColor(absMs: number): string {
  if (absMs < 0.7) return '#3ecf6e';
  if (absMs < 3.0) return WARN;
  return '#e05c5c';
}

interface LoupeBuffers {
  mins: Int8Array;
  maxs: Int8Array;
  camRaw: Float32Array;
  extRaw: Float32Array;
}

/** x → camera-time within this loupe's window (LoupeView.swift cameraTime). */
function cameraTime(localX: number, width: number, anchor: number, winMs: number): number {
  if (width <= 0) return anchor;
  const winS = winMs / 1000;
  const frac = Math.max(0, Math.min(1, localX / width));
  return anchor - winS / 2 + frac * winS;
}

/** Stepped per-bin band columns, locally normalized to 86% of half-height. */
function drawBand(
  ctx: CanvasRenderingContext2D,
  track: TrackHandle,
  buf: LoupeBuffers,
  t0: number,
  t1: number,
  cols: number,
  w: number,
  midY: number,
  halfH: number,
  color: string
): void {
  try {
    track.band(t0, t1, cols, buf.mins, buf.maxs);
  } catch {
    return;
  }
  let peak = 0;
  for (let i = 0; i < cols; i++) {
    peak = Math.max(peak, Math.abs(buf.mins[i]), Math.abs(buf.maxs[i]));
  }
  if (peak <= 0) return;
  const scale = (0.86 * halfH) / peak;
  const colW = w / cols;
  ctx.beginPath();
  for (let i = 0; i < cols; i++) {
    const yTop = midY - buf.maxs[i] * scale;
    const yBot = midY - buf.mins[i] * scale;
    ctx.rect(i * colW, yTop, colW, Math.max(yBot - yTop, 0.7));
  }
  ctx.globalAlpha = 0.8;
  ctx.fillStyle = color;
  ctx.fill();
  ctx.globalAlpha = 1;
}

/** Raw-sample rendering for deep zoom (< 0.5 s window). */
function drawRaw(
  ctx: CanvasRenderingContext2D,
  track: TrackHandle,
  out: Float32Array,
  t0: number,
  t1: number,
  cols: number,
  w: number,
  midY: number,
  halfH: number,
  color: string
): void {
  let frames = 0;
  try {
    frames = track.rawWindow(t0, t1, out);
  } catch {
    return;
  }
  if (frames <= 8) return;
  let peak = 0;
  for (let i = 0; i < frames; i++) peak = Math.max(peak, Math.abs(out[i]));
  if (peak <= 0) return;
  const scale = (0.86 * halfH) / peak;
  if (frames > cols * 3) {
    const colW = w / cols;
    ctx.beginPath();
    for (let c = 0; c < cols; c++) {
      const a = Math.floor((c * frames) / cols);
      const b = Math.max(Math.floor(((c + 1) * frames) / cols), a + 1);
      let lo = Infinity;
      let hi = -Infinity;
      for (let i = a; i < Math.min(b, frames); i++) {
        lo = Math.min(lo, out[i]);
        hi = Math.max(hi, out[i]);
      }
      if (hi < lo) continue;
      ctx.rect(c * colW, midY - hi * scale, colW, Math.max((hi - lo) * scale, 0.7));
    }
    ctx.globalAlpha = 0.8;
    ctx.fillStyle = color;
    ctx.fill();
    ctx.globalAlpha = 1;
  } else {
    const dx = w / Math.max(frames - 1, 1);
    ctx.beginPath();
    ctx.moveTo(0, midY - out[0] * scale);
    for (let i = 1; i < frames; i++) ctx.lineTo(i * dx, midY - out[i] * scale);
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.4;
    ctx.lineJoin = 'round';
    ctx.lineCap = 'round';
    ctx.globalAlpha = 0.9;
    ctx.stroke();
    ctx.globalAlpha = 1;
  }
}

function drawLoupe(
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  couple: CoupleRow,
  pair: PairSession,
  index: number,
  buf: LoupeBuffers
): void {
  ctx.fillStyle = CANVAS_BG;
  ctx.fillRect(0, 0, w, h);
  const cam = pair.cameraTrack;
  const ext = pair.externalTrack;
  const a = pair.anchors[index];
  if (!cam || !ext || !a) return;
  const winS = a.winMs / 1000;
  const t0 = a.t - winS / 2;
  const t1 = a.t + winS / 2;
  const m = pair.mapping();
  const cols = Math.min(Math.max(Math.floor(w), 1), MAX_COLS);
  if (cols <= 8) return;
  const midY = h / 2;
  const halfH = h / 2 - 2;
  const accent = ANCHOR_COLORS[index % 4];

  // Additive blend: overlap literally brightens (DECIDED).
  ctx.globalCompositeOperation = 'lighter';
  if (winS < RAW_THRESHOLD_S) {
    drawRaw(ctx, cam, buf.camRaw, t0, t1, cols, w, midY, halfH, CAMERA);
    drawRaw(ctx, ext, buf.extRaw, m.u(t0), m.u(t1), cols, w, midY, halfH, EXTERNAL);
  } else {
    drawBand(ctx, cam, buf, t0, t1, cols, w, midY, halfH, CAMERA);
    drawBand(ctx, ext, buf, m.u(t0), m.u(t1), cols, w, midY, halfH, EXTERNAL);
  }
  ctx.globalCompositeOperation = 'source-over';

  // Anchor center line.
  ctx.strokeStyle = accent;
  ctx.globalAlpha = 0.65;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(w / 2, 0);
  ctx.lineTo(w / 2, h);
  ctx.stroke();
  ctx.globalAlpha = 1;

  // Playhead in the focused loupe during audition.
  const audition = appModel.audition;
  if (audition.playing && audition.couple === couple && pair.focusedAnchor === index) {
    const clock = audition.clock();
    if (Number.isFinite(clock)) {
      const frac = (clock - t0) / winS;
      if (frac >= 0 && frac <= 1) {
        ctx.strokeStyle = 'rgba(255,255,255,0.8)';
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(frac * w, 0);
        ctx.lineTo(frac * w, h);
        ctx.stroke();
      }
    }
  }

  // Residual guidance arrow (|Δ| beyond ~12% of the window).
  const delta = pair.displayedDelta(index);
  if (delta !== null && Math.abs(delta) > 0.12 * a.winMs) {
    const y = 16;
    const cx = w / 2;
    const dir = delta > 0 ? 1 : -1;
    const len = Math.min(w * 0.28, 90);
    ctx.strokeStyle = WARN;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(cx, y);
    ctx.lineTo(cx + dir * len, y);
    ctx.moveTo(cx + dir * len, y);
    ctx.lineTo(cx + dir * (len - 5), y - 4);
    ctx.moveTo(cx + dir * len, y);
    ctx.lineTo(cx + dir * (len - 5), y + 4);
    ctx.stroke();
    ctx.fillStyle = WARN;
    ctx.font = '9px ui-monospace, monospace';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'bottom';
    const label =
      delta > 0 ? `ext content → ${Math.abs(delta).toFixed(1)} ms` : `← ext content ${Math.abs(delta).toFixed(1)} ms`;
    ctx.fillText(label, cx + dir * (len / 2), y - 6);
    ctx.textAlign = 'left';
  }
}

function LoupeView({ couple, pair, index }: { couple: CoupleRow; pair: PairSession; index: number }): React.JSX.Element {
  const bufRef = useRef<LoupeBuffers | null>(null);
  if (bufRef.current === null) {
    bufRef.current = {
      mins: new Int8Array(MAX_COLS),
      maxs: new Int8Array(MAX_COLS),
      camRaw: new Float32Array(1 << 15),
      extRaw: new Float32Array(1 << 15),
    };
  }
  const drag = useRef<{ lastX: number; moved: number } | null>(null);
  const canvasRef = useCanvas((ctx, w, h) => drawLoupe(ctx, w, h, couple, pair, index, bufRef.current!));

  const a = pair.anchors[index];
  const accent = ANCHOR_COLORS[index % 4];
  const delta = pair.displayedDelta(index);
  const winMs = a?.winMs ?? 500;

  return (
    <div className="loupe" style={{ borderColor: `${accent}73` }}>
      <div className="loupe-header">
        <span className="loupe-badge" style={{ background: accent }}>
          {index + 1}
        </span>
        <span className="loupe-time">{hmmss(a?.t ?? 0)}</span>
        {delta !== null ? (
          <span className="loupe-delta" style={{ color: deltaColor(Math.abs(delta)) }}>
            local Δ {delta >= 0 ? '+' : ''}
            {delta.toFixed(2)} ms
          </span>
        ) : (
          <span className="loupe-delta dim">local Δ —</span>
        )}
        <span className="loupe-win">±{(winMs / 2).toFixed(winMs >= 2 ? 0 : 1)}ms</span>
        <button
          className={`loupe-chip${a?.auto ? ' on' : ''}`}
          onClick={() => pair.armAuto(index)}
          title="Auto-frame this loupe"
        >
          auto
        </button>
        <span className="loupe-spacer" />
        <button className="loupe-chip" title="Re-snap to nearest transient" onClick={() => pair.snapAnchor(index)}>
          ⌖
        </button>
        <button
          className="loupe-chip"
          title="Align: zero this anchor's Δ"
          disabled={delta === null}
          onClick={() => pair.alignAnchor(index)}
        >
          ⇥
        </button>
        <button
          className="loupe-chip"
          title="Remove anchor"
          disabled={pair.anchors.length <= 2}
          onClick={() => pair.removeAnchor(index)}
        >
          ×
        </button>
      </div>
      <canvas
        ref={canvasRef}
        className="loupe-canvas"
        onWheel={(e) => {
          if (!a) return;
          pair.manualZoom(index, Math.exp(e.deltaY * 0.0015));
        }}
        onPointerDown={(e) => {
          if (!pair.inputsEnabled || !a) return;
          e.currentTarget.setPointerCapture(e.pointerId);
          drag.current = { lastX: e.clientX, moved: 0 };
        }}
        onPointerMove={(e) => {
          const d = drag.current;
          if (!d || !a) return;
          const rect = e.currentTarget.getBoundingClientRect();
          const dx = e.clientX - d.lastX;
          d.lastX = e.clientX;
          d.moved += Math.abs(dx);
          const msPerPx = a.winMs / Math.max(1, rect.width);
          const deltaL = -dx * msPerPx; // content follows the pointer (§2.1 sign)
          if (e.shiftKey) pair.pivotAnchorDrag(index, deltaL);
          else pair.setOffsetLive(pair.mapping().offsetMs + deltaL);
        }}
        onPointerUp={(e) => {
          const d = drag.current;
          drag.current = null;
          e.currentTarget.releasePointerCapture(e.pointerId);
          if (!a) return;
          pair.commitDrift();
          if (d && d.moved < 3) {
            const rect = e.currentTarget.getBoundingClientRect();
            const t = cameraTime(e.clientX - rect.left, rect.width, a.t, a.winMs);
            pair.setPlayhead(t);
            pair.focusedAnchor = index;
            if (appModel.audition.couple === couple) appModel.audition.seek(t);
            pair.invalidate();
          }
        }}
      />
    </div>
  );
}

/** The loupe row (grid) / column (stack) area. */
export function LoupeArea({ couple, pair }: { couple: CoupleRow; pair: PairSession }): React.JSX.Element {
  useModel(pair);
  if (pair.anchors.length === 0) {
    return <div className="loupe-area empty">No anchors — add one from the waveform toolbar.</div>;
  }
  return (
    <div className={`loupe-area ${pair.loupeLayout}`}>
      {pair.anchors.map((_, i) => (
        <LoupeView key={i} couple={couple} pair={pair} index={i} />
      ))}
    </div>
  );
}
