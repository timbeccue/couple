// ArrangementToggle.tsx — the ONE control that switches the window between its
// compact and full arrangements (UX §V2.13; port of ArrangementToggle.swift). Window
// chrome, not a block: App.tsx places it at the root's top-right corner in BOTH
// arrangements, level with the traffic lights on macOS, so it sits where window-shape
// controls live and never moves when the arrangement flips. A small text button —
// "⤢ Full" while compact, "⤡ Compact" while full (it names the arrangement it
// opens; the diagonal arrow says expand / contract) — 24 px tall with a hover background (the toolbar-button idiom); the owner found an
// icon alone unclear. Same action as View ▸ Compact Window / Full Window
// (⌘⇧C / Ctrl+Shift+C).
import React from 'react';
import { appModel } from '../app/AppModel';
import { useModel } from '../app/observable';

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
      {compact ? '⤢ Full' : '⤡ Compact'}
    </button>
  );
}
