// UndoStack.test.ts (F5a) — the queue-mutation undo/redo idiom: a registered
// inverse lands on redo while undoing and back on undo while redoing, and a
// fresh registration clears the redo timeline. Pure; no binding needed.
import { describe, it, expect } from 'vitest';
import { UndoStack } from './UndoStack';

/** A symmetric undoable counter: each set re-registers the inverse (prior value),
 * so undo/redo alternate through the same mechanism (the AppModel idiom). */
function counter(stack: UndoStack) {
  const box = { value: 0 };
  const set = (v: number): void => {
    const old = box.value;
    box.value = v;
    stack.register('set', () => set(old));
  };
  return { box, set };
}

describe('UndoStack', () => {
  it('U1 register→undo runs the inverse once and toggles canUndo/canRedo', () => {
    const stack = new UndoStack();
    const { box, set } = counter(stack);
    expect(stack.canUndo).toBe(false);
    set(5);
    expect(stack.canUndo).toBe(true);
    expect(stack.canRedo).toBe(false);
    stack.undo();
    expect(box.value).toBe(0);
    expect(stack.canUndo).toBe(false);
    expect(stack.canRedo).toBe(true);
  });

  it('U2 an inverse registered during undo lands on redo and replays symmetrically', () => {
    const stack = new UndoStack();
    const { box, set } = counter(stack);
    set(1);
    set(2);
    expect(box.value).toBe(2);
    stack.undo();
    expect(box.value).toBe(1);
    expect(stack.canRedo).toBe(true);
    stack.redo();
    expect(box.value).toBe(2);
    expect(stack.canUndo).toBe(true);
    stack.undo();
    expect(box.value).toBe(1);
  });

  it('U3 a fresh registration clears the redo stack', () => {
    const stack = new UndoStack();
    const { set } = counter(stack);
    set(1);
    stack.undo();
    expect(stack.canRedo).toBe(true);
    set(9); // a new action invalidates the redo timeline
    expect(stack.canRedo).toBe(false);
    expect(stack.canUndo).toBe(true);
  });

  it('U4 clear() empties both stacks', () => {
    const stack = new UndoStack();
    const { set } = counter(stack);
    set(1);
    stack.undo();
    stack.clear();
    expect(stack.canUndo).toBe(false);
    expect(stack.canRedo).toBe(false);
  });
});
