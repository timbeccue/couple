// layoutFlip.ts — the animated arrangement switch (UX §V2.11; PLAN-COMPACT-MODE
// Phase 5/6). The reference is design/mockups-compact/index.html `toggleFullD`:
// FLIP the four blocks (session line, preview, couple list, export panel) —
// measure their boxes, switch the CSS layout, measure again, pin each block at its
// old box, then release so a ~0.5 s transition carries it to its new place, while
// the window frame animates (main: `setBounds`). Surfaces unique to one
// arrangement (overview, loupes bar, Files, media pool, footer) fade in after.
// Both arrangements are ONE DOM tree (CompactLayout.tsx), so nothing remounts: the
// <video> element and the audition keep playing through the switch. Under
// `prefers-reduced-motion: reduce` the switch is instant.
import { flushSync } from 'react-dom';
import { appModel } from '../app/AppModel';
import { LAYOUT_ANIMATION_MS, type LayoutMode } from '../app/layoutMode';
import { WindowLayoutBridge } from '../app/WindowLayout';

const BLOCKS = ['hd', 'pv', 'ls', 'ex'] as const;

interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

function measure(root: HTMLElement): Partial<Record<(typeof BLOCKS)[number], Box>> {
  const base = root.getBoundingClientRect();
  const out: Partial<Record<(typeof BLOCKS)[number], Box>> = {};
  for (const k of BLOCKS) {
    const el = root.querySelector<HTMLElement>(`[data-blk="${k}"]`);
    if (!el) continue;
    const r = el.getBoundingClientRect();
    out[k] = { left: r.left - base.left, top: r.top - base.top, width: r.width, height: r.height };
  }
  return out;
}

function place(el: HTMLElement, b: Box): void {
  el.style.left = `${b.left}px`;
  el.style.top = `${b.top}px`;
  el.style.width = `${b.width}px`;
  el.style.height = `${b.height}px`;
}

function reducedMotion(): boolean {
  try {
    return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  } catch {
    return false;
  }
}

let busy = false;

/** Install the switcher the model calls (`appModel.requestLayoutMode`). Resolves
 * once the blocks have arrived (the window may still be settling). */
export function installLayoutSwitcher(getRoot: () => HTMLElement | null): () => void {
  const switcher = async (mode: LayoutMode): Promise<void> => {
    if (busy || mode === appModel.layoutMode) return;
    const root = getRoot();
    if (!root || reducedMotion()) {
      flushSync(() => appModel.setLayoutMode(mode));
      WindowLayoutBridge.apply(mode, false);
      return;
    }
    busy = true;
    try {
      const first = measure(root);
      const target = await WindowLayoutBridge.target(mode);
      // Lay the incoming arrangement out at its FINAL size (the window gets there
      // over the animation), synchronously, so the last boxes are known.
      flushSync(() => appModel.setLayoutMode(mode));
      if (target) {
        root.style.width = `${target.width}px`;
        root.style.height = `${target.height}px`;
      }
      void root.offsetWidth;
      const last = measure(root);
      root.classList.add('animating');
      const els = BLOCKS.map((k) => [k, root.querySelector<HTMLElement>(`[data-blk="${k}"]`)] as const);
      // Pin every block at its old box.
      for (const [k, el] of els) {
        if (!el) continue;
        const f = first[k] ?? last[k];
        if (!f) continue;
        el.style.transition = 'none';
        place(el, f);
      }
      void root.offsetWidth;
      const arrived = settled(mode, 1500); // listen before main can answer
      WindowLayoutBridge.apply(mode, true); // the window frame starts moving now
      await new Promise<void>((resolve) => {
        requestAnimationFrame(() => {
          // Release: the transitions carry every block to its new box.
          for (const [k, el] of els) {
            if (!el) continue;
            const l = last[k];
            el.style.transition = '';
            if (l) place(el, l);
          }
          setTimeout(resolve, LAYOUT_ANIMATION_MS + 40);
        });
      });
      root.classList.remove('animating');
      for (const [, el] of els) el?.removeAttribute('style');
      // Keep the final size until the window has arrived, then follow the viewport.
      await arrived;
      root.style.width = '';
      root.style.height = '';
    } finally {
      busy = false;
    }
  };
  appModel.layoutSwitcher = switcher;
  return () => {
    if (appModel.layoutSwitcher === switcher) appModel.layoutSwitcher = null;
  };
}

/** Resolve when main reports the window frame arrived (or after `timeoutMs`). */
function settled(mode: LayoutMode, timeoutMs: number): Promise<void> {
  return new Promise((resolve) => {
    const { ipcRenderer } = (window as unknown as { require: NodeRequire }).require('electron') as typeof import('electron');
    let done = false;
    const finish = (): void => {
      if (done) return;
      done = true;
      ipcRenderer.removeListener('couple:layout-settled', onSettled);
      resolve();
    };
    const onSettled = (_e: unknown, m: LayoutMode): void => {
      if (m === mode) finish();
    };
    ipcRenderer.on('couple:layout-settled', onSettled);
    setTimeout(finish, timeoutMs);
  });
}
