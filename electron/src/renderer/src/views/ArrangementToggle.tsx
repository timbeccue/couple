// ArrangementToggle.tsx — the ONE control that switches the window between its
// compact and full arrangements (UX §V2.13; port of ArrangementToggle.swift). Window
// chrome, not a block: App.tsx places it at the root's top-right corner in BOTH
// arrangements, level with the traffic lights on macOS, so it sits where window-shape
// controls live and never moves when the arrangement flips. A 26 × 26 icon button
// with a hover background (the toolbar-button idiom); the icon names the arrangement
// it opens (three columns ⇒ full, a tall rectangle ⇒ compact). Same action as
// View ▸ Compact Window / Full Window (⌘⇧C / Ctrl+Shift+C).
import React from 'react';
import { appModel } from '../app/AppModel';
import { useModel } from '../app/observable';

/** Three columns — "open the full window". */
function FullIcon(): React.JSX.Element {
  return (
    <svg width="14" height="12" viewBox="0 0 14 12" aria-hidden="true">
      <rect x="0.75" y="0.75" width="12.5" height="10.5" rx="1.75" fill="none" stroke="currentColor" strokeWidth="1.5" />
      <path d="M5 1v10M9 1v10" stroke="currentColor" strokeWidth="1.5" />
    </svg>
  );
}

/** A tall narrow rectangle — "go to the compact window". */
function CompactIcon(): React.JSX.Element {
  return (
    <svg width="14" height="12" viewBox="0 0 14 12" aria-hidden="true">
      <rect x="3.75" y="0.75" width="6.5" height="10.5" rx="1.75" fill="none" stroke="currentColor" strokeWidth="1.5" />
    </svg>
  );
}

export function ArrangementToggle(): React.JSX.Element {
  const model = useModel(appModel);
  const compact = model.layoutMode === 'compact';
  const label = compact ? 'Full Window' : 'Compact Window';
  return (
    <button
      className="arrangement-toggle"
      aria-label={label}
      title={
        compact
          ? 'Full Window — overview, loupes, sync tools, media pool (⌘⇧C)'
          : 'Compact Window (⌘⇧C)'
      }
      onClick={() => void appModel.toggleLayoutMode()}
    >
      {compact ? <FullIcon /> : <CompactIcon />}
    </button>
  );
}
