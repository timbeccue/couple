// SessionStore.ts — per-pair alignment autosave/restore (SPEC §11.5, §14 6.8);
// TS port of mac/Sources/Couple/SessionStore.swift. Writes the SAME versioned
// `couple-session/1` file the mac shell does, keyed by the pair's content keys,
// under userData/Couple/sessions. A restored file lets a reopened pair recover
// its offset/drift/anchors WITHOUT a fresh sync.
import { env } from '../kit/native';
import type { AlignmentState } from '../kit/Wire';

const nodeRequire: NodeRequire = (window as unknown as { require: NodeRequire }).require;
const fs = nodeRequire('node:fs') as typeof import('node:fs');
const pathMod = nodeRequire('node:path') as typeof import('node:path');

export interface TrackRef {
  path: string;
  contentKey: string;
  durationS: number;
}

export interface SessionFile {
  schema: string;
  savedAt: string;
  camera: TrackRef;
  external: TrackRef;
  alignment: AlignmentState;
}

const SCHEMA = 'couple-session/1';

/** userData, as resolved by the main process (COUPLE_USER_DATA); falls back to
 * the engine cache's parent, then cwd — enough for a headless run. */
export function userDataDir(): string {
  if (env.COUPLE_USER_DATA) return env.COUPLE_USER_DATA;
  if (env.COUPLE_CACHE_DIR) return pathMod.dirname(env.COUPLE_CACHE_DIR);
  return '.';
}

function sessionsDir(): string {
  const dir = pathMod.join(userDataDir(), 'Couple', 'sessions');
  try {
    fs.mkdirSync(dir, { recursive: true });
  } catch {
    /* best-effort; write below surfaces a real failure */
  }
  return dir;
}

function fileFor(cameraKey: string, externalKey: string): string {
  return pathMod.join(sessionsDir(), `${cameraKey}_${externalKey}.json`);
}

export const SessionStore = {
  save(camera: TrackRef, external: TrackRef, alignment: AlignmentState): void {
    const file: SessionFile = {
      schema: SCHEMA,
      savedAt: new Date().toISOString(),
      camera,
      external,
      alignment,
    };
    try {
      fs.writeFileSync(fileFor(camera.contentKey, external.contentKey), JSON.stringify(file, null, 2));
    } catch {
      /* autosave is best-effort — a failed write must never break the edit */
    }
  },

  load(cameraKey: string, externalKey: string): SessionFile | null {
    try {
      const data = fs.readFileSync(fileFor(cameraKey, externalKey), 'utf8');
      const file = JSON.parse(data) as SessionFile;
      return file.schema === SCHEMA ? file : null;
    } catch {
      return null;
    }
  },

  /** Wipe every per-pair session file (Clear Session, TODO-001). */
  clearAll(): void {
    try {
      fs.rmSync(pathMod.join(userDataDir(), 'Couple', 'sessions'), { recursive: true, force: true });
    } catch {
      /* idempotent */
    }
  },
};
