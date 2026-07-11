// Tiny observation base — the TS stand-in for Swift's @Observable model
// classes. Models mutate in place and call bump(); React views subscribe via
// useModel (useSyncExternalStore on a version counter).
import { useSyncExternalStore } from 'react';

export class Observable {
  private version = 0;
  private listeners = new Set<() => void>();

  /** Notify subscribers that this model changed. */
  protected bump(): void {
    this.version += 1;
    for (const l of this.listeners) l();
  }

  /** Public form for controllers that drive a model they don't subclass. */
  invalidate(): void {
    this.bump();
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  snapshot = (): number => this.version;
}

/** Re-render whenever `model` bumps. Returns the model itself for chaining. */
export function useModel<T extends Observable>(model: T): T {
  useSyncExternalStore(model.subscribe, model.snapshot);
  return model;
}
