// CoupleList.tsx — the two workflow sections (UX §V2.11 / §V2.12; port of
// CoupleList.swift): TO FIX (hidden when empty, D11) above READY (always shown,
// dimmed "—" when empty), and the couple rows. One view for BOTH arrangements: the
// full window's left rail and the compact arrangement's flexible middle block.
// Only the rows scroll; selecting a row previews it and never moves the list
// (stable keys keep the scroll offset across redraws). Each row is a
// drag-to-repair drop target (§V2.6); the include-for-export checkbox lives on
// includable rows only (D1).
import React from 'react';
import { appModel } from '../app/AppModel';
import type { CoupleRow } from '../app/CoupleRow';
import { useModel } from '../app/observable';
import { POOL_DRAG_MIME } from './QueueRail';
import { useThumbnail } from './thumbnails';

function fmtDuration(s: number): string {
  const t = Math.max(0, Math.floor(s));
  const h = Math.floor(t / 3600);
  const m = Math.floor((t % 3600) / 60);
  const sec = t % 60;
  return h > 0
    ? `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}`
    : `${m}:${String(sec).padStart(2, '0')}`;
}

/** "HH:mm" wall-clock time of an ISO stamp — the "✓ <time>" export mark (D8). */
export function fmtClock(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/** The "pick one" / "no audio" / "forced" capsule (verdict colors stay here, D3). */
export function ActionWord({ couple }: { couple: CoupleRow }): React.JSX.Element | null {
  const word = couple.actionWord;
  if (word === null) return null;
  return <span className={`action-word ${couple.verdict}`}>{word}</span>;
}

function CoupleItem({ couple }: { couple: CoupleRow }): React.JSX.Element {
  const model = useModel(appModel);
  useModel(couple);
  useModel(couple.pair);
  const [dropHover, setDropHover] = React.useState(false);
  const thumb = useThumbnail(couple.video);
  const selected = model.selectedID === couple.id;
  const audioName = couple.selectedCandidate?.audioName ?? null;

  // UX §V2.12 / D3: no verdict dot, spine, wash or coverage strip on rows. The
  // verdict still decides the section and the action word; coverage is drawn in
  // the preview's waveform strip.
  return (
    <div
      className={`couple-item${selected ? ' selected' : ''}${dropHover ? ' drop-hover' : ''}`}
      data-couple-id={couple.id}
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
        // Drag an audio file from the pool onto a couple to re-pair (§V2.6): the
        // shared repair-drop path — no-op on the current source, replaces at once
        // when unpinned, asks before replacing a PINNED couple's pick (D9).
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
        <span className="couple-title">
          <span className="couple-name" title={couple.video.name}>
            {couple.video.name}
          </span>
          {couple.video.durationS > 0 && <span className="couple-dur">{fmtDuration(couple.video.durationS)}</span>}
        </span>
        {/* Second line: "syncing…" while this couple's auto-sync runs, else the
            action word when action is needed, else the chosen audio's name. */}
        {couple.isSyncRunning ? (
          <span className="couple-audio syncing">syncing…</span>
        ) : couple.actionWord !== null ? (
          <span className="couple-audio">
            <ActionWord couple={couple} />
          </span>
        ) : (
          <span className="couple-audio" title={audioName ?? undefined}>
            {audioName ?? '—'}
          </span>
        )}
      </span>
      <span className="couple-marks">
        {couple.pinned && (
          <span className="couple-pin" title="Audio chosen by you — a re-pair never replaces it">
            📌
          </span>
        )}
        {couple.exportedAt !== null && (
          <span className="exported-stamp" title={`Exported at ${fmtClock(couple.exportedAt)}`}>
            ✓ {fmtClock(couple.exportedAt)}
          </span>
        )}
        {/* D1: the include checkbox is offered on includable rows only (verdict good). */}
        {couple.canInclude && (
          <button
            className={`couple-include${couple.included ? ' on' : ''}`}
            role="checkbox"
            aria-checked={couple.included}
            title={
              couple.included ? 'Included in the export — click to leave it out' : 'Not in the export — click to include it'
            }
            onClick={(e) => {
              e.stopPropagation();
              appModel.toggleInclude(couple); // undoable (⌘Z); no key binding (D4)
            }}
          >
            {couple.included ? '☑' : '☐'}
          </button>
        )}
      </span>
    </div>
  );
}

function Section({ title, couples }: { title: string; couples: CoupleRow[] }): React.JSX.Element {
  return (
    <div className={`queue-section${couples.length === 0 ? ' section-empty' : ''}`}>
      <div className="section-label">
        <span>{title}</span>
        <span className="section-count">{couples.length}</span>
      </div>
      {couples.length === 0 ? (
        <div className="section-dash">—</div>
      ) : (
        couples.map((c) => <CoupleItem key={c.id} couple={c} />)
      )}
    </div>
  );
}

export function CoupleList({
  scrollRef,
  scrolls = true,
}: {
  scrollRef?: React.RefObject<HTMLDivElement | null>;
  /** False = the sections at their natural height (the compact empty state). */
  scrolls?: boolean;
}): React.JSX.Element {
  const model = useModel(appModel);
  return (
    <div className={`couple-list${scrolls ? ' scrolls' : ''}`} ref={scrollRef}>
      {/* D2/D11: To fix above Ready; an empty To fix is hidden, Ready always shows. */}
      {model.toFix.length > 0 && <Section title="TO FIX" couples={model.toFix} />}
      <Section title="READY" couples={model.ready} />
    </div>
  );
}
