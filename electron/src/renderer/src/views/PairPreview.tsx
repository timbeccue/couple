// PairPreview.tsx — the selected couple's preview block (UX §V2.11; port of
// PairPreview.swift), one view for BOTH arrangements:
//  - title line (video name + chosen audio / action word; "Change…" after a
//    manual pick);
//  - the video hero with the two-lane PairStrip seek surface between the time
//    labels, and the monitor picker — 4-way in full, Camera · Synced · External
//    in compact (D10);
//  - the tertiary line (compact: "offset · fit"; both: the candidate-audition
//    readout or "Audio chosen by you · Undo · Compare others");
//  - the inline candidate list (ResolveBanner) for a "pick one" couple or a
//    reopened "Compare others"; in compact a "no audio" couple gets the one line
//    "No matching audio in this session · Choose file…" instead.
// Full: the hero is greedy (the overview/loupes/bottom bar below are the editor's).
// Compact: the block is fixed (16:9 player) and never scrolls with the list. An
// empty session shows the import surface in the hero slot.
import React from 'react';
import { appModel } from '../app/AppModel';
import type { CoupleRow } from '../app/CoupleRow';
import { useModel } from '../app/observable';
import type { RowCandidate } from '../kit/Wire';
import { ActionWord } from './CoupleList';
import { LoadSurface } from './LoadSurface';
import { VideoHero } from './VideoHero';

const { ipcRenderer } = (window as unknown as { require: NodeRequire }).require(
  'electron'
) as typeof import('electron');

/** A manual pick (📌 with an audio): the banner folds into the "Audio chosen by
 * you" line until Compare others / Change… reopens it. */
function isManualPick(couple: CoupleRow): boolean {
  return couple.pinned && couple.selectedAudioKey !== null;
}

function signed(v: number, digits: number): string {
  return `${v >= 0 ? '+' : '−'}${Math.abs(v).toFixed(digits)}`;
}

/** "▶ ZOOM0027.WAV at best fit · offset +0.38 s · fit 5.1 ms" (fit only when the
 * candidate carries fine-verify telemetry). */
export function auditionReadout(c: RowCandidate): string {
  let s = `▶ ${c.audioName} at best fit · offset ${signed(c.lagMs / 1000, 2)} s`;
  if (typeof c.fitRmsMs === 'number') s += ` · fit ${c.fitRmsMs.toFixed(1)} ms`;
  return s;
}

export function PairPreview({ compact }: { compact: boolean }): React.JSX.Element {
  const model = useModel(appModel);
  useModel(model.pool);
  const couple = model.selected;

  if (!model.hasContent) {
    // Empty session (revised 2026-07-11): the import surface takes the hero slot.
    return (
      <div className="pair-preview empty">
        <LoadSurface compact={compact} />
      </div>
    );
  }
  if (!couple) {
    return (
      <div className="pair-preview">
        <div className="center-empty">Select a couple from the list</div>
      </div>
    );
  }
  return <PreviewBody couple={couple} compact={compact} />;
}

