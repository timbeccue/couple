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
//   - A determinate `fraction` is returned ONLY for work whose underlying job reports
//     a trustworthy 0…1 (import phase %, batch export %). Everything else returns a
//     nil fraction ⇒ the app draws an INDETERMINATE bar, never a fabricated number.
//   - PAIRING specifically has NO real fraction with the current API (one synchronous
//     FFI call), so it is ALWAYS indeterminate: a count summary
//     (`Analyzing N files for matches...` / `Comparing V videos with A audio files...`)
//     then `Resolving matches...`. No percentage, no simulated Swift progress loop.
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
//      with a count line (and `Resolving matches...` in its resolve sub-phase); never
//      a percentage.
//   4. IMPORT / EXPORT ARE DETERMINATE — `.importing`/`.exporting` yield a non-nil
//      `fraction` equal to the job's reported fraction (clamped to `phaseFloor` for
//      monotonicity) and a phase+target line.
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
        public init(videoCount: Int, audioCount: Int, resolving: Bool = false,
                    afterImport: Bool = false) {
            self.videoCount = videoCount; self.audioCount = audioCount
            self.resolving = resolving; self.afterImport = afterImport
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

    public var importing: Importing?
    public var pairing: Pairing?
    public var syncing: Syncing?
    public var exporting: Exporting?
    /// An active operation FAILED and should surface as an actionable error instead of
    /// leaving a stuck bar (TODO-005). Shown immediately (no debounce) and clears when
    /// the app drops it. The app supplies an already-actionable message.
    public var failure: String?

    public init(importing: Importing? = nil, pairing: Pairing? = nil,
                syncing: Syncing? = nil, exporting: Exporting? = nil,
                failure: String? = nil) {
        self.importing = importing; self.pairing = pairing
        self.syncing = syncing; self.exporting = exporting; self.failure = failure
    }

    /// `true` when ANY signal would produce a strip line (a job is running OR a failure
    /// is pending). The controller uses this to know when to arm/clear the debounce —
    /// it is exactly "is there something to potentially show?". Idle ⇒ false.
    public var hasActiveWork: Bool {
        importing != nil || pairing != nil || syncing != nil || exporting != nil
            || failure != nil
    }
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
    public static func winningLine(_ s: ActivitySignals) -> ActivitySummary? {
        if let e = s.exporting { return exportLine(e) }
        if let i = s.importing { return importLine(i) }
        if let p = s.pairing { return pairingLine(p) }
        if let y = s.syncing { return syncLine(y) }
        return nil
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

    /// Import (determinate, TODO-005). The phase names are honest — only phases the
    /// job reports distinctly get a distinct verb; a batch shows `X of N: file`.
    private static func importLine(_ i: ActivitySignals.Importing) -> ActivitySummary {
        let f = i.fraction.clamped01
        let pct = Int((f * 100).rounded())
        let verb: String
        switch i.phase {
        case .probing: verb = "Inspecting"
        case .decoding: verb = "Decoding audio"
        case .waveform: verb = "Building waveform"
        case .finishing: verb = "Finishing import"
        }
        // Probing has no trustworthy fraction yet (the core hasn't started decoding),
        // so it is the one import sub-state shown indeterminately with the file name.
        if i.phase == .probing {
            let text = batchPrefix("Inspecting", index: i.index, total: i.total, name: i.filename)
            return ActivitySummary(text: text, fraction: nil, isError: false, phaseKey: nil)
        }
        // Determinate phases: `<verb> - NN%`, with the file/batch context when known.
        var text = "\(verb) — \(pct)%"
        if let name = i.filename {
            if let idx = i.index, let total = i.total, total > 1 {
                text = "\(verb) \(idx) of \(total): \(name) — \(pct)%"
            } else {
                text = "\(verb) — \(name) — \(pct)%"
            }
        }
        _ = verb
        return ActivitySummary(text: text, fraction: f, isError: false,
                               phaseKey: "import.\(i.phase.rawValue)")
    }

    /// Pairing (ALWAYS indeterminate, TODO-005). Counts, never a percentage; the
    /// resolve sub-phase reads `Resolving matches...`. A re-resolve after a later
    /// import reads `Updating matches after import...`.
    private static func pairingLine(_ p: ActivitySignals.Pairing) -> ActivitySummary {
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

    private static func batchPrefix(_ verb: String, index: Int?, total: Int?, name: String?) -> String {
        guard let name else { return "\(verb)…" }
        if let idx = index, let total = total, total > 1 {
            return "\(verb) \(idx) of \(total): \(name)"
        }
        return "\(verb) \(name)…"
    }

    private static func plural(_ word: String, _ n: Int) -> String {
        n == 1 ? word : word + "s"
    }
}

private extension Double {
    /// Clamp a reported fraction into 0…1 so a slightly-out-of-range job value can
    /// never produce a >100% / negative bar.
    var clamped01: Double { Swift.min(1, Swift.max(0, self)) }
}
