// Unified activity summary (TODO-005) — the PURE, testable core behind the
// single global activity strip that lives beneath the queue/session header. Several
// operations do meaningful background work (import, candidate PAIRING, queued
// auto-sync, batch export), and today their feedback is scattered across a header
// spinner, per-couple text, and the export rail. This type answers ONE question, as
// a pure function so `swift test` can lock its honesty down: "given the current job
// signals (and how long the current operation has been running), what is the SINGLE
// summary line the strip should show, and does it have a TRUSTWORTHY fraction?"
//
// It is lifted here (out of the app target / SwiftUI) mirroring the codebase's other
// pure-decision idioms (RowMergePolicy.plan, PoolSelection.select,
// MatchQualityPolicy.decide, RepairDropPolicy.decide, MediaRemovalPolicy.plan,
// AnchorProposal.propose). The app feeds it a snapshot built from the EXISTING
// signals — it invents no new jobs — and renders what it returns. The app owns the
// side effects (debounce timer, rendering the bar); this owns the decision.
//
// HONESTY is the central constraint (TODO-005 acceptance criteria):
//   - A determinate `fraction` is returned ONLY for work backed by trustworthy job
//     progress: IMPORT (the OVERALL batch position, summed from each file's reported
//     0…1) and batch EXPORT (per-row %). Everything else returns a nil fraction ⇒ the
//     app draws an INDETERMINATE (moving) bar, never a fabricated number.
//   - IMPORT is ONE determinate bar for the whole dropped batch, not a per-file reset:
//     the files climb it 0→1 (a new import starts a fresh bar). The MATCHING that follows
//     is a SEPARATE phase.
//   - PAIRING has NO real fraction with the current API (one synchronous FFI call), so it
//     is ALWAYS indeterminate. The matching that FOLLOWS an import reads `Matching
//     couples…` with a MOVING bar (never a frozen number) and clears when it finishes; a
//     standalone re-resolve is a count summary (`Comparing V videos with A audio files…`
//     / `Resolving matches…`). No simulated Swift progress loop either way.
//   - A determinate fraction is MONOTONIC within a phase: the app passes the value it
//     last showed for the same phase as `phaseFloor` and this clamps regressions up to
//     it, so a bar never visibly goes backwards inside one phase.
//   - The strip appears only AFTER the work has lasted a debounce (~200–300 ms) so
//     quick tasks don't flash; `elapsedMs < debounceMs` ⇒ no line (nil summary).
//   - FAILURE surfaces as an actionable error rather than a stuck bar; SUCCESS / idle
//     clears (nil summary). The app clears its timer when the summary goes nil.
//
// CONTRACT (the testable invariants):
//   1. IDLE ⇒ NO STRIP — no active signal ⇒ `summary == nil`.
//   2. DEBOUNCE — an active determinate/indeterminate job younger than `debounceMs`
//      ⇒ `nil` (no flash); at/after it ⇒ a line. (A `.failed` signal is shown
//      immediately — an error is not "quick work", it is a result to act on.)
//   3. PAIRING IS INDETERMINATE — a `.pairing` signal ALWAYS yields `fraction == nil`
//      (never a percentage): `Matching couples…` when it is an import's `importTail` (a
//      moving bar that clears when matching finishes), else a count line / `Resolving
//      matches...`.
//   4. IMPORT / EXPORT ARE DETERMINATE — `.exporting` yields the job's per-row %, and
//      `.importing` yields the OVERALL batch fraction (0…1), each clamped to `phaseFloor`
//      for monotonicity. Import is ONE continuous bar across the batch (a single
//      `phaseKey`), not a per-file reset.
//   5. QUEUED WORK SHOWS POSITION — multiple queued sync couples yield
//      `Syncing couple X of N: <video>` (indeterminate at the summary level — most
//      sync phases report no fraction; the per-couple line carries the detail).
//   6. PRIORITY WHEN SEVERAL RUN — the strip is a SUMMARY, so exactly one line wins:
//      a failure to act on first, then the most user-relevant foreground operation
//      (export ▸ import ▸ pairing ▸ sync), since the user is usually driving that one.

import Foundation

