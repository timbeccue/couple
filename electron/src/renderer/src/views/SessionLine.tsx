// SessionLine.tsx — the session's identity and media counts and — per arrangement —
// the session controls (UX §V2.11; port of SessionLine.swift). One view for BOTH
// arrangements:
//  - full (the left rail's header): the wordmark, then the media counts with
//    Clear Session… and "N to export". In an EMPTY session only the wordmark
//    shows (sidebar cleanup, UX §V2.13): no zero counts, no Clear Session… —
//    the import surface is already the start screen;
//  - compact: an empty title-bar row (the window has a hidden title bar on macOS;
//    the traffic lights sit at its left and the arrangement toggle — App.tsx's
//    corner control, ArrangementToggle.tsx, UX §V2.14 — at its right), then
//    "<session> · N video · N audio" with "+ Add" — the ONLY media-pool affordance
//    in compact (§6.4). Clear Session… is in its context menu there.
import React from 'react';
import { appModel } from '../app/AppModel';
import { useModel } from '../app/observable';
import { CoupleWordmark } from './CoupleWordmark';

const { ipcRenderer } = (window as unknown as { require: NodeRequire }).require(
  'electron'
) as typeof import('electron');

async function addMedia(): Promise<void> {
  const paths = (await ipcRenderer.invoke('couple:pick-media')) as string[];
  if (paths.length > 0) appModel.ingest.importDropped(paths);
}

export function SessionLine({ compact }: { compact: boolean }): React.JSX.Element {
  const model = useModel(appModel);
  useModel(model.pool);
  useModel(model.batch);
  const clearDisabled = model.batch.isRunning || !model.hasContent;

  if (!compact) {
    return (
      <div className="rail-header session-line">
        <div className="rail-header-top">
          {/* 22 px (was 15; owner 2026-11-05: larger). Alone on this row: Clear
              Session… sits on the counts row, and the arrangement toggle is the
              window-corner control (ArrangementToggle.tsx, UX §V2.14). */}
          <CoupleWordmark height={22} />
          <span className="session-spacer" />
        </div>
        {/* Counts only once there is media (UX §V2.13): an empty rail reads as
            quiet, not as a list of zeros. Clear Session (TODO-001) at the right of
            the row: returns to the import screen; confirms with media/couple/
            included counts; disabled mid-export (the batch owns the files); hidden
            with the row while the session is empty (nothing to clear). */}
        {model.hasContent && (
          <>
            <div className="rail-counts">
              <span>
                {model.pool.videos.length} video · {model.pool.audios.length} audio
              </span>
              <span className="session-spacer" />
              <button
                className="clear-session"
                disabled={clearDisabled}
                title="Discard this session and start over (does not delete source or exported files)"
                onClick={() => appModel.requestClearSession()}
              >
                Clear Session…
              </button>
            </div>
            <div className="rail-to-export">{model.includedCount} to export</div>
          </>
        )}
      </div>
    );
  }

  const name = model.sessionName;
  return (
    <div
      className="session-line compact"
      onContextMenu={async (e) => {
        e.preventDefault();
        const picked = (await ipcRenderer.invoke('couple:context-menu', [
          { id: 'clear', label: 'Clear Session…', enabled: !clearDisabled },
        ])) as string | null;
        if (picked === 'clear') appModel.requestClearSession();
      }}
    >
      {/* Title-bar row: traffic lights at the left (system), the arrangement toggle
          at the right (App.tsx's corner control). Nothing of its own. */}
      <div className="session-titlebar" />
      <div className="session-identity">
        {name !== null && (
          <>
            <span className="session-name" title={name}>
              {name}
            </span>
            <span className="session-dot">·</span>
          </>
        )}
        {/* Counts only once there is media (UX §V2.13); "+ Add" stays — the only
            pool affordance in compact (§6.4). */}
        {model.hasContent && (
          <span className="session-counts">
            {model.pool.videos.length} video · {model.pool.audios.length} audio
          </span>
        )}
        <span className="session-spacer" />
        <button
          className="session-add"
          title="Add videos, audio files or a folder to this session"
          onClick={() => void addMedia()}
        >
          + Add
        </button>
      </div>
    </div>
  );
}
