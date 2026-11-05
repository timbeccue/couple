// ExportRail.tsx — the full arrangement's right column (UX §V2.12 / §V2.8 / §6;
// port of ExportRail.swift) = the shared ExportPanel (INTO / AS / keep camera audio
// / Export N files — also the compact arrangement's pinned bottom block) followed
// by the FILES list: included ∪ already exported (rule in core `export_include`),
// one `<stem>_synced.<ext>` per couple, stem editable in place with ↺ reset,
// per-file status; an exported row shows its ✓ time from `exportedAt` (D8). Each
// per-couple job is the unchanged core export pipeline, so a batch is
// byte-identical to the CLI export, couple by couple. The arrangement composes the
// panel and this list (CompactLayout.tsx).
import React, { useEffect, useRef, useState } from 'react';
import { appModel } from '../app/AppModel';
import { useModel } from '../app/observable';
import { extForFormat } from '../app/BatchExport';
import type { CoupleRow } from '../app/CoupleRow';
import { fmtClock } from './CoupleList';
import './export.css';

/** FILES · n — included ∪ exported (D1/D8); hidden while empty. */
export function FilesList(): React.JSX.Element | null {
  const model = useModel(appModel);
  useModel(model.batch);
  const files = model.filesList;
  if (files.length === 0) return null;
  return (
    <div className="files-list">
      <div className="section-label">
        <span>FILES · {files.length}</span>
      </div>
      <div className="export-rows">
        {files.map((couple) => (
          <ExportRow key={couple.id} couple={couple} />
        ))}
      </div>
    </div>
  );
}

/** One file row: status column, an in-place editable filename (stem + fixed
 * extension), the ✓ export time, and a failure line. The filename IS the row (§V2.8). */
function ExportRow({ couple }: { couple: CoupleRow }): React.JSX.Element {
  const model = useModel(appModel);
  useModel(couple);
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
        {couple.exportedAt !== null && (
          <span className="exported-stamp" title={`Exported at ${fmtClock(couple.exportedAt)}`}>
            ✓ {fmtClock(couple.exportedAt)}
          </span>
        )}
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
