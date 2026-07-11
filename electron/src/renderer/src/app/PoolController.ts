// PoolController.ts — media-pool selection + preview + removal (owner decision 3;
// TODO-001/002); TS port of mac/Sources/Couple/PoolController.swift. Owns the
// Finder-style pool selection/primary/anchor, the sticky preview player, and the
// per-file removal flow. AppModel keeps the couples list, audition, and export
// settings; this drives them through the back-reference, and re-runs the SAME
// resolver path (via `model.ingest`) so removal regenerates couples exactly as a
// fresh import would. Views reach these members through `appModel.poolController`.
//
// The pure selection transition (PoolSelection.swift) and the pool-list height
// clamp (PoolRailLayout.swift) are ported verbatim as the module-level pure
// functions below — decisions live in a testable, framework-free layer
// (PLAN-CORE-SHELL-BOUNDARY §1); this class only applies their results + side effects.
import type { AppModel } from './AppModel';
import type { PoolEntry } from './MediaPool';
import { Observable } from './observable';
import { PoolPreviewController } from './PoolPreview';
import * as Policies from '../kit/Policies';

// ---------- PoolSelection (pure; PoolSelection.swift port) ----------

/** Which modifier drove the click (Finder semantics): plain replace / ⌘ toggle /
 * ⇧ contiguous range over the displayed order. */
export type PoolSelectModifier = 'plain' | 'command' | 'shift';

export interface PoolSelectionResult {
  selection: Set<string>;
  primary: string | null;
  anchor: string | null;
}

/** Decide the new (selection, primary, anchor) for a pool click (TODO-001). A click
 * on a key absent from `ordered` is inert (returns the input unchanged). Ported from
 * CoupleKit/PoolSelection.swift — the invariants are locked there. */
export function poolSelect(args: {
  current: Set<string>;
  primary: string | null;
  anchor: string | null;
  clicked: string;
  ordered: string[];
  modifier: PoolSelectModifier;
}): PoolSelectionResult {
  const { current, primary, anchor, clicked, ordered, modifier } = args;
  // A click can only ever name a live pool entry; an unknown key is a true no-op.
  if (!ordered.includes(clicked)) return { selection: current, primary, anchor };

  switch (modifier) {
    case 'plain':
      // Replace everything; the clicked entry is selection, primary, and anchor.
      return { selection: new Set([clicked]), primary: clicked, anchor: clicked };

    case 'command': {
      const next = new Set(current);
      if (next.has(clicked)) {
        next.delete(clicked); // toggle OFF
        if (next.size === 0) return { selection: new Set(), primary: null, anchor: null };
        // Removing a NON-primary keeps the primary (and anchor); only when the
        // removed key WAS the primary do we re-home it to the nearest remaining
        // member in display order, and the anchor follows the new primary.
        if (clicked === primary) {
          const newPrimary = nearestSelected(clicked, ordered, next) ?? firstSelected(ordered, next);
          return { selection: next, primary: newPrimary, anchor: newPrimary };
        }
        const keptAnchor = anchor !== null && next.has(anchor) ? anchor : primary;
        return { selection: next, primary, anchor: keptAnchor };
      }
      // Toggle ON: add, and the freshly-added entry leads (primary + anchor).
      next.add(clicked);
      return { selection: next, primary: clicked, anchor: clicked };
    }

    case 'shift': {
      // Inclusive contiguous range in displayed order from the anchor to the clicked
      // key; the anchor is preserved so the range can be re-dragged. With no usable
      // anchor, ⇧ degrades to a plain click.
      const ai = anchor !== null ? ordered.indexOf(anchor) : -1;
      const ci = ordered.indexOf(clicked);
      if (ai < 0 || ci < 0) return { selection: new Set([clicked]), primary: clicked, anchor: clicked };
      const lo = Math.min(ai, ci);
      const hi = Math.max(ai, ci);
      return { selection: new Set(ordered.slice(lo, hi + 1)), primary: clicked, anchor };
    }
  }
}

/** The selected key nearest to `key` in display order (ties resolve to the earlier
 * one), used to re-home the primary when a ⌘-click removes it. */
function nearestSelected(key: string, ordered: string[], selection: Set<string>): string | null {
  const idx = ordered.indexOf(key);
  if (idx < 0) return null;
  let best: string | null = null;
  let bestDist = Number.MAX_SAFE_INTEGER;
  for (let i = 0; i < ordered.length; i++) {
    if (!selection.has(ordered[i])) continue;
    const d = Math.abs(i - idx);
    if (d < bestDist) {
      bestDist = d;
      best = ordered[i];
    }
  }
  return best;
}