/// One imported snapshot of every long-running signal in the session (TODO-005),
/// built by the app from the EXISTING job state each time it recomputes the strip.
/// Absent (`nil`) members mean "that kind of work is not happening". The app never
/// fabricates these; each maps 1:1 to a real signal (MediaPool import job,
/// `AppModel.resolving` + pool counts, the per-couple sync queue, BatchExport).
public struct ActivitySignals: Sendable, Equatable {
    /// Import / analysis / extraction (MediaPool's import job). Determinate: the core
    /// import job reports a trustworthy `fraction` per phase.
    public struct Importing: Sendable, Equatable {
        public var phase: ImportPhase
        public var fraction: Double      // 0…1, trustworthy (from the core job)
        public var index: Int?           // 1-based file position when a batch is known
        public var total: Int?           // batch size when known
        public var filename: String?
        public init(phase: ImportPhase, fraction: Double, index: Int? = nil,
                    total: Int? = nil, filename: String? = nil) {
            self.phase = phase; self.fraction = fraction
            self.index = index; self.total = total; self.filename = filename
        }
    }

    /// Candidate PAIRING (the resolver). Indeterminate by construction (TODO-005):
    /// the graph-adjusted candidate matrix is one synchronous FFI call with no
    /// progress, so this carries COUNTS only — never a fraction.
    public struct Pairing: Sendable, Equatable {
        public var videoCount: Int
        public var audioCount: Int
        /// `true` once the matrix is built and rows are being resolved
        /// (`Resolving matches...`); the app may leave it false for the whole call.
        public var resolving: Bool
        /// `true` when this pairing is a re-resolve after a later import (so the line
        /// reads `Updating matches after import...`) rather than the first analysis.
        public var afterImport: Bool
        /// `true` when this pairing is the MATCHING TAIL of an import operation — it ran
        /// right after the dropped batch finished importing (inside `ingest`). The strip
        /// then reads "Matching couples…" (the comparison stage of the import→couples
        /// process) as an INDETERMINATE moving bar, rather than a standalone pairing count
        /// line. A standalone re-resolve (e.g. after a removal, with no preceding import)
        /// leaves it false.
        public var importTail: Bool
        public init(videoCount: Int, audioCount: Int, resolving: Bool = false,
                    afterImport: Bool = false, importTail: Bool = false) {
            self.videoCount = videoCount; self.audioCount = audioCount
            self.resolving = resolving; self.afterImport = afterImport
            self.importTail = importTail
        }
    }

    /// Queued / running per-couple auto-sync (the §5.2 serial queue). Summary-level
    /// indeterminate: most sync phases report no fraction, and a queue of couples is
    /// better summarized by position than by one couple's partial bar (the per-couple
    /// `syncActivity` line keeps the phase detail).
    public struct Syncing: Sendable, Equatable {
        public var index: Int            // 1-based position of the running couple
        public var total: Int            // couples in this sync batch (running + queued)
        public var videoName: String?
        public init(index: Int, total: Int, videoName: String? = nil) {
            self.index = index; self.total = total; self.videoName = videoName
        }
    }

    /// Batch export (BatchExport + per-row status). Determinate: the export job
    /// reports a trustworthy per-row `fraction`, combined with the row position.
    public struct Exporting: Sendable, Equatable {
        public var index: Int            // 1-based row being exported
        public var total: Int            // approved couples in the batch
        public var rowFraction: Double   // 0…1 of the current row (from the core job)
        public var preflight: Bool       // disk preflight before the first row
        public var filename: String?
        public init(index: Int, total: Int, rowFraction: Double,
                    preflight: Bool = false, filename: String? = nil) {
            self.index = index; self.total = total; self.rowFraction = rowFraction
            self.preflight = preflight; self.filename = filename
        }
    }

    /// Incremental MATCHING progress (the graph-adjusted candidate matrix, now reported as a
    /// determinate JOB — `cpl_match_candidates_graph_start`). Determinate: each `MatchPhase`
    /// carries the core job's 0…1 within that phase, re-joined into one 0→1 across the whole
    /// match stage by `perMatchFraction` (the analogue of `perFileFraction` for import). Used
    /// ONLY by the unified `Ingest` descriptor — the standalone `pairing` line stays
    /// indeterminate (a re-resolve does not yet run the determinate job, Default 6).
    public struct Matching: Sendable, Equatable {
        public var phase: MatchPhase
        public var fraction: Double      // 0…1 within `phase` (from the core job)
        public init(phase: MatchPhase, fraction: Double) {
            self.phase = phase; self.fraction = fraction
        }
    }

