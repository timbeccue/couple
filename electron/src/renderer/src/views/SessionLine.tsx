// SessionLine.tsx — the session's identity and media counts, the arrangement
// toggle and the session controls (UX §V2.11; port of SessionLine.swift). One view
// for BOTH arrangements:
//  - full (the left rail's header): the wordmark, Clear Session…, "⤡ Compact",
//    the media counts and "N to export";
//  - compact: a title-bar row carrying "⤢ Full" at the right (the window has a
//    hidden title bar; the traffic lights sit at the left of that row on macOS),
//    then "<session> · N video · N audio" with "+ Add" — the ONLY media-pool
//    affordance in compact (§6.4). Clear Session… is in its context menu there.
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

/** "⤢ Full" in compact / "⤡ Compact" in full. Same action as the View menu's
 * Compact Window / Full Window item (⌘⇧C / Ctrl+Shift+C). */
export function ArrangementToggle(): React.JSX.Element {
  const model = useModel(appModel);
  const compact = model.layoutMode === 'compact';
  return (
    <button
      className="arrangement-toggle"
      title={
        compact
          ? 'Open the full window: overview, loupes, sync tools, media pool (⌘⇧C)'
          : 'Switch to the compact window (⌘⇧C)'
      }
      onClick={() => void appModel.toggleLayoutMode()}
    >
      {compact ? '⤢ Full' : '⤡ Compact'}
    </button>
  );
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
          <CoupleWordmark height={12} />
          <span className="session-spacer" />
          {/* Clear Session (TODO-001): returns to the import screen; confirms with
              media/couple/included counts; disabled mid-export (the batch owns the files). */}
          <button
            className="clear-session"
            disabled={clearDisabled}
            title="Discard this session and start over (does not delete source or exported files)"
            onClick={() => appModel.requestClearSession()}
          >
            Clear Session…
          </button>
          <ArrangementToggle />
        </div>
        <div className="rail-counts">
          {model.pool.videos.length} video · {model.pool.audios.length} audio
        </div>
        <div className="rail-to-export">{model.includedCount} to export</div>
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
      {/* Title-bar row: traffic lights at the left (system), the toggle at the right. */}
      <div className="session-titlebar">
        <span className="session-spacer" />
        <ArrangementToggle />
      </div>
      <div className="session-identity">
        {name !== null && (
          <>
            <span className="session-name" title={name}>
              {name}
            </span>
            <span className="session-dot">·</span>
          </>
        )}
        <span className="session-counts">
          {model.pool.videos.length} video · {model.pool.audios.length} audio
        </span>
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