function firstSelected(ordered: string[], selection: Set<string>): string | null {
  return ordered.find((k) => selection.has(k)) ?? null;
}

// ---------- PoolRailLayout (pure; PoolRailLayout.swift port) ----------

/** The v2.7 fixed cap, now the FLOOR: the pool list never shrinks below the original
 * "a few rows" height (SPEC-CHANGE 2026-07-01). */
export const POOL_MIN_LIST_HEIGHT = 168;

/** The pool list's height budget: at least the floor, growing into rail height the
 * couples list leaves unused (`available - couplesContent`), bounded by the pool's
 * own content so the box never inflates past its rows. */
export function poolListHeight(available: number, couplesContent: number, poolContent: number): number {
  const spare = available - couplesContent;
  return Math.max(POOL_MIN_LIST_HEIGHT, Math.min(poolContent, spare));
}

// ---------- PoolController ----------

export class PoolController extends Observable {
  private readonly model: AppModel;
  /** The pool's sticky preview player — the single source of truth for what plays
   * (SELECT loads it paused; the play button starts it). */
  readonly preview = new PoolPreviewController();

  /** Finder-style multi-select highlight set (owner decision 3). */
  poolSelection: Set<string> = new Set();
  /** The single last-clicked entry the sticky player follows. */
  poolPrimary: string | null = null;
  /** The ⇧-range start. */
  private poolAnchor: string | null = null;

  constructor(model: AppModel) {
    super();
    this.model = model;
    // The sticky bar reads the preview state; forward its bumps so the rail repaints.
    this.preview.subscribe(() => this.bump());
  }

  /** The primary entry as a pool entry (the sticky bar reads name/path/duration off
   * it). */
  get poolPrimaryEntry(): PoolEntry | undefined {
    return this.poolPrimary !== null ? this.model.pool.entry(this.poolPrimary) : undefined;
  }

  // ---------- selection (owner decision 3; TODO-001 foundation) ----------

  /** Finder-style pool click (owner decision 3): map the modifiers to the pure
   * `poolSelect` transition, apply the (selection, primary, anchor), then follow the
   * SINGLE primary — a changed primary loads it into the sticky bar PAUSED (SELECT =
   * load-paused, decision 1); a nil primary stops the preview. */
  selectPool(key: string, modifier: PoolSelectModifier): void {
    const result = poolSelect({
      current: this.poolSelection,
      primary: this.poolPrimary,
      anchor: this.poolAnchor,
      clicked: key,
      ordered: this.model.pool.entries.map((e) => e.id),
      modifier,
    });
    const primaryChanged = result.primary !== this.poolPrimary;
    this.poolSelection = result.selection;
    this.poolPrimary = result.primary;
    this.poolAnchor = result.anchor;
    this.bump();
    if (primaryChanged) this.followPrimary();
  }

  /** Drive the sticky preview to the current primary: load-paused for a real entry,
   * stop when there is none. The single place primary→player flows. */
  private followPrimary(): void {
    const entry = this.poolPrimaryEntry;
    if (!entry) {
      this.preview.stop();
      return;
    }
    this.preview.select(entry.id, entry.path, entry.durationS);
  }

  /** Drop selection/primary/anchor that no longer name a live pool entry, re-homing
   * the player if the primary vanished (the removal path relies on this). */
  prunePoolSelection(): void {
    const live = new Set(this.model.pool.entries.map((e) => e.id));
    const prunedSelection = new Set([...this.poolSelection].filter((k) => live.has(k)));
    const prunedPrimary =
      this.poolPrimary !== null && live.has(this.poolPrimary)
        ? this.poolPrimary
        : (this.model.pool.entries.find((e) => prunedSelection.has(e.id))?.id ?? null);
    const prunedAnchor =
      this.poolAnchor !== null && live.has(this.poolAnchor) ? this.poolAnchor : prunedPrimary;
    const primaryChanged = prunedPrimary !== this.poolPrimary;
    this.poolSelection = prunedSelection;
    this.poolPrimary = prunedPrimary;
    this.poolAnchor = prunedAnchor;
    this.bump();
    if (primaryChanged) this.followPrimary();
  }

  // ---------- remove media (TODO-001) ----------

  /** Couples projected for the pure removal policy. */
  private get coupleRefs(): Policies.RemovalCouple[] {
    return this.model.couples.map((c) => ({
      videoKey: c.id,
      selectedAudioKey: c.selectedAudioKey,
      approved: c.approved,
    }));
  }

