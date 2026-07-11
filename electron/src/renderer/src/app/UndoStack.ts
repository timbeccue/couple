// UndoStack.ts — the queue-mutation undo/redo stack (TODO-015); the TS stand-in
// for the AppKit UndoManager the mac shell drives (⌘Z / ⇧⌘Z). It covers ONLY
// cheaply-invertible queue mutations — approve/unapprove, re-pair / candidate
// pick, and export-name rename — never import (whose inverse is a re-import) or
// alignment edits (the core session is the source of truth there).
//
// Redo comes free from the UndoManager idiom: while UNDOING, a registered inverse
// lands on the REDO stack; while REDOING, it lands back on the UNDO stack; a
// fresh (normal) registration clears the redo stack. So each mutation registers
// exactly one closure that restores the prior state and re-registers its own
// inverse, and undo/redo alternate through the same mechanism.

type Mode = 'normal' | 'undo' | 'redo';

interface Entry {
  name: string;
  run: () => void;
}

export class UndoStack {
  private undoEntries: Entry[] = [];
  private redoEntries: Entry[] = [];
  private mode: Mode = 'normal';

  /** Register the inverse of the mutation that just happened. `run` must restore
   * the prior state AND re-register its own inverse (symmetry gives redo). */
  register(name: string, run: () => void): void {
    const entry: Entry = { name, run };
    if (this.mode === 'undo') {
      this.redoEntries.push(entry);
    } else if (this.mode === 'redo') {
      this.undoEntries.push(entry);
    } else {
      this.undoEntries.push(entry);
      this.redoEntries = []; // a new action invalidates the redo timeline
    }
  }

  get canUndo(): boolean {
    return this.undoEntries.length > 0;
  }

  get canRedo(): boolean {
    return this.redoEntries.length > 0;
  }

  undo(): void {
    const entry = this.undoEntries.pop();
    if (!entry) return;
    this.mode = 'undo';
    try {
      entry.run();
    } finally {
      this.mode = 'normal';
    }
  }

  redo(): void {
    const entry = this.redoEntries.pop();
    if (!entry) return;
    this.mode = 'redo';
    try {
      entry.run();
    } finally {
      this.mode = 'normal';
    }
  }

  /** Discard both stacks (Clear Session, or a session teardown). */
  clear(): void {
    this.undoEntries = [];
    this.redoEntries = [];
    this.mode = 'normal';
  }
}
