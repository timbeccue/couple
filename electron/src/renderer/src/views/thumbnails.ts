// thumbnails.ts — video poster frames for the couple rows and the media pool
// (AVAssetImageGenerator twin), shared by CoupleList and the pool rows.
import React from 'react';
import type { PoolEntry } from '../app/MediaPool';
import { mediaUrl as fileURL } from '../app/mediaUrl';

// ---------- pool: video thumbnails (AVAssetImageGenerator twin) ----------

// Cache per content key: a captured frame is stable for the file's lifetime, and
// fixture mpeg4 that never decodes caches `null` so we don't retry each render.
const thumbCache = new Map<string, string | null>();

/** Capture a frame from a video file via a hidden <video> + canvas. Resolves to a
 * data URL, or null when the codec can't decode (fixture mpeg4) — the caller then
 * shows the role glyph, never a broken image. */
function captureThumbnail(path: string): Promise<string | null> {
  return new Promise((resolve) => {
    const v = document.createElement('video');
    v.muted = true;
    v.preload = 'auto';
    // CORS-clean load (couple-media:// sends ACAO *): without it the frame
    // grab taints the canvas and toDataURL throws — glyph fallback forever.
    v.crossOrigin = 'anonymous';
    let settled = false;
    const finish = (url: string | null): void => {
      if (settled) return;
      settled = true;
      v.removeAttribute('src');
      v.load();
      resolve(url);
    };
    v.addEventListener('error', () => finish(null));
    v.addEventListener('loadeddata', () => {
      const grab = (): void => {
        try {
          const w = 64;
          const ratio = v.videoWidth > 0 ? v.videoHeight / v.videoWidth : 0.5625;
          const h = Math.max(1, Math.round(w * ratio));
          const c = document.createElement('canvas');
          c.width = w;
          c.height = h;
          const ctx = c.getContext('2d');
          if (!ctx) return finish(null);
          ctx.drawImage(v, 0, 0, w, h);
          finish(c.toDataURL('image/png'));
        } catch {
          finish(null);
        }
      };
      v.addEventListener('seeked', grab, { once: true });
      try {
        v.currentTime = Math.min(1.0, (v.duration || 2) / 2 || 1);
      } catch {
        grab();
      }
    });
    v.src = fileURL(path);
    // Guard: a file that never fires loadeddata/error (won't decode) falls back.
    setTimeout(() => finish(null), 4000);
  });
}

export function useThumbnail(entry: PoolEntry): string | null {
  const [url, setUrl] = React.useState<string | null>(() => thumbCache.get(entry.id) ?? null);
  React.useEffect(() => {
    if (entry.role !== 'video') return;
    if (thumbCache.has(entry.id)) {
      setUrl(thumbCache.get(entry.id) ?? null);
      return;
    }
    let cancelled = false;
    void captureThumbnail(entry.path)
      .then(async (u) => {
        // The first grab can lose its <video> decode slot to import/sync load
        // and time out; one retry after the rush is nearly always enough. Only
        // successes are cached — a cached null would pin the glyph forever.
        if (u === null && !cancelled) u = await captureThumbnail(entry.path);
        return u;
      })
      .then((u) => {
        if (u !== null) thumbCache.set(entry.id, u);
        if (!cancelled) setUrl(u);
      });
    return () => {
      cancelled = true;
    };
  }, [entry.id, entry.role, entry.path]);
  return url;
}