    /// The ONE in-flight INGEST operation (import → match → sync), composed into a single 0→100%
    /// bar by `ActivitySummaryPolicy.unifiedSummarize` (the unified-bar boundary). Set ONLY by the
    /// app's ingest orchestration (a dropped batch); a standalone export / re-sync / re-resolve
    /// leaves it nil and uses the per-kind `winningLine` instead. `stagesPlanned` records which
    /// stages this ingest will run (e.g. an all-duplicates drop is import-only), so the weighted
    /// bar renormalizes over exactly the present stages and always spans the full range.
    public struct Ingest: Sendable, Equatable {
        public var importing: Importing?
        public var matching: Matching?
        public var syncing: Syncing?
        public var stagesPlanned: Stages
        public init(importing: Importing? = nil, matching: Matching? = nil,
                    syncing: Syncing? = nil, stagesPlanned: Stages) {
            self.importing = importing; self.matching = matching
            self.syncing = syncing; self.stagesPlanned = stagesPlanned
        }
    }

    public var importing: Importing?
    public var pairing: Pairing?
    public var syncing: Syncing?
    public var exporting: Exporting?
    /// The single unified ingest descriptor (import→match→sync), set only by the app's ingest
    /// orchestration; nil for standalone operations (which use the per-kind lines above).
    public var ingest: Ingest?
    /// An active operation FAILED and should surface as an actionable error instead of
    /// leaving a stuck bar (TODO-005). Shown immediately (no debounce) and clears when
    /// the app drops it. The app supplies an already-actionable message.
    public var failure: String?

    public init(importing: Importing? = nil, pairing: Pairing? = nil,
                syncing: Syncing? = nil, exporting: Exporting? = nil,
                ingest: Ingest? = nil, failure: String? = nil) {
        self.importing = importing; self.pairing = pairing
        self.syncing = syncing; self.exporting = exporting
        self.ingest = ingest; self.failure = failure
    }

    /// `true` when ANY signal would produce a strip line (a job is running OR a failure
    /// is pending). The controller uses this to know when to arm/clear the debounce —
    /// it is exactly "is there something to potentially show?". Idle ⇒ false.
    public var hasActiveWork: Bool {
        importing != nil || pairing != nil || syncing != nil || exporting != nil
            || ingest != nil || failure != nil
    }
}

/// The three core matching phases (incremental-matching boundary), each mapping a core phase
/// string EXACTLY: `MatchFingerprint` ⇒ `.fingerprinting`, `MatchCorrelate` ⇒ `.comparing`,
/// `MatchGraph` ⇒ `.crosschecking`. Keep this mapping stable in lockstep with the core `Phase`
/// names (Open Decision 7).
public enum MatchPhase: String, Sendable, Equatable {
    case fingerprinting   // core "MatchFingerprint": building each track's fingerprint
    case comparing        // core "MatchCorrelate": filling the candidate matrix row by row
    case crosschecking    // core "MatchGraph": the §13 cross-device graph-consistency pass
}

/// Which ingest STAGES are planned for the current operation (unified-bar boundary). An OptionSet so
/// the weighted bar renormalizes over exactly the present stages: a full drop is
/// `[.importStage, .matchStage, .syncStage]`; an all-duplicates drop is `[.importStage]`; a
/// re-resolve after a removal is `[.matchStage, .syncStage]`; a standalone re-sync is `[.syncStage]`.
public struct Stages: OptionSet, Sendable, Equatable {
    public let rawValue: Int
    public init(rawValue: Int) { self.rawValue = rawValue }
    public static let importStage = Stages(rawValue: 1 << 0)
    public static let matchStage = Stages(rawValue: 1 << 1)
    public static let syncStage = Stages(rawValue: 1 << 2)
}

/// Honest import phases (TODO-005). Only phases the core job reports DISTINCTLY get a
/// distinct label; the rest collapse into `Building waveform` / `Finishing import`
/// rather than inventing precision (e.g. there is no separate "analyzing transients"
/// pass in the current pipeline, so we never claim one).
public enum ImportPhase: String, Sendable, Equatable {
    case probing      // "Probe": Inspecting / Reading the file
    case decoding     // "Decode": Decoding audio
    case waveform     // "Pyramid": Building waveform
    case finishing    // "Finalize"/other: Finishing import
}

