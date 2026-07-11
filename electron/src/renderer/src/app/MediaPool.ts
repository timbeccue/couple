// MediaPool.ts — the imported-media pool (SPEC-V2 §5.1); TS port of
// mac/Sources/Couple/MediaPool.swift. Identity is the content key (same file
// dropped twice dedups to one entry; a live entry pins its cache content).
import type { Engine, TrackRole } from '../kit/Engine';
import type { TrackHandle } from '../kit/Track';
import type { MediaInfo } from '../kit/Wire';
import { Observable } from './observable';
import type { ToastCenter } from './Toasts';

export type PoolRole = 'video' | 'audio';

export interface ImportSignal {
  phase: 'probing' | 'decoding' | 'waveform' | 'finishing';
  fraction: number;
  index: number; // 1-based file position in the batch
  total: number;
  filename: string;
}

export class PoolEntry {
  readonly id: string; // content key
  readonly path: string;
  readonly name: string;
  readonly role: PoolRole;
  readonly durationS: number;
  readonly creationTimeUtc: string | null;
  readonly info: MediaInfo | null;
  /** On-disk byte size captured at import via fs.statSync (TODO-003); `null` if the
   * filesystem did not report it. The metadata popover reads it — never re-probed. */
  readonly fileSizeBytes: number | null;
  handle: TrackHandle | null;

  constructor(args: {
    id: string;
    path: string;
    name: string;
    role: PoolRole;
    durationS: number;
    creationTimeUtc: string | null;
    info: MediaInfo | null;
    fileSizeBytes: number | null;
    handle: TrackHandle | null;
  }) {
    this.id = args.id;
    this.path = args.path;
    this.name = args.name;
    this.role = args.role;
    this.durationS = args.durationS;
    this.creationTimeUtc = args.creationTimeUtc;
    this.info = args.info;
    this.fileSizeBytes = args.fileSizeBytes;
    this.handle = args.handle;
  }
}

const nodeRequire: NodeRequire = (window as unknown as { require: NodeRequire }).require;
const fs = nodeRequire('node:fs') as typeof import('node:fs');
const pathMod = nodeRequire('node:path') as typeof import('node:path');

export class MediaPool extends Observable {
  readonly engine: Engine;
  private readonly toasts: ToastCenter;

  /** Pool order = import order (display only; resolution orders itself). */
  entries: PoolEntry[] = [];
  skipped: string[] = [];
  /** Gate for the import line: cleared synchronously when a batch ends. */
  importing = false;
  importActivity: ImportSignal | null = null;

  constructor(engine: Engine, toasts: ToastCenter) {
    super();
    this.engine = engine;
    this.toasts = toasts;
  }

  get videos(): PoolEntry[] {
    return this.entries.filter((e) => e.role === 'video');
  }

  /** Audio entries that imported successfully (handle = pairing precondition). */
  get audios(): PoolEntry[] {
    return this.entries.filter((e) => e.role === 'audio' && e.handle !== null);
  }

  get isEmpty(): boolean {
    return this.entries.length === 0;
  }

  entry(key: string): PoolEntry | undefined {
    return this.entries.find((e) => e.id === key);
  }

  /** Import a batch of dropped files/folders; returns the newly added entries. */
  /** Serializes concurrent drops: two batches importing the same file at once
   * would race the core's cache write for that content key. The mac app gets
   * this ordering from MainActor serialization; here batches chain. */
  private importChain: Promise<unknown> = Promise.resolve();

  importDropped(paths: string[]): Promise<PoolEntry[]> {
    const run = this.importChain.then(() => this.importBatch(paths));
    // The chain itself never rejects (a failed batch surfaces via its caller).
    this.importChain = run.catch(() => undefined);
    return run;
  }

