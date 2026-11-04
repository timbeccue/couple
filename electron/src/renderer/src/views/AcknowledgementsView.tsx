// Acknowledgements (AcknowledgementsView.swift port; docs/LICENSING.md §3):
// every bundled third-party component with its license name, and the license
// text as shipped in the Licenses folder (ffmpeg's COPYING.LGPLv2.1 /
// LICENSE.md / SOURCE.txt and the generated per-tree notices). Left:
// components grouped by tree; right: the notices file, scrolled to the
// selected component's section. Reached from the About panel and the Help menu.
import React, { useEffect, useMemo, useState } from 'react';
import { isEmpty, loadAcknowledgements, type NoticeDocument, type NoticeTree } from '../app/Licenses';
import { useModalKeys } from './AboutView';
import './about.css';

export function AcknowledgementsView({ onClose }: { onClose: () => void }): React.JSX.Element {
  useModalKeys(onClose);
  const data = useMemo(() => loadAcknowledgements(), []);
  const [selected, setSelected] = useState<string | null>(
    () => data.trees.find((t) => t.components.length > 0)?.components[0]?.id ?? null
  );
  const [documentID, setDocumentID] = useState<string | null>(null);

  const tree: NoticeTree | undefined = selected
    ? data.trees.find((t) => selected in t.documentByComponent)
    : undefined;
  const document: NoticeDocument | undefined =
    (documentID && tree?.documents.find((d) => d.id === documentID)) || tree?.documents[0];

  const select = (id: string) => {
    setSelected(id);
    setDocumentID(data.trees.find((t) => id in t.documentByComponent)?.documentByComponent[id] ?? null);
  };

  // Scroll the selected component's section to the top of the text pane.
  useEffect(() => {
    if (!selected) return;
    const el = window.document.getElementById(`ack-${selected}`);
    el?.scrollIntoView({ block: 'start' });
  }, [selected, document?.id]);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="ack" onClick={(e) => e.stopPropagation()}>
        <div className="ack-titlebar">
          <span>Acknowledgements</span>
          <button onClick={onClose}>Close</button>
        </div>
        {isEmpty(data) ? (
          <div className="ack-missing">
            <div>No license files in this build.</div>
            <div className="dim">
              Packaged builds carry them in the application’s resources folder (Licenses/). Expected at:
            </div>
            <code>{data.folder ?? '(not set)'}</code>
          </div>
        ) : (
          <div className="ack-body">
            <div className="ack-list">
              <div className="ack-intro">Couple includes the following third-party software.</div>
              {data.trees.map((t) => (
                <div key={t.id} className="ack-group">
                  <div className="ack-group-title">
                    {t.components.length > 0 ? `${t.title} (${t.components.length})` : t.title}
                  </div>
                  {t.components.length === 0 && <div className="ack-row dim">None</div>}
                  {t.components.map((c) => (
                    <div
                      key={c.id}
                      className={`ack-row${c.id === selected ? ' selected' : ''}`}
                      onClick={() => select(c.id)}
                    >
                      <span className="ack-name">{c.name}</span>
                      <span className="ack-version">{c.version}</span>
                      <span className="ack-license">{c.usedUnder ?? c.license}</span>
                    </div>
                  ))}
                </div>
              ))}
            </div>
            <div className="ack-text">
              <div className="ack-files">
                {tree && tree.documents.length > 1 ? (
                  tree.documents.map((d) => (
                    <button
                      key={d.id}
                      className={d.id === document?.id ? 'primary' : ''}
                      onClick={() => setDocumentID(d.id)}
                    >
                      {d.fileName}
                    </button>
                  ))
                ) : (
                  <span className="dim mono">{document?.fileName}</span>
                )}
              </div>
              <div className="ack-scroll">
                {document?.sections.map((s) => (
                  <section key={s.id} id={`ack-${s.id}`} className="ack-section">
                    <div className="ack-section-title">{s.title}</div>
                    {s.header.map((line, i) => (
                      <div key={i} className="ack-section-header">
                        {line}
                      </div>
                    ))}
                    <pre>{s.body}</pre>
                  </section>
                ))}
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
