// Runtime loader for the couple-node binding. The renderer runs with
// nodeIntegration, so Electron's require is available; the binding is loaded
// from COUPLE_NATIVE_DIR (set by the main process) — never bundled by vite.
import type * as CoupleNode from '../../../../native/index';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const nodeRequire: NodeRequire = (window as any).require;

/** The renderer's REAL environment. Vite statically substitutes `process.env.*`
 * in client bundles, so env reads must go through the runtime require. */
export const env: NodeJS.ProcessEnv = (
  nodeRequire('node:process') as typeof import('node:process')
).env;

export const platform: NodeJS.Platform = (
  nodeRequire('node:process') as typeof import('node:process')
).platform;

const nativeDir = env.COUPLE_NATIVE_DIR;
if (!nativeDir) {
  throw new Error('COUPLE_NATIVE_DIR is not set (main process must set it before window load)');
}

const path = nodeRequire('node:path') as typeof import('node:path');
export const native: typeof CoupleNode = nodeRequire(path.join(nativeDir, 'index.cjs'));

export type { ProgressCallback, BandInfo, AuditionFillInfo } from '../../../../native/index';