  private async importBatch(paths: string[]): Promise<PoolEntry[]> {
    const files = MediaPool.expand(paths);
    this.importing = true;
    this.bump();
    const added: PoolEntry[] = [];
    const newlySkipped: string[] = [];
    try {
      const total = files.length;
      for (let idx = 0; idx < files.length; idx++) {
        const file = files[idx];
        const name = pathMod.basename(file);
        if (this.entries.some((e) => e.path === file)) continue;

        this.importActivity = { phase: 'probing', fraction: 0, index: idx + 1, total, filename: name };
        this.bump();

        let info: MediaInfo;
        try {
          info = this.engine.probe(file);
        } catch {
          newlySkipped.push(name);
          continue;
        }
        const role: PoolRole = info.video !== null ? 'video' : 'audio';
        if (role === 'audio' && info.audio.length === 0) {
          newlySkipped.push(name);
          continue;
        }

        try {
          const meta = await this.importTrack(file, role === 'video' ? 'camera' : 'external', name, idx + 1, total);
          // True content dedup: same bytes already pooled via another path.
          if (this.entries.some((e) => e.id === meta.contentKey)) {
            this.engine.track(meta).free();
            continue;
          }
          const entry = new PoolEntry({
            id: meta.contentKey,
            path: file,
            name,
            role,
            durationS: meta.durationS,
            creationTimeUtc: info.creationTimeUtc,
            info,
            fileSizeBytes: MediaPool.fileSize(file),
            handle: this.engine.track(meta),
          });
          this.entries.push(entry);
          added.push(entry);
          this.bump();
        } catch {
          newlySkipped.push(name);
        }
      }
      if (newlySkipped.length > 0) {
        this.skipped.push(...newlySkipped);
        this.toasts.info(
          `Skipped ${newlySkipped.length} unrecognized file${newlySkipped.length === 1 ? '' : 's'}`,
          newlySkipped.join(', ')
        );
      }
      return added;
    } finally {
      this.importing = false;
      this.importActivity = null;
      this.bump();
    }
  }

  private importTrack(path: string, role: TrackRole, filename: string, index: number, total: number) {
    const job = this.engine.importStart(path, role);
    job.onProgress((p) => {
      this.importActivity = {
        phase: MediaPool.importPhase(p.phase),
        fraction: p.fraction,
        index,
        total,
        filename,
      };
      this.bump();
    });
    return job.value;
  }

  static importPhase(phase: string): ImportSignal['phase'] {
    switch (phase) {
      case 'Probe':
        return 'probing';
      case 'Decode':
        return 'decoding';
      case 'Pyramid':
        return 'waveform';
      default:
        return 'finishing';
    }
  }

  /** On-disk size in bytes via fs.statSync (TODO-003) — a structured API, never
   * inferred. `null` if the filesystem does not report it. */
  static fileSize(path: string): number | null {
    try {
      return fs.statSync(path).size;
    } catch {
      return null;
    }
  }

  /** Flatten dropped folders one level deep into plain files, dotfiles ignored. */
  static expand(paths: string[]): string[] {
    const out: string[] = [];
    for (const p of paths) {
      try {
        const st = fs.statSync(p);
        if (st.isDirectory()) {
          for (const n of fs.readdirSync(p).sort()) {
            if (n.startsWith('.')) continue;
            const child = pathMod.join(p, n);
            if (fs.statSync(child).isFile()) out.push(child);
          }
        } else if (st.isFile() && !pathMod.basename(p).startsWith('.')) {
          out.push(p);
        }
      } catch {
        // unreadable path — surfaced by probe skip later if it matters
      }
    }
    return out;
  }

  /** Free every handle (app teardown). The pool is the sole owner of pool tracks. */
  teardown(): void {
    for (const e of this.entries) e.handle?.free();
    this.entries = [];
    this.importing = false;
    this.importActivity = null;
    this.bump();
  }

  /** Remove content keys from the open session; returns keys actually removed. */
  remove(keys: Set<string>): Set<string> {
    const removed = new Set<string>();
    if (keys.size === 0) return removed;
    this.entries = this.entries.filter((e) => {
      if (!keys.has(e.id)) return true;
      e.handle?.free();
      removed.add(e.id);
      return false;
    });
    this.bump();
    return removed;
  }
}
