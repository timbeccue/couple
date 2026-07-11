// QueueRail.tsx — left rail (QueueRail.swift port): the couples list in the three
// live workflow sections + the media pool. The pool is a bottom-pinned box
// (redesign 2026-06-15): a fixed header, a bounded inner scroll of Finder-style
// selectable rows, and a STICKY footer preview player that never scrolls, so the box
// height stays constant. Selection/preview/removal decisions come from the core
// policies via appModel.poolController (PLAN-CORE-SHELL-BOUNDARY §1); this only
// presents them.
import React from 'react';
import { appModel } from '../app/AppModel';
import type { CoupleRow } from '../app/CoupleRow';
import type { PoolEntry } from '../app/MediaPool';
import { useModel } from '../app/observable';
import type { PoolSelectModifier } from '../app/PoolController';
import { poolListHeight } from '../app/PoolController';
import { env } from '../kit/native';
import { MediaMetaPopover } from './MediaMetaPopover';
import { useCanvas } from './useCanvas';
import './pool.css';

import { mediaUrl as fileURL } from '../app/mediaUrl';

const POOL_DRAG_MIME = 'application/x-couple-pool-key';
const CAMERA = '#2ec8c2';
const EXTERNAL = '#e0a63c';
const MAX_SEEK_COLS = 1024;

