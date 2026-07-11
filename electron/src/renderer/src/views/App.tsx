// App.tsx — root layout (RootView.swift port): empty ⇒ LoadSurface; else the
// 3-column QueueRail / CenterColumn / ExportRail split + FooterBar. The whole
// window is the drop target.
import React, { useCallback, useEffect, useState } from 'react';
import { appModel } from '../app/AppModel';
import { useModel } from '../app/observable';
import { env, platform } from '../kit/native';
import { runParityHarnesses } from '../app/harness';
import { QueueRail } from './QueueRail';
import { CenterColumn } from './CenterColumn';
import { ExportRail } from './ExportRail';
import { ActivityStrip } from './ActivityStrip';
import { ToastStack } from './ToastStack';

const { webUtils, ipcRenderer } = (window as unknown as { require: NodeRequire }).require(
  'electron'
) as typeof import('electron');

function droppedPaths(e: React.DragEvent): string[] {
  // Electron 32+ removed File.path; webUtils.getPathForFile is the supported
  // way to map a dropped File to its filesystem path.
  return Array.from(e.dataTransfer.files)
    .map((f) => {
      try {
        return webUtils.getPathForFile(f);
      } catch {
        return '';
      }
    })
    .filter((p) => p.length > 0);
}

/** Native open dialog fallback for adding media without drag-drop. */
async function pickAndIngest(): Promise<void> {
  const paths = (await ipcRenderer.invoke('couple:pick-media')) as string[];
  if (paths.length > 0) appModel.ingest.importDropped(paths);
}

/** One-shot guard for the startup effect (React StrictMode double-runs
 * effects in dev; two concurrent ingests of the same batch would race). */
let startupRan = false;

export function App(): React.JSX.Element {
  const model = useModel(appModel);
  const [dropActive, setDropActive] = useState(false);

  const onDrop = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    setDropActive(false);
    const paths = droppedPaths(e);
    if (paths.length > 0) appModel.ingest.importDropped(paths);
  }, []);

  useEffect(() => {
    const beforeUnload = () => appModel.teardown();
    window.addEventListener('beforeunload', beforeUnload);
    // One-shot startup work, guarded against React StrictMode's double effect
    // run in dev (two concurrent ingests of the same folder would race).
    if (!startupRan) {
      startupRan = true;
      // Dev harness: auto-ingest a directory at startup (COUPLE_OPEN_DIR), the
      // Electron twin of the Swift harness's COUPLE_PAIR_DIR.
      const openDir = env.COUPLE_OPEN_DIR;
      if (openDir) appModel.ingest.importDropped([openDir]);
      // Launch restore offer (§5.4): a non-blocking toast when a saved queue
      // exists and no debug drive is loading content (the method self-guards).
      appModel.offerRestoreOnLaunch();
      // Env-gated persistence/undo/restore verification hooks (prod no-ops).
      runParityHarnesses(appModel);
      // COUPLE_PLAY_TEST: once the selected couple syncs, start audition
      // playback (drives the VideoHero slaving loop for headless HUD runs).
      if (env.COUPLE_PLAY_TEST) {
        const poll = setInterval(() => {
          const c = appModel.selected;
          if (c && c.hasSession && c.pair.syncStatus === 'synced') {
            clearInterval(poll);
            appModel.attachAudition(c);
            appModel.audition.play();
          }
        }, 500);
      }
    }
    return () => window.removeEventListener('beforeunload', beforeUnload);
  }, []);

  // Keymap (InputRouter.swift port, app-level slice): j/k walk the visual
  // queue order, v toggles approve, space/l play-pause, esc stops audition.
  // Waveform-local keys (nudges, brackets) live on the stage itself.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
      const ordered = appModel.orderedCouples;
      const idx = ordered.findIndex((c) => c.id === appModel.selectedID);
      // Undo / redo (⌘Z / ⇧⌘Z) for cheaply-invertible queue mutations (TODO-015).
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'z') {
        if (e.shiftKey) appModel.performRedo();
        else appModel.performUndo();
        e.preventDefault();
        return;
      }
      switch (e.key) {
        case 'j':
        case 'ArrowDown':
          if (ordered.length > 0) appModel.select(ordered[Math.min(idx + 1, ordered.length - 1)].id);
          e.preventDefault();
          break;
        case 'k':
        case 'ArrowUp':
          if (ordered.length > 0) appModel.select(ordered[Math.max(idx - 1, 0)].id);
          e.preventDefault();
          break;
        case 'v': {
          const c = appModel.selected;
          if (c && c.hasSession && c.pair.syncStatus !== 'syncing') {
            appModel.toggleApprove(c); // undoable (⌘Z)
          }
          break;
        }
        case 'Backspace':
        case 'Delete':
          // ⌘⌫ clears the whole session (TODO-001); confirms with the counts.
          if (e.metaKey) {
            appModel.requestClearSession();
            e.preventDefault();
          }
          break;
        case ' ':
        case 'l': {
          const c = appModel.selected;
          if (c && c.hasSession) {
            if (!appModel.audition.attached || appModel.audition.couple !== c) {
              appModel.attachAudition(c);
            }
            appModel.audition.toggle();
          }
          e.preventDefault();
          break;
        }
        case 'Escape':
          appModel.audition.pause();
          break;
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  return (
    <div
      className={`root${dropActive ? ' drop-active' : ''}`}
      onDragOver={(e) => {
        e.preventDefault();
        setDropActive(true);
      }}
      onDragLeave={(e) => {
        if (e.currentTarget === e.target) setDropActive(false);
      }}
      onDrop={onDrop}
    >
      {platform === 'darwin' && <div className="titlebar-drag" />}
      {model.pool.isEmpty && !model.pool.importing ? (
        <div className="load-surface">
          <h1>Couple</h1>
          <p>Drop two recordings of the same event — video and audio — anywhere in this window.</p>
          <p>They are paired and synced by matching their sound.</p>
          <button className="primary" style={{ marginTop: 12 }} onClick={() => void pickAndIngest()}>
            Choose files…
          </button>
        </div>
      ) : (
        <div className="columns">
          <QueueRail />
          <CenterColumn />
          <ExportRail />
        </div>
      )}
      <ActivityStrip />
      <div className="footer-bar">
        <span
          style={{
            fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
            fontSize: 10,
            color: 'rgba(216, 218, 222, 0.32)',
          }}
        >
          j/k move · v approve (=queue) · ⏎/⚙ sync tools · l/◫ loupes · esc collapse
          <span style={{ marginLeft: 24 }}>
            ←/→ offset 1 ms · ⇧ 10 · ⌥ 0.1 · [ ] drift · drag loupe = offset · ⇧-drag = pivot
          </span>
        </span>
        <span style={{ marginLeft: 'auto', fontSize: 10, color: 'rgba(216, 218, 222, 0.32)' }}>
          waveforms render real peak data
        </span>
      </div>
      <ToastStack />
    </div>
  );
}
