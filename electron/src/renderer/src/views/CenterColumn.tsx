// CenterColumn.tsx — the selected-pair editor (§V2.1 layout, port of
// CenterColumn.swift): video hero (greedy), the §V2.6 resolve banner, the loupe
// area (summoned on demand), the waveform stage + drift sparkline, and the
// persistent bottom tool bar (⚙ sync tools / ◫ loupes / Approve). Re-picking
// routes through the ingest controller's ensureSession so the session/sync
// lifecycle stays single-path. Presentation only — decisions live in core
// policies (PLAN-CORE-SHELL-BOUNDARY §1).
import React, { useEffect, useState } from 'react';
import { appModel } from '../app/AppModel';
import type { CoupleRow } from '../app/CoupleRow';
import type { PairSession } from '../app/PairSession';
import { useModel } from '../app/observable';
import { env } from '../kit/native';
import type { MonitorMode } from '../kit/Audition';
import type { DriftMode } from '../kit/Session';
import type { RowCandidate } from '../kit/Wire';
import { WaveformStage } from './WaveformStage';
import { LoupeArea } from './LoupeView';
import { VideoHero } from './VideoHero';
import { LoadSurface } from './LoadSurface';

function pickCandidate(coupleId: string, candidate: RowCandidate): void {
  const couple = appModel.couples.find((c) => c.id === coupleId);
  if (!couple || couple.selectedAudioKey === candidate.audioKey) return;
  couple.applyManualPick(candidate.audioKey, candidate.z, appModel.zMin);
  appModel.ingest.ensureSession(couple);
  appModel.invalidate();
}

export function CenterColumn(): React.JSX.Element {
  const model = useModel(appModel);
  useModel(model.pool);
  // Empty session (UX-DECISIONS 2026-07-11): the import surface takes the hero
  // slot inside the always-visible three-column shell — no separate splash.
  if (!model.hasContent) {
    return (
      <div className="center-column">
        <LoadSurface />
      </div>
    );
  }
  const couple = model.selected;
  if (!couple) {
    return (
      <div className="center-column">
        <div className="center-empty">Select a couple from the queue</div>
      </div>
    );
  }
  return <PairEditor key={couple.id} couple={couple} />;
}

function PairEditor({ couple }: { couple: CoupleRow }): React.JSX.Element {
  useModel(couple);
  const pair = useModel(couple.pair);
  const [loupesShown, setLoupesShown] = useState(false);
  const [toolsExpanded, setToolsExpanded] = useState(false);
  const hasSession = pair.session !== null;

  // Headless verification: reveal the on-demand surfaces so a screenshot run can
  // confirm the loupes + sync tools render.
  useEffect(() => {
    if (env.COUPLE_ANCHOR_TEST && pair.syncStatus === 'synced') {
      setLoupesShown(true);
      setToolsExpanded(true);
    }
  }, [pair.syncStatus]);

  return (
    <div className="center-column">
      <div className="editor-stack">
        <div className="editor-title">
          <span className={`verdict-dot ${couple.verdict}`} />
          <h2>{couple.video.name}</h2>
          {couple.selectedCandidate && (
            <span className="editor-audio">× {couple.selectedCandidate.audioName}</span>
          )}
        </div>

        <VideoHero couple={couple} />

        {(couple.needsResolve || couple.alternates.length > 0) && (
          <div className="resolve-banner">
            <div className="title">{couple.resolveTitle}</div>
            {couple.candidates.map((c) => (
              <div
                key={c.audioKey}
                className={`candidate-row${c.audioKey === couple.selectedAudioKey ? ' selected' : ''}`}
                onClick={() => pickCandidate(couple.id, c)}
              >
                <span className="role-tag audio">A</span>
                <span className="couple-name">{c.audioName}</span>
                <span className="z">z {c.z.toFixed(1)}</span>
              </div>
            ))}
          </div>
        )}

        {hasSession && loupesShown && pair.anchors.length > 0 && <LoupeArea couple={couple} pair={pair} />}

        {hasSession && (
          <WaveformStage couple={couple} pair={pair} onShowLoupes={() => setLoupesShown(true)} />
        )}

        {hasSession && (
          <SyncToolsBar
            couple={couple}
            pair={pair}
            loupesShown={loupesShown}
            setLoupesShown={setLoupesShown}
            toolsExpanded={toolsExpanded}
            setToolsExpanded={setToolsExpanded}
          />
        )}
      </div>
    </div>
  );
}

// MARK: - Bottom tool bar (§V2.4) + expandable sync readouts

