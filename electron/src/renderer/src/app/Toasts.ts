// Toasts.ts — transient notification center; TS port of Toasts.swift (minimal).
import { Observable } from './observable';

export interface Toast {
  id: number;
  style: 'info' | 'success' | 'error';
  title: string;
  detail?: string;
  /** Optional action button (e.g. Reveal in Finder). */
  action?: { label: string; run: () => void };
}

export class ToastCenter extends Observable {
  toasts: Toast[] = [];
  private nextId = 1;

  show(style: Toast['style'], title: string, detail?: string, action?: Toast['action']): void {
    const t: Toast = { id: this.nextId++, style, title, detail, action };
    this.toasts.push(t);
    this.bump();
    setTimeout(() => this.dismiss(t.id), 6000);
  }

  info(title: string, detail?: string): void {
    this.show('info', title, detail);
  }

  success(title: string, detail?: string, action?: Toast['action']): void {
    this.show('success', title, detail, action);
  }

  error(title: string, detail?: string): void {
    this.show('error', title, detail);
  }

  dismiss(id: number): void {
    const before = this.toasts.length;
    this.toasts = this.toasts.filter((t) => t.id !== id);
    if (this.toasts.length !== before) this.bump();
  }
}