/// The single line the strip renders (TODO-005), or the app shows nothing when the
/// computation returns `nil`. `fraction == nil` ⇒ INDETERMINATE bar (no number);
/// non-nil ⇒ determinate bar at that 0…1. `isError` drives the error treatment
/// (color + the bar suppressed) instead of a stuck progress bar.
public struct ActivitySummary: Sendable, Equatable {
    public var text: String
    /// Trustworthy 0…1 for a determinate bar, or `nil` for an indeterminate one. Never
    /// fabricated — only import/export ever populate this.
    public var fraction: Double?
    /// This summary is a failure to act on, not progress; the app renders it as an
    /// error (no bar). `fraction` is always nil when this is true.
    public var isError: Bool
    /// A stable token for the current determinate PHASE (e.g. "import.decoding",
    /// "export.3"), so the app can carry a per-phase monotonic floor across recomputes
    /// and reset it when the phase changes. `nil` for indeterminate/error lines.
    public var phaseKey: String?

    public init(text: String, fraction: Double? = nil, isError: Bool = false,
                phaseKey: String? = nil) {
        self.text = text; self.fraction = fraction
        self.isError = isError; self.phaseKey = phaseKey
    }
}

public enum ActivitySummaryPolicy {
    /// The debounce before any non-error strip appears (TODO-005: "~200–300 ms so
    /// quick tasks do not flash"). The midpoint of the spec window.
    public static let debounceMs: Double = 250

    /// One stable phase token for the whole IMPORT phase, so the determinate import bar is
    /// monotonic across every file and sub-phase (the controller carries its floor across
    /// the batch and resets it only on idle — a new import therefore starts a fresh bar).
    /// MATCHING is a SEPARATE, indeterminate phase (no fraction), so it does not share this.
    public static let ingestPhaseKey = "ingest"

    /// Compute the single summary line for the strip (TODO-005).
    ///
    /// - Parameters:
    ///   - signals: a snapshot of every long-running signal (see `ActivitySignals`).
    ///   - elapsedMs: how long the CURRENT foreground operation has been active, in
    ///     milliseconds. Below `debounceMs` a non-error operation returns `nil` (no
    ///     flash). Modeling time as an input keeps the debounce testable.
    ///   - phaseFloor: the highest determinate fraction the app has ALREADY shown for
    ///     the winning phase (matched by the returned `phaseKey`), or `nil` if the
    ///     phase just changed / is indeterminate. The result's `fraction` is clamped
    ///     UP to this so a determinate bar is monotonic within a phase. Pass `nil` to
    ///     get the raw fraction (the app tracks the floor across calls).
    /// - Returns: the line to show, or `nil` for "show nothing".
    public static func summarize(_ signals: ActivitySignals,
                                 elapsedMs: Double,
                                 phaseFloor: Double? = nil) -> ActivitySummary? {
        // (2)/(failure) A failure is a result to act on — surface it immediately,
        // ahead of any debounce and any concurrent progress.
        if let failure = signals.failure {
            return ActivitySummary(text: failure, fraction: nil, isError: true, phaseKey: nil)
        }

        // (6) Exactly one foreground operation wins the summary, most user-relevant
        // first: the user is usually driving export ▸ import ▸ pairing, and a sync
        // queue is the most ambient. Compute the winning RAW line first…
        guard let raw = winningLine(signals) else { return .none } // (1) idle ⇒ nothing

        // (2) …then gate non-error lines on the debounce so quick work never flashes.
        guard elapsedMs >= debounceMs else { return .none }

        // (4) Monotonic clamp for determinate phases only.
        if let f = raw.fraction, let floor = phaseFloor {
            return ActivitySummary(text: raw.text, fraction: max(f, floor),
                                   isError: false, phaseKey: raw.phaseKey)
        }
        return raw
    }

    /// The winning raw line (no debounce / no monotonic clamp) given the priority
    /// order. Split out so the priority + per-kind text is independently testable, and
    /// so the app-side controller can read the winning `phaseKey` for its floor.
    ///
    /// When an INGEST operation is in flight (`signals.ingest` set), it OWNS the line: the unified
    /// import→match→sync bar under the single stable `phaseKey = "ingest"` (the controller's
    /// per-phaseKey floor then makes the WHOLE bar monotonic with no further change). The line is
    /// produced for the ENTIRE in-flight span — including the brief hand-off gaps between stages
    /// where no sub-signal is momentarily live — so the bar stays visible the whole 0→100% (the
    /// floor holds its value across a gap). Standalone export/import/pairing/sync keep their lines.
    public static func winningLine(_ s: ActivitySignals) -> ActivitySummary? {
        if let ingest = s.ingest { return unifiedLine(ingest) }
        if let e = s.exporting { return exportLine(e) }
        if let i = s.importing { return importLine(i) }
        if let p = s.pairing { return pairingLine(p) }
        if let y = s.syncing { return syncLine(y) }
        return nil
    }

