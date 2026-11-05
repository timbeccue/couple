// CenterColumn.tsx — the full arrangement's editing surfaces under the preview
// (§V2.1 layout, port of the CenterColumn.swift editor below PairPreview): the
// loupe area (summoned on demand), the waveform stage + drift sparkline, and the
// persistent bottom tool bar (⚙ sync tools / ◫ loupes). There is NO Approve
// control (UX §V2.12 / D4) — inclusion is the row checkbox only. The preview block
// above (title, video, PairStrip, candidate list) is PairPreview, shared with the
// compact arrangement. Presentation only — decisions live in core policies
// (PLAN-CORE-SHELL-BOUNDARY §1).
import React, { useEffect, useState } from 'react';
import { appModel } from '../app/AppModel';
import type { CoupleRow } from '../app/CoupleRow';
import type { PairSession } from '../app/PairSession';
import { useModel } from '../app/observable';
import { env } from '../kit/native';
import type { MonitorMode } from '../kit/Audition';
import type { DriftMode } from '../kit/Session';
import { WaveformStage } from './WaveformStage';
import { LoupeArea } from './LoupeView';

/** Loupes glyph: inline-SVG twin of the SF Symbol `rectangle.split.3x1` the Swift
 * CenterColumn toggle uses. Drawn rather than typed as ◫ (U+25EB) because that
 * code point is not in the UI font stack (-apple-system / Segoe UI / Roboto) and
 * only renders via per-platform fallback fonts — tofu where none has it. */
function LoupesIcon(): React.JSX.Element {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.3"
      strokeLinejoin="round"
      aria-hidden="true"
      style={{ verticalAlign: '-2px', marginRight: 2 }}
    >
      <rect x="1.5" y="3.5" width="13" height="9" rx="1.5" />
      <path d="M5.8 3.5 V12.5 M10.2 3.5 V12.5" />
    </svg>
  );
}

/** The editor surfaces below the preview (full arrangement only). */
export function EditorTools(): React.JSX.Element | null {
  const model = useModel(appModel);
  const couple = model.selected;
  if (!model.hasContent || !couple) return null;
  return <PairEditorTools key={couple.id} couple={couple} />;
}

function PairEditorTools({ couple }: { couple: CoupleRow }): React.JSX.Element | null {
  const model = useModel(appModel);
  useModel(couple);
  const pair = useModel(couple.pair);
  const [loupesShown, setLoupesShown] = useState(false);
  const hasSession = pair.session !== null;
  const toolsExpanded = model.syncToolsExpanded;
  const setToolsExpanded = (v: boolean): void => {
    appModel.syncToolsExpanded = v;
    appModel.invalidate();
  };

  // Headless verification: reveal the on-demand surfaces so a screenshot run can
  // confirm the loupes + sync tools render.
  useEffect(() => {
    if (env.COUPLE_ANCHOR_TEST && pair.syncStatus === 'synced') {
      setLoupesShown(true);
      appModel.syncToolsExpanded = true;
      appModel.invalidate();
    }
  }, [pair.syncStatus]);

  if (!hasSession) return null;
  return (
    <div className="editor-tools">
      {loupesShown && pair.anchors.length > 0 && <LoupeArea couple={couple} pair={pair} />}
      <WaveformStage couple={couple} pair={pair} onShowLoupes={() => setLoupesShown(true)} />
      <SyncToolsBar
        pair={pair}
        loupesShown={loupesShown}
        setLoupesShown={setLoupesShown}
        toolsExpanded={toolsExpanded}
        setToolsExpanded={setToolsExpanded}
      />
    </div>
  );
}

// MARK: - Bottom tool bar (§V2.4) + expandable sync readouts

function SyncToolsBar({
  pair,
  loupesShown,
  setLoupesShown,
  toolsExpanded,
  setToolsExpanded,
}: {
  pair: PairSession;
  loupesShown: boolean;
  setLoupesShown: (v: boolean) => void;
  toolsExpanded: boolean;
  setToolsExpanded: (v: boolean) => void;
}): React.JSX.Element {
  const audition = useModel(appModel.audition);
  const state = pair.session ? pair.session.state() : null;
  const driftMode = (state?.driftMode as DriftMode | undefined) ?? 'resample';
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
          <LoupesIcon /> loupes
        </button>
        <span className="wave-spacer" />
      </div>
    </div>
  );
}
