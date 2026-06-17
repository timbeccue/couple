// Unified activity summary (TODO-005) — the single global activity strip beneath the
// queue/session header. Several operations do meaningful background work (import,
// candidate PAIRING, queued auto-sync, batch export), and today their feedback is
// scattered across a header spinner, per-couple text, and the export rail. This file
// answers ONE question: "given the current job signals (and how long the current
// operation has been running), what is the SINGLE summary line the strip should show, and
// does it have a TRUSTWORTHY fraction?"
//
// RELOCATION (PLAN-CORE-SHELL-BOUNDARY Phase F): the pure DECISION MATH — priority,
// weighting, the determinate fraction, the debounce decision, and the monotonic clamp —
// now lives in `couple_core::policy::activity` and is reached over the FFI
// (`cpl_policy_activity`). This file is the WRAPPER + the PRESENTATION layer:
//   - it gathers the snapshot, calls core, and decodes the uniform `{ok,value|error}`
//     envelope;
//   - from the structured `Decision` (a tagged line KIND + the counts/percentage/names it
//     needs) it ASSEMBLES the localized strings the UI shows — all user-facing copy +
//     localization STAY here, never crossing the FFI line.
//
// WHAT STAYS SHELL-SIDE (genuinely stateful / presentation):
//   - the DEBOUNCE TIMER and the CLOCK: the app measures `elapsedMs` from a real clock and
//     passes it in as an explicit input (core never reads a clock). `ActivityStrip` owns
//     the timer; this just forwards the value.
//   - the per-phase MONOTONIC FLOOR: the controller carries the highest fraction it already
//     showed for a `phaseKey` across recomputes and passes it back as `phaseFloor`. Core
//     applies the clamp; the shell remembers the value.
//   - all STRINGS / pluralization / localization (the `render` mapper below).
// On an envelope error (a malformed/stale request — which should never happen with this
// in-process call) the wrapper maps to the CONSERVATIVE fallback: show NOTHING (a `nil`
// summary), the idle/empty line — never a crash and never a wrong-but-confident line.
//
// HONESTY is the central constraint (TODO-005 acceptance criteria), now enforced in core:
//   - A determinate `fraction` is returned ONLY for work backed by trustworthy job progress:
//     IMPORT (the OVERALL batch position) and batch EXPORT (per-row %), plus the unified
//     INGEST bar that composes them. Everything else is INDETERMINATE (nil fraction ⇒ a
//     moving bar, never a fabricated number).
//   - PAIRING has NO real fraction (one synchronous FFI call) ⇒ ALWAYS indeterminate.
//   - A determinate fraction is MONOTONIC within a phase (`phaseFloor`).
//   - The strip appears only AFTER a debounce (~250 ms) so quick tasks don't flash; a
//     FAILURE surfaces immediately (an error is a result to act on).
//   - PRIORITY: exactly one line wins — failure ▸ ingest ▸ export ▸ import ▸ pairing ▸ sync.