    /// Compose an INGEST operation into ONE 0→100% summary (unified-bar boundary, decision B-i: the
    /// pure policy owns the math). The bar is a WEIGHTED sub-range blend across the planned stages —
    /// import 50% / match 20% / sync 30% (Open Decision 2), RENORMALIZED over exactly the stages in
    /// `stagesPlanned` so it always spans 0→1 (e.g. a re-resolve `[.match,.sync]` ⇒ 40/60; an
    /// all-duplicates drop `[.import]` ⇒ the import stage alone reaches 1.0). The TEXT follows the
    /// currently-active stage ("Importing 2 of 4: clip.mov — 47%" → "Matching couples…" → "Syncing
    /// couple 3 of 7: clip.mov"); the FRACTION is the blended overall (Open Decision 5: per-stage
    /// text + a global bar, no blended % in the text). Always carries `phaseKey = "ingest"`.
    ///
    /// Pure and total: the SAME input always yields the SAME line. A present ingest ALWAYS yields a
    /// line (even with no sub-stage momentarily live — a hand-off gap), so the bar stays visible for
    /// the whole span; the controller's floor HOLDS the fraction at the top of the previous stage
    /// across the gap (⇒ monotonic, never a flicker to nil). The bar clears only when the ingest
    /// itself ends (`signals.ingest == nil`).
    public static func unifiedSummarize(_ signals: ActivitySignals,
                                        elapsedMs: Double,
                                        phaseFloor: Double? = nil) -> ActivitySummary? {
        // A failure still wins immediately, even mid-ingest (an error is a result to act on).
        if let failure = signals.failure {
            return ActivitySummary(text: failure, fraction: nil, isError: true, phaseKey: nil)
        }
        guard let ingest = signals.ingest else { return .none }
        guard elapsedMs >= debounceMs else { return .none } // (2) debounce still applies
        let raw = unifiedLine(ingest)
        if let f = raw.fraction, let floor = phaseFloor {
            return ActivitySummary(text: raw.text, fraction: max(f, floor),
                                   isError: false, phaseKey: raw.phaseKey)
        }
        return raw
    }

    /// The raw unified ingest line (no debounce / no clamp) — the blended fraction + active-stage
    /// text. Split out so `winningLine` (controller floor) and `unifiedSummarize` share it.
    private static func unifiedLine(_ ingest: ActivitySignals.Ingest) -> ActivitySummary {
        let overall = unifiedFraction(ingest)
        let text = unifiedText(ingest)
        return ActivitySummary(text: text, fraction: overall, isError: false,
                               phaseKey: ingestPhaseKey)
    }

    /// The weighted, renormalized 0…1 across the planned ingest stages (unified-bar boundary).
    private static func unifiedFraction(_ ingest: ActivitySignals.Ingest) -> Double {
        let stages = ingest.stagesPlanned
        // Default top-level weights (Open Decision 2); summed only over the PRESENT stages.
        let w: (Stages) -> Double = { s in
            switch s {
            case .importStage: return 0.50
            case .matchStage:  return 0.20
            case .syncStage:   return 0.30
            default:           return 0
            }
        }
        var total = 0.0
        for s in [Stages.importStage, .matchStage, .syncStage] where stages.contains(s) {
            total += w(s)
        }
        guard total > 0 else { return 0 }

        // The latest-started present stage is "active"; every EARLIER present stage reads complete
        // (1.0) so the bar never regresses when a stage hands off. (A stage that has not started yet
        // contributes 0 of its sub-range = the top of the previous stage.)
        let matchActive = ingest.matching != nil
        let syncActive = ingest.syncing != nil

        func stageFraction(_ s: Stages) -> Double {
            switch s {
            case .importStage:
                if let i = ingest.importing { return batchImportFraction(i) }
                // Import is planned but no longer reporting ⇒ a later stage started ⇒ import done.
                return (matchActive || syncActive) ? 1.0 : 0.0
            case .matchStage:
                if let m = ingest.matching { return perMatchFraction(phase: m.phase, fraction: m.fraction) }
                // Matching planned, not yet active: 0 while import runs; 1.0 once sync started.
                if syncActive { return 1.0 }
                return 0.0
            case .syncStage:
                if let y = ingest.syncing, y.total > 0 {
                    // Position-only (Open Decision 4): index/total, clamped 0…1.
                    return Swift.min(1, Swift.max(0, Double(y.index) / Double(y.total)))
                }
                return 0.0
            default:
                return 0
            }
        }

        var sum = 0.0
        for s in [Stages.importStage, .matchStage, .syncStage] where stages.contains(s) {
            sum += (w(s) / total) * stageFraction(s)
        }
        return Swift.min(1, Swift.max(0, sum))
    }

