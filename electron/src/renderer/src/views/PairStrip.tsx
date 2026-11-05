// PairStrip.tsx — the two-lane waveform SEEK strip under the video (PLAN-COMPACT-
// MODE Phase 3 step 1; UX §V2.11), both arrangements; port of PairStrip.swift.
// Camera lane (teal) above over the whole clip; external lane (amber) below, placed
// in video time through the pair's mapping and drawn ONLY inside its coverage, with
// a faint baseline elsewhere. Playhead from the shared audition; click or drag
// anywhere seeks (direct manipulation: scrub, never snap — absolute x → time).
//
// Both lanes come from `TrackHandle.band` (cpl_band) at ~2 px/column — the same
// read path as the overview, no extra decode. While a candidate is auditioned the
// amber lane is that candidate's audio at its best-fit lag with its coverage, so a
// late-starting recorder shows as a gap.
import React, { useEffect, useRef, useState } from 'react';
import { appModel, type CandidateAudition } from '../app/AppModel';
import type { CoupleRow } from '../app/CoupleRow';
import { Mapping } from '../app/Mapping';
import { useModel } from '../app/observable';
import type { TrackHandle } from '../kit/Track';
import { useCanvas } from './useCanvas';

const CAMERA = '#2ec8c2';
const EXTERNAL = '#e0a63c';
const MAX_COLS = 2048;

interface StripExternal {
  track: TrackHandle;
  mapping: Mapping;
  /** Video-time window the audio covers; null = draw nothing but the baseline. */
  coverage: [number, number] | null;
}

/** The lane to draw: the audition candidate, else the open session's live mapping
 * + overlap, else the selected candidate's lag + coverage. */
function externalLane(couple: CoupleRow, candidate: CandidateAudition | null): StripExternal | null {
  if (candidate) {
    const cov = candidate.candidate.coverage;
    return {
      track: candidate.track,
      mapping: new Mapping(candidate.candidate.lagMs, 0),
      coverage: cov.t1 > cov.t0 ? [cov.t0, cov.t1] : null,
    };
  }
  const pair = couple.pair;
  if (pair.session && pair.externalTrack) {
    return { track: pair.externalTrack, mapping: pair.mapping(), coverage: pair.overlap() };
  }
  const sel = couple.selectedCandidate;
  const handle = sel ? (appModel.pool.entry(sel.audioKey)?.handle ?? null) : null;
  if (sel && handle) {
    const cov = sel.coverage;
    return {
      track: handle,
      mapping: new Mapping(sel.lagMs, 0),
      coverage: cov.t1 > cov.t0 ? [cov.t0, cov.t1] : null,
    };
  }
  return null;
}

/** Re-render every animation frame while `active` (the moving playhead / time). */
export function useFrameTicker(active: boolean): void {
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!active) return;
    let raf = 0;
    const loop = (): void => {
      setTick((t) => (t + 1) % 1_000_000);
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [active]);
}

