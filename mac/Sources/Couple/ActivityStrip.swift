// Unified activity strip controller (TODO-005) — the thin, app-side shell that owns
// the DEBOUNCE timer + the per-phase MONOTONIC floor and publishes the single
// observable `summary` the header strip renders. All the "what line, what fraction,
// is it honest?" logic lives in the PURE `ActivitySummaryPolicy` (CoupleKit); this
// only (1) snapshots the EXISTING job signals into `ActivitySignals`, (2) gates the
// first appearance on ~250 ms so quick work never flashes, and (3) clamps a
// determinate fraction up to the highest already shown for the same phase.
//
// It is fed by `refresh()`, called from every place a feeding signal changes:
// MediaPool import progress (`onImportChange`), `AppModel.resolving`, the per-couple
// sync queue pump, and `BatchExport.phase` / per-row progress. Nothing here polls.
//
// Mirrors the codebase's controller-over-pure-core split (PoolPreviewController over
// PoolPreviewState): the testable decision is the pure policy; this is the observable
// SwiftUI surface + the bits that genuinely need wall-clock time.

import CoupleKit
import Foundation
import Observation

@MainActor
@Observable
final class ActivityStrip {
    @ObservationIgnored private unowned let model: AppModel

    /// The line the header strip renders, or `nil` for "show nothing" (idle, or a job
    /// younger than the debounce). The single source of truth for the strip view.
    private(set) var summary: ActivitySummary?

    /// When the CURRENT foreground operation became active (for the debounce). Reset to
    /// nil whenever the snapshot goes idle so the next operation re-debounces.
    @ObservationIgnored private var activeSince: Date?
    /// The pending debounce task that publishes the line once `debounceMs` elapses.
    @ObservationIgnored private var debounceTask: Task<Void, Never>?
    /// The phase whose monotonic floor we are tracking, and that floor (TODO-005:
    /// determinate progress is monotonic within a phase). Reset when the phase changes.
    @ObservationIgnored private var floorPhaseKey: String?
    @ObservationIgnored private var phaseFloor: Double = 0
    /// A transient, app-set failure line (export/import error) that overrides the
    /// snapshot until it auto-clears, so a failed operation transitions to an
    /// actionable error rather than leaving a stuck or vanishing bar (TODO-005).
    @ObservationIgnored private var failureMessage: String?
    @ObservationIgnored private var failureClearTask: Task<Void, Never>?

    init(model: AppModel) {
        self.model = model
    }

    // MARK: - Driving

    /// Recompute the strip from the current signals. Cheap; safe to call often. Arms or
    /// cancels the debounce as work starts/stops, advances a live determinate bar
    /// immediately once visible, and clears to `nil` when everything is idle.
    func refresh() {
        let signals = snapshot()
        let active = signals.hasActiveWork

        // Idle ⇒ clear everything (no stuck bar on success/cancellation, TODO-005).
        guard active else {
            activeSince = nil
            debounceTask?.cancel(); debounceTask = nil
            floorPhaseKey = nil; phaseFloor = 0
            if summary != nil { summary = nil }
            return
        }

        // Work is active: start (or keep) the debounce clock.
        if activeSince == nil { activeSince = Date() }
        let elapsed = Date().timeIntervalSince(activeSince!) * 1000

        if elapsed >= ActivitySummaryPolicy.debounceMs {
            publish(signals)            // already past the debounce → update live
        } else if debounceTask == nil {
            // Schedule the first appearance exactly at the debounce boundary.
            let wait = ActivitySummaryPolicy.debounceMs - elapsed
            debounceTask = Task { [weak self] in
                try? await Task.sleep(nanoseconds: UInt64(wait * 1_000_000))
                guard !Task.isCancelled else { return }
                self?.debounceTask = nil
                self?.refresh()
            }
        }
    }

    /// Hard-reset to idle (Clear Session, TODO-001): drop any shown line, the debounce
    /// clock, the monotonic floor, and any pending failure, so a fresh session starts
    /// clean with no stale bar carried over from the discarded one.
    func reset() {
        activeSince = nil
        debounceTask?.cancel(); debounceTask = nil
        floorPhaseKey = nil; phaseFloor = 0
        failureMessage = nil
        failureClearTask?.cancel(); failureClearTask = nil
        if summary != nil { summary = nil }
    }

    // MARK: - Debug seam (COUPLE_ACTIVITY_TEST)

    /// When set, `snapshot()` returns this instead of reading the live model, so the
    /// env-gated DebugDrive harness can exercise the LIVE controller (debounce timer +
    /// per-phase monotonic floor + idle-clear) deterministically without media. Nil in
    /// production. Setting it (and calling `refresh()`) is the only effect.
    @ObservationIgnored var debugSignalsOverride: ActivitySignals?

    /// Snapshot the controller is using right now (for the harness to assert against).
    var debugCurrentSummary: ActivitySummary? { summary }