    /// The active-stage text for the unified ingest line (Open Decision 5: per-stage text). The
    /// active stage is the latest-started present stage; import ▸ match ▸ sync hand off in order.
    private static func unifiedText(_ ingest: ActivitySignals.Ingest) -> String {
        if let y = ingest.syncing {
            var text = "Syncing couple \(y.index) of \(y.total)"
            if let name = y.videoName { text += ": \(name)" }
            return text
        }
        if ingest.matching != nil {
            return "Matching couples…"
        }
        if let i = ingest.importing {
            // Reuse the import line's text (batch position + current file + overall %).
            return importLine(i).text
        }
        return "Working…"
    }

    // MARK: - Per-kind lines

    /// Export (determinate, TODO-005: `Exporting 2 of 5 - 61%`, `Preparing export…`,
    /// `Finishing export…`). The per-row fraction is the trustworthy number; the row
    /// position gives the batch context an item count requires.
    private static func exportLine(_ e: ActivitySignals.Exporting) -> ActivitySummary {
        if e.preflight {
            return ActivitySummary(text: "Preparing export…", fraction: nil,
                                   isError: false, phaseKey: nil)
        }
        let f = e.rowFraction.clamped01
        // A finished-but-not-yet-idle row reads "Finishing export…" rather than a
        // stuck 100% bar.
        if f >= 1.0 {
            return ActivitySummary(text: "Finishing export…", fraction: 1.0,
                                   isError: false, phaseKey: "export.\(e.index)")
        }
        let pct = Int((f * 100).rounded())
        var text = "Exporting \(e.index) of \(e.total) — \(pct)%"
        if let name = e.filename { text = "Exporting \(e.index) of \(e.total): \(name) — \(pct)%" }
        return ActivitySummary(text: text, fraction: f, isError: false,
                               phaseKey: "export.\(e.index)")
    }

    /// Import — ONE determinate bar for the whole batch, not a per-file reset (TODO). The
    /// fraction is the OVERALL import progress across the dropped batch (0…1; see
    /// `batchImportFraction`), under a single stable `phaseKey` so the controller carries
    /// its monotonic floor across every file and phase — the bar only ever climbs, and
    /// never restarts at 0 per file. The MATCHING that follows is a SEPARATE INDETERMINATE
    /// phase (see `pairingLine`). The text gives the batch position + current file; the
    /// percentage shown is the OVERALL one the bar draws.
    private static func importLine(_ i: ActivitySignals.Importing) -> ActivitySummary {
        let overall = batchImportFraction(i)
        let pct = Int((overall * 100).rounded())
        let text: String
        if let idx = i.index, let total = i.total, total > 1, let name = i.filename {
            text = "Importing \(idx) of \(total): \(name) — \(pct)%"
        } else if let name = i.filename {
            text = "Importing \(name) — \(pct)%"
        } else {
            text = "Importing — \(pct)%"
        }
        return ActivitySummary(text: text, fraction: overall, isError: false,
                               phaseKey: ingestPhaseKey)
    }

    /// Overall determinate progress of the IMPORT phase (0…1 across the whole batch): the
    /// files already finished (`index - 1`) plus the current file's progress, over the
    /// batch size. With an unknown batch size it falls back to the single file's progress.
    /// Reaches 1.0 only when the last file truly finishes.
    private static func batchImportFraction(_ i: ActivitySignals.Importing) -> Double {
        let pf = perFileFraction(phase: i.phase, fraction: i.fraction.clamped01)
        let v: Double
        if let idx = i.index, let total = i.total, total > 0 {
            v = (Double(idx - 1) + pf) / Double(total)
        } else {
            v = pf
        }
        return Swift.min(1, Swift.max(0, v))
    }

