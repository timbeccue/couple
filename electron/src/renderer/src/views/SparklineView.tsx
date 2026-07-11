// SparklineView.tsx — the drift sparkline (UX §1.4 row 2, deliberately
// peripheral); a port of mac/Sources/Couple/Views/SparklineView.swift. Measured
// per-window local lags are plotted as dots (opacity = confidence) against the
// current mapping line L(t). The x-domain is the MEASURED span, not the full
// video (2026-07-03 UX-DECISIONS), with corner time labels for context. Windows
// attempted but not accepted are baseline ticks, never dots (a failed measure
// carries no meaningful lag). Read-only: it reflects the last sync's telemetry.
import React from 'react';
import type { PairSession } from '../app/PairSession';
import type { DriftWindow } from '../kit/Wire';
import { useModel } from '../app/observable';
import { useCanvas } from './useCanvas';
import './editor.css';

const ACCENT = '#4a9eda';
const EXTERNAL = '#e0a63c';
const CANVAS_BG = '#0f1113';
const TICK_COLOR = 'rgba(216, 218, 222, 0.32)';

function mmss(t: number): string {
  const s = Math.round(t);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

function paint(ctx: CanvasRenderingContext2D, w: number, h: number, pair: PairSession): void {
  ctx.fillStyle = CANVAS_BG;
  ctx.fillRect(0, 0, w, h);

  // Accepted windows are the PERSISTED fit points (state.driftWindows), like the
  // mac SparklineView's `pair.driftWindows`; rejected attempts come only from the
  // last sync's telemetry (windows where accepted === false).
  const accepted = pair.driftWindows();
  if (!pair.cameraTrack || accepted.length === 0) {
    ctx.fillStyle = TICK_COLOR;
    ctx.font = '9px ui-monospace, monospace';
    ctx.textBaseline = 'middle';
    ctx.fillText('not measured', 6, h / 2);
    return;
  }
  const rejected: DriftWindow[] = (pair.lastSyncResult?.windows ?? []).filter((win) => !win.accepted);
  const mapping = pair.mapping();

  // x domain: the measured span (accepted ∪ rejected), padded.
  const times = accepted.map((win) => win.t).concat(rejected.map((win) => win.t));
  let tMin = Math.min(...times);
  let tMax = Math.max(...times);
  const tPad = Math.max((tMax - tMin) * 0.08, 1.5);
  tMin -= tPad;
  tMax += tPad;

  // y domain: accepted lags ∪ the mapping line across the domain endpoints.
  const lags = accepted.map((win) => win.lagMs);
  lags.push(mapping.localLagMs(tMin), mapping.localLagMs(tMax));
  const lo = Math.min(...lags);
  const hi = Math.max(...lags);
  const pad = Math.max((hi - lo) * 0.25, 0.5);
  const yLo = lo - pad;
  const yHi = hi + pad;

  const labelH = 9;
  const plotH = h - labelH;
  const xAt = (t: number): number => ((t - tMin) / (tMax - tMin)) * w;
  const yAt = (lag: number): number => plotH - ((lag - yLo) / (yHi - yLo)) * plotH;

  // Mapping line.
  ctx.strokeStyle = ACCENT;
  ctx.globalAlpha = 0.8;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(xAt(tMin), yAt(mapping.localLagMs(tMin)));
  ctx.lineTo(xAt(tMax), yAt(mapping.localLagMs(tMax)));
  ctx.stroke();
  ctx.globalAlpha = 1;

  // Rejected attempts: short baseline ticks (no lag to plot).
  ctx.strokeStyle = 'rgba(216, 218, 222, 0.19)';
  ctx.beginPath();
  for (const win of rejected) {
    const x = xAt(win.t);
    ctx.moveTo(x, plotH - 4);
    ctx.lineTo(x, plotH);
  }
  ctx.stroke();

  // Accepted measurement dots (opacity = confidence).
  ctx.fillStyle = EXTERNAL;
  for (const win of accepted) {
    const x = xAt(win.t);
    const y = yAt(win.lagMs);
    ctx.globalAlpha = Math.min(Math.max(win.ncc, 0.15), 1);
    ctx.beginPath();
    ctx.arc(x, y, 2.2, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.globalAlpha = 1;

  // Corner time labels: where this measured span sits in the video.
  ctx.fillStyle = TICK_COLOR;
  ctx.font = '7px ui-monospace, monospace';
  ctx.textBaseline = 'bottom';
  ctx.textAlign = 'left';
  ctx.fillText(mmss(tMin), 3, h);
  ctx.textAlign = 'right';
  ctx.fillText(mmss(tMax), w - 3, h);
  ctx.textAlign = 'left';
}

export function SparklineView({ pair }: { pair: PairSession }): React.JSX.Element {
  useModel(pair);
  const canvasRef = useCanvas((ctx, w, h) => paint(ctx, w, h, pair));
  return (
    <div className="sparkline">
      <canvas ref={canvasRef} />
    </div>
  );
}
