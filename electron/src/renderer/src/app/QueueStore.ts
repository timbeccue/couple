// QueueStore.ts — queue persistence (SPEC-V2 §5.4); TS port of
// mac/Sources/Couple/QueueStore.swift. Writes the SAME versioned `couple-queue/1`
// file the mac shell does (media-pool file list, each couple's resolution +
// pinned/forced + export overrides, the include-for-export inputs — the user's
// explicit toggle + `exportedAt`, PLAN-COMPACT-MODE D8 — and the export
// destination/format), under userData/Couple/queue.json, so reopening the app
// restores the whole queue. Saved on the v1 triggers (include toggle / export /
// quit) plus pairing changes.
import type { SelectionProvenance } from '../kit/Policies';
import type { AppModel } from './AppModel';
import { userDataDir } from './SessionStore';

const nodeRequire: NodeRequire = (window as unknown as { require: NodeRequire }).require;
const fs = nodeRequire('node:fs') as typeof import('node:fs');
const pathMod = nodeRequire('node:path') as typeof import('node:path');

export interface SavedFile {
  path: string;
  contentKey: string;
  role: 'video' | 'audio';
}

export interface SavedCouple {
  videoKey: string;
  selectedAudioKey: string | null;
  pinned: boolean;
  forced: boolean;
  stemOverride: string | null;
  // TODO-009 (optional, back-compat): a pre-009 file lacks these and decodes to
  // undefined; applyRestore then infers provenance from `pinned`. No schema bump.
  selectionProvenance?: SelectionProvenance;
  manualMatchConfident?: boolean | null;
  // PLAN-COMPACT-MODE D8: the include-for-export inputs, both OPTIONAL with the same
  // back-compat idiom (schema stays `couple-queue/1`). A file saved under the approve
  // model has neither (its `approved` key is simply not read), so it loads as
  // null/null ⇒ the policy default: every good couple included, no export history.
  explicit?: boolean | null;
  /** ISO 8601 without fractional seconds — the form the mac shell's `.iso8601`
   * coder reads and writes. */
  exportedAt?: string | null;
}

export interface SavedExport {
  destination: string | null;
  globalFormat: string;
  keepCameraAudio: boolean;
}

export interface SavedQueue {
  schema: string;
  savedAt: string;
  files: SavedFile[];
  couples: SavedCouple[];
  export: SavedExport;
}

const SCHEMA = 'couple-queue/1';

/** `new Date()` as ISO 8601 at whole seconds ("2026-11-05T08:14:03Z") — the mac
 * shell's JSONEncoder `.iso8601` form, which rejects fractional seconds. */
export function isoNow(date: Date = new Date()): string {
  return date.toISOString().replace(/\.\d{3}Z$/, 'Z');
}

function queueFile(): string {
  const dir = pathMod.join(userDataDir(), 'Couple');
  try {
    fs.mkdirSync(dir, { recursive: true });
  } catch {
    /* best-effort */
  }
  return pathMod.join(dir, 'queue.json');
}

export const QueueStore = {
  save(model: AppModel): void {
    // Don't clobber the saved session with an empty one mid-teardown.
    if (model.couples.length === 0 && model.pool.isEmpty) return;
    const files: SavedFile[] = model.pool.entries.map((e) => ({
      path: e.path,
      contentKey: e.id,
      role: e.role,
    }));
    const couples: SavedCouple[] = model.couples.map((c) => ({
      videoKey: c.id,
      selectedAudioKey: c.selectedAudioKey,
      pinned: c.pinned,
      forced: c.forced,
      stemOverride: c.stemOverride,
      selectionProvenance: c.selectionProvenance,
      manualMatchConfident: c.manualMatchConfident,
      explicit: c.explicit,
      exportedAt: c.exportedAt,
    }));
    const saved: SavedQueue = {
      schema: SCHEMA,
      savedAt: new Date().toISOString(),
      files,
      couples,
      export: {
        destination: model.exportDestination,
        globalFormat: model.globalFormat,
        keepCameraAudio: model.globalKeepCameraAudio,
      },
    };
    try {
      fs.writeFileSync(queueFile(), JSON.stringify(saved, null, 2));
    } catch {
      /* persistence is best-effort — never break the edit that triggered it */
    }
  },

  /** Remove the persisted restore record (Clear Session, TODO-001). Deleting the
   * file IS persisting the cleared state: `load()` then returns null. Idempotent. */
  clear(): void {
    try {
      fs.rmSync(queueFile(), { force: true });
    } catch {
      /* idempotent */
    }
  },

  /** The saved queue, if one exists whose files are all still on disk. */
  load(): SavedQueue | null {
    try {
      const saved = JSON.parse(fs.readFileSync(queueFile(), 'utf8')) as SavedQueue;
      if (saved.schema !== SCHEMA || saved.files.length === 0) return null;
      // Only offer restore if every file still exists (paths are stable).
      if (!saved.files.every((f) => fs.existsSync(f.path))) return null;
      return saved;
    } catch {
      return null;
    }
  },
};
