// LoadSurface.tsx — the center column's empty-session import surface (port of
// LoadSurface.swift, UX-DECISIONS 2026-07-11): shown in the video hero's slot
// inside the always-visible three-column shell — there is no separate splash
// screen. Drop anywhere in the window (App.tsx owns the drop target), or choose
// files; a plain spinner (no percentage bar) while a drop is actively importing.
// Copy (owner 2026-11-05, UX §V2.13): "Drop audio and video files", one
// explanatory line, the button — no orientation paragraph about the rails.
// Compact (UX §V2.11): the same drop target fills the preview block (the
// `compact` class only drops the margin).
import React from 'react';
import { appModel } from '../app/AppModel';
import { useModel } from '../app/observable';

const { ipcRenderer } = (window as unknown as { require: NodeRequire }).require(
  'electron'
) as typeof import('electron');

async function pickAndIngest(): Promise<void> {
  const paths = (await ipcRenderer.invoke('couple:pick-media')) as string[];
  if (paths.length > 0) appModel.ingest.importDropped(paths);
}

export function LoadSurface({ compact = false }: { compact?: boolean }): React.JSX.Element {
  const model = useModel(appModel);
  useModel(model.pool);
  return (
    <div className={`import-surface${compact ? ' compact' : ''}`}>
      <svg
        className="import-icon"
        width="34"
        height="34"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M12 3v9" />
        <path d="m8.5 8.5 3.5 3.5 3.5-3.5" />
        <path d="M3 13h4.5l1.5 2.5h6L16.5 13H21" />
        <path d="M3 13v5a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-5" />
      </svg>
      <h2>Drop audio and video files</h2>
      <p className="import-sub">
        Any number of videos and audio files, or a whole folder. Couple finds the matching pairs
        automatically.
      </p>
      <button className="primary" onClick={() => void pickAndIngest()}>
        Choose files…
      </button>
      {model.pool.importing && (
        <div className="import-busy">
          <span className="import-spinner" />
          Importing…
        </div>
      )}
    </div>
  );
}