function fmtDuration(s: number): string {
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${String(sec).padStart(2, '0')}`;
}

function fmtClock(s: number): string {
  const t = Math.max(0, Math.floor(s));
  return `${Math.floor(t / 60)}:${String(t % 60).padStart(2, '0')}`;
}

// ---------- couples sections ----------

function VerdictDot({ couple }: { couple: CoupleRow }): React.JSX.Element {
  useModel(couple);
  const cls = couple.pair.syncStatus === 'syncing' ? 'syncing' : couple.verdict;
  return <span className={`verdict-dot ${cls}`} />;
}

function CoupleItem({ couple }: { couple: CoupleRow }): React.JSX.Element {
  const model = useModel(appModel);
  useModel(couple);
  const [dropHover, setDropHover] = React.useState(false);
  const thumb = useThumbnail(couple.video);
  // Second line = the PAIRING (the row's whole meaning): selected audio name,
  // or the action word / chip when nothing is selected yet.
  const audioName = couple.selectedCandidate?.audioName ?? null;
  return (
    <div
      className={`couple-item${model.selectedID === couple.id ? ' selected' : ''}${dropHover ? ' drop-hover' : ''}`}
      onClick={() => appModel.select(couple.id)}
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes(POOL_DRAG_MIME)) {
          e.preventDefault();
          e.stopPropagation();
          setDropHover(true);
        }
      }}
      onDragLeave={() => setDropHover(false)}
      onDrop={(e) => {
        const key = e.dataTransfer.getData(POOL_DRAG_MIME);
        setDropHover(false);
        if (!key) return;
        e.preventDefault();
        e.stopPropagation();
        appModel.select(couple.id);
        appModel.repairDrop(couple, key);
      }}
    >
      <span className="couple-thumb">
        {thumb ? <img src={thumb} alt="" draggable={false} /> : <span className="couple-thumb-glyph">V</span>}
      </span>
      <span className="couple-lines">
        <span className="couple-name" title={couple.video.name}>
          {couple.video.name}
        </span>
        <span className={`couple-audio${audioName ? '' : ' needs-action'}`}>
          {audioName ? (
            <>
              <span className="pair-x">×</span> {audioName}
            </>
          ) : (
            (couple.actionWord ?? couple.chip)
          )}
        </span>
      </span>
      {couple.actionWord && audioName && <span className="action-word">{couple.actionWord}</span>}
      <span className="couple-duration">{fmtDuration(couple.video.durationS)}</span>
      <VerdictDot couple={couple} />
    </div>
  );
}

function Section({ title, couples }: { title: string; couples: CoupleRow[] }): React.JSX.Element {
  return (
    <div className={couples.length === 0 ? 'section-empty' : undefined}>
      <div className="section-label">
        {title} · {couples.length}
      </div>
      {couples.length === 0 ? (
        <div className="section-dash">—</div>
      ) : (
        couples.map((c) => <CoupleItem key={c.id} couple={c} />)
      )}
    </div>
  );
}

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

function useThumbnail(entry: PoolEntry): string | null {
  const [url, setUrl] = React.useState<string | null>(() => thumbCache.get(entry.id) ?? null);
  React.useEffect(() => {
    if (entry.role !== 'video') return;
    if (thumbCache.has(entry.id)) {
      setUrl(thumbCache.get(entry.id) ?? null);
      return;
    }
    let cancelled = false;
    void captureThumbnail(entry.path).then((u) => {
      thumbCache.set(entry.id, u);
      if (!cancelled) setUrl(u);
    });
    return () => {
      cancelled = true;
    };
  }, [entry.id, entry.role, entry.path]);
  return url;
}

// ---------- pool: one row ----------

function modifierOf(e: React.MouseEvent): PoolSelectModifier {
  if (e.shiftKey) return 'shift';
  if (e.metaKey || e.ctrlKey) return 'command';
  return 'plain';
}

/** Live usage state (§V2.7), kept concise (TODO-003): a state word or a couple count
 * — never a list of filenames (that detail lives in the popover). */
function usageWord(entry: PoolEntry): string {
  const couples = appModel.couples;
  if (entry.role === 'video') {
    const c = couples.find((x) => x.id === entry.id);
    return c ? c.chip : 'video';
  }
  const usedBy = couples.filter((c) => c.selectedAudioKey === entry.id).length;
  if (usedBy > 0) return `used by ${usedBy} couple${usedBy === 1 ? '' : 's'}`;
  const altOf = couples.filter((c) => c.alternates.some((a) => a.audioKey === entry.id)).length;
  if (altOf > 0) return `alternate for ${altOf} couple${altOf === 1 ? '' : 's'}`;
  return 'unused';
}

function PoolRow({ entry }: { entry: PoolEntry }): React.JSX.Element {
  const model = useModel(appModel);
  const pool = model.poolController;
  const thumb = useThumbnail(entry);
  const selected = pool.poolSelection.has(entry.id);
  const isPrimary = pool.poolPrimary === entry.id;

  const dur = entry.durationS > 0 ? fmtDuration(entry.durationS) : null;
  const secondLine = [dur, usageWord(entry)].filter((p) => p !== null).join(' · ');

  return (
    <div
      className={`pool-row${selected ? ' selected' : ''}${isPrimary ? ' primary' : ''}`}
      title={entry.path}
      draggable={entry.role === 'audio'}
      onClick={(e) => pool.selectPool(entry.id, modifierOf(e))}
      onDragStart={(ev) => {
        if (entry.role !== 'audio') return;
        ev.dataTransfer.setData(POOL_DRAG_MIME, entry.id);
        ev.dataTransfer.effectAllowed = 'link';
      }}
    >
      {entry.role === 'video' && thumb ? (
        <img className="pool-thumb" src={thumb} alt="" draggable={false} />
      ) : (
        <span
          className="pool-dot"
          style={{ background: entry.role === 'video' ? CAMERA : EXTERNAL }}
        />
      )}
      <div className="pool-row-main">
        <span className="pool-row-name">{entry.name}</span>
        <span className="pool-row-sub">{secondLine}</span>
      </div>
    </div>
  );
}

// ---------- pool: sticky preview bar ----------

interface BandBufs {
  mins: Int8Array;
  maxs: Int8Array;
}

function SeekStrip({ entry, duration }: { entry: PoolEntry; duration: number }): React.JSX.Element {
  const model = useModel(appModel);
  const preview = model.poolController.preview;
  const bufRef = React.useRef<BandBufs | null>(null);
  if (bufRef.current === null) {
    bufRef.current = { mins: new Int8Array(MAX_SEEK_COLS), maxs: new Int8Array(MAX_SEEK_COLS) };
  }
  const buf = bufRef.current;
  const scrubbing = React.useRef(false);

  const t = preview.state.currentTime;
  const active = preview.state.isPlaying || scrubbing.current;

  const canvasRef = useCanvas((ctx, w, h) => {
    // Clear the whole (dpr-scaled) box before every repaint: useCanvas only wipes
    // the backing store when the CSS box RESIZES, so same-size redraws — every
    // playback tick and every seek — must clear or the moving playhead and the
    // alpha-blended played span accumulate into stacked ghost copies. Transparent
    // clear lets the sticky bar's own background show through.
    ctx.clearRect(0, 0, w, h);
    const midY = h / 2;
    const color = entry.role === 'video' ? CAMERA : EXTERNAL;
    const cols = Math.min(Math.floor(w), MAX_SEEK_COLS);
    const handle = entry.handle;
    let filled = false;
    if (duration > 0 && cols > 4 && handle) {
      try {
        handle.band(0, duration, cols, buf.mins, buf.maxs);
        filled = true;
      } catch {
        filled = false;
      }
    }
    if (!filled) {
      // No envelope (handle mid-import / band error): an honest flat baseline.
      ctx.strokeStyle = color;
      ctx.globalAlpha = 0.3;
      ctx.beginPath();
      ctx.moveTo(0, midY);
      ctx.lineTo(w, midY);
      ctx.stroke();
      ctx.globalAlpha = 1;
      return;
    }
    // Same normalization as the overview lanes (UX §4.1): gain/2 for min/max spans.
    const gain = 1.44 / Math.max(handle!.meta.stats.activeP959600, 1e-6);
    const scale = (0.5 * gain * handle!.meta.normalization) / 127.0;
    const colW = w / cols;
    const halfH = midY - 1;
    const playX = duration > 0 ? (t / duration) * w : 0;
    const played = new Path2D();
    const rest = new Path2D();
    for (let i = 0; i < cols; i++) {
      const vMin = Math.max(-1, Math.min(1, buf.mins[i] * scale));
      const vMax = Math.max(-1, Math.min(1, buf.maxs[i] * scale));
      if (vMax - vMin <= 1e-4) continue;
      const x = i * colW;
      const yTop = midY - vMax * halfH;
      const yBot = midY - vMin * halfH;
      const rect: [number, number, number, number] = [x, yTop, colW, Math.max(yBot - yTop, 0.7)];
      (x + colW <= playX ? played : rest).rect(...rect);
    }
    ctx.fillStyle = color;
    ctx.globalAlpha = 0.35;
    ctx.fill(rest);
    ctx.globalAlpha = 0.9;
    ctx.fill(played);
    ctx.globalAlpha = 1;
    // Playhead: bright while playing/scrubbing, grey when parked.
    ctx.strokeStyle = active ? 'rgba(255,255,255,0.85)' : 'rgba(255,255,255,0.5)';
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(playX, 0);
    ctx.lineTo(playX, h);
    ctx.stroke();
  });

  const timeAtX = (clientX: number, el: HTMLElement): number => {
    const rect = el.getBoundingClientRect();
    if (rect.width <= 0 || duration <= 0) return 0;
    const x = Math.min(Math.max(0, clientX - rect.left), rect.width);
    return (x / rect.width) * duration;
  };

  return (
    <canvas
      ref={canvasRef}
      className="seek-strip"
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId);
        scrubbing.current = true;
        preview.scrubBegin();
        preview.scrubTo(timeAtX(e.clientX, e.currentTarget));
      }}
      onPointerMove={(e) => {
        if (!scrubbing.current) return;
        preview.scrubTo(timeAtX(e.clientX, e.currentTarget));
      }}
      onPointerUp={(e) => {
        if (!scrubbing.current) return;
        preview.scrubTo(timeAtX(e.clientX, e.currentTarget));
        preview.scrubEnd();
        scrubbing.current = false;
        e.currentTarget.releasePointerCapture(e.pointerId);
      }}
    />
  );
}

function StickyBar(): React.JSX.Element {
  const model = useModel(appModel);
  const pool = model.poolController;
  const preview = pool.preview;
  // COUPLE_POOL_INFO_TEST opens the popover by default so a headless run can
  // screenshot the metadata projection (TODO-003).
  const [infoShown, setInfoShown] = React.useState(() => !!env.COUPLE_POOL_INFO_TEST);
  const primary = pool.poolPrimaryEntry;

  React.useEffect(() => {
    if (!primary) setInfoShown(false);
    else if (env.COUPLE_POOL_INFO_TEST) setInfoShown(true);
  }, [primary]);

  if (!primary) {
    return (
      <div className="sticky-bar">
        <div className="sticky-placeholder">
          <span className="sticky-play-glyph">▶</span>
          <span>Select a file to preview</span>
        </div>
      </div>
    );
  }

  // The strip domain is the PROBE duration (authoritative), matching the mac shell.
  // The element's own duration is under-reported with chunked serving, so it is only
  // a defensive fallback when the probe gave nothing.
  const duration = primary.durationS > 0 ? primary.durationS : Math.max(preview.state.duration, 0.01);
  const isPlaying = preview.state.isPlaying;

  return (
    <div className="sticky-bar">
      <button
        className={`play-btn${isPlaying ? ' playing' : ''}`}
        title={isPlaying ? 'Pause preview' : 'Play preview'}
        onClick={() => preview.toggle(primary.id, primary.path, primary.durationS)}
      >
        {isPlaying ? '❚❚' : '▶'}
      </button>
      <span className="seek-time">{fmtClock(preview.state.currentTime)}</span>
      <SeekStrip entry={primary} duration={duration} />
      <span className="seek-time right">{fmtClock(duration)}</span>
      <div className="info-wrap">
        <button className="info-btn" title="Show file details" onClick={() => setInfoShown((s) => !s)}>
          ⓘ
        </button>
        {infoShown && (
          <>
            <div className="info-backdrop" onClick={() => setInfoShown(false)} />
            <MediaMetaPopover entry={primary} />
          </>
        )}
      </div>
    </div>
  );
}

// ---------- pool: bottom-pinned box ----------

/** Pool-list height budget (SPEC-CHANGE 2026-07-01): the list grows into rail height
 * the couples list leaves unused, bounded by its own content, never below the floor.
 * Measures the two scroll regions (their viewport sum is invariant under the split,
 * so `poolListHeight` converges) and returns the pool list's max-height. */
function useRailLayout(): {
  couplesRef: React.RefObject<HTMLDivElement | null>;
  listRef: React.RefObject<HTMLDivElement | null>;
  poolMax: number;
} {
  const couplesRef = React.useRef<HTMLDivElement>(null);
  const listRef = React.useRef<HTMLDivElement>(null);
  const [poolMax, setPoolMax] = React.useState(320);

  const recompute = React.useCallback(() => {
    const couples = couplesRef.current;
    const list = listRef.current;
    if (!couples || !list) return;
    const available = couples.clientHeight + list.clientHeight;
    const next = poolListHeight(available, couples.scrollHeight, list.scrollHeight);
    setPoolMax((prev) => (Math.abs(prev - next) > 0.5 ? next : prev));
  }, []);

  React.useLayoutEffect(() => {
    recompute();
  });

  React.useEffect(() => {
    const couples = couplesRef.current;
    const list = listRef.current;
    if (!couples || !list) return;
    const ro = new ResizeObserver(() => recompute());
    ro.observe(couples);
    ro.observe(list);
    return () => ro.disconnect();
  }, [recompute]);

  return { couplesRef, listRef, poolMax };
}

function PoolBox({
  listRef,
  poolMax,
}: {
  listRef: React.RefObject<HTMLDivElement | null>;
  poolMax: number;
}): React.JSX.Element {
  const model = useModel(appModel);
  const pool = model.poolController;
  const [collapsed, setCollapsed] = React.useState(false);
  const count = model.pool.entries.length;

  return (
    <div className="pool-box">
      <div className="pool-header" onClick={() => setCollapsed((c) => !c)}>
        <span className="pool-chevron">{collapsed ? '▸' : '▾'}</span>
        <span className="pool-title">MEDIA POOL · {count}</span>
        <span className="pool-spacer" />
        {pool.poolSelection.size > 0 && (
          <button
            className="pool-trash"
            title={`Remove the selected file${pool.poolSelection.size === 1 ? '' : 's'} from this session (does not delete from disk) — Delete`}
            onClick={(e) => {
              e.stopPropagation();
              pool.removeSelectedMedia();
            }}
          >
            🗑
          </button>
        )}
      </div>
      {!collapsed && (
        <>
          <div className="pool-list" ref={listRef} style={{ maxHeight: poolMax }}>
            {model.pool.entries.map((entry) => (
              <PoolRow key={entry.id} entry={entry} />
            ))}
          </div>
          <StickyBar />
        </>
      )}
    </div>
  );
}

// ---------- rail ----------

export function QueueRail(): React.JSX.Element {
  const model = useModel(appModel);
  useModel(model.pool);
  useModel(model.poolController);
  const { couplesRef, listRef, poolMax } = useRailLayout();

  // Delete/Backspace removes the pool selection (TODO-001) — the Delete-key twin of
  // the trash button. Scoped to the rail: it only acts when the pool has a selection
  // and no text field is focused, so it never fights typing elsewhere.
  React.useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Delete' && e.key !== 'Backspace') return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
      if (model.poolController.poolSelection.size === 0) return;
      e.preventDefault();
      model.poolController.removeSelectedMedia();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [model]);

  // COUPLE_REMOVE_TEST (headless verification): once a pool audio exists and ingest
  // settles, select it and run the removal path through PoolController, which logs
  // the policy plan + resulting pool/couple counts.
  const removeTested = React.useRef(false);
  React.useEffect(() => {
    if (removeTested.current || !env.COUPLE_REMOVE_TEST) return;
    const audio = model.pool.audios[0];
    if (!audio || model.ingest.ingestInFlight) return;
    removeTested.current = true;
    model.poolController.selectPool(audio.id, 'plain');
    model.poolController.debugRemove(new Set([audio.id]));
  });

  // COUPLE_POOL_PREVIEW_TEST (headless verification): once the pool has entries,
  // make the video the SELECTED primary so the sticky bar shows a loaded player
  // (play button + waveform seek strip + info glyph) instead of the placeholder.
  const previewSelected = React.useRef(false);
  React.useEffect(() => {
    if (previewSelected.current || !env.COUPLE_POOL_PREVIEW_TEST) return;
    const video = model.pool.videos[0];
    if (!video || model.ingest.ingestInFlight) return;
    previewSelected.current = true;
    model.poolController.selectPool(video.id, 'plain');
  });

  // COUPLE_POOL_SEEK_TEST (seek-behavior verification): select an audio, play 1s,
  // seek to 60% of the PROBE duration, play 1s — logging element duration/readyState
  // and expected-vs-actual currentTime after each step so the harness can assert.
  const seekTested = React.useRef(false);
  React.useEffect(() => {
    if (seekTested.current || !env.COUPLE_POOL_SEEK_TEST) return;
    const audio = model.pool.audios[0];
    if (!audio || model.ingest.ingestInFlight) return;
    seekTested.current = true;
    const pc = model.poolController;
    const preview = pc.preview;
    const dom = audio.durationS; // strip domain = probe duration
    const log = (tag: string, expect: number): void => {
      const s = preview.debugSnapshot();
      // eslint-disable-next-line no-console
      console.log(
        `[seek-test] ${tag} elDur=${s.elDuration.toFixed?.(2)} elReady=${s.elReady} stDur=${s.stDuration.toFixed(2)} expect=${expect.toFixed(2)} actual=${s.stCurrent.toFixed(2)} el=${s.elCurrent.toFixed(2)}`
      );
    };
    void (async () => {
      const wait = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));
      pc.selectPool(audio.id, 'plain');
      await wait(400);
      preview.toggle(audio.id, audio.path, audio.durationS); // play from 0
      await wait(1000);
      log('after-play-1s', 1.0);
      const target = 0.6 * dom;
      preview.scrubBegin();
      preview.scrubTo(target);
      preview.scrubEnd();
      await wait(200);
      log('after-seek', target);
      await wait(1000);
      log('after-seek-play-1s', target + 1.0);
      // eslint-disable-next-line no-console
      console.log(`[seek-test] domain(probe)=${dom.toFixed(2)}`);
    })();
  });

  // COUPLE_POOL_GHOST_TEST (seek-strip repaint verification): select an audio and
  // fire ~10 scattered seeks so a screenshot reveals any canvas ghosting (stale
  // playhead/waveform copies left by a missing clear before each redraw).
  const ghostTested = React.useRef(false);
  React.useEffect(() => {
    if (ghostTested.current || !env.COUPLE_POOL_GHOST_TEST) return;
    const audio = model.pool.audios[0];
    if (!audio || model.ingest.ingestInFlight) return;
    ghostTested.current = true;
    const preview = model.poolController.preview;
    model.poolController.selectPool(audio.id, 'plain');
    const dom = audio.durationS;
    const fracs = [0.1, 0.85, 0.3, 0.65, 0.2, 0.9, 0.45, 0.75, 0.15, 0.55];
    void (async () => {
      const wait = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));
      await wait(400);
      for (const f of fracs) {
        preview.scrubBegin();
        preview.scrubTo(f * dom);
        preview.scrubEnd();
        await wait(120);
      }
    })();
  });

  return (
    <div className="queue-rail">
      <div className="rail-couples" ref={couplesRef}>
        <Section title="QUEUED FOR EXPORT" couples={model.queued} />
        <Section title="TO APPROVE" couples={model.toApprove} />
        <Section title="TO FIX" couples={model.toFix} />
      </div>
      <PoolBox listRef={listRef} poolMax={poolMax} />
    </div>
  );
}