    /// Surface a job FAILURE on the strip as an actionable error that auto-clears
    /// (TODO-005: "failure transitions to an actionable error instead of leaving a
    /// stuck bar"). The detailed/toast surfacing of each job is unchanged; this is the
    /// header summary's honest end-state for a failed foreground operation.
    func reportFailure(_ message: String) {
        failureMessage = message
        failureClearTask?.cancel()
        // Show it immediately (failures skip the debounce in the pure policy).
        refresh()
        failureClearTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 6_000_000_000)
            guard !Task.isCancelled else { return }
            self?.failureMessage = nil
            self?.refresh()
        }
    }

    // MARK: - Snapshot + publish

    /// Project the EXISTING signals into the pure policy's input (TODO-005). Each field
    /// maps 1:1 to a real signal; nothing is fabricated.
    private func snapshot() -> ActivitySignals {
        if var override = debugSignalsOverride {   // COUPLE_ACTIVITY_TEST seam
            if override.failure == nil { override.failure = failureMessage }
            return override
        }
        var s = ActivitySignals()
        s.failure = failureMessage

        // The per-stage signals, each gated on its late-final-frame flag (so a stale 100% frame
        // from a finished stage's progress task cannot stick): import on `pool.importing`, matching
        // on `model.resolving`, sync on the live queue position.
        let importSignal: ActivitySignals.Importing? =
            model.pool.importing ? model.pool.importActivity : nil
        // Matching: the determinate JOB's phase+fraction (incremental-matching boundary), gated on
        // `resolving` exactly like import is gated on `importing` (R3). While resolving but BEFORE
        // the job's first progress frame, show the match stage at 0 (it has honestly started —
        // `resolving` is true — just not reported yet) so the import→match hand-off reads "Matching
        // couples…" held at the import top, not a bare gap.
        let matchSignal: ActivitySignals.Matching? =
            model.resolving ? (model.matchingActivity
                               ?? ActivitySignals.Matching(phase: .fingerprinting, fraction: 0)) : nil
        let syncSignal: ActivitySignals.Syncing? =
            (model.syncTotal > 0 && model.syncIndex > 0)
            ? ActivitySignals.Syncing(index: model.syncIndex, total: model.syncTotal,
                                      videoName: model.syncVideoName)
            : nil

        if model.ingestInFlight {
            // UNIFIED ingest (unified-bar boundary): import → match → sync is ONE 0→100% bar. Feed
            // the per-stage signals into the single `Ingest` descriptor and leave the standalone
            // import/pairing/sync members nil so `winningLine` routes to the unified summary.
            s.ingest = ActivitySignals.Ingest(
                importing: importSignal,
                matching: matchSignal,
                syncing: syncSignal,
                stagesPlanned: model.ingestStages)
        } else {
            // Standalone operations (no preceding import) keep today's per-kind behavior
            // (Open Decision 6: only the import-tail path is unified for v1).
            s.importing = importSignal

            // Pairing: AppModel.resolving + live pool counts (always indeterminate for a standalone
            // re-resolve — the determinate matching JOB feeds the unified bar, not this line).
            if model.resolving {
                s.pairing = ActivitySignals.Pairing(
                    videoCount: model.pool.videos.count,
                    audioCount: model.pool.audios.count,
                    resolving: false,
                    afterImport: model.resolveIsUpdateForStrip,
                    importTail: model.resolveIsImportTailForStrip)
            }

            // Queued / running per-couple sync (summary: position; detail stays per-couple).
            s.syncing = syncSignal
        }

        // Batch export: BatchExport phase + the running row's per-row fraction.
        switch model.batch.phase {
        case .preflight:
            s.exporting = ActivitySignals.Exporting(index: 0, total: model.includedCount,
                                                    rowFraction: 0, preflight: true)
        case .running(let done, let total):
            let idx = model.batch.exportRowIndex
            s.exporting = ActivitySignals.Exporting(
                index: idx > 0 ? idx : done + 1, total: total,
                rowFraction: model.batch.runningRowFraction,
                preflight: false, filename: model.batch.exportRowName)
        case .idle, .finished:
            break
        }
        return s
    }

    /// Run the pure policy with the current debounce elapsed + monotonic floor, update
    /// the floor for the next call, and store the line.
    private func publish(_ signals: ActivitySignals) {
        let elapsed = activeSince.map { Date().timeIntervalSince($0) * 1000 } ?? .greatestFiniteMagnitude

        // Determine which phase the winning line belongs to so the floor tracks it.
        let winning = ActivitySummaryPolicy.winningLine(signals)
        if winning?.phaseKey != floorPhaseKey {
            floorPhaseKey = winning?.phaseKey   // phase changed (or none) ⇒ reset floor
            phaseFloor = 0
        }
        let floor: Double? = floorPhaseKey != nil ? phaseFloor : nil

        let result = ActivitySummaryPolicy.summarize(signals, elapsedMs: elapsed, phaseFloor: floor)
        if let f = result?.fraction, result?.phaseKey == floorPhaseKey {
            phaseFloor = max(phaseFloor, f)     // carry the monotonic floor forward
        }
        if summary != result { summary = result }
    }
}
