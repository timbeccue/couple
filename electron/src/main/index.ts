// Electron main process — deliberately thin (the plan's process model): window,
// menus, native dialogs, reveal-in-file-manager. The engine lives in the
// renderer (nodeIntegration) so hot-path calls stay synchronous in-process.
import { app, BrowserWindow, dialog, ipcMain, protocol, shell } from 'electron';
import { createReadStream, existsSync } from 'node:fs';
import { stat } from 'node:fs/promises';
import { extname, join } from 'node:path';
import { Readable } from 'node:stream';

function nativeDir(): string {
  // Dev: the repo's electron/native (the main bundle runs from out/main).
  // Packaged: unpacked next to the asar.
  return app.isPackaged
    ? join(process.resourcesPath, 'native')
    : join(__dirname, '..', '..', 'native');
}

function ffmpegDir(): string | null {
  if (!app.isPackaged) return null; // dev: core falls back to COUPLE_FFMPEG / $PATH
  // Bundled per-platform ffmpeg is optional (resources/ffmpeg/<platform>/);
  // when absent the core's discovery chain (COUPLE_FFMPEG / $PATH) applies.
  const dir = join(process.resourcesPath, 'ffmpeg');
  return existsSync(dir) ? dir : null;
}

function createWindow(): void {
  const win = new BrowserWindow({
    width: 1280,
    height: 820,
    minWidth: 980,
    minHeight: 600,
    backgroundColor: '#161719',
    titleBarStyle: process.platform === 'darwin' ? 'hiddenInset' : 'default',
    webPreferences: {
      // Local-only app, no remote content: the renderer hosts the engine so
      // band()/audition calls stay synchronous (no IPC on the paint path).
      nodeIntegration: true,
      contextIsolation: false,
      sandbox: false,
    },
  });

  if (process.env.ELECTRON_RENDERER_URL) {
    void win.loadURL(process.env.ELECTRON_RENDERER_URL);
  } else {
    void win.loadFile(join(__dirname, '../renderer/index.html'));
  }

  // Dev-harness hooks (the Electron twin of the Swift DebugDrive env family):
  // COUPLE_SHOT_PATH captures a window PNG after COUPLE_SHOT_DELAY_MS (default
  // 3000); COUPLE_AUTOQUIT_MS exits after that many ms. The renderer reads
  // COUPLE_OPEN_DIR itself and ingests it at startup.
  const shotPath = process.env.COUPLE_SHOT_PATH;
  if (shotPath) {
    const delay = Number(process.env.COUPLE_SHOT_DELAY_MS ?? 3000);
    setTimeout(async () => {
      try {
        const image = await win.webContents.capturePage();
        const { writeFileSync } = await import('node:fs');
        writeFileSync(shotPath, image.toPNG());
        console.log(`[harness] screenshot -> ${shotPath}`);
      } catch (e) {
        console.error('[harness] screenshot failed', e);
      }
    }, delay);
  }
  const autoquit = process.env.COUPLE_AUTOQUIT_MS;
  if (autoquit) {
    setTimeout(() => app.quit(), Number(autoquit));
  }
}

// Persistence root: honor a COUPLE_USER_DATA override (test isolation / a shared
// dir across relaunches) BEFORE anything reads userData, so the engine cache and
// the queue/session stores all resolve under it. Must precede any getPath call.
if (process.env.COUPLE_USER_DATA) app.setPath('userData', process.env.COUPLE_USER_DATA);

// Renderer-facing environment: where the native binding and (packaged) ffmpeg
// live, plus the per-app engine cache dir and the resolved userData root (the
// queue/session stores write under it). Env is inherited by the renderer.
process.env.COUPLE_NATIVE_DIR = nativeDir();
process.env.COUPLE_USER_DATA = app.getPath('userData');
process.env.COUPLE_CACHE_DIR = join(app.getPath('userData'), 'engine-cache');
const ff = ffmpegDir();
if (ff) {
  const exe = process.platform === 'win32' ? '.exe' : '';
  process.env.COUPLE_FFMPEG_PATH = join(ff, `ffmpeg${exe}`);
  process.env.COUPLE_FFPROBE_PATH = join(ff, `ffprobe${exe}`);
}

