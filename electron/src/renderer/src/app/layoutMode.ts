// layoutMode.ts — the two arrangements of the one window (UX §V2.11). Shared by
// the renderer and the main process (main imports this file too), so the frame
// constants cannot diverge (WindowLayout.swift twins).
export type LayoutMode = 'compact' | 'full';

export const COMPACT_DEFAULT_SIZE = { width: 360, height: 700 };
export const FULL_DEFAULT_SIZE = { width: 1280, height: 820 };
export const COMPACT_MIN_SIZE = { width: 340, height: 560 };
export const COMPACT_MAX_WIDTH = 420;
export const FULL_MIN_SIZE = { width: 640, height: 400 };
/** The FLIP's duration (design/mockups-compact `toggleFullD`: 0.5 s). */
export const LAYOUT_ANIMATION_MS = 500;
