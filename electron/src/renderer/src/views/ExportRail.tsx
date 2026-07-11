// ExportRail.tsx — right column (§V2.8 / §6): one destination folder, the v1
// Auto/MOV/MP4/WAV format selector with keep-camera-audio, editable per-couple
// filename stems, "Export (n files)" running serial jobs with per-couple status
// and continue-past-failure, and a whole-queue disk preflight. TS port of
// mac/Sources/Couple/Views/ExportRail.swift. Each per-couple job is the unchanged
// core export pipeline, so a batch is byte-identical to the CLI export.
import React, { useEffect, useRef, useState } from 'react';
import { appModel } from '../app/AppModel';
import { useModel } from '../app/observable';
import { EXPORT_FORMATS, extForFormat, maybeRunExportHarness, type ExportFormat } from '../app/BatchExport';
import type { CoupleRow } from '../app/CoupleRow';
import './export.css';

const { ipcRenderer } = (window as unknown as { require: NodeRequire }).require('electron');

export function ExportRail(): React.JSX.Element {
  const model = useModel(appModel);
  useModel(model.batch);

  // Headless export harness (verification only): fires once COUPLE_EXPORT_TEST is set.
  useEffect(() => {
    maybeRunExportHarness(model);
  }, [model]);

  const phase = model.batch.phase;
  const running = model.batch.isRunning;

  return (
    <div className="export-rail">
      <div className="section-label" style={{ padding: '0 0 4px' }}>
        EXPORT
      </div>

      {/* Destination folder */}
      <div className="section-label">INTO</div>
      <button
        className={`dest-button${model.exportDestination ? '' : ' unset'}`}
        onClick={async () => {
          const dir = (await ipcRenderer.invoke('couple:pick-folder', 'Choose export folder')) as
            | string
            | null;
          if (dir) model.chooseExportDestination(dir);
        }}
      >
        <svg
          className="dest-glyph"
          width="12"
          height="12"
          viewBox="0 0 16 16"
          fill="none"
          aria-hidden="true"
        >
          <path
            d="M1.5 4.5a1 1 0 0 1 1-1h3.3a1 1 0 0 1 .7.3l.9.9a1 1 0 0 0 .7.3h4.7a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1v-7.5Z"
            stroke="currentColor"
            strokeWidth="1.1"
          />
        </svg>
        <span className="dest-name">
          {model.exportDestination ? model.exportDestination.split('/').pop() : 'Choose folder…'}
        </span>
      </button>

      {/* Global format + keep-camera-audio (one selector for the whole batch) */}
      <div className="section-label" style={{ marginTop: 12 }}>
        FORMAT
      </div>
      <div className="format-seg">
        {EXPORT_FORMATS.map((f) => (
          <button
            key={f}
            className={model.globalFormat === f ? 'on' : ''}
            disabled={running}
            onClick={() => {
              model.globalFormat = f;
              model.invalidate();
            }}
          >
            {f}
          </button>
        ))}
      </div>
      <label className={`keep-audio${model.globalFormat === 'WAV' ? ' disabled' : ''}`}>
        <input
          type="checkbox"
          checked={model.globalKeepCameraAudio}
          disabled={running || model.globalFormat === 'WAV'}
          onChange={(e) => {
            model.globalKeepCameraAudio = e.target.checked;
            model.invalidate();
          }}
        />
        keep camera audio as muted track
      </label>

      {/* Export button / progress */}
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
          disabled={model.approvedCount === 0 || model.exportDestination === null}
          onClick={() => void model.batch.run()}
        >
          Export ({model.approvedCount} file{model.approvedCount === 1 ? '' : 's'})
        </button>
      )}

      {/* Per-couple export rows */}
      {model.queued.length > 0 && (
        <>
          <div className="section-label" style={{ marginTop: 12 }}>
            ROWS
          </div>
          <div className="export-rows">
            {model.queued.map((couple) => (
              <ExportRow key={couple.id} couple={couple} />
            ))}
          </div>
        </>
      )}
    </div>
  );
}

/** One export row: status dot, an in-place editable filename (stem + fixed
 * extension), and a failure line. The filename IS the row (§V2.8). */
function ExportRow({ couple }: { couple: CoupleRow }): React.JSX.Element {
  const model = useModel(appModel);
  const status = couple.exportStatus;
  const ext = extForFormat(model.globalFormat, couple.video.path);
  const selected = model.selectedID === couple.id;

  return (
    <div
      className={`export-row${selected ? ' selected' : ''}`}
      title={couple.video.name}
      onClick={() => model.select(couple.id)}
    >
      <div className="row-head">
        <span className={`status-dot ${status.kind}`}>
          {status.kind === 'running' ? (
            <span className="mini-spin">{Math.round(status.fraction * 100)}</span>
          ) : status.kind === 'done' ? (
            '●'
          ) : status.kind === 'failed' ? (
            '✕'
          ) : (
            ''
          )}
        </span>
        <EditableFilename couple={couple} ext={ext} />
      </div>
      {status.kind === 'failed' && <div className="row-error">{status.message}</div>}
    </div>
  );
}

/** Finder-style inline rename: the filename shows as text; clicking the stem
 * turns it into a field with the fixed extension alongside. Commit on Enter or
 * blur, cancel (revert) on Esc. An empty commit is a no-op (keeps the last saved
 * name); a commit equal to the default clears the override (§V2.8). */
function EditableFilename({ couple, ext }: { couple: CoupleRow; ext: string }): React.JSX.Element {
  const model = useModel(appModel);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (editing) inputRef.current?.focus();
  }, [editing]);

  const begin = (e: React.MouseEvent): void => {
    e.stopPropagation();
    model.select(couple.id);
    setDraft(couple.outputStem);
    setEditing(true);
  };

  const commit = (): void => {
    if (!editing) return;
    setEditing(false);
    const trimmed = draft.trim();
    if (trimmed.length === 0) return; // empty edit = keep the last saved name
    // Through the model so the rename is undoable (⌘Z) and persisted (§V2.8).
    model.setStemOverride(couple, trimmed === couple.defaultStem ? null : trimmed);
  };

  const cancel = (): void => setEditing(false);

  if (editing) {
    return (
      <span className="filename" onClick={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === 'Enter') commit();
            else if (e.key === 'Escape') cancel();
          }}
        />
        <span className="ext">.{ext}</span>
      </span>
    );
  }

  return (
    <span className="filename">
      <span className="stem" onClick={begin} title="Click to rename">
        {couple.outputStem}
      </span>
      <span className="ext">.{ext}</span>
      {couple.stemOverride !== null && (
        <button
          className="reset"
          title="Reset to default name"
          onClick={(e) => {
            e.stopPropagation();
            model.setStemOverride(couple, null);
          }}
        >
          ↺
        </button>
      )}
    </span>
  );
}