function SyncToolsBar({
  couple,
  pair,
  loupesShown,
  setLoupesShown,
  toolsExpanded,
  setToolsExpanded,
}: {
  couple: CoupleRow;
  pair: PairSession;
  loupesShown: boolean;
  setLoupesShown: (v: boolean) => void;
  toolsExpanded: boolean;
  setToolsExpanded: (v: boolean) => void;
}): React.JSX.Element {
  const audition = useModel(appModel.audition);
  const state = pair.session ? pair.session.state() : null;
  const driftMode = (state?.driftMode as DriftMode | undefined) ?? 'resample';
  const busy = pair.syncStatus === 'syncing';
  const fit = pair.fitStatus();

  return (
    <div className="sync-controls">
      {toolsExpanded && (
        <div className="sync-tools">
          <div className="tool-col">
            <span className="tool-label">OFFSET</span>
            <div className="readout">{(state?.offsetMs ?? 0).toFixed(2)} ms</div>
            <div className="nudge-row">
              {[-10, -1, 1, 10].map((s) => (
                <button key={s} disabled={!pair.inputsEnabled} onClick={() => pair.nudgeOffset(s)}>
                  {s > 0 ? `+${s}` : s}
                </button>
              ))}
            </div>
          </div>
          <div className="tool-col" style={{ opacity: driftMode === 'none' ? 0.45 : 1 }}>
            <span className="tool-label">DRIFT</span>
            <div className="readout">{(state?.driftPpm ?? 0).toFixed(2)} ppm</div>
            <div className="nudge-row">
              {[-1, -0.1, 0.1, 1].map((s) => (
                <button key={s} disabled={!pair.inputsEnabled} onClick={() => pair.nudgeDrift(s)}>
                  {s > 0 ? `+${s}` : s}
                </button>
              ))}
            </div>
          </div>
          <div className="tool-col">
            <span className="tool-label">DRIFT MODE</span>
            <div className="seg">
              {(['resample', 'none'] as DriftMode[]).map((m) => (
                <button
                  key={m}
                  className={driftMode === m ? 'on' : undefined}
                  disabled={!pair.inputsEnabled}
                  onClick={() => pair.setDriftMode(m)}
                >
                  {m === 'resample' ? 'Resample' : 'None'}
                </button>
              ))}
            </div>
            <div className="fit-line">
              {fit ? `fit RMS ${fit.rmsMs.toFixed(2)} ms · ${fit.windows} win` : 'not measured'}
            </div>
          </div>
          <div className="tool-col">
            <span className="tool-label">MONITOR</span>
            <div className="seg">
              {(['pan', 'overlay'] as MonitorMode[]).map((m) => (
                <button
                  key={m}
                  className={audition.mode === m ? 'on' : undefined}
                  onClick={() => audition.setMode(m)}
                >
                  {m === 'pan' ? 'Pan L/R' : 'Overlay'}
                </button>
              ))}
            </div>
            {/* Same shared lane-gain state as the on-lane controls, so the two
                surfaces cannot diverge. */}
            <div className="gain-row">
              <span className="gain-tag cam">V</span>
              <input
                type="range"
                min={0}
                max={2}
                step={0.05}
                value={audition.camGain}
                disabled={audition.camMuted}
                onChange={(e) => audition.setCamGain(Number(e.target.value))}
              />
              <span className="gain-tag ext">A</span>
              <input
                type="range"
                min={0}
                max={2}
                step={0.05}
                value={audition.extGain}
                disabled={audition.extMuted}
                onChange={(e) => audition.setExtGain(Number(e.target.value))}
              />
            </div>
          </div>
        </div>
      )}
      <div className="bottom-bar">
        <button
          className={`bar-toggle${toolsExpanded ? ' on' : ''}`}
          onClick={() => setToolsExpanded(!toolsExpanded)}
        >
          ⚙ sync tools
        </button>
        <button
          className={`bar-toggle${loupesShown ? ' on' : ''}`}
          disabled={pair.anchors.length === 0}
          onClick={() => setLoupesShown(!loupesShown)}
        >
          ◫ loupes
        </button>
        <span className="wave-spacer" />
        <button
          className={`approve-btn${couple.approved ? ' approved' : ''}`}
          disabled={busy}
          onClick={() => {
            pair.setVerified(!couple.approved);
            appModel.invalidate();
          }}
        >
          <span>{couple.approved ? '✓' : '○'}</span>
          <span>{couple.approved ? 'Approved' : 'Approve'}</span>
          <span className="approve-key">v</span>
        </button>
      </div>
    </div>
  );
}
