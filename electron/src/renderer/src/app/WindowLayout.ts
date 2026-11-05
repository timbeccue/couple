// WindowLayout.ts — the renderer half of the arrangement switch (UX §V2.11;
// PLAN-COMPACT-MODE Phase 6, twin of mac/Sources/Couple/WindowLayout.swift). One
// window, two arrangements of the same session. The MAIN process owns the window
// frame: it remembers each arrangement's last frame and the mode (persisted under
// userData/Couple/window-layout.json; COMPACT when absent — the first launch after
// the update opens compact, then last-used), clamps compact to 340–420 wide with a
// free height (min 560) and full to min 640 × 400, and animates `setBounds`. The
// renderer asks for the target content size first (so it can lay the incoming
// arrangement out at its final size for the FLIP), then tells main to move.
// Under a dev-harness run main forces the arrangement and never persists it.
import type { LayoutMode } from './layoutMode';

export type { LayoutMode } from './layoutMode';

interface Ipc {
  sendSync(channel: string, ...args: unknown[]): unknown;
  invoke(channel: string, ...args: unknown[]): Promise<unknown>;
  send(channel: string, ...args: unknown[]): void;
  on(channel: string, listener: (...args: unknown[]) => void): void;
  removeListener(channel: string, listener: (...args: unknown[]) => void): void;
}

/** Electron's ipcRenderer, or null outside Electron (vitest's node shim). */
function ipc(): Ipc | null {
  try {
    const electron = (window as unknown as { require: NodeRequire }).require('electron') as {
      ipcRenderer?: Ipc;
    };
    const r = electron?.ipcRenderer;
    return r && typeof r.sendSync === 'function' ? r : null;
  } catch {
    return null;
  }
}

export interface ContentSize {
  width: number;
  height: number;
}

/** A window frame in screen coordinates (main's `BrowserWindow.getBounds()`). */
export interface Frame {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface LayoutState {
  mode: LayoutMode;
  bounds: Frame;
  minSize: [number, number];
  maxSize: [number, number];
  frames: Partial<Record<LayoutMode, Frame>>;
  persist: boolean;
}

export const WindowLayoutBridge = {
  /** The arrangement the window was created in (main decides; compact default). */
  initialMode(): LayoutMode {
    const r = ipc();
    if (!r) return 'compact';
    try {
      const v = r.sendSync('couple:layout-get') as { mode?: string } | null;
      return v?.mode === 'full' ? 'full' : 'compact';
    } catch {
      return 'compact';
    }
  },

  /** The content size the window will have in `mode` (its last frame, clamped). */
  async target(mode: LayoutMode): Promise<ContentSize | null> {
    const r = ipc();
    if (!r) return null;
    try {
      return (await r.invoke('couple:layout-target', mode)) as ContentSize;
    } catch {
      return null;
    }
  },

  /** Move the window to `mode`'s frame (animated unless `animate` is false) and
   * apply that arrangement's resize limits; main persists the mode. */
  apply(mode: LayoutMode, animate: boolean): void {
    ipc()?.send('couple:layout-apply', { mode, animate });
  },

  /** Window state for the harness (frames, limits, mode as main sees it). */
  async state(): Promise<LayoutState | null> {
    const r = ipc();
    if (!r) return null;
    try {
      return (await r.invoke('couple:layout-state')) as LayoutState;
    } catch {
      return null;
    }
  },

  /** Harness: set the window frame directly (a distinctive frame to restore). */
  async setBounds(frame: Frame): Promise<void> {
    const r = ipc();
    if (!r) return;
    try {
      await r.invoke('couple:layout-set-bounds', frame);
    } catch {
      /* best-effort */
    }
  },

  /** The View menu's Compact Window / Full Window item (⌘⇧C / Ctrl+Shift+C). */
  onToggleRequest(listener: () => void): () => void {
    const r = ipc();
    if (!r) return () => {};
    const l = (): void => listener();
    r.on('couple:toggle-layout', l);
    return () => r.removeListener('couple:toggle-layout', l);
  },
};
