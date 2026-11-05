// Electron main process — deliberately thin (the plan's process model): window,
// menus, native dialogs, reveal-in-file-manager. The engine lives in the
// renderer (nodeIntegration) so hot-path calls stay synchronous in-process.
import {
  app,
  BrowserWindow,
  dialog,
  ipcMain,
  Menu,
  protocol,
  screen,
  shell,
  type MenuItemConstructorOptions,
  type Rectangle,
} from 'electron';
import { createReadStream, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { stat } from 'node:fs/promises';
import { extname, join } from 'node:path';
import { Readable } from 'node:stream';
import {
  COMPACT_DEFAULT_SIZE,
  COMPACT_MAX_WIDTH,
  COMPACT_MIN_SIZE,
  FULL_DEFAULT_SIZE,
  FULL_MIN_SIZE,
  LAYOUT_ANIMATION_MS,
  type LayoutMode,
} from '../renderer/src/app/layoutMode';

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

function licensesDir(): string {
  // Dev: the repo's third-party/ (scripts/third-party-notices.sh output).
  // Packaged: resources/Licenses (electron-builder.yml extraResources).
  return app.isPackaged
    ? join(process.resourcesPath, 'Licenses')
    : join(__dirname, '..', '..', '..', 'third-party');
}

function windowIcon(): string | undefined {
  // macOS and Windows take the window/dock/taskbar icon from the bundle/exe
  // (electron-builder's build/icon.{icns,ico}); only Linux needs one on the
  // BrowserWindow. Dev: the repo's electron/build/icon.png (the main bundle
  // runs from out/main). Packaged: resources/icon.png (extraResources).
  if (process.platform !== 'linux') return undefined;
  return app.isPackaged
    ? join(process.resourcesPath, 'icon.png')
    : join(__dirname, '..', '..', 'build', 'icon.png');
}

/** Application menu: the defaults by role, with About routed to the
 * renderer's own panel (which links to Acknowledgements) instead of the
 * native about box, and Acknowledgements listed under Help as well. */
function installMenu(): void {
  // View ▸ Compact Window / Full Window (⌘⇧C / Ctrl+Shift+C) — the same action as
  // the session line's ⤢ Full / ⤡ Compact (UX §V2.11); the renderer runs the
  // switch (window move + the blocks' FLIP).
  const layoutItem: MenuItemConstructorOptions = {
    label: layout.mode === 'compact' ? 'Full Window' : 'Compact Window',
    accelerator: 'CmdOrCtrl+Shift+C',
    click: () => {
      const win = BrowserWindow.getFocusedWindow() ?? BrowserWindow.getAllWindows()[0];
      win?.webContents.send('couple:toggle-layout');
    },
  };
  const viewMenu: MenuItemConstructorOptions = {
    label: 'View',
    submenu: [
      layoutItem,
      { type: 'separator' },
      { role: 'reload' },
      { role: 'forceReload' },
      { role: 'toggleDevTools' },
      { type: 'separator' },
      { role: 'resetZoom' },
      { role: 'zoomIn' },
      { role: 'zoomOut' },
      { type: 'separator' },
      { role: 'togglefullscreen' },
    ],
  };
  const send = (channel: string) => () => {
    const win = BrowserWindow.getFocusedWindow() ?? BrowserWindow.getAllWindows()[0];
    win?.webContents.send(channel);
  };
  const about: MenuItemConstructorOptions = { label: 'About Couple', click: send('couple:show-about') };
  const acknowledgements: MenuItemConstructorOptions = {
    label: 'Acknowledgements',
    click: send('couple:show-acknowledgements'),
  };
  const mac = process.platform === 'darwin';
  const template: MenuItemConstructorOptions[] = [
    ...(mac
      ? [
          {
            label: 'Couple',
            submenu: [
              about,
              { type: 'separator' },
              { role: 'services' },
              { type: 'separator' },
              { role: 'hide' },
              { role: 'hideOthers' },
              { role: 'unhide' },
              { type: 'separator' },
              { role: 'quit' },
            ],
          } as MenuItemConstructorOptions,
        ]
      : []),
    { role: 'fileMenu' },
    { role: 'editMenu' },
    viewMenu,
    { role: 'windowMenu' },
    { role: 'help', submenu: mac ? [acknowledgements] : [acknowledgements, about] },
  ];
  Menu.setApplicationMenu(Menu.buildFromTemplate(template));
}

// ---------- arrangement + window frame (UX §V2.11; PLAN-COMPACT-MODE Phase 6) ----------
// One window, two arrangements. Main owns the frame: each arrangement's last frame
// and the mode persist in userData/Couple/window-layout.json (mode COMPACT when
// absent — the first launch after the update opens compact, then last-used).
// Compact: default 360 × 700, width 340–420, height free (min 560). Full: default
// 1280 × 820, min 640 × 400. A dev-harness run forces the arrangement (full for
// every existing COUPLE_*_TEST, compact for COMPACT / LAYOUT_TOGGLE) and never
// writes the file, so a harness run cannot leak into the user's layout.

interface LayoutFile {
  mode?: LayoutMode;
  frames?: Partial<Record<LayoutMode, Rectangle>>;
}

const harnessRun =
  Object.keys(process.env).some((k) => /^COUPLE_[A-Z_]+_TEST$/.test(k) && !!process.env[k]) ||
  !!process.env.COUPLE_SHOT_PATH ||
  !!process.env.COUPLE_OPEN_DIR ||
  !!process.env.COUPLE_AUTOQUIT_MS;

const layout = {
  mode: 'compact' as LayoutMode,
  frames: {} as Partial<Record<LayoutMode, Rectangle>>,
  persist: !harnessRun,
  /** Set while main itself moves the window: frame events are not recorded. */
  applying: false,
};

function layoutFile(): string {
  return join(app.getPath('userData'), 'Couple', 'window-layout.json');
}

function loadLayout(): void {
  if (harnessRun) {
    const compactRun = !!process.env.COUPLE_COMPACT_TEST || !!process.env.COUPLE_LAYOUT_TOGGLE_TEST;
    layout.mode = compactRun ? 'compact' : 'full';
    return;
  }
  try {
    const saved = JSON.parse(readFileSync(layoutFile(), 'utf8')) as LayoutFile;
    layout.mode = saved.mode === 'full' ? 'full' : 'compact';
    for (const m of ['compact', 'full'] as LayoutMode[]) {
      const f = saved.frames?.[m];
      if (f && f.width > 0 && f.height > 0) layout.frames[m] = f;
    }
  } catch {
    layout.mode = 'compact'; // nothing stored ⇒ the first launch after the update
  }
}

function saveLayout(): void {
  if (!layout.persist) return;
  try {
    mkdirSync(join(app.getPath('userData'), 'Couple'), { recursive: true });
    writeFileSync(layoutFile(), JSON.stringify({ mode: layout.mode, frames: layout.frames }, null, 2));
  } catch {
    /* best-effort */
  }
}

/** Resize limits per arrangement: [min w, min h], [max w, max h] (0 = unbounded). */
function limitsFor(mode: LayoutMode): { min: [number, number]; max: [number, number] } {
  return mode === 'compact'
    ? { min: [COMPACT_MIN_SIZE.width, COMPACT_MIN_SIZE.height], max: [COMPACT_MAX_WIDTH, 0] }
    : { min: [FULL_MIN_SIZE.width, FULL_MIN_SIZE.height], max: [0, 0] };
}

/** The frame the window takes for `mode`: its last frame (clamped), else the
 * default size anchored at the current top-left; kept on the current display. */
function targetBounds(mode: LayoutMode, current: Rectangle | null): Rectangle {
  const size = mode === 'compact' ? COMPACT_DEFAULT_SIZE : FULL_DEFAULT_SIZE;
  let f: Rectangle = layout.frames[mode] ?? {
    x: current?.x ?? 0,
    y: current?.y ?? 0,
    width: size.width,
    height: size.height,
  };
  if (mode === 'compact') {
    f = {
      ...f,
      width: Math.min(Math.max(f.width, COMPACT_MIN_SIZE.width), COMPACT_MAX_WIDTH),
      height: Math.max(f.height, COMPACT_MIN_SIZE.height),
    };
  } else {
    f = { ...f, width: Math.max(f.width, FULL_MIN_SIZE.width), height: Math.max(f.height, FULL_MIN_SIZE.height) };
  }
  const area = screen.getDisplayMatching(current ?? f).workArea;
  const width = Math.min(f.width, area.width);
  const height = Math.min(f.height, area.height);
  const x = Math.min(Math.max(f.x, area.x), area.x + area.width - width);
  const y = Math.min(Math.max(f.y, area.y), area.y + area.height - height);
  return { x: Math.round(x), y: Math.round(y), width: Math.round(width), height: Math.round(height) };
}

/** Remember the window frame as the current arrangement's last frame — unless main
 * is moving it, or the frame is outside the arrangement's clamps (a transient). */
function recordFrame(win: BrowserWindow): void {
  if (layout.applying || win.isDestroyed() || win.isFullScreen() || win.isMinimized()) return;
  const b = win.getBounds();
  if (b.width <= 0) return;
  if (layout.mode === 'compact' && b.width > COMPACT_MAX_WIDTH + 0.5) return;
  if (layout.mode === 'full' && b.width < FULL_MIN_SIZE.width - 0.5) return;
  layout.frames[layout.mode] = b;
}

let saveTimer: ReturnType<typeof setTimeout> | null = null;
function recordAndSaveSoon(win: BrowserWindow): void {
  recordFrame(win);
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(saveLayout, 400);
}

function applyLimits(win: BrowserWindow, mode: LayoutMode): void {
  const l = limitsFor(mode);
  win.setMinimumSize(l.min[0], l.min[1]);
  win.setMaximumSize(l.max[0], l.max[1]);
}

/** Move the window to `mode`'s frame. The outgoing arrangement's limits would
 * refuse the incoming frame, so they are lifted for the move and the incoming
 * ones applied once the window has arrived. macOS: the native animated
 * `setBounds(…, true)`; elsewhere a ~0.5 s eased tween of setBounds. */
function applyLayout(win: BrowserWindow, mode: LayoutMode, animate: boolean): void {
  const target = targetBounds(mode, win.getBounds());
  layout.mode = mode;
  layout.applying = true;
  win.setMinimumSize(COMPACT_MIN_SIZE.width, FULL_MIN_SIZE.height);
  win.setMaximumSize(0, 0);
  const finish = (): void => {
    if (win.isDestroyed()) return;
    const b = win.getBounds();
    if (b.x !== target.x || b.y !== target.y || b.width !== target.width || b.height !== target.height) {
      win.setBounds(target, false);
    }
    applyLimits(win, mode);
    layout.applying = false;
    recordFrame(win);
    saveLayout();
    installMenu(); // the View item's label follows the mode
    win.webContents.send('couple:layout-settled', mode);
  };
  if (!animate) {
    win.setBounds(target, false);
    finish();
    return;
  }
  if (process.platform === 'darwin') {
    win.setBounds(target, true);
    // The native animation may run asynchronously: wait until the frame arrives.
    const started = Date.now();
    const poll = setInterval(() => {
      const b = win.isDestroyed() ? target : win.getBounds();
      const there = b.width === target.width && b.height === target.height && b.x === target.x && b.y === target.y;
      if (there || Date.now() - started > 1500) {
        clearInterval(poll);
        finish();
      }
    }, 16);
    return;
  }
  const from = win.getBounds();
  const t0 = Date.now();
  // cubic-bezier(.2,.75,.2,1)-like ease-out (the mockup's curve), as an easeOutCubic.
  const ease = (u: number): number => 1 - Math.pow(1 - u, 3);
  const tick = setInterval(() => {
    if (win.isDestroyed()) {
      clearInterval(tick);
      return;
    }
    const u = Math.min(1, (Date.now() - t0) / LAYOUT_ANIMATION_MS);
    const k = ease(u);
    const lerp = (a: number, b: number): number => Math.round(a + (b - a) * k);
    win.setBounds(
      {
        x: lerp(from.x, target.x),
        y: lerp(from.y, target.y),
        width: lerp(from.width, target.width),
        height: lerp(from.height, target.height),
      },
      false
    );
    if (u >= 1) {
      clearInterval(tick);
      finish();
    }
  }, 16);
}

/** Content size for window bounds `b` (the frame insets measured on `win`). */
function contentSizeFor(win: BrowserWindow, b: Rectangle): { width: number; height: number } {
  const outer = win.getBounds();
  const inner = win.getContentBounds();
  return { width: b.width - (outer.width - inner.width), height: b.height - (outer.height - inner.height) };
}

function installLayoutIpc(): void {
  ipcMain.on('couple:layout-get', (e) => {
    e.returnValue = { mode: layout.mode, persist: layout.persist };
  });
  ipcMain.handle('couple:layout-target', (e, mode: LayoutMode) => {
    const win = BrowserWindow.fromWebContents(e.sender);
    if (!win) return null;
    recordFrame(win); // the outgoing arrangement's last frame
    return contentSizeFor(win, targetBounds(mode, win.getBounds()));
  });
  ipcMain.on('couple:layout-apply', (e, req: { mode: LayoutMode; animate: boolean }) => {
    const win = BrowserWindow.fromWebContents(e.sender);
    if (!win) return;
    recordFrame(win);
    applyLayout(win, req.mode === 'full' ? 'full' : 'compact', !!req.animate);
  });
  ipcMain.handle('couple:layout-state', (e) => {
    const win = BrowserWindow.fromWebContents(e.sender);
    if (!win) return null;
    return {
      mode: layout.mode,
      bounds: win.getBounds(),
      minSize: win.getMinimumSize(),
      maxSize: win.getMaximumSize(),
      frames: layout.frames,
      persist: layout.persist,
    };
  });
  ipcMain.handle('couple:layout-set-bounds', async (e, frame: Rectangle) => {
    const win = BrowserWindow.fromWebContents(e.sender);
    if (!win) return;
    win.setBounds(frame, false);
    await new Promise((r) => setTimeout(r, 150));
    recordFrame(win);
  });
  // Harness-only page capture (COUPLE_COMPACT_TEST / COUPLE_LAYOUT_TOGGLE_TEST shots).
  ipcMain.handle('couple:capture', async (e, path: string) => {
    if (!harnessRun) return false;
    const win = BrowserWindow.fromWebContents(e.sender);
    if (!win) return false;
    const image = await win.webContents.capturePage();
    writeFileSync(path, image.toPNG());
    return true;
  });
  // A native context menu (the compact session line's Clear Session…): resolves to
  // the picked item's id, or null when dismissed.
  ipcMain.handle(
    'couple:context-menu',
    (e, items: Array<{ id: string; label: string; enabled?: boolean }>) =>
      new Promise<string | null>((resolve) => {
        const win = BrowserWindow.fromWebContents(e.sender);
        let done = false;
        const settle = (v: string | null): void => {
          if (!done) {
            done = true;
            resolve(v);
          }
        };
        const menu = Menu.buildFromTemplate(
          items.map((i) => ({ label: i.label, enabled: i.enabled !== false, click: () => settle(i.id) }))
        );
        // The close callback can precede the click handler: let a click win.
        menu.popup({ window: win ?? undefined, callback: () => setTimeout(() => settle(null), 100) });
      })
  );
}

function createWindow(): void {
  const initial = targetBounds(layout.mode, null);
  const limits = limitsFor(layout.mode);
  const hasFrame = layout.frames[layout.mode] !== undefined;
  const win = new BrowserWindow({
    ...(hasFrame ? initial : { width: initial.width, height: initial.height }),
    minWidth: limits.min[0],
    minHeight: limits.min[1],
    ...(limits.max[0] > 0 ? { maxWidth: limits.max[0] } : {}),
    backgroundColor: '#161719',
    icon: windowIcon(),
    titleBarStyle: process.platform === 'darwin' ? 'hiddenInset' : 'default',
    webPreferences: {
      // Local-only app, no remote content: the renderer hosts the engine so
      // band()/audition calls stay synchronous (no IPC on the paint path).
      nodeIntegration: true,
      contextIsolation: false,
      sandbox: false,
    },
  });

  win.on('resize', () => recordAndSaveSoon(win));
  win.on('move', () => recordAndSaveSoon(win));
  win.on('close', () => {
    recordFrame(win);
    saveLayout();
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
  // Its license texts + SOURCE.txt sit next to the binaries (docs/LICENSING §2).
  process.env.COUPLE_FFMPEG_LICENSE_DIR = ff;
}
// Third-party notices for the About / Acknowledgements views, and the version
// they show (package.json in dev, the packaged app's version otherwise).
process.env.COUPLE_LICENSES_DIR = licensesDir();
process.env.COUPLE_APP_VERSION = app.getVersion();

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
// "No audio" → Choose file… (compact preview): one audio file.
ipcMain.handle('couple:pick-audio', async () => {
  const r = await dialog.showOpenDialog({
    title: 'Choose the audio for this take',
    properties: ['openFile'],
    filters: [
      { name: 'Audio', extensions: ['wav', 'aif', 'aiff', 'mp3', 'm4a', 'flac', 'ogg', 'caf', 'mov', 'mp4'] },
      { name: 'All files', extensions: ['*'] },
    ],
  });
  return r.canceled ? null : r.filePaths[0];
});
ipcMain.handle('couple:reveal', (_e, path: string) => {
  shell.showItemInFolder(path);
});

// Defense-in-depth for the nodeIntegration renderer: this shell is local-only
// content, so no webContents may ever open a child window or navigate away from
// the app's own pages (the dev server origin or the packaged file:// bundle).
// Programmatic loadURL/loadFile don't emit will-navigate, so the app's own
// loads are unaffected; external links, if ever added, must use shell.openExternal.
app.on('web-contents-created', (_event, contents) => {
  contents.setWindowOpenHandler(() => ({ action: 'deny' }));
  contents.on('will-navigate', (event, url) => {
    const dev = process.env.ELECTRON_RENDERER_URL;
    const allowed = url.startsWith('file://') || (!!dev && url.startsWith(dev));
    if (!allowed) event.preventDefault();
  });
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
  loadLayout();
  installLayoutIpc();
  installMenu();
  createWindow();
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit();
});
