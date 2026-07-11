// Loader for the couple-node native binding. The binary is produced by
// `npm run build:native` (scripts/build-native.mjs), which copies the cargo
// cdylib here under a per-platform name. COUPLE_NODE_BINARY overrides (CI,
// cross-target tests).
'use strict';

const { existsSync } = require('fs');
const { join } = require('path');

const candidates = [
  process.env.COUPLE_NODE_BINARY,
  join(__dirname, `couple-node.${process.platform}-${process.arch}.node`),
].filter(Boolean);

const found = candidates.find((p) => existsSync(p));
if (!found) {
  throw new Error(
    `couple-node binary not found (looked at: ${candidates.join(', ')}). ` +
      'Run `npm run build:native` in electron/ first.'
  );
}
module.exports = require(found);
