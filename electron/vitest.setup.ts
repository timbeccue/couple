// vitest.setup.ts — node-env shims so the renderer's app/kit modules load
// outside Electron: window.require (native.ts / the stores), window.confirm,
// COUPLE_NATIVE_DIR (the real built binding), and an isolated COUPLE_USER_DATA
// so persistence tests never touch a real profile. Runs before each test file.
import { mkdtempSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
process.env.COUPLE_NATIVE_DIR = join(here, 'native');
process.env.COUPLE_USER_DATA = mkdtempSync(join(tmpdir(), 'couple-vitest-'));
process.env.COUPLE_CACHE_DIR = join(process.env.COUPLE_USER_DATA, 'engine-cache');
delete process.env.COUPLE_OPEN_DIR;

const req = createRequire(import.meta.url);
(globalThis as unknown as { window: unknown }).window = Object.assign(
  (globalThis as { window?: object }).window ?? {},
  { require: req, confirm: () => true }
);
