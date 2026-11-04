// Bundled third-party notices (docs/LICENSING.md §3) — the Electron twin of
// mac/Sources/Couple/Licenses.swift: locate the Licenses folder, list the
// components, split each notices file into its `==== name version` sections.
// Pure loading + parsing of what scripts/third-party-notices.sh generated;
// nothing here decides anything.
//
// Packaged layout (electron-builder.yml extraResources):
//   resources/Licenses/components.json, rust-crates.txt, npm-packages.txt,
//                      swift-packages.txt
//   resources/ffmpeg/   optional LGPL ffmpeg with SOURCE.txt and its license
//                      texts (in licenses/ for a fetch-ffmpeg.sh build, or
//                      alongside the binaries for a win/linux lgpl build)
//   resources/LICENSE.electron.txt, LICENSES.chromium.html — placed by
//                      electron-builder; pointed at, not rendered (huge).
// The main process hands the renderer the resolved folders as
// COUPLE_LICENSES_DIR and COUPLE_FFMPEG_LICENSE_DIR (dev: the repo's
// third-party/; no ffmpeg).
import { env } from '../kit/native';

const nodeRequire: NodeRequire = (window as unknown as { require: NodeRequire }).require;
const fs = nodeRequire('node:fs') as typeof import('node:fs');
const path = nodeRequire('node:path') as typeof import('node:path');

export interface LicenseComponent {
  id: string;
  name: string;
  version: string;
  license: string;
  /** Set when the declared license is an OR and a single alternative was chosen. */
  usedUnder: string | null;
  source: string | null;
}

/** One `==== name version` block of a notices file. */
export interface NoticeSection {
  id: string;
  title: string;
  /** `License:` / `Used under:` / `Source:` lines, as written. */
  header: string[];
  body: string;
}

/** One file the Acknowledgements view can show in full. */
export interface NoticeDocument {
  id: string;
  fileName: string;
  sections: NoticeSection[];
}

/** A group in the component list and the documents holding its notices. */
export interface NoticeTree {
  id: string;
  title: string;
  components: LicenseComponent[];
  documents: NoticeDocument[];
  /** Which document a component's section lives in, by component id. */
  documentByComponent: Record<string, string>;
}

export interface Acknowledgements {
  folder: string | null;
  trees: NoticeTree[];
}

interface Manifest {
  trees: { id: string; title: string; file: string }[];
  components: {
    name: string;
    version: string;
    license: string;
    usedUnder?: string;
    tree: string;
    source?: string;
  }[];
}

function readText(file: string): string | null {
  try {
    return fs.readFileSync(file, 'utf8');
  } catch {
    return null;
  }
}

export function licensesFolder(): string | null {
  return env.COUPLE_LICENSES_DIR || null;
}

export function loadAcknowledgements(): Acknowledgements {
  const folder = licensesFolder();
  const trees: NoticeTree[] = [];
  const ffmpeg = loadFFmpeg(env.COUPLE_FFMPEG_LICENSE_DIR || null);
  if (ffmpeg) trees.push(ffmpeg);
  if (folder && fs.existsSync(folder)) trees.push(...loadGenerated(folder));
  return { folder, trees };
}

export function isEmpty(a: Acknowledgements): boolean {
  return a.trees.every((t) => t.components.length === 0);
}

// --- ffmpeg (optional bundle; files written by mac/Scripts/fetch-ffmpeg.sh or
// placed by hand per docs/LICENSING.md §2) ---

function loadFFmpeg(dir: string | null): NoticeTree | null {
  if (!dir) return null;
  const source = readText(path.join(dir, 'SOURCE.txt'));
  if (source === null) return null;
  // First line: "ffmpeg/ffprobe 9.0.2 — LGPL v2.1 or later build for macOS arm64".
  const firstLine = source.split('\n', 1)[0] ?? '';
  const version = firstLine.split(' ').find((w) => /^\d/.test(w)) ?? '';
  const documents: NoticeDocument[] = [];
  for (const name of ['COPYING.LGPLv2.1', 'LICENSE.md', 'SOURCE.txt']) {
    const text = readText(path.join(dir, 'licenses', name)) ?? readText(path.join(dir, name));
    if (text === null) continue;
    const id = `ffmpeg/${name}`;
    documents.push({ id, fileName: name, sections: [{ id, title: name, header: [], body: text }] });
  }
  if (documents.length === 0) return null;
  return {
    id: 'ffmpeg',
    title: 'ffmpeg',
    components: [
      {
        id: 'ffmpeg',
        name: 'ffmpeg / ffprobe',
        version,
        license: 'LGPL-2.1-or-later',
        usedUnder: null,
        source: 'https://ffmpeg.org',
      },
    ],
    documents,
    documentByComponent: { ffmpeg: documents[0].id },
  };
}

// --- generated trees (components.json + one notices file each) ---

function loadGenerated(folder: string): NoticeTree[] {
  const raw = readText(path.join(folder, 'components.json'));
  if (raw === null) return [];
  let manifest: Manifest;
  try {
    manifest = JSON.parse(raw) as Manifest;
  } catch {
    return [];
  }
  return manifest.trees.map((tree) => {
    const components: LicenseComponent[] = manifest.components
      .filter((c) => c.tree === tree.id)
      .map((c) => ({
        id: `${tree.id}/${c.name}@${c.version}`,
        name: c.name,
        version: c.version,
        license: c.license,
        usedUnder: c.usedUnder && c.usedUnder !== c.license ? c.usedUnder : null,
        source: c.source ?? null,
      }));
    const text = readText(path.join(folder, tree.file)) ?? '';
    const document: NoticeDocument = { id: tree.file, fileName: tree.file, sections: parseSections(text, tree.id) };
    const documentByComponent: Record<string, string> = {};
    for (const c of components) documentByComponent[c.id] = document.id;
    return { id: tree.id, title: tree.title, components, documents: [document], documentByComponent };
  });
}

/** Splits a generated notices file into its `==== name version` sections. The
 * preamble before the first marker becomes a section of its own. Section ids
 * match `LicenseComponent.id` so a list selection can scroll to its text. */
export function parseSections(text: string, prefix: string): NoticeSection[] {
  const sections: NoticeSection[] = [];
  let title = 'About this file';
  let id = `${prefix}/preamble`;
  let header: string[] = [];
  let body: string[] = [];
  let inHeader = false;
  const flush = () => {
    const joined = body.join('\n').trim();
    if (joined.length > 0 || header.length > 0) sections.push({ id, title, header, body: joined });
  };
  for (const line of text.split('\n')) {
    if (line.startsWith('==== ')) {
      flush();
      title = line.slice(5);
      const space = title.indexOf(' ');
      id = `${prefix}/${space < 0 ? title : title.slice(0, space)}@${space < 0 ? '' : title.slice(space + 1)}`;
      header = [];
      body = [];
      inHeader = true;
    } else if (inHeader) {
      if (line === '----') inHeader = false;
      else header.push(line);
    } else {
      body.push(line);
    }
  }
  flush();
  return sections;
}
