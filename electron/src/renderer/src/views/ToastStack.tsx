import React from 'react';
import { appModel } from '../app/AppModel';
import { useModel } from '../app/observable';

export function ToastStack(): React.JSX.Element {
  const toasts = useModel(appModel.toasts);
  return (
    <div className="toasts">
      {toasts.toasts.map((t) => (
        <div key={t.id} className={`toast ${t.style}`} onClick={() => toasts.dismiss(t.id)}>
          <div className="title">{t.title}</div>
          {t.detail && <div className="detail">{t.detail}</div>}
          {t.action && (
            <button
              style={{ marginTop: 6 }}
              onClick={(e) => {
                e.stopPropagation();
                t.action!.run();
              }}
            >
              {t.action.label}
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
