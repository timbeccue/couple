// harness.ts — env-gated verification hooks for the persistence/undo parity work
// (the Electron twin of the Swift DebugDrive family). Each writes only console
// lines a headless run greps; none runs in production (the env vars are unset).
import { env } from '../kit/native';
import type { AppModel } from './AppModel';
import type { CoupleRow } from './CoupleRow';
import { QueueStore } from './QueueStore';

function settled(c: CoupleRow): boolean {
  return c.pair.session === null || c.pair.syncStatus === 'synced' || c.pair.syncStatus === 'failed';
}

/** Resolve once ingest has drained and every couple's sync is settled. */
async function waitSettled(model: AppModel): Promise<void> {
  for (;;) {
    if (!model.ingest.ingestInFlight && model.couples.length > 0 && model.couples.every(settled)) return;
    await new Promise((r) => setTimeout(r, 200));
  }
}

export function runParityHarnesses(model: AppModel): void {
  // PERSIST: ingest (COUPLE_OPEN_DIR), leave the first includable couple OUT of the
  // export (an explicit untick — the only stored include state, D1/D8), persist, and
  // let COUPLE_AUTOQUIT exit. Relaunch with COUPLE_RESTORE_TEST reads it back.
  if (env.COUPLE_PERSIST_TEST) {
    void (async () => {
      await waitSettled(model);
      const first = model.couples.find((c) => c.pair.session && c.canInclude);
      if (first && first.included) model.toggleInclude(first);
      model.saveAll();
      console.log(
        `[persist-test] included=${model.includedCount} excluded=${first?.explicit === false ? 1 : 0} ` +
          `couples=${model.couples.length} saved`
      );
    })();
  }

  // RESTORE: no COUPLE_OPEN_DIR — apply the saved queue and assert the offsets
  // came back with NO fresh sync (syncTotal stays 0 on the suppressAutoSync path).
  if (env.COUPLE_RESTORE_TEST) {
    void (async () => {
      const saved = QueueStore.load();
      if (!saved) {
        console.log('[restore-test] no saved queue found');
        return;
      }
      await model.applyRestore(saved);
      const offsets = model.couples.map((c) =>
        c.pair.session ? Number(c.pair.session.state().offsetMs.toFixed(3)) : null
      );
      console.log(
        `[restore-test] couples=${model.couples.length} included=${model.includedCount} ` +
          `excluded=${model.couples.filter((c) => c.explicit === false).length} ` +
          `syncRan=${model.ingest.syncTotal} offsets=${JSON.stringify(offsets)}`
      );
    })();
  }

  // QUALITY (TODO-009): with media loaded, a MANUAL pick of a genuine match must
  // green — its manualMatchConfident flips null→true via the core classifier
  // (Engine.matchPair), and only that flips the dot. Guards the Electron parity bug
  // where the probe never ran, so a confident manual pick stuck at questionable.
  // App-target mirror of the Swift DebugDrive COUPLE_QUALITY_TEST, but media-driven
  // (the real probe needs handles). Re-picks a resolved couple's own audio so the
  // pair is a known match; asserts null→true and the resulting good verdict.
  if (env.COUPLE_QUALITY_TEST) {
    void (async () => {
      await waitSettled(model);
      const c = model.couples.find((x) => x.selectedAudioKey !== null && x.pair.session);
      if (!c) {
        console.log('[quality-test] no resolved couple to probe');
        return;
      }
      const key = c.selectedAudioKey!;
      const z = c.candidates.find((x) => x.audioKey === key)?.z ?? 0;
      c.applyManualPick(key, z, model.zMin); // becomes a MANUAL pick: provenance=manual, confident=null
      const beforeProbe = c.manualMatchConfident;
      await model.reevaluateManualMatch(c);
      console.log(
        `[quality-test] key=${key} beforeProbe=${beforeProbe} manualMatchConfident=${c.manualMatchConfident} ` +
          `forced=${c.forced} verdict=${c.verdict} ` +
          `pass=${beforeProbe === null && c.manualMatchConfident === true && c.verdict === 'good'}`
      );
    })();
  }

  // UNDO: untick the first included couple, undo, assert it is included again (TODO-015).
  if (env.COUPLE_UNDO_TEST) {
    void (async () => {
      await waitSettled(model);
      const c = model.orderedCouples.find((x) => x.pair.session && x.canInclude);
      if (!c) {
        console.log('[undo-test] no includable couple with a session');
        return;
      }
      model.select(c.id);
      const before = c.included;
      model.toggleInclude(c);
      const afterToggle = c.included;
      model.performUndo();
      const afterUndo = c.included;
      console.log(
        `[undo-test] before=${before} toggle=${afterToggle} undo=${afterUndo} ` +
          `pass=${afterToggle === !before && afterUndo === before && c.explicit === null}`
      );
    })();
  }
}
