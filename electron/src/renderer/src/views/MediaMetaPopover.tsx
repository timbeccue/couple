// MediaMetaPopover.tsx — the media-pool info popover (TODO-003); TS port of
// CoupleKit/MediaMeta.swift (the pure `MediaMetaCard.make` projection) + the
// MediaMetaPopover view from mac/Sources/Couple/Views/QueueRail.swift. The app
// probes a file once (`Engine.probe` → `MediaInfo`) and captures its on-disk size at
// import; this turns that already-acquired evidence into ordered, displayable rows.
// It performs NO I/O and NO probing — the popover just renders the projection.
//
// CONTRACT (mirrors the Swift invariants): STRUCTURED ONLY (every value comes from
// the probe or a filesystem scalar, never inferred from the filename); CLEAN OMISSION
// (an unavailable field produces NO row, so there are never dangling separators);
// STABLE FORMATTING (deterministic, locale-independent).
import React from 'react';
import type { PoolEntry } from '../app/MediaPool';
import type { AudioStreamInfo, MediaInfo, VideoStreamInfo } from '../kit/Wire';

export interface MediaMetaRow {
  label: string;
  value: string;
}

export interface MediaMetaCard {
  common: MediaMetaRow[];
  stream: MediaMetaRow[];
  /** Embedded-audio rows for a video (empty for audio files or silent video). */
  embeddedAudio: MediaMetaRow[];
}

// ---------- projection (MediaMetaCard.make port) ----------

export function makeMetaCard(args: {
  role: 'video' | 'audio';
  durationS: number;
  info: MediaInfo | null;
  fileSizeBytes: number | null;
  creationTimeUtc: string | null;
}): MediaMetaCard {
  const { role, durationS, info, fileSizeBytes, creationTimeUtc } = args;
  const common: MediaMetaRow[] = [];
  common.push({ label: 'Role', value: role === 'video' ? 'Video' : 'Audio' });
  if (durationS > 0) common.push({ label: 'Duration', value: formatDuration(durationS) });
  const created = formatCreationDate(creationTimeUtc);
  if (created) common.push({ label: 'Created', value: created });
  if (fileSizeBytes !== null) common.push({ label: 'Size', value: formatBytes(fileSizeBytes) });
  const path = nonEmpty(info?.path ?? null);
  if (path) common.push({ label: 'Path', value: path });

  const stream: MediaMetaRow[] = [];
  let embeddedAudio: MediaMetaRow[] = [];

  if (role === 'video') {
    if (info?.video) {
      stream.push(...videoStreamRows(info.container, info.video));
    } else {
      const container = nonEmpty(info?.container ?? null);
      if (container) stream.push({ label: 'Container', value: container });
    }
    embeddedAudio = embeddedAudioRows(info?.audio ?? []);
  } else {
    const primary = (info?.audio ?? [])[0];
    if (primary) {
      stream.push(...audioStreamRows(info?.container ?? null, primary));
    } else {
      const container = nonEmpty(info?.container ?? null);
      if (container) stream.push({ label: 'Container', value: container });
    }
  }

  return { common, stream, embeddedAudio };
}

function videoStreamRows(container: string | null, v: VideoStreamInfo): MediaMetaRow[] {
  const rows: MediaMetaRow[] = [];
  const cc = formatContainerCodec(container, v.codec);
  if (cc) rows.push({ label: 'Format', value: cc });
  if (v.width > 0 && v.height > 0) rows.push({ label: 'Dimensions', value: `${v.width}×${v.height}` });
  const fps = formatFps(v.avgFps, v.nominalFps);
  if (fps) rows.push({ label: 'Frame rate', value: fps });
  return rows;
}

function audioStreamRows(container: string | null, a: AudioStreamInfo): MediaMetaRow[] {
  const rows: MediaMetaRow[] = [];
  const cc = formatContainerCodec(container, a.codec);
  if (cc) rows.push({ label: 'Format', value: cc });
  if (a.sampleRate > 0) rows.push({ label: 'Sample rate', value: formatSampleRate(a.sampleRate) });
  if (a.channels > 0) rows.push({ label: 'Channels', value: formatChannels(a.channels) });
  if (a.bitsPerSample && a.bitsPerSample > 0) rows.push({ label: 'Bit depth', value: `${a.bitsPerSample}-bit` });
  return rows;
}

/** Compact embedded-audio summary for a video: one readable line per stream (codec ·
 * sample rate · channels · bit depth), only the known parts joined; numbered only
 * when there is more than one stream. */
function embeddedAudioRows(audio: AudioStreamInfo[]): MediaMetaRow[] {
  if (audio.length === 0) return [];
  const multiple = audio.length > 1;
  return audio.map((a, i) => {
    const parts: string[] = [];
    const codec = nonEmpty(a.codec);
    if (codec) parts.push(codec);
    if (a.sampleRate > 0) parts.push(formatSampleRate(a.sampleRate));
    if (a.channels > 0) parts.push(formatChannels(a.channels));
    if (a.bitsPerSample && a.bitsPerSample > 0) parts.push(`${a.bitsPerSample}-bit`);
    return {
      label: multiple ? `Audio ${i + 1}` : 'Audio',
      value: parts.length === 0 ? 'present' : parts.join(' · '),
    };
  });
}

