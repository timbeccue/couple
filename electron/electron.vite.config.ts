import { defineConfig } from 'electron-vite';
import react from '@vitejs/plugin-react';
import { resolve } from 'node:path';

export default defineConfig({
  main: {
    build: {
      rollupOptions: {
        input: { index: resolve(__dirname, 'src/main/index.ts') },
      },
    },
  },
  // No preload: the renderer runs with nodeIntegration (local-only app; the
  // engine's hot paths must be synchronous in-process calls — see the plan).
  renderer: {
    plugins: [react()],
    build: {
      rollupOptions: {
        input: { index: resolve(__dirname, 'src/renderer/index.html') },
        // The native binding is loaded at runtime via require() (Electron's
        // node integration), never bundled.
        external: [/couple-node.*\.node$/],
      },
    },
  },
});