// Native dialogs + reveal stay main-process (the only IPC in the app).
ipcMain.handle('couple:pick-folder', async (_e, title: string) => {
  const r = await dialog.showOpenDialog({ title, properties: ['openDirectory', 'createDirectory'] });
  return r.canceled ? null : r.filePaths[0];
});
ipcMain.handle('couple:pick-media', async () => {
  const r = await dialog.showOpenDialog({
    title: 'Add media',
    properties: ['openFile', 'openDirectory', 'multiSelections'],
  });
  return r.canceled ? [] : r.filePaths;
});
ipcMain.handle('couple:reveal', (_e, path: string) => {
  shell.showItemInFolder(path);
});

// couple-media://: local media for <video>/<audio>/thumbnails. Chromium blocks
// file:// subresources on the dev server's http origin, so both dev and
// packaged builds load media through this scheme instead. Range headers are
// forwarded so the elements can seek.
protocol.registerSchemesAsPrivileged([
  {
    scheme: 'couple-media',
    // `standard` + `stream` are what let <video>/<audio> consume the scheme;
    // without `standard` the media stack rejects it (readyState stays 0).
    // `corsEnabled` + the handler's ACAO header let a <video crossorigin>
    // load succeed (thumbnail frame grabs need an untainted canvas; without
    // corsEnabled a CORS-mode request to the scheme dies as a format error).
    privileges: { standard: true, secure: true, stream: true, supportFetchAPI: true, corsEnabled: true },
  },
]);

const MEDIA_MIME: Record<string, string> = {
  '.mp4': 'video/mp4',
  '.m4v': 'video/mp4',
  // .mov is ISO-BMFF; Chromium REJECTS the truthful video/quicktime type
  // outright (canPlayType('video/quicktime') is ''), but demuxes the same
  // bytes fine under video/mp4.
  '.mov': 'video/mp4',
  '.mkv': 'video/x-matroska',
  '.webm': 'video/webm',
  '.wav': 'audio/wav',
  '.aif': 'audio/aiff',
  '.aiff': 'audio/aiff',
  '.mp3': 'audio/mpeg',
  '.m4a': 'audio/mp4',
  '.flac': 'audio/flac',
  '.ogg': 'audio/ogg',
};

app.whenReady().then(() => {
  // Serve local media with REAL range support: <video>/<audio> stream via 206
  // partial responses, and Chromium's demuxer read-errors without them.
  protocol.handle('couple-media', async (request) => {
    const filePath = decodeURIComponent(new URL(request.url).pathname).replace(/^\/+/, '/');
    if (process.env.COUPLE_MEDIA_DEBUG) {
      console.log('[media]', request.url, '→', filePath, 'range:', request.headers.get('range'));
    }
    let size: number;
    try {
      size = (await stat(filePath)).size;
    } catch {
      return new Response('not found', { status: 404 });
    }
    const type = MEDIA_MIME[extname(filePath).toLowerCase()] ?? 'application/octet-stream';
    // Streamed range serving: return the WHOLE requested range as a stream —
    // Chromium consumes what it needs and aborts the rest (short 206 chunks
    // are NOT re-requested on custom schemes). Requires the scheme's
    // `standard` + `stream` privileges to be honored by the media stack.
    const range = /bytes=(\d+)-(\d*)/.exec(request.headers.get('range') ?? '');
    const start = range ? Number(range[1]) : 0;
    if (start >= size) {
      return new Response(null, { status: 416, headers: { 'Content-Range': `bytes */${size}` } });
    }
    const end = range && range[2] ? Math.min(Number(range[2]), size - 1) : size - 1;
    const body = Readable.toWeb(
      createReadStream(filePath, { start, end })
    ) as unknown as ReadableStream;
    return new Response(body, {
      status: range ? 206 : 200,
      headers: {
        ...(range ? { 'Content-Range': `bytes ${start}-${end}/${size}` } : {}),
        'Accept-Ranges': 'bytes',
        'Content-Length': String(end - start + 1),
        'Content-Type': type,
        // The renderer page origin (dev http://localhost, packaged file://) is
        // cross-origin to this scheme, so a <video crossorigin> frame grab
        // (thumbnails) taints its canvas without this — playback doesn't care,
        // toDataURL does.
        'Access-Control-Allow-Origin': '*',
      },
    });
  });
  createWindow();
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit();
});
