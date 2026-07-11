// Overlay scrollbars (mac parity): the mac shell shows macOS overlay bars —
// invisible at rest, a thin transient thumb only while the surface is actually
// scrolling. Chromium paints a persistent bar instead, so we mark a surface
// `.scrolling` while it is in motion and clear the class ~900ms after it stops;
// styles.css paints the thumb only for `.scrolling` (and on hover). Presentation
// only — no engine coupling (PLAN-CORE-SHELL-BOUNDARY §1).
const IDLE_MS = 900;
const timers = new WeakMap<Element, number>();

function onScroll(e: Event): void {
  const el = e.target;
  if (!(el instanceof Element)) return; // document/window scroll targets are non-Element
  el.classList.add('scrolling');
  const prev = timers.get(el);
  if (prev !== undefined) window.clearTimeout(prev);
  timers.set(
    el,
    window.setTimeout(() => el.classList.remove('scrolling'), IDLE_MS)
  );
}

export function installOverlayScrollbars(): void {
  // Scroll events don't bubble, so listen in the capture phase to catch every surface.
  document.addEventListener('scroll', onScroll, { capture: true, passive: true });
}