function PreviewBody({ couple, compact }: { couple: CoupleRow; compact: boolean }): React.JSX.Element {
  const model = useModel(appModel);
  useModel(couple);
  const pair = useModel(couple.pair);
  const manual = isManualPick(couple);
  const comparing = model.compareCoupleID === couple.id && manual;
  const auditioning = model.candidateAudition?.coupleID === couple.id;
  const audio = couple.selectedCandidate?.audioName ?? null;

  return (
    <div className={`pair-preview${compact ? ' compact' : ''}`}>
      <div className="preview-title">
        {!compact && <span className={`verdict-dot ${couple.verdict}`} />}
        <h2 title={couple.video.name}>{couple.video.name}</h2>
        {audio !== null && (
          <>
            <span className="preview-join">{compact ? '+' : '×'}</span>
            <span className={`preview-audio${compact ? '' : ' external'}`} title={audio}>
              {audio}
            </span>
          </>
        )}
        {couple.actionWord !== null &&
          (compact ? (
            <ActionWord couple={couple} />
          ) : (
            <span className={`action-chip ${couple.verdict}`}>{couple.actionWord.toUpperCase()}</span>
          ))}
        <span className="preview-spacer" />
        {/* After a manual pick: reopen the candidate list inline (§V2.11). */}
        {manual && model.compareCoupleID !== couple.id && (
          <button
            className="link-button"
            title="Compare this take's other audio candidates"
            onClick={() => {
              appModel.compareCoupleID = couple.id;
              appModel.invalidate();
            }}
          >
            Change…
          </button>
        )}
      </div>

      {/* The SAME element in both arrangements (keyed by couple only), so an
          arrangement switch never remounts the <video>. */}
      <VideoHero key={couple.id} couple={couple} compact={compact} />

      {compact && !auditioning && pair.session !== null && <OffsetFitLine couple={couple} />}
      <PreviewStatusLine couple={couple} manualPick={manual && !comparing} />
      {compact && couple.state === 'noMatch' && couple.candidates.length === 0 ? (
        <NoAudioLine couple={couple} />
      ) : comparing || (couple.needsResolve && !manual) ? (
        <ResolveBanner
          couple={couple}
          maxHeight={compact ? 132 : 200}
          onDone={
            comparing
              ? () => {
                  appModel.compareCoupleID = null;
                  appModel.invalidate();
                }
              : null
          }
        />
      ) : null}
    </div>
  );
}

/** "offset +1.28 s" … "fit 4.6 ms" (compact only; the full arrangement's numeric
 * readouts live in the sync tools / fit status line). */
function OffsetFitLine({ couple }: { couple: CoupleRow }): React.JSX.Element {
  const pair = couple.pair;
  let offsetMs = 0;
  try {
    offsetMs = pair.session?.state().offsetMs ?? 0;
  } catch {
    offsetMs = 0;
  }
  const fit = pair.fitStatus();
  return (
    <div className="offset-fit-line">
      <span>offset {signed(offsetMs / 1000, 2)} s</span>
      <span className="preview-spacer" />
      {pair.syncStatus === 'syncing' ? <span>syncing…</span> : fit ? <span>fit {fit.rmsMs.toFixed(1)} ms</span> : null}
    </div>
  );
}

/** "No matching audio in this session · Choose file…" — the file picker pairs the
 * chosen audio with this take as a manual pick (the same repair path a drop uses). */
function NoAudioLine({ couple }: { couple: CoupleRow }): React.JSX.Element {
  return (
    <div className="no-audio-line">
      <span className="dim">No matching audio in this session</span>
      <span className="sep">·</span>
      <button
        className="link-button"
        title="Pick the audio recording for this take"
        onClick={async () => {
          const path = (await ipcRenderer.invoke('couple:pick-audio')) as string | null;
          if (path) appModel.pairWithChosenFile(couple, path);
        }}
      >
        Choose file…
      </button>
    </div>
  );
}

/** One tertiary line under the player: while a candidate is auditioned, the
 * readout "▶ <file> at best fit · offset <lag> s · fit <rms>" with ■; after a
 * manual pick, "Audio chosen by you · Undo · Compare others". Otherwise nothing. */
function PreviewStatusLine({ couple, manualPick }: { couple: CoupleRow; manualPick: boolean }): React.JSX.Element | null {
  const model = useModel(appModel);
  const a = model.candidateAudition;
  if (a && a.coupleID === couple.id) {
    return (
      <div className="preview-status audition">
        <span className="readout" title={auditionReadout(a.candidate)}>
          {auditionReadout(a.candidate)}
        </span>
        <span className="preview-spacer" />
        <button
          className="link-button"
          title="Stop auditioning — back to this couple's own audio"
          onClick={() => appModel.stopCandidateAudition()}
        >
          ■
        </button>
      </div>
    );
  }
  if (!manualPick) return null;
  return (
    <div className="preview-status">
      <span className="dim">Audio chosen by you</span>
      <span className="preview-spacer" />
      {model.canUndoManualPick(couple) && (
        <button
          className="link-button"
          title="Undo the pick — the couple goes back to To fix"
          onClick={() => appModel.undoManualPick(couple)}
        >
          Undo
        </button>
      )}
      <button
        className="link-button"
        title="Reopen the candidate list to listen to the alternatives"
        onClick={() => {
          appModel.compareCoupleID = couple.id;
          appModel.invalidate();
        }}
      >
        Compare others
      </button>
    </div>
  );
}

