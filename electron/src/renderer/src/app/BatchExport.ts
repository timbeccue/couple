// BatchExport.ts — serial batch export coordinator (SPEC-V2 §6 / §V2.8); TS
// port of mac/Sources/Couple/BatchExport.swift. Runs one export job at a time
// over the approved (queued) couples, with a whole-queue disk preflight,
// per-couple live status, and continue-past-failure. Each per-couple job is the
// unchanged core export pipeline (cpl_export_start applies the session's mapping —
// timing is never re-derived here; PLAN-CORE-SHELL-BOUNDARY §1), so a batch is
// byte-identical to the CLI export, couple by couple.
import { env } from '../kit/native';
import type { ExportOptions } from '../kit/Wire';
import type { AppModel } from './AppModel';
import type { CoupleRow } from './CoupleRow';
import { Observable } from './observable';

const nodeRequire: NodeRequire = (window as unknown as { require: NodeRequire }).require;
const fs = nodeRequire('node:fs') as typeof import('node:fs');
const pathMod = nodeRequire('node:path') as typeof import('node:path');
const { ipcRenderer } = nodeRequire('electron') as typeof import('electron');

/** The v1 Auto/MOV/MP4/WAV format selector (PairSession.ExportFormat). */
export type ExportFormat = 'Auto' | 'MOV' | 'MP4' | 'WAV';
export const EXPORT_FORMATS: readonly ExportFormat[] = ['Auto', 'MOV', 'MP4', 'WAV'];

/** Output extension for a format (Auto resolves from the video container);
 * mirrors CoupleRow.ext(format:) in Couple.swift. */
export function extForFormat(format: ExportFormat, videoPath: string): string {
  switch (format) {
    case 'WAV':
      return 'wav';
    case 'MOV':
      return 'mov';
    case 'MP4':
      return 'mp4';
    case 'Auto':
      return pathMod.extname(videoPath).toLowerCase() === '.mov' ? 'mov' : 'mp4';
  }
}

/** Core export options for a format, mirroring PairSession.exportAwait. */
function exportOptions(format: ExportFormat, keepCameraAudio: boolean, outputPath: string): ExportOptions {
  return {
    kind: format === 'WAV' ? 'wav' : 'mux',
    outputPath,
    keepCameraAudio: keepCameraAudio && format !== 'WAV',
    container: format === 'MOV' ? 'mov' : format === 'MP4' ? 'mp4' : 'auto',
    faststart: false,
    overwrite: true,
  };
}

export type BatchPhase =
  | { kind: 'idle' }
  | { kind: 'preflight' }
  | { kind: 'running'; done: number; total: number }
  | { kind: 'finished'; exported: number; failed: number };

export class BatchExport extends Observable {
  private readonly model: AppModel;
  phase: BatchPhase = { kind: 'idle' };
  lastError: string | null = null;

  constructor(model: AppModel) {
    super();
    this.model = model;
  }

  get isRunning(): boolean {
    return this.phase.kind === 'preflight' || this.phase.kind === 'running';
  }

  /** Export every approved couple, serially, into the destination folder. Disk
   * preflight sums the queue first; per-couple status updates live; a failed
   * couple records its error and the batch proceeds (continue-past-failure); a
   * closing toast reports "n exported, m failed". */
  async run(): Promise<void> {
    if (this.isRunning) return;
    const dest = this.model.exportDestination;
    if (!dest) return;
    const couples = this.model.queued;
    if (couples.length === 0) return;

    this.phase = { kind: 'preflight' };
    this.lastError = null;
    this.bump();

    // Disk preflight: stream-copy mux ≈ the source video size; WAV is small.
    // Compare the sum against the destination volume's free space.
    const shortfall = this.diskShortfall(couples, dest);
    if (shortfall) {
      this.phase = { kind: 'idle' };
      this.bump();
      this.model.toasts.error('Not enough disk space', shortfall);
      return;
    }

    const usedNames = new Set<string>();
    let exported = 0;
    let failed = 0;
    let firstError: string | null = null;
    for (const couple of couples) couple.exportStatus = { kind: 'idle' };
    this.phase = { kind: 'running', done: 0, total: couples.length };
    this.bump();

    for (let i = 0; i < couples.length; i++) {
      const couple = couples[i];
      const outputPath = this.uniqueOutputPath(dest, couple, usedNames);
      couple.exportStatus = { kind: 'running', fraction: 0 };
      couple.invalidate();
      this.bump();
      try {
        await this.exportCouple(couple, outputPath);
        couple.exportStatus = { kind: 'done', outputPath };
        exported += 1;
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        couple.exportStatus = { kind: 'failed', message: msg };
        if (firstError === null) firstError = msg;
        failed += 1;
      }
      couple.invalidate();
      this.phase = { kind: 'running', done: i + 1, total: couples.length };
      this.bump();
    }

    this.phase = { kind: 'finished', exported, failed };
    this.lastError = firstError;
    this.bump();

    const reveal = { label: 'Reveal', run: () => void ipcRenderer.invoke('couple:reveal', dest) };
    if (failed === 0) {
      const n = exported === 1 ? 'file' : 'files';
      this.model.toasts.success(`Exported ${exported} ${n}`, pathMod.basename(dest), reveal);
    } else {
      this.model.toasts.error(`${exported} exported, ${failed} failed`, firstError ?? '');
      // A failed batch transitions the activity strip to an actionable error line
      // rather than leaving a stuck bar (TODO-005).
      this.model.activity.reportFailure(`${exported} exported, ${failed} failed — ${firstError ?? 'see details'}`);
    }
  }