    /// One file's import progress 0…1. The core runs ONE 0→1 per-file progress but emits
    /// it SPLIT across phases — decode is 80% of it, waveform the next 15%, finishing the
    /// last 5% (core/src/import.rs), each reported as its OWN 0→1. A naive read would show
    /// a file "done" at the end of DECODE; this RE-JOINS the phases back into the single
    /// per-file 0→1 using the core's own split, so the value tracks real import time and
    /// reaches 1.0 only at true completion. (Probe precedes the 0→1 work ⇒ 0.)
    private static func perFileFraction(phase: ImportPhase, fraction f: Double) -> Double {
        switch phase {
        case .probing:   return 0
        case .decoding:  return 0.80 * f          // core: overall 0 → 0.80
        case .waveform:  return 0.80 + 0.15 * f   // core: overall 0.80 → 0.95
        case .finishing: return 0.95 + 0.05 * f   // core: overall 0.95 → 1.0
        }
    }

    /// One matching stage's progress 0…1 (unified-bar boundary), re-joining the three core match
    /// phases into a single 0→1 over the whole match stage — the analogue of `perFileFraction` for
    /// matching. The core job emits each phase's OWN 0→1 (`MatchFingerprint` is the dominant FFT
    /// cost, `MatchCorrelate` the per-row matrix fill, `MatchGraph` the cheap §13 pass), so a naive
    /// read would show the stage "done" at the end of fingerprinting; this splits the stage
    /// 60% / 35% / 5% (Open Decision 1) so the value tracks real matching time and reaches 1.0 only
    /// when the graph pass finishes.
    public static func perMatchFraction(phase: MatchPhase, fraction f: Double) -> Double {
        let f = f.clamped01
        switch phase {
        case .fingerprinting: return 0.60 * f          // overall 0 → 0.60
        case .comparing:      return 0.60 + 0.35 * f   // overall 0.60 → 0.95
        case .crosschecking:  return 0.95 + 0.05 * f   // overall 0.95 → 1.0
        }
    }

    /// Pairing — ALWAYS indeterminate (the candidate matrix is one synchronous FFI call
    /// with no live %, TODO-005). The MATCHING TAIL of an import (`importTail`) reads
    /// "Matching couples…" — the comparison stage of the import→couples process; the app
    /// draws a MOVING bar for it (never a frozen number) and clears it when resolution
    /// finishes. A STANDALONE pairing (no preceding import) is a count summary: the resolve
    /// sub-phase reads `Resolving matches...`, a re-resolve after a later import reads
    /// `Updating matches after import...`, else the V×A comparison counts.
    private static func pairingLine(_ p: ActivitySignals.Pairing) -> ActivitySummary {
        if p.importTail {
            // Matching is the COMPARISON stage of the same import→couples process, but it
            // is ONE engine call with no live % — so it is INDETERMINATE: the app draws a
            // MOVING bar (never a frozen number), and it clears when resolution finishes.
            return ActivitySummary(text: "Matching couples…", fraction: nil,
                                   isError: false, phaseKey: nil)
        }
        let text: String
        if p.resolving {
            text = "Resolving matches…"
        } else if p.afterImport {
            text = "Updating matches after import…"
        } else if p.videoCount > 0 && p.audioCount > 0 {
            text = "Comparing \(p.videoCount) \(plural("video", p.videoCount)) "
                + "with \(p.audioCount) audio \(plural("file", p.audioCount))…"
        } else {
            let n = p.videoCount + p.audioCount
            text = "Analyzing \(n) \(plural("file", n)) for matches…"
        }
        return ActivitySummary(text: text, fraction: nil, isError: false, phaseKey: nil)
    }

    /// Queued auto-sync (summary-level indeterminate, TODO-005:
    /// `Syncing couple 2 of 7: video.mov`). The per-couple `syncActivity` line keeps
    /// the phase detail (Searching/Refining/Measuring drift/Fitting).
    private static func syncLine(_ y: ActivitySignals.Syncing) -> ActivitySummary {
        var text = "Syncing couple \(y.index) of \(y.total)"
        if let name = y.videoName { text += ": \(name)" }
        return ActivitySummary(text: text, fraction: nil, isError: false, phaseKey: nil)
    }

    // MARK: - helpers

    private static func plural(_ word: String, _ n: Int) -> String {
        n == 1 ? word : word + "s"
    }
}

private extension Double {
    /// Clamp a reported fraction into 0…1 so a slightly-out-of-range job value can
    /// never produce a >100% / negative bar.
    var clamped01: Double { Swift.min(1, Swift.max(0, self)) }
}