  /** Remove the currently selected pool media from the open session (TODO-001) — the
   * SHARED entry point for the trash button and the Delete key. The pure
   * `mediaRemovalPlan` decides: an empty plan no-ops; an UNUSED-only removal happens
   * immediately; a removal touching ≥1 referenced file asks ONE confirmation stating
   * the affected couple/approved counts. Never deletes source media from disk. */
  removeSelectedMedia(): void {
    const plan = Policies.mediaRemovalPlan({
      selectedKeys: [...this.poolSelection],
      poolKeys: this.model.pool.entries.map((e) => e.id),
      couples: this.coupleRefs,
    });
    if (plan.removableKeys.length === 0) return;
    if (plan.needsConfirmation) {
      const couples = `${plan.affectedCouples} couple${plan.affectedCouples === 1 ? '' : 's'}`;
      const approved =
        plan.affectedApproved > 0 ? ` (${plan.affectedApproved} approved)` : '';
      if (!window.confirm(`Remove the selected media? This re-resolves ${couples}${approved}.`)) return;
    }
    this.performMediaRemoval(new Set(plan.removableKeys));
  }

  /** Apply a removal of `keys` from the open session (TODO-001). Ordering mirrors
   * PoolController.swift.performMediaRemoval: (1) stop work touching a removed key
   * before any handle is freed, (2) drop removed couples + reset rows whose SELECTED
   * audio vanished, (3) free the pool handles, then (4) re-run the SAME resolver path
   * the importer uses so survivors regenerate exactly as a fresh import would. */
  performMediaRemoval(keys: Set<string>): void {
    if (keys.size === 0) return;
    const model = this.model;

    // (1) Stop the preview if it follows a removed entry, BEFORE freeing handles.
    const active = this.preview.state.activeKey;
    if (active !== null && keys.has(active)) this.preview.stop();

    // (2) Tear down each couple whose video OR selected audio is leaving, so its core
    // session (and the shared audition, if selected) closes before its tracks are
    // freed. A removed-video couple is dropped; a removed-audio couple is reset to a
    // clean unresolved row so the re-resolve treats it as never-paired.
    const survivingCouples: typeof model.couples = [];
    for (const couple of model.couples) {
      if (keys.has(couple.id)) {
        if (couple.id === model.selectedID) model.audition.teardown();
        couple.pair.teardown();
        continue;
      }
      const audioRemoved = couple.selectedAudioKey !== null && keys.has(couple.selectedAudioKey);
      if (audioRemoved) {
        if (couple.id === model.selectedID) model.audition.teardown();
        couple.pair.teardown();
        couple.selectedAudioKey = null;
        couple.pinned = false;
        couple.forced = false;
        couple.selectionProvenance = 'none';
        couple.manualMatchConfident = null;
      }
      // Drop any candidate (selected or alternate) naming a removed key.
      couple.candidates = couple.candidates.filter((c) => !keys.has(c.audioKey));
      survivingCouples.push(couple);
    }
    model.couples = survivingCouples;
    if (model.selectedID !== null && keys.has(model.selectedID)) model.selectedID = null;

    // (3) Free the pool handles for the removed entries (releases the cache-GC pin;
    // not a cache purge). Source files on disk are untouched.
    model.pool.remove(keys);

    // Drop removed keys from the pool selection/primary/anchor + re-home the player.
    this.prunePoolSelection();

    // (4) Re-resolve over the REMAINING media via the normal resolver path.
    if (model.pool.isEmpty) {
      // Nothing left to resolve: empty the queue (same end state as clearing).
      model.couples = [];
      model.selectedID = null;
      model.invalidate();
    } else {
      void model.ingest.resolveQueue();
    }
    model.invalidate();
  }

  /** Test-only removal (COUPLE_REMOVE_TEST): compute + log the plan, then perform the
   * removal WITHOUT the confirmation dialog (headless runs cannot answer it). */
  debugRemove(keys: Set<string>): void {
    const plan = Policies.mediaRemovalPlan({
      selectedKeys: [...keys],
      poolKeys: this.model.pool.entries.map((e) => e.id),
      couples: this.coupleRefs,
    });
    // eslint-disable-next-line no-console
    console.log(
      `[remove-test] plan removableKeys=${plan.removableKeys.length} needsConfirmation=${plan.needsConfirmation} affectedCouples=${plan.affectedCouples} affectedApproved=${plan.affectedApproved}`
    );
    this.performMediaRemoval(new Set(plan.removableKeys));
    // eslint-disable-next-line no-console
    console.log(
      `[remove-test] after pool.entries=${this.model.pool.entries.length} couples=${this.model.couples.length} selection=${this.poolSelection.size}`
    );
  }

  /** App teardown: stop the preview player. */
  teardown(): void {
    this.preview.stop();
    this.poolSelection = new Set();
    this.poolPrimary = null;
    this.poolAnchor = null;
    this.bump();
  }
}
