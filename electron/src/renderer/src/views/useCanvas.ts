// useCanvas — a devicePixelRatio-correct Canvas-2D paint hook shared by the
// editor views (WaveformStage, SparklineView). The backing store is sized from
// the canvas's laid-out CSS box × dpr and the context is pre-scaled, so every
// `draw` works in CSS pixels and lines stay crisp (owner cares about crisp
// rendering). A ResizeObserver repaints on layout changes; callers repaint on
// state changes by depending on the model via useModel (each render re-runs the
// layout effect below).
import { useCallback, useEffect, useLayoutEffect, useRef, type RefObject } from 'react';

export type CanvasDraw = (ctx: CanvasRenderingContext2D, cssW: number, cssH: number) => void;

export function useCanvas(draw: CanvasDraw): RefObject<HTMLCanvasElement | null> {
  const ref = useRef<HTMLCanvasElement>(null);
  const drawRef = useRef(draw);
  drawRef.current = draw;

  const paint = useCallback(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    const cssW = Math.max(1, Math.round(rect.width));
    const cssH = Math.max(1, Math.round(rect.height));
    const dpr = window.devicePixelRatio || 1;
    const bw = Math.round(cssW * dpr);
    const bh = Math.round(cssH * dpr);
    if (canvas.width !== bw || canvas.height !== bh) {
      canvas.width = bw;
      canvas.height = bh;
    }
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    drawRef.current(ctx, cssW, cssH);
  }, []);

  // Repaint after every render (state changes flow through here).
  useLayoutEffect(() => {
    paint();
  });

  // Repaint on layout/size changes.
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const ro = new ResizeObserver(() => paint());
    ro.observe(canvas);
    return () => ro.disconnect();
  }, [paint]);

  return ref;
}
