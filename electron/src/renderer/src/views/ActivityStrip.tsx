// ActivityStrip.tsx — the unified global activity strip (TODO-005). All the
// "what line, what fraction, is it honest?" logic lives in the pure core policy
// behind AppModel.activity (ActivityController); this view only renders the one
// line the controller publishes: nothing when idle/debouncing, an ERROR line
// (no bar) for a failure, a DETERMINATE bar when the summary carries a
// trustworthy fraction, an INDETERMINATE (moving) bar otherwise. It never
// fabricates a percentage — that honesty is enforced upstream.
import React from 'react';
import { appModel } from '../app/AppModel';
import { useModel } from '../app/observable';

// Self-contained styling for the two states styles.css doesn't cover — an ERROR
// line and an INDETERMINATE (moving) bar — injected once so this view owns its
// own presentation without editing the shared stylesheet.
const STYLE_ID = 'activity-strip-extra';
if (typeof document !== 'undefined' && !document.getElementById(STYLE_ID)) {
  const style = document.createElement('style');
  style.id = STYLE_ID;
  style.textContent = `
    .activity-strip.error { color: var(--danger); }
    .activity-strip .bar.indeterminate .fill {
      width: 40%;
      transition: none;
      animation: activity-indeterminate 1.1s ease-in-out infinite;
    }
    @keyframes activity-indeterminate {
      0% { margin-left: -40%; }
      100% { margin-left: 100%; }
    }`;
  document.head.appendChild(style);
}

export function ActivityStrip(): React.JSX.Element | null {
  const activity = useModel(appModel.activity);
  const summary = activity.summary;
  if (summary === null) return null;

  return (
    <div className={`activity-strip${summary.isError ? ' error' : ''}`}>
      <span>{summary.text}</span>
      {/* A bar only when there is progress to show; an error line carries no bar
          (failure is an actionable line, not a stuck bar, TODO-005). */}
      {!summary.isError &&
        (summary.fraction !== null ? (
          <div className="bar">
            <div className="fill" style={{ width: `${Math.round(summary.fraction * 100)}%` }} />
          </div>
        ) : (
          <div className="bar indeterminate">
            <div className="fill" />
          </div>
        ))}
    </div>
  );
}
