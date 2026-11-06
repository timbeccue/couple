// ShortcutsView.tsx — keyboard-shortcut reference (KeyboardShortcutsView.swift
// port; UX §V2.13, 2026-11-05; replaces the footer cheat strip, which was too
// small to read): a readable key / action list grouped by surface, as a modal
// overlay like About. Opens from the export rail's "Keyboard shortcuts" button
// (ShortcutsButton below), Help ▸ Keyboard Shortcuts (⌘/) or the `?` key; Esc / click-away / Close
// dismisses it. Presentation only — every binding listed here is routed in
// App.tsx (queue keys), WaveformStage.tsx (nudges, brackets, drags) or the
// application menu (main/index.ts); keep them in step. In the compact
// arrangement only the rows that apply there are listed (UX §V2.11).
import React from 'react';
import { appModel } from '../app/AppModel';
import { useModel } from '../app/observable';
import { useModalKeys } from './AboutView';
import './about.css';

interface Row {
  keys: string;
  action: string;
  /** What the key does in compact; undefined = the same as `action`. */
  compactAction?: string;
  /** False = not listed in compact (no surface for it there). */
  inCompact?: boolean;
}

interface Group {
  title: string;
  rows: Row[];
}

const GROUPS: Group[] = [
  {
    title: 'SESSION',
    rows: [
      { keys: 'Space', action: 'Play / pause' },
      { keys: 'J  K', action: 'Next / previous couple' },
      {
        keys: '⏎',
        action: 'Open / close the sync tools',
        compactAction: 'Open the full window on this couple',
      },
      { keys: 'Esc', action: 'Stop playback' },
      { keys: '⌘Z  ⇧⌘Z', action: 'Undo / redo' },
      { keys: '⌘⌫', action: 'Clear the session', inCompact: false },
      { keys: '⌘⇧C', action: 'Compact window / full window' },
      { keys: '?  ⌘/', action: 'This list' },
    ],
  },
  {
    title: 'MONITOR',
    rows: [
      { keys: 'A', action: 'Camera ⇄ external (solo)' },
      { keys: 'B', action: 'Hear both again' },
    ],
  },
  {
    title: 'SYNC TOOLS',
    rows: [
      { keys: '←  →', action: 'Offset by 1 ms', inCompact: false },
      { keys: '⇧ ←  →', action: 'Offset by 10 ms', inCompact: false },
      { keys: '⌥ ←  →', action: 'Offset by 0.1 ms', inCompact: false },
      { keys: '[  ]', action: 'Drift one step (⌥ a tenth)', inCompact: false },
      { keys: 'L', action: 'Show / hide the loupes', inCompact: false },
      { keys: 'Drag a loupe', action: 'Slide the offset', inCompact: false },
      { keys: '⇧-drag a loupe', action: 'Pivot the drift', inCompact: false },
    ],
  },
  {
    title: 'MEDIA POOL',
    rows: [
      { keys: 'Click  ⌘  ⇧', action: 'Select / add / range', inCompact: false },
      { keys: '⌫', action: 'Remove the selected files from the session', inCompact: false },
    ],
  },
];

/** DOM event the ShortcutsButton raises; App.tsx listens and opens the overlay. */
export const SHOW_SHORTCUTS_EVENT = 'couple:show-shortcuts';

/** The "Keyboard shortcuts" button at the bottom-right of the export rail (full
 * arrangement; ShortcutsButton in KeyboardShortcutsView.swift). It took a footer
 * row of its own at first; the owner moved it into the rail so no window height
 * goes to it. */
export function ShortcutsButton(): React.JSX.Element {
  return (
    <button
      className="shortcuts-button"
      title="Show the keyboard shortcuts (? or ⌘/)"
      onClick={() => window.dispatchEvent(new Event(SHOW_SHORTCUTS_EVENT))}
    >
      <span className="key-cap">?</span>
      Keyboard shortcuts
    </button>
  );
}

export function ShortcutsView({ onClose }: { onClose: () => void }): React.JSX.Element {
  useModalKeys(onClose);
  const model = useModel(appModel);
  const compact = model.layoutMode === 'compact';
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="shortcuts" onClick={(e) => e.stopPropagation()}>
        <div className="shortcuts-head">
          <span>Keyboard shortcuts</span>
          <button onClick={onClose}>Close</button>
        </div>
        {GROUPS.map((group) => {
          const rows = group.rows
            .filter((r) => !compact || r.inCompact !== false)
            .map((r) => ({ keys: r.keys, action: compact ? (r.compactAction ?? r.action) : r.action }));
          if (rows.length === 0) return null;
          return (
            <div className="shortcuts-group" key={group.title}>
              <div className="section-label">
                <span>{group.title}</span>
              </div>
              <table>
                <tbody>
                  {rows.map((r) => (
                    <tr key={r.keys + r.action}>
                      <td className="keys">{r.keys}</td>
                      <td>{r.action}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          );
        })}
      </div>
    </div>
  );
}