// ---------- formatters (deterministic; no locale leakage) ----------

/** `h:mm:ss` / `m:ss`, matching the pool's own duration format. */
export function formatDuration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  if (s >= 3600) {
    return `${Math.floor(s / 3600)}:${pad2(Math.floor((s % 3600) / 60))}:${pad2(s % 60)}`;
  }
  return `${Math.floor(s / 60)}:${pad2(s % 60)}`;
}

/** File-style byte count (base 1000, matching ByteCountFormatter.countStyle .file). */
export function formatBytes(bytes: number): string {
  if (bytes < 1000) return `${bytes} bytes`;
  const units = ['KB', 'MB', 'GB', 'TB', 'PB'];
  let value = bytes / 1000;
  let u = 0;
  while (value >= 1000 && u < units.length - 1) {
    value /= 1000;
    u += 1;
  }
  // One decimal below 100, none above — the readable convention ByteCountFormatter uses.
  const rounded = value >= 100 ? Math.round(value).toString() : (Math.round(value * 10) / 10).toString();
  return `${rounded} ${units[u]}`;
}

/** kHz when it divides cleanly (44.1 kHz, 48 kHz), Hz-derived otherwise. */
export function formatSampleRate(hz: number): string {
  const khz = hz / 1000;
  if (khz === Math.round(khz)) return `${Math.round(khz)} kHz`;
  return `${khz.toFixed(1)} kHz`;
}

export function formatChannels(ch: number): string {
  if (ch === 1) return 'mono';
  if (ch === 2) return 'stereo';
  return `${ch} ch`;
}

/** Average fps when usable, else nominal; `null` when neither is. Integer rates
 * render without a decimal; fractional rates keep up to three places. */
export function formatFps(avg: number | null, nominal: number | null): string | null {
  const chosen = avg !== null && Number.isFinite(avg) && avg > 0 ? avg : nominal;
  if (chosen === null || !Number.isFinite(chosen) || chosen <= 0) return null;
  if (chosen === Math.round(chosen)) return `${Math.round(chosen)} fps`;
  let s = chosen.toFixed(3);
  while (s.endsWith('0')) s = s.slice(0, -1);
  if (s.endsWith('.')) s = s.slice(0, -1);
  return `${s} fps`;
}

/** `CODEC (container)` when both known, one when the other is blank, `null` when both
 * are — avoids dangling parens. */
export function formatContainerCodec(container: string | null, codec: string | null): string | null {
  const cont = nonEmpty(container);
  const cod = nonEmpty(codec);
  if (cod && cont) return `${cod} (${cont})`;
  if (cod) return cod;
  if (cont) return cont;
  return null;
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** Parse the probe's UTC creation timestamp and render it in a stable, readable
 * form (`MMM d, yyyy 'at' h:mm a`, matching the mac shell). `null` when absent or
 * unparseable — never a fabricated value. */
export function formatCreationDate(utc: string | null): string | null {
  const raw = nonEmpty(utc);
  if (!raw) return null;
  const ms = Date.parse(raw);
  if (Number.isNaN(ms)) return null;
  const d = new Date(ms);
  let h = d.getHours();
  const ampm = h >= 12 ? 'PM' : 'AM';
  h = h % 12;
  if (h === 0) h = 12;
  return `${MONTHS[d.getMonth()]} ${d.getDate()}, ${d.getFullYear()} at ${h}:${pad2(d.getMinutes())} ${ampm}`;
}

function nonEmpty(s: string | null): string | null {
  if (s === null) return null;
  const t = s.trim();
  return t.length === 0 ? null : t;
}

function pad2(n: number): string {
  return String(n).padStart(2, '0');
}

// ---------- view ----------

function MetaGroup({ rows }: { rows: MediaMetaRow[] }): React.JSX.Element {
  return (
    <div className="meta-group">
      {rows.map((r) => (
        <div className="meta-row" key={r.label}>
          <span className="meta-label">{r.label}</span>
          <span className="meta-value">{r.value}</span>
        </div>
      ))}
    </div>
  );
}

/** The compact metadata popover body (TODO-003): the filename header, a common block
 * (role/duration/created/size/path), the audio- or video-stream technical block, and
 * — for video — an embedded-audio summary. Every group omits missing fields. */
export function MediaMetaPopover({ entry }: { entry: PoolEntry }): React.JSX.Element {
  const card = makeMetaCard({
    role: entry.role,
    durationS: entry.durationS,
    info: entry.info,
    fileSizeBytes: entry.fileSizeBytes,
    creationTimeUtc: entry.creationTimeUtc,
  });
  return (
    <div className="meta-popover">
      <div className="meta-title" title={entry.name}>
        {entry.name}
      </div>
      <MetaGroup rows={card.common} />
      {card.stream.length > 0 && <div className="meta-divider" />}
      {card.stream.length > 0 && <MetaGroup rows={card.stream} />}
      {card.embeddedAudio.length > 0 && <div className="meta-divider" />}
      {card.embeddedAudio.length > 0 && <MetaGroup rows={card.embeddedAudio} />}
    </div>
  );
}
