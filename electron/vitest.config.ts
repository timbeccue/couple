// Vitest over the renderer's app/kit layer (F5a). Runs in plain node against the
// REAL couple-node binding (built by `npm run build:native`), so every
// cpl_policy_* decision asserted is core's own — no re-encoded policy tables in
// mocks. vitest.setup.ts supplies the window/env shims the modules expect.
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/renderer/src/**/*.test.ts'],
    setupFiles: ['./vitest.setup.ts'],
  },
});