function resolveReason(couple: CoupleRow): string {
  switch (couple.state) {
    case 'noMatch':
      return "Nothing in the pool correlates with this take's camera audio.";
    case 'ambiguous': {
      const m = couple.detail.margin;
      if (typeof m === 'number') return `Two takes are too close to call (margin ${m.toFixed(1)}). A human must pick.`;
      return 'Candidates conflict — a human must pick.';
    }
    default:
      return 'Pick the audio source for this take.';
  }
}

/** The manual-choice surface (§V2.6), inline under the player: candidate rows with
 * ▶ audition at best fit (■ while it plays; nothing is pinned), the name, a
 * relative strength bar + coverage (never the raw z — it lives in the tooltip) and
 * "use" (manual pin + re-sync, undoable) or "✓ current". Closing it (Done, a pick,
 * another row) ends any audition of its candidates. */
function ResolveBanner({
  couple,
  maxHeight,
  onDone,
}: {
  couple: CoupleRow;
  maxHeight: number;
  onDone: (() => void) | null;
}): React.JSX.Element {
  const model = useModel(appModel);
  const maxZ = Math.max(1, ...couple.candidates.map((c) => c.z));
  React.useEffect(
    () => () => {
      if (appModel.candidateAudition?.coupleID === couple.id) appModel.stopCandidateAudition();
    },
    [couple.id]
  );
  return (
    <div className={`resolve-banner ${couple.verdict}`}>
      <div className="resolve-head">
        <span className="resolve-icon">{couple.state === 'noMatch' ? '⚠' : '?'}</span>
        <span className="title">{couple.resolveTitle}</span>
        <span className="preview-spacer" />
        {onDone && (
          <button className="link-button strong" onClick={onDone}>
            Done
          </button>
        )}
      </div>
      <div className="resolve-reason">{resolveReason(couple)}</div>
      {couple.candidates.length === 0 ? (
        <div className="resolve-reason dim">
          No audio cleared the floor — drag a file from the media pool to force a pairing.
        </div>
      ) : (
        <div className="candidate-list" style={{ maxHeight }}>
          {couple.candidates.map((cand) => {
            const selected = couple.selectedAudioKey === cand.audioKey;
            const playing =
              model.candidateAudition?.coupleID === couple.id &&
              model.candidateAudition.candidate.audioKey === cand.audioKey;
            const playable =
              model.selectedID === couple.id &&
              couple.video.handle !== null &&
              (model.pool.entry(cand.audioKey)?.handle ?? null) !== null;
            return (
              <div
                key={cand.audioKey}
                className={`candidate-row${selected ? ' selected' : ''}${playing ? ' playing' : ''}`}
                title={`z ${cand.z.toFixed(1)} · lag ${cand.lagMs.toFixed(0)} ms`}
              >
                <button
                  className={`cand-play${playing ? ' on' : ''}`}
                  disabled={!playable}
                  title={playing ? 'Stop' : 'Play the video with this audio at its best-fit offset (nothing is changed)'}
                  onClick={() => {
                    if (playing) appModel.stopCandidateAudition();
                    else appModel.auditionCandidate(couple, cand);
                  }}
                >
                  {playing ? '■' : '▶'}
                </button>
                <span className="cand-name">{cand.audioName}</span>
                <span className="preview-spacer" />
                <span className="cand-strength">
                  <span style={{ width: `${Math.max(6 / 54, cand.z / maxZ) * 100}%` }} />
                </span>
                <span className="cand-cov">{Math.round(cand.coverage.fraction * 100)}%</span>
                {selected ? (
                  <span className="cand-current">✓ current</span>
                ) : (
                  <button
                    className="link-button strong cand-use"
                    title="Use this audio for the take (pins it and re-syncs; ⌘Z undoes)"
                    onClick={() => appModel.repair(couple, cand.audioKey)}
                  >
                    use
                  </button>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
