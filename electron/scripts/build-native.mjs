// Build the couple-node napi cdylib and place it where native/index.cjs looks:
// native/couple-node.<platform>-<arch>.node. Pure cargo — no napi CLI, no
// electron-rebuild (N-API is ABI-stable, the same binary serves Node and Electron).
import { execSync } from 'node:child_process';
import { copyFileSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..');

// npm >= 11.20 skips package install scripts unless approved, so electron's
// postinstall (which downloads the Electron binary into dist/) may not have
// run after `npm ci`. Every `npm run` target goes through this script, so
// fetch it here rather than failing later with "Electron uninstall".
const electronPkg = join(here, '..', 'node_modules', 'electron');
if (existsSync(electronPkg) && !existsSync(join(electronPkg, 'dist'))) {
  console.log('electron binary missing (install script skipped); fetching it');
  execSync('node install.js', { cwd: electronPkg, stdio: 'inherit' });
}

const profile = process.env.COUPLE_NATIVE_PROFILE === 'debug' ? 'debug' : 'release';
const flags = profile === 'release' ? '--release' : '';
execSync(`cargo build ${flags} -p couple-node`, { cwd: repoRoot, stdio: 'inherit' });

const libName = {
  darwin: 'libcouple_node.dylib',
  linux: 'libcouple_node.so',
  win32: 'couple_node.dll',
}[process.platform];
if (!libName) throw new Error(`unsupported platform: ${process.platform}`);

const built = join(repoRoot, 'target', profile, libName);
if (!existsSync(built)) throw new Error(`expected cargo output missing: ${built}`);

const dest = join(here, '..', 'native', `couple-node.${process.platform}-${process.arch}.node`);
copyFileSync(built, dest);
console.log(`couple-node -> ${dest}`);
