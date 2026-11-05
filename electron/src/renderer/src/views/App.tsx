// App.tsx — root layout (RootView.swift port): the window's two arrangements
// (UX §V2.11) — COMPACT (session line, fixed preview, couple list, export panel)
// or FULL (the three columns + footer) — rendered as one tree by Arrangement
// (CompactLayout.tsx) and switched with the FLIP in layoutFlip.ts. The empty
// session is not a separate splash screen (UX-DECISIONS 2026-07-11): the import
// surface takes the preview slot in either arrangement. The whole window is the
// drop target.
import React, { useCallback, useEffect, useRef, useState } from 'react';
import { appModel } from '../app/AppModel';
import { useModel } from '../app/observable';
import { env, platform } from '../kit/native';
import { runParityHarnesses } from '../app/harness';
import { runLayoutHarnesses } from '../app/layoutHarness';
import { WindowLayoutBridge } from '../app/WindowLayout';
import { Arrangement } from './CompactLayout';
import { installLayoutSwitcher } from './layoutFlip';
import { ActivityStrip } from './ActivityStrip';
import './layout.css';
import { ToastStack } from './ToastStack';
import { AboutView } from './AboutView';
import { AcknowledgementsView } from './AcknowledgementsView';

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

/** One-shot guard for the startup effect (React StrictMode double-runs
 * effects in dev; two concurrent ingests of the same batch would race). */
let startupRan = false;

export function App(): React.JSX.Element {
  const model = useModel(appModel);
  const compact = model.layoutMode === 'compact';
  const [dropActive, setDropActive] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  // The arrangement switch (window move + FLIP) and the View menu's toggle item.
  useEffect(() => {
    const uninstall = installLayoutSwitcher(() => rootRef.current);
    const unlisten = WindowLayoutBridge.onToggleRequest(() => void appModel.toggleLayoutMode());
    return () => {
      uninstall();
      unlisten();
    };
  }, []);
  // About / Acknowledgements overlays, opened from the application menu (main
  // process) — the About panel is the way into Acknowledgements (docs/LICENSING §3).
  // COUPLE_ACK_TEST opens Acknowledgements at startup for a harness screenshot.
  const [panel, setPanel] = useState<'about' | 'acknowledgements' | null>(() =>
    env.COUPLE_ACK_TEST ? 'acknowledgements' : null
  );
  useEffect(() => {
    const showAbout = () => setPanel('about');
    const showAck = () => setPanel('acknowledgements');
    ipcRenderer.on('couple:show-about', showAbout);
    ipcRenderer.on('couple:show-acknowledgements', showAck);
    return () => {
      ipcRenderer.removeListener('couple:show-about', showAbout);
      ipcRenderer.removeListener('couple:show-acknowledgements', showAck);
    };
  }, []);

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
      // COUPLE_COMPACT_TEST / COUPLE_LAYOUT_TOGGLE_TEST (prod no-ops).
      runLayoutHarnesses(appModel);
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
  // queue order, space/l play-pause, a/b monitor, esc stops audition. `v` is
  // UNBOUND (UX §V2.12 / D4: inclusion is the row checkbox only). Waveform-local
  // keys (nudges, brackets) live on the stage itself, which compact does not show.
  // Compact (UX §V2.11): Return switches to full with the selected couple's editor
  // open; `l`, ⌘⌫ and the pool's Delete are no-ops (their surfaces are absent).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
      const ordered = appModel.orderedCouples;
      const idx = ordered.findIndex((c) => c.id === appModel.selectedID);
      const inCompact = appModel.layoutMode === 'compact';
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
        case 'Enter':
          if (inCompact && !e.metaKey && !e.ctrlKey) {
            void appModel.openFullEditor();
            e.preventDefault();
          }
          break;
        case 'Backspace':
        case 'Delete':
          // ⌘⌫ clears the whole session (TODO-001); confirms with the counts.
          if (e.metaKey && !inCompact) {
            appModel.requestClearSession();
            e.preventDefault();
          }
          break;
        case ' ':
        case 'l': {
          if (e.key === 'l' && inCompact) break; // no loupes / editor in compact
          const c = appModel.selected;
          // The couple's own session, or a candidate audition streaming for it.
          if (c && (c.hasSession || appModel.audition.couple === c)) {
            if (!appModel.audition.attached || appModel.audition.couple !== c) {
              appModel.attachAudition(c);
            }
            appModel.audition.toggle();
          }
          e.preventDefault();
          break;
        }
        case 'a': {
          // A/B: camera ⇄ external solo (UX-DECISIONS 2026-11-03; InputRouter.swift "a").
          const c = appModel.selected;
          if (c && c.hasSession) appModel.audition.flipMonitor();
          break;
        }
        case 'b': {
          // Back to hearing both (the pan/overlay mode used last).
          const c = appModel.selected;
          if (c && c.hasSession) appModel.audition.restoreBothMonitor();
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
      ref={rootRef}
      className={`root ${compact ? 'layout-compact' : 'layout-full'} platform-${platform}${dropActive ? ' drop-active' : ''}`}
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
      <Arrangement />
      <ActivityStrip />
      {!compact && (
      <div className="footer-bar full-only">
        <span
          style={{
            fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
            fontSize: 10,
            color: 'rgba(216, 218, 222, 0.32)',
          }}
        >
          j/k move · space play · ⚙ sync tools · ◫ loupes · a camera/external · b both · esc stop · ⌘⇧C compact
          <span style={{ marginLeft: 24 }}>
            ←/→ offset 1 ms · ⇧ 10 · ⌥ 0.1 · [ ] drift · drag loupe = offset · ⇧-drag = pivot
          </span>
        </span>
        <span style={{ marginLeft: 'auto', fontSize: 10, color: 'rgba(216, 218, 222, 0.32)' }}>
          waveforms render real peak data
        </span>
      </div>
      )}
      <ToastStack />
      {panel === 'about' && (
        <AboutView onClose={() => setPanel(null)} onAcknowledgements={() => setPanel('acknowledgements')} />
      )}
      {panel === 'acknowledgements' && <AcknowledgementsView onClose={() => setPanel(null)} />}
    </div>
  );
}