export function PairStrip({ couple }: { couple: CoupleRow }): React.JSX.Element {
  const model = useModel(appModel);
  const audition = useModel(model.audition);
  useModel(couple);
  useModel(couple.pair);
  const bufs = useRef({
    camMins: new Int8Array(MAX_COLS),
    camMaxs: new Int8Array(MAX_COLS),
    extMins: new Int8Array(MAX_COLS),
    extMaxs: new Int8Array(MAX_COLS),
  });
  const scrubbing = useRef(false);
  const playingHere = audition.couple === couple && audition.playing;
  useFrameTicker(playingHere);

  const candidate = model.candidateAudition?.coupleID === couple.id ? model.candidateAudition : null;
  const cam = couple.video.handle ?? couple.pair.cameraTrack;
  const dur = couple.video.durationS > 0 ? couple.video.durationS : (cam?.meta.durationS ?? 0);
  const ext = externalLane(couple, candidate);
  const headT = audition.playheadFor(couple);

  const canvasRef = useCanvas((ctx, w, h) => {
    ctx.clearRect(0, 0, w, h);
    const laneH = h / 2;
    const cols = Math.min(Math.max(Math.floor(w / 2), 1), MAX_COLS);
    const colW = w / cols;
    // Faint baselines: visible wherever a lane has nothing to draw.
    ctx.fillStyle = 'rgba(255,255,255,0.10)';
    for (const mid of [laneH / 2, laneH + laneH / 2]) ctx.fillRect(0, mid - 0.5, w, 1);
    const b = bufs.current;
    const drawLane = (
      track: TrackHandle,
      mins: Int8Array,
      maxs: Int8Array,
      midY: number,
      color: string,
      include: (i: number) => boolean
    ): void => {
      const meta = track.meta;
      // Same lane gain as the overview (UX §4.1): half a span.
      const gain = 1.44 / Math.max(meta.stats.activeP959600, 1e-6);
      const scale = (0.5 * gain * meta.normalization) / 127.0;
      const halfH = laneH / 2 - 1;
      const path = new Path2D();
      for (let i = 0; i < cols; i++) {
        if (!include(i)) continue;
        const vMin = Math.max(-1, Math.min(1, mins[i] * scale));
        const vMax = Math.max(-1, Math.min(1, maxs[i] * scale));
        const yTop = midY - Math.max(vMax, 0.02) * halfH;
        const yBot = midY - Math.min(vMin, -0.02) * halfH;
        path.rect(i * colW, yTop, Math.max(colW - 0.6, 0.8), Math.max(yBot - yTop, 0.8));
      }
      ctx.globalAlpha = 0.85;
      ctx.fillStyle = color;
      ctx.fill(path);
      ctx.globalAlpha = 1;
    };
    if (cam && dur > 0 && cols > 4) {
      try {
        cam.band(0, dur, cols, b.camMins, b.camMaxs);
        drawLane(cam, b.camMins, b.camMaxs, laneH / 2, CAMERA, () => true);
      } catch {
        /* band failure: baseline only */
      }
      if (ext && ext.coverage) {
        const [c0, c1] = ext.coverage;
        try {
          // The u-range is affine in t, so one band query stays column-aligned.
          ext.track.band(ext.mapping.u(0), ext.mapping.u(dur), cols, b.extMins, b.extMaxs);
          drawLane(ext.track, b.extMins, b.extMaxs, laneH + laneH / 2, EXTERNAL, (i) => {
            const t = ((i + 0.5) / cols) * dur;
            return t >= c0 && t <= c1;
          });
        } catch {
          /* band failure: baseline only */
        }
      }
    }
    if (dur > 0) {
      const x = Math.max(0, Math.min(w, (headT / dur) * w));
      const active = playingHere || scrubbing.current;
      ctx.fillStyle = `rgba(255,255,255,${active ? 0.9 : 0.6})`;
      ctx.fillRect(x - 0.75, 0, 1.5, h);
    }
  });

  const timeAt = (clientX: number, el: HTMLElement): number => {
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || dur <= 0) return 0;
    return (Math.min(Math.max(0, clientX - r.left), r.width) / r.width) * dur;
  };

  return (
    <canvas
      ref={canvasRef}
      className="pair-strip"
      aria-label="Waveform seek strip"
      onPointerDown={(e) => {
        if (dur <= 0) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        scrubbing.current = true;
        appModel.seekPreview(couple, timeAt(e.clientX, e.currentTarget));
      }}
      onPointerMove={(e) => {
        if (!scrubbing.current) return;
        appModel.seekPreview(couple, timeAt(e.clientX, e.currentTarget));
      }}
      onPointerUp={(e) => {
        if (!scrubbing.current) return;
        scrubbing.current = false;
        appModel.seekPreview(couple, timeAt(e.clientX, e.currentTarget));
        e.currentTarget.releasePointerCapture(e.pointerId);
      }}
    />
  );
}