  /** One approved couple's export: the unchanged core pipeline, awaited, with
   * per-row progress into `couple.exportStatus`. The session's mapping is applied
   * by the core (cpl_export_start) — never re-derived here. */
  private async exportCouple(couple: CoupleRow, outputPath: string): Promise<void> {
    const session = couple.pair.session;
    if (!session || !couple.approved) throw new Error('not approved');
    const opts = exportOptions(this.model.globalFormat, this.model.globalKeepCameraAudio, outputPath);
    const job = session.exportStart(opts);
    job.onProgress((u) => {
      couple.exportStatus = { kind: 'running', fraction: u.fraction };
      couple.invalidate();
      this.bump();
    });
    await job.value;
  }

  /** Output path for a couple, avoiding collisions with this batch's earlier
   * outputs and existing files (append _2, _3, …). */
  private uniqueOutputPath(dest: string, couple: CoupleRow, used: Set<string>): string {
    const ext = extForFormat(this.model.globalFormat, couple.video.path);
    const stem = couple.outputStem;
    let name = `${stem}.${ext}`;
    let n = 2;
    while (used.has(name) || fs.existsSync(pathMod.join(dest, name))) {
      name = `${stem}_${n}.${ext}`;
      n += 1;
    }
    used.add(name);
    return pathMod.join(dest, name);
  }

  /** null if there's room; otherwise a human description of the shortfall. */
  private diskShortfall(couples: CoupleRow[], dest: string): string | null {
    let needed = 0;
    for (const couple of couples) {
      if (this.model.globalFormat === 'WAV') {
        needed += couple.video.durationS * 48000 * 2 * 2; // 48k stereo s16 est.
      } else {
        try {
          needed += fs.statSync(couple.video.path).size; // stream copy ≈ source video size
        } catch {
          /* unreadable source — omit from the estimate */
        }
      }
    }
    let free: number;
    try {
      const st = fs.statfsSync(dest);
      free = st.bavail * st.bsize; // available to unprivileged users
    } catch {
      return null; // can't read free space — proceed (mac returns nil here)
    }
    // Keep a 5% headroom margin.
    if (needed * 1.05 > free) {
      const gb = (b: number): string => `${(b / 1e9).toFixed(1)} GB`;
      return `need ≈ ${gb(needed)}, ${gb(free)} free on ${pathMod.basename(dest)}`;
    }
    return null;
  }
}

// ---------- headless export harness (verification only) ----------

let harnessStarted = false;

/** Dev/CI hook: when COUPLE_EXPORT_TEST names an output folder, wait for ingest
 * to settle, approve every couple, point the destination at that folder, and run
 * the batch. Writes nothing else; the Electron twin of the Swift export harness. */
export function maybeRunExportHarness(model: AppModel): void {
  const dest = env.COUPLE_EXPORT_TEST;
  if (!dest || harnessStarted) return;
  harnessStarted = true;

  const settled = (c: CoupleRow): boolean =>
    c.pair.session === null || c.pair.syncStatus === 'synced' || c.pair.syncStatus === 'failed';

  void (async () => {
    // Poll until the import→match→sync window drains and every couple is settled.
    for (;;) {
      const ready =
        !model.ingest.ingestInFlight &&
        model.couples.length > 0 &&
        model.couples.every(settled);
      if (ready) break;
      await new Promise((r) => setTimeout(r, 200));
    }
    for (const c of model.couples) c.pair.setVerified(true); // no-op when session is null
    model.exportDestination = dest;
    model.destinationAuto = false;
    model.invalidate();
    await model.batch.run();
  })();
}
