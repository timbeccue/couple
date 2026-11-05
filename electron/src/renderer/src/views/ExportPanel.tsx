// ExportPanel.tsx — the export panel (UX §V2.12 / §V2.8 / §6; port of
// ExportPanel.swift): INTO (destination path + Change…; auto-tracks the last
// import folder until pinned), AS (Auto/MOV/MP4/WAV, one global format), "keep
// camera audio as a muted track" on its own line (unavailable for WAV), and
// "Export N files" (N = INCLUDED couples, D1; disabled at 0) running serial jobs
// with disk preflight and continue-past-failure (BatchExport; the finished toast
// carries Reveal). One view for BOTH arrangements: pinned at the bottom of the
// compact arrangement, and the top of the full window's export rail (above Files).
import React, { useEffect } from 'react';
import { appModel } from '../app/AppModel';
import { useModel } from '../app/observable';
import { EXPORT_FORMATS, maybeRunExportHarness } from '../app/BatchExport';
import './export.css';

const nodeRequire = (window as unknown as { require: NodeRequire }).require;
const { ipcRenderer } = nodeRequire('electron') as typeof import('electron');
const os = nodeRequire('node:os') as typeof import('node:os');

/** The destination as a home-abbreviated path ("~/Movies/bach session"). */
function abbreviate(path: string): string {
  const home = os.homedir();
  return home && (path === home || path.startsWith(home + '/')) ? '~' + path.slice(home.length) : path;
}

/** A middle-ellipsized single line: the head shrinks with an ellipsis, the tail
 * (the folder name) always stays visible. */
function MiddleEllipsis({ text }: { text: string }): React.JSX.Element {
  const cut = Math.max(0, text.length - 16);
  return (
    <span className="middle-ellipsis" title={text}>
      <span className="head">{text.slice(0, cut)}</span>
      <span className="tail">{text.slice(cut)}</span>
    </span>
  );
}

export function ExportPanel(): React.JSX.Element {
  const model = useModel(appModel);
  useModel(model.batch);

  // Headless export harness (verification only): fires once COUPLE_EXPORT_TEST is set.
  useEffect(() => {
    maybeRunExportHarness(model);
  }, [model]);

  const phase = model.batch.phase;
  const running = model.batch.isRunning;
  const dest = model.exportDestination;
  const n = model.includedCount;

  return (
    <div className="export-panel">
      {/* INTO — the destination path (mono, ellipsized) + Change… */}
      <div className="export-line">
        <span className="export-label">INTO</span>
        {dest ? (
          <span className="dest-path">
            <MiddleEllipsis text={abbreviate(dest)} />
          </span>
        ) : (
          <span className="dest-path unset">no folder chosen</span>
        )}
        <button
          className="link-button"
          onClick={async () => {
            const dir = (await ipcRenderer.invoke('couple:pick-folder', 'Choose export folder')) as
              | string
              | null;
            if (dir) model.chooseExportDestination(dir);
          }}
        >
          {dest ? 'Change…' : 'Choose…'}
        </button>
      </div>

      {/* AS — one global format for the whole batch */}
      <div className="export-line">
        <span className="export-label">AS</span>
        <div className="format-seg">
          {EXPORT_FORMATS.map((f) => (
            <button
              key={f}
              className={model.globalFormat === f ? 'on' : ''}
              disabled={running}
              onClick={() => {
                model.globalFormat = f;
                model.saveQueue();
                model.invalidate();
              }}
            >
              {f}
            </button>
          ))}
        </div>
      </div>

      <label
        className={`keep-audio${model.globalFormat === 'WAV' ? ' disabled' : ''}`}
        title={model.globalFormat === 'WAV' ? 'Not available for WAV (audio only)' : undefined}
      >
        <input
          type="checkbox"
          checked={model.globalKeepCameraAudio}
          disabled={running || model.globalFormat === 'WAV'}
          onChange={(e) => {
            model.globalKeepCameraAudio = e.target.checked;
            model.saveQueue();
            model.invalidate();
          }}
        />
        keep camera audio as a muted track
      </label>

      {phase.kind === 'preflight' ? (
        <div className="export-progress">Checking disk…</div>
      ) : phase.kind === 'running' ? (
        <div className="export-progress">
          <div className="bar">
            <span style={{ width: `${(phase.done / Math.max(phase.total, 1)) * 100}%` }} />
          </div>
          Exporting {phase.done}/{phase.total}…
        </div>
      ) : (
        <button
          className="export-run"
          disabled={n === 0 || dest === null}
          onClick={() => void model.batch.run()}
        >
          {n === 0 ? 'Export' : `Export ${n} file${n === 1 ? '' : 's'}`}
        </button>
      )}
    </div>
  );
}