import CCouple
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
    /// bar by `couple_core::policy::activity` (the unified-bar boundary). Set ONLY by the app's
    /// ingest orchestration (a dropped batch); a standalone export / re-sync / re-resolve leaves it
    /// nil and uses the per-kind line instead. `stagesPlanned` records which stages this ingest will
    /// run (e.g. an all-duplicates drop is import-only), so the weighted bar renormalizes over
    /// exactly the present stages and always spans the full range.
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
/// names (Open Decision 7). The `wireValue` matches `couple_core::policy::activity::MatchPhase`.
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
/// pass in the current pipeline, so we never claim one). The `wireValue` matches
/// `couple_core::policy::activity::ImportPhase`.
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
    /// quick tasks do not flash"). The midpoint of the spec window. Mirrors
    /// `couple_core::policy::activity::DEBOUNCE_MS`; the controller reads it to drive its timer.
    public static let debounceMs: Double = 250

    /// One stable phase token for the whole IMPORT phase, so the determinate import bar is
    /// monotonic across every file and sub-phase (the controller carries its floor across
    /// the batch and resets it only on idle — a new import therefore starts a fresh bar).
    /// MATCHING is a SEPARATE, indeterminate phase (no fraction), so it does not share this.
    /// Mirrors `couple_core::policy::activity::INGEST_PHASE_KEY`.
    public static let ingestPhaseKey = "ingest"

    /// Compute the single summary line for the strip (TODO-005).
    ///
    /// Round-trips the snapshot + the explicit time inputs through `cpl_policy_activity` (the
    /// decision math now lives in `couple_core::policy::activity`), then ASSEMBLES the localized
    /// line from the structured decision core returns.
    ///
    /// - Parameters:
    ///   - signals: a snapshot of every long-running signal (see `ActivitySignals`).
    ///   - elapsedMs: how long the CURRENT foreground operation has been active, in
    ///     milliseconds. Below `debounceMs` a non-error operation returns `nil` (no
    ///     flash). Modeling time as an input keeps the debounce testable AND keeps core pure.
    ///   - phaseFloor: the highest determinate fraction the app has ALREADY shown for
    ///     the winning phase (matched by the returned `phaseKey`), or `nil` if the
    ///     phase just changed / is indeterminate. The result's `fraction` is clamped
    ///     UP to this so a determinate bar is monotonic within a phase. Pass `nil` to
    ///     get the raw fraction (the app tracks the floor across calls).
    /// - Returns: the line to show, or `nil` for "show nothing".
    public static func summarize(_ signals: ActivitySignals,
                                 elapsedMs: Double,
                                 phaseFloor: Double? = nil) -> ActivitySummary? {
        decide(signals, elapsedMs: elapsedMs, phaseFloor: phaseFloor)
    }

    /// Compose an INGEST operation into ONE 0→100% summary (unified-bar boundary). Identical to
    /// `summarize` when an ingest descriptor is set — both route to the same core decision (an
    /// in-flight ingest OWNS the line). Kept as a named entry point for the app's unified-bar call
    /// sites and the §F gate tests.
    public static func unifiedSummarize(_ signals: ActivitySignals,
                                        elapsedMs: Double,
                                        phaseFloor: Double? = nil) -> ActivitySummary? {
        decide(signals, elapsedMs: elapsedMs, phaseFloor: phaseFloor)
    }

    /// The winning RAW line (no debounce / no monotonic clamp) given the priority order. Split out
    /// so the priority + per-kind text is independently testable, and so the app-side controller can
    /// read the winning `phaseKey` for its floor. Implemented by asking core with `elapsedMs` past
    /// the debounce and no floor, so the result is the un-gated, un-clamped winning line.
    public static func winningLine(_ s: ActivitySignals) -> ActivitySummary? {
        decide(s, elapsedMs: debounceMs, phaseFloor: nil)
    }

    /// One matching stage's progress 0…1 (unified-bar boundary), re-joining the three core match
    /// phases into a single 0→1 over the whole match stage (fingerprint 60% / compare 35% /
    /// cross-check 5%). The MATH lives in `couple_core::policy::activity::per_match_fraction`; this
    /// reaches it by asking core for a match-only ingest's blended fraction (the match weight is then
    /// the whole renormalized bar ⇒ exactly the per-match value).
    public static func perMatchFraction(phase: MatchPhase, fraction f: Double) -> Double {
        let signals = ActivitySignals(ingest: .init(
            matching: .init(phase: phase, fraction: f),
            stagesPlanned: [.matchStage]))
        return decide(signals, elapsedMs: debounceMs, phaseFloor: nil)?.fraction ?? 0
    }

    // MARK: - core round-trip + presentation

    /// The snapshot FILENAMES core does not carry (they are shell strings) — threaded from the
    /// snapshot the wrapper just sent into the localized line so the active stage can splice them in.
    struct DisplayNames {
        var importFilename: String?
        var exportFilename: String?
        var syncVideoName: String?
    }

    /// Build the request, call `cpl_policy_activity`, decode the envelope, and ASSEMBLE the localized
    /// line. The CONSERVATIVE fallback for any envelope error / parse failure / unknown shape is
    /// `nil` (show nothing) — never a crash, never a wrong-but-confident line.
    static func decide(_ signals: ActivitySignals,
                       elapsedMs: Double,
                       phaseFloor: Double?) -> ActivitySummary? {
        let request = requestObject(signals, elapsedMs: elapsedMs, phaseFloor: phaseFloor)
        // The active stage's filename (import / sync) is what the line shows; capture from the
        // standalone signals AND the unified ingest descriptor (whichever is present).
        let names = DisplayNames(
            importFilename: signals.importing?.filename ?? signals.ingest?.importing?.filename,
            exportFilename: signals.exporting?.filename,
            syncVideoName: signals.syncing?.videoName ?? signals.ingest?.syncing?.videoName)
        let value: [String: Any]?
        if let data = try? JSONSerialization.data(withJSONObject: request),
           let json = String(data: data, encoding: .utf8) {
            value = PolicyEnvelope.decodeValue(cpl_policy_activity(json))
        } else {
            value = nil
        }
        return render(fromEnvelopeValue: value, names: names)
    }

    /// Map a decoded envelope `value` (`{"summary": Decision|null}`) to the localized
    /// `ActivitySummary`, applying the CONSERVATIVE fallback. A nil value (core returned an
    /// `{ok:false,error}` envelope, or the call/parse failed), a missing/`null` summary, or an
    /// unknown line `kind` ⇒ `nil` (show nothing) — the idle/empty line, NEVER a fabricated line.
    /// Internal so a test can drive the error path (a stale/invalid shape ⇒ the fallback) without
    /// reaching the C ABI (PLAN §3.2). `@inline(never)` keeps a standalone symbol the test links.
    @inline(never)
    static func render(fromEnvelopeValue value: [String: Any]?,
                       names: DisplayNames = DisplayNames()) -> ActivitySummary? {
        guard let value, let summary = value["summary"] as? [String: Any] else {
            return nil  // error envelope, or "show nothing"
        }
        guard let line = summary["line"] as? [String: Any],
              let kind = line["kind"] as? String else {
            return nil  // malformed/unknown decision ⇒ conservative: show nothing
        }
        let fraction = (summary["fraction"] as? NSNumber)?.doubleValue
        let isError = summary["isError"] as? Bool ?? false
        let phaseKey = summary["phaseKey"] as? String
        guard let text = text(forKind: kind, line: line, names: names) else {
            return nil  // unknown kind ⇒ conservative: show nothing
        }
        return ActivitySummary(text: text, fraction: fraction, isError: isError, phaseKey: phaseKey)
    }

    /// Assemble the user-facing line text for the structured decision `kind` (all copy +
    /// pluralization stays shell-side). Returns nil for an unknown kind (the caller shows nothing).
    private static func text(forKind kind: String, line: [String: Any],
                             names: DisplayNames) -> String? {
        switch kind {
        case "failure":
            return line["message"] as? String ?? ""
        case "import", "ingestImport":
            // Batch position + current file + the OVERALL percentage core computed.
            let pct = intField(line, "percent")
            if let idx = optIntField(line, "index"), let total = optIntField(line, "total"),
               total > 1, let name = names.importFilename {
                return "Importing \(idx) of \(total): \(name) — \(pct)%"
            } else if let name = names.importFilename {
                return "Importing \(name) — \(pct)%"
            } else {
                return "Importing — \(pct)%"
            }
        case "export":
            let pct = intField(line, "percent")
            let idx = intField(line, "index")
            let total = intField(line, "total")
            if let name = names.exportFilename {
                return "Exporting \(idx) of \(total): \(name) — \(pct)%"
            }
            return "Exporting \(idx) of \(total) — \(pct)%"
        case "exportPreflight":
            return "Preparing export…"
        case "exportFinishing":
            return "Finishing export…"
        case "pairingMatchingTail", "ingestMatching":
            return "Matching couples…"
        case "pairingResolving":
            return "Resolving matches…"
        case "pairingAfterImport":
            return "Updating matches after import…"
        case "pairingComparing":
            let v = intField(line, "videoCount")
            let a = intField(line, "audioCount")
            return "Comparing \(v) \(plural("video", v)) with \(a) audio \(plural("file", a))…"
        case "pairingAnalyzing":
            let n = intField(line, "count")
            return "Analyzing \(n) \(plural("file", n)) for matches…"
        case "syncing", "ingestSyncing":
            let idx = intField(line, "index")
            let total = intField(line, "total")
            var text = "Syncing couple \(idx) of \(total)"
            if let name = names.syncVideoName { text += ": \(name)" }
            return text
        case "ingestWorking":
            return "Working…"
        default:
            return nil  // an unknown kind ⇒ conservative fallback (show nothing)
        }
    }

    /// Build the camelCase request object for `cpl_policy_activity` from the snapshot + the explicit
    /// time inputs (the FILENAMES are threaded separately by `decide`, since core does not carry
    /// them).
    private static func requestObject(_ s: ActivitySignals,
                                      elapsedMs: Double,
                                      phaseFloor: Double?) -> [String: Any] {
        var req: [String: Any] = ["elapsedMs": elapsedMs, "debounceMs": debounceMs]
        if let f = phaseFloor { req["phaseFloor"] = f }
        if let i = s.importing { req["importing"] = importingObject(i) }
        if let p = s.pairing {
            req["pairing"] = [
                "videoCount": p.videoCount, "audioCount": p.audioCount,
                "resolving": p.resolving, "afterImport": p.afterImport, "importTail": p.importTail,
            ]
        }
        if let y = s.syncing { req["syncing"] = ["index": y.index, "total": y.total] }
        if let e = s.exporting {
            req["exporting"] = [
                "index": e.index, "total": e.total,
                "rowFraction": e.rowFraction, "preflight": e.preflight,
            ]
        }
        if let g = s.ingest {
            var ingest: [String: Any] = ["stagesPlanned": stagesObject(g.stagesPlanned)]
            if let i = g.importing { ingest["importing"] = importingObject(i) }
            if let m = g.matching {
                ingest["matching"] = ["phase": m.phase.rawValue, "fraction": m.fraction]
            }
            if let y = g.syncing { ingest["syncing"] = ["index": y.index, "total": y.total] }
            req["ingest"] = ingest
        }
        if let failure = s.failure { req["failure"] = failure }
        return req
    }

    private static func importingObject(_ i: ActivitySignals.Importing) -> [String: Any] {
        var obj: [String: Any] = ["phase": i.phase.rawValue, "fraction": i.fraction]
        if let idx = i.index { obj["index"] = idx }
        if let total = i.total { obj["total"] = total }
        return obj
    }

    private static func stagesObject(_ s: Stages) -> [String: Any] {
        [
            "importStage": s.contains(.importStage),
            "matchStage": s.contains(.matchStage),
            "syncStage": s.contains(.syncStage),
        ]
    }

    // MARK: - helpers

    private static func plural(_ word: String, _ n: Int) -> String {
        n == 1 ? word : word + "s"
    }

    private static func intField(_ line: [String: Any], _ key: String) -> Int {
        (line[key] as? NSNumber)?.intValue ?? 0
    }
    private static func optIntField(_ line: [String: Any], _ key: String) -> Int? {
        (line[key] as? NSNumber)?.intValue
    }
}
