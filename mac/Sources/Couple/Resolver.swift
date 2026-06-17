// Pairing resolution (SPEC-V2 §3.3), lifted from the Phase-7 CLI (cli/src/pair.rs)
// into the shell so the user can override it. Core supplies mechanical
// measurements (pairCandidates / pairCorrelate); THIS layer owns coverage
// intervals, disjoint/overlapping discrimination, the §3.4 simultaneity check
// (triangle closure), the §3.5 margin rule, and the §3.3 row states.
//
// Honesty posture (V2-F2): conflicting candidates that aren't separable resolve
// `ambiguous` with NO selection — never a coin-flip. Orderings are content/name
// deterministic (timestamps never order a pairing, SPEC §1.4), so the shell
// reproduces `couple pair` byte-for-byte: bach 169 partial→Gigue, 170 ambiguous,
// 171 confident→Prelude, 172 confident→ZOOM0033 on the owner's multifile-session.

import CoupleKit
import Foundation

/// SPEC-V2 §9 resolution constants. [TUNE] values are surfaced (raw z/lag/margin
/// ride in `ResolutionDetail`) so the owner can calibrate on real sessions —
/// v1's RMS thresholds do not transfer (real true-pairs run ~5 ms, V2-F2/§V2.9).
struct PairParams: Sendable, Equatable {
    var zMin = 8.5            // [VALIDATED] Session-C pairing floor
    var zMargin = 6.0         // [TUNE] decisive-best margin (V2-F2)
    var triangleTolMs = 250.0 // [SPEC, TUNE] §3.4 closure tolerance
    var coverageFull = 0.98   // [SPEC, TUNE] single-match `confident` floor
    var disjointTolS = 0.5    // [SPEC, TUNE] interval-overlap to be one cluster
    // Phase-2 RMS tie-breaker (PLAN-RMS-TIEBREAKER §5). Only consulted for a
    // contested, non-simultaneous cluster the z-margin rule left ambiguous; the
    // drift-fit RMS confirms the take. Generous floor (real true-pairs ~5 ms) and a
    // scale-free ratio keep it conservative; below-window fits are never trusted.
    var rmsTrustFloorMs = 9.0 // [TUNE] best fit must lock at/under this to be trusted
    var rmsRatio        = 3.0 // [TUNE] a rival fit must be ≥ this × the leader to be "comparably clean"
    var minFitWindows   = 6   // [TUNE/SPEC] mirrors core synced_min_windows; fewer ⇒ don't trust RMS

    // ---- resolver-owned classifier policy (PLAN-AUDIO-MATCHING-PREWORK §6.6) ----
    // The fine-verifier THRESHOLDS now live in core `PairingParams` (decoded via
    // `Engine.pairingParamsDefault()`), so the resolver keeps NO copies: `isMatch` reads the
    // core-computed `VerifyFit.matched`. Phase 4 (§9) RETIRED the old top-K cost cap as a
    // silent false-negative path: every `prefilterCandidate` is now verified (in chunks), and
    // the only bound on work is the core-owned `maxVerifyPairsPerVideo`/`maxVerifyPairsTotal`
    // SAFETY CEILING (read from `pairingParamsDefault()`), which surfaces as honest
    // `ambiguous` (a `candidateOverflow`) rather than dropping candidates.
    // `verifyClassifierActive` is the Phase-1↔Phase-2 boundary switch (AppModel defaults ON).
    var verifyClassifierActive = false // [PHASE-2] flag

    // ---- spectral-shadow telemetry (PLAN-SPECTRAL-FINGERPRINT-MATCHING §14.2 / §14.4) ----
    // OPT-IN, OFF BY DEFAULT. When OFF the LEGACY resolver does NOT call `Engine.matchCandidates`.
    // When ON (and the legacy override is active), the spectral `PairEvidence` matrix is computed
    // ALONGSIDE the legacy resolution and attached to each row's `ResolutionDetail.shadowEvidence` as
    // OBSERVATIONAL display telemetry. AppModel wires this to the COUPLE_SHADOW_MATCHER env var. (Under
    // the fingerprint-primary default the rows ARE the spectral evidence, so this shadow attach is a
    // legacy-override-only diagnostic.)
    var shadowMatcherActive = false // [BOUNDARY-9] opt-in observational shadow

    // ---- boundary-13 default matcher switch (PLAN-SPECTRAL-FINGERPRINT-MATCHING §16.1) ----
    // The DEFAULT resolver: resolve rows from the core graph-adjusted `PairEvidence` matrix
    // (`Engine.matchCandidatesGraph`), with `classification` the SOLE identity authority and the
    // audio-audio triangle the SHARED core `Engine.triangleCloses` predicate (NO re-derived thresholds
    // in the shell, invariant 8). Coverage clustering + user override stay shell policy. The DEVELOPER
    // OVERRIDE back to the proven envelope resolution (for ≥ 1 release, §16.1) sets this false — AppModel
    // wires it to COUPLE_MATCHER=legacy (mirrors the CLI `--matcher legacy`).
    var matcherFingerprintPrimary = true // [BOUNDARY-13] default ON; COUPLE_MATCHER=legacy forces off
}

/// Span of the video timeline an audio covers (§3.2; overlap_info with ppm 0).
struct Coverage: Sendable, Equatable {
    var t0: Double
    var t1: Double
    var seconds: Double
    var fraction: Double

    /// SPEC §5.6 overlap arithmetic at drift 0 (delta = 1).
    init(durVideo: Double, durAudio: Double, lagMs: Double) {
        let offS = lagMs / 1000
        t0 = max(0, -offS)
        t1 = min(durVideo, durAudio - offS)
        seconds = max(0, t1 - t0)
        fraction = durVideo > 0 ? seconds / durVideo : 0
    }
}

/// §3.3 row states (chip vocabulary mirrors the CLI/UX-DECISIONS-V2).
enum RowState: String, Sendable, Codable {
    case confident, partial, split, twoSources, ambiguous, noMatch

    var chip: String {
        switch self {
        case .confident: return "confident"
        case .partial: return "partial"
        case .split: return "split across files"
        case .twoSources: return "2 sources"
        case .ambiguous: return "ambiguous — check me"
        case .noMatch: return "no match"
        }
    }

    /// Verdict-model bucket (UX-DECISIONS-V2 §V2.3): good→TO APPROVE,
    /// questionable/none→TO FIX. Coverage is never a quality penalty, so a
    /// `partial` row with a real match is still good.
    var verdict: Verdict {
        switch self {
        case .confident, .partial, .split, .twoSources: return .good
        case .ambiguous: return .questionable
        case .noMatch: return .none
        }
    }
}

/// Three-category pairing quality (UX-DECISIONS-V2 §V2.3) — not a continuous score.
/// This is the UI-facing color; the EVIDENCE decision is the pure CoupleKit
/// `MatchQuality` (TODO-009), which this maps 1:1 to/from.
enum Verdict: Sendable { case good, questionable, none }

extension Verdict {
    /// Map the pure-evidence `MatchQuality` (CoupleKit) to the UI verdict color.
    init(_ q: MatchQuality) {
        switch q {
        case .confident: self = .good
        case .questionable: self = .questionable
        case .none: self = .none
        }
    }
}

extension MatchQuality {
    /// Lift a UI `Verdict` (e.g. the resolver's `RowState.verdict`) into the pure
    /// `MatchQuality` the decision policy consumes.
    init(_ v: Verdict) {
        switch v {
        case .good: self = .confident
        case .questionable: self = .questionable
        case .none: self = .none
        }
    }
}

/// One candidate audio source for a video, with its measured fit and coverage.
/// `audioKey` is the pool content key (stable identity); `audioName` drives the
/// CLI-identical name tie-breaks (SPEC §1.4 — clock-free).
struct RowCandidate: Sendable, Identifiable {
    var id: String { audioKey }
    var audioKey: String
    var audioName: String
    var z: Double
    var lagMs: Double
    var coverage: Coverage
    // Phase-1 tie-breaker telemetry (PLAN-RMS-TIEBREAKER §4.4): drift-fit RMS and
    // its accepted-window count, populated ONLY for candidates in a contested,
    // non-simultaneous cluster. Default nil ⇒ every other construction site
    // compiles unchanged and non-contested candidates carry no fit. Informational
    // only in Phase 1 (no `if` reads them); excluded from any stability surface
    // (RowCandidate is not Equatable/Hashable; not persisted by QueueStore).
    var fitRmsMs: Double? = nil
    var fitWindows: Int? = nil
    // Phase-1 fine-verify telemetry (PLAN-MATCH-CLASSIFIER §4.4): the richer
    // VerifyFit signal beyond rms/windows. `fitRmsMs`/`fitWindows` keep mapping from
    // VerifyFit.rmsMs/locked; these add lockFraction/expected/ppm/clamped. All
    // optional, all informational — like the tie-breaker fields they are NEVER read
    // by any decision in Phase 1 and excluded from any stability surface (RowCandidate
    // is not Equatable/Hashable; QueueStore persists only key/flags, never metrics).
    var lockFraction: Double? = nil
    var expected: Int? = nil
    var ppm: Double? = nil
    var clamped: Bool? = nil
}

/// §3.4 record for one overlapping audio pair (raw numbers for V2-F2 calibration).
struct TriangleCheck: Sendable {
    var audioA: String
    var audioB: String
    var abZ: Double
    var abLagMs: Double
    var residualMs: Double
    var simultaneous: Bool
}

/// Why a row's candidates could not be verified within the configured resource
/// ceiling (PLAN-AUDIO-MATCHING-PREWORK §9.2). Constructed ONLY when the eligible
/// candidate count for a row would exceed `maxVerifyPairsPerVideo`, or the running
/// total across rows would exceed `maxVerifyPairsTotal`. The honest alternative to
/// silently dropping candidates: NO subset is verified, the row is `ambiguous` (never
/// `noMatch`/`confident`), and every candidate name stays visible. Non-persisted
/// (resolver-local telemetry; QueueStore never serializes it — mirrors the CLI field).
struct CandidateOverflow: Sendable {
    /// Number of eligible (`prefilterCandidate`) cells for this row.
    var eligible: Int
    /// The resource ceiling that was exceeded.
    var limit: Int
    /// Which ceiling: "perVideo" (`maxVerifyPairsPerVideo`) or "total"
    /// (`maxVerifyPairsTotal`).
    var scope: String
}

/// The honesty trail behind a row's state — surfaced raw so the owner can tune.
struct ResolutionDetail: Sendable {
    var margin: Double?
    var clusters: Int
    var triangles: [TriangleCheck] = []
    var note: String?
    // Phase-1 telemetry summary (PLAN-RMS-TIEBREAKER §4.4): the two best contested
    // candidates' drift-fit RMS, so the ambiguous banner can show "fit separates
    // them: 4.2 ms vs 21.8 ms". Optional; informational only.
    var fitBestRmsMs: Double? = nil
    var fitSecondRmsMs: Double? = nil
    // Phase-4 (§9.2): set ONLY when a resource ceiling forced this row to honest
    // ambiguity instead of silently truncating candidates. Optional + non-persisted.
    var candidateOverflow: CandidateOverflow? = nil
    // Boundary-9 (§14.2/§14.4): OBSERVATIONAL spectral shadow telemetry — set ONLY when
    // `shadowMatcherActive` is ON, otherwise nil (so shadow-off is byte-identical, §5.2). One
    // projected `ShadowEvidenceRow` per candidate audio for THIS video, ordered by `score` desc
    // (the pure `ShadowTelemetry.rows` projection). DISPLAY-ONLY: it surfaces core's `classification`
    // verbatim (acceptance stays in core), never re-decides a row, and appears ONLY in the existing
    // diagnostic disclosure — never as jargon in a normal queue row (§14.4). Optional + non-persisted
    // (QueueStore never serializes ResolutionDetail; it is telemetry, not queue state).
    var shadowEvidence: [ShadowEvidenceRow]? = nil
}

/// A resolved queue row = one video + its selected audio (nil for ambiguous /
/// no-match: never a coin-flip) + the alternates. Mirrors `couple pair`'s Row.
struct ResolvedRow: Sendable {
    var videoKey: String
    var videoName: String
    var state: RowState
    var chip: String
    var selected: RowCandidate?
    var alternates: [RowCandidate]
    var detail: ResolutionDetail
}

/// The video side of a pool entry as the resolver needs it.
struct ResolverVideo: Sendable {
    var key: String
    var name: String
    var durationS: Double
    var handle: TrackHandle?   // nil ⇒ stripped audio stream ⇒ no match
    var hasAudio: Bool
}

/// The audio side of a pool entry.
struct ResolverAudio: Sendable {
    var key: String
    var name: String
    var durationS: Double
    var handle: TrackHandle
}

enum Resolver {
    /// Full §3.3 resolution over the media pool. Pure measurement + arithmetic;
    /// safe to run off the main actor (Engine/TrackHandle are Sendable). The
    /// returned rows are in video input order; unused audio is anything no row
    /// selects (computed by the caller from the pool).
    /// `prebuiltMatrix` (incremental-matching boundary): when the caller already obtained the
    /// graph-adjusted `[[PairEvidence]]` matrix from the determinate JOB
    /// (`Engine.matchCandidatesGraphStart`), it is passed in here so `resolveFingerprint` consumes it
    /// instead of making its own synchronous `matchCandidatesGraph` call — the per-row resolution
    /// policy is byte-identical either way (the matrix IS the only input). It is indexed over the
    /// IMPORTABLE videos (handle != nil), exactly as the synchronous call returns. `nil` ⇒ the
    /// synchronous fallback (every non-app caller, e.g. the CLI tests).
    static func resolve(videos: [ResolverVideo], audios: [ResolverAudio],
                        engine: Engine, params: PairParams = PairParams(),
                        prebuiltMatrix: [[PairEvidence]]? = nil) -> [ResolvedRow] {
        // BOUNDARY 13 (§16.1): the DEFAULT resolves from the core graph-adjusted `PairEvidence`
        // matrix; `--matcher legacy`'s equivalent (matcherFingerprintPrimary == false) keeps the
        // proven envelope resolver below. The fingerprint resolver is a SEPARATE single-pass path.
        if params.matcherFingerprintPrimary {
            return resolveFingerprint(videos: videos, audios: audios, engine: engine,
                                      params: params, prebuiltMatrix: prebuiltMatrix)
        }
        // Candidate matrix over the analyzable videos only (a stripped-audio
        // video has no handle and resolves `no match` without a matrix row).
        let importable = videos.filter { $0.handle != nil }
        let matrix = engine.pairCandidates(videos: importable.compactMap(\.handle),
                                           audios: audios.map(\.handle)) ?? []
        // Core owns the pairing/verifier defaults (PLAN-AUDIO-MATCHING-PREWORK §6) AND the
        // candidacy predicate (§7.2): the loose Stage-A pre-filter is `PairCorrelation.
        // prefilterCandidate`, read off each cell below — the resolver keeps no threshold,
        // so the verifier (null params ⇒ core defaults) and this pre-filter never drift.

        // Map matrix rows back onto the full video list, keeping per-video the
        // candidate cells (audio index, measurement). Flag OFF (the ONLY default):
        // candidacy is exactly today's z-gate — byte-identical. Flag ON
        // (PLAN-MATCH-CLASSIFIER §5.1): a loose Stage-A pre-filter (NCC floor; `z` no
        // longer rejects) admits cells; the fine classifier below disposes.
        var perVideo: [[(Int, PairCorrelation)]] = Array(repeating: [], count: videos.count)
        var matrixRow = 0
        for (vi, v) in videos.enumerated() where v.handle != nil {
            let row = matrixRow < matrix.count ? matrix[matrixRow] : []
            matrixRow += 1
            for (ai, m) in row.enumerated() {
                // Flag ON: core's loose Stage-A pre-filter boolean (ncc ≥ refineNccMin, §7.2),
                // consumed not re-derived (invariant 8); `z` no longer rejects, the fine
                // classifier disposes. Flag OFF: the unchanged default z-gate against the
                // RESOLVER's own `zMin` (a resolution constant distinct from the pairing
                // `legacyCandidate`), so the row decision is byte-identical to today.
                let passes = params.verifyClassifierActive
                    ? m.prefilterCandidate
                    : (!m.insufficientOverlap && m.z >= params.zMin)
                if passes { perVideo[vi].append((ai, m)) }
            }
        }

        // Match classifier (PLAN-MATCH-CLASSIFIER §5.2 / Stage B; Phase-4 §9 rework) — flag
        // ON only. We verify EVERY `prefilterCandidate` (no top-K truncation — that was a
        // silent false-negative path, §2.5/§9.1); the real-match set (isMatch survivors)
        // REPLACES `perVideo[vi]` feeding the UNCHANGED §3.3 pipeline. The ONLY bound on work
        // is the core-owned `maxVerifyPairsPerVideo`/`maxVerifyPairsTotal` SAFETY CEILING; a
        // row that would exceed it verifies NO subset, records a `CandidateOverflow`, and is
        // forced to honest `ambiguous` with every candidate visible (§9.2). Flag OFF: this is
        // skipped, `perVideo` is the z-passing set, and overflow is all-nil — byte-identical.
        var overflow: [CandidateOverflow?] = Array(repeating: nil, count: videos.count)
        if params.verifyClassifierActive {
            let r = classifyMatches(videos: videos, audios: audios, perVideo: perVideo,
                                    engine: engine, params: params)
            perVideo = r.perVideo
            overflow = r.overflow
        }

        var abMemo: [PairKey: PairCorrelation] = [:]
        // Pass 1: decide every row. Non-contested rows finish here, byte-identical
        // to before. A row with a contested, non-simultaneous cluster that the
        // §3.5 z-margin rule could NOT settle is produced as provisionally
        // `ambiguous` but carries a `Pending` context — its contested candidate
        // pairs are also recorded as fit requests. This lets the whole resolve
        // issue a SINGLE `pairFitBatch` (PLAN-RMS-TIEBREAKER §4.4), not one call per
        // cluster, while still letting Phase 2 re-decide those rows post-batch.
        var outcomes: [RowOutcome] = []
        var fitReqs: [FitRequest] = []
        outcomes.reserveCapacity(videos.count)
        for (vi, v) in videos.enumerated() {
            var reqs: [FitRequest] = []
            let outcome = resolveRow(video: v, cands: perVideo[vi], overflow: overflow[vi],
                                     audios: audios, engine: engine, params: params,
                                     abMemo: &abMemo, fitReqs: &reqs)
            outcomes.append(outcome)
            fitReqs.append(contentsOf: reqs)
        }

        // ONE batch fit for every contested pair across the resolve (zero calls when
        // nothing is contested — the common single-candidate/confident path never
        // populates `fitReqs`). Returns RMS per (videoKey, audioKey). This drives the
        // ACTIVE Phase-2 RMS tie-breaker (escalate) and is left exactly as-is.
        let fitByCand = batchFits(fitReqs, engine: engine)

        // Phase-1 fine-verify TELEMETRY (PLAN-MATCH-CLASSIFIER §4.4): one batched
        // verifyBatch over EVERY z-passing candidate the owner can see (the
        // `perVideo[vi]` set), so each candidate carries its full VerifyFit. This is
        // PURELY ADDITIVE — no `if` reads it; the decision is unchanged. The richer
        // VerifyFit (lockFraction/expected/ppm/clamped + rms/locked) is attached in
        // `finalize` alongside the tie-breaker RMS. The z-rejected short clip is NOT
        // in `perVideo` — that case is the job of the full-matrix COUPLE_VERIFY_DUMP.
        let verifyByCand = batchVerify(videos: videos, audios: audios,
                                       perVideo: perVideo, engine: engine)

        // Boundary-9 (§14.2): OBSERVATIONAL spectral shadow telemetry — computed ONLY when the
        // opt-in flag is ON, AFTER all legacy resolution work above (so the legacy decision is
        // already fully formed and untouched). Flag OFF (the default): `shadowByVideo` is empty,
        // NO `Engine.matchCandidates` call happens, and the resolve is byte-identical to today
        // (§5.2 — shadow is never an accidental cost). The map is keyed by video key; `finalize`
        // attaches each row's projection to `detail.shadowEvidence` (display-only, resolved rows
        // stay legacy).
        let shadowByVideo = params.shadowMatcherActive
            ? shadowEvidence(videos: videos, audios: audios, engine: engine)
            : [:]

        // Pass 2: attach telemetry to every candidate (Phase 1) and — for the
        // provisional rows — consult the fits to rescue a genuinely-decidable take
        // (Phase 2 tie-breaker). A row with no Pending is unaffected by the fits
        // (telemetry only): its verdict is byte-identical to before.
        return outcomes.map {
            finalize($0, fitByCand: fitByCand, verifyByCand: verifyByCand,
                     shadowByVideo: shadowByVideo, params: params)
        }
    }

    // MARK: - boundary-13 fingerprint resolver (§13.4)

    /// The DEFAULT resolution (§16.1): rows come from the core graph-adjusted `PairEvidence` matrix
    /// (`Engine.matchCandidatesGraph`). `classification` is the SOLE identity authority — a cell is a
    /// candidate iff `classification == .match` (NO z-gate / acceptance threshold is re-derived here,
    /// invariant 8). Coverage clustering, disjoint/split/2-sources discrimination, the §3.4
    /// simultaneity triangle (the SHARED core `Engine.triangleCloses` predicate over AFFINE evidence +
    /// composed-map residuals), the §3.5 margin rule, and user override stay shell policy. `score`
    /// gives the deterministic order. Single-pass: the legacy two-pass RMS escalation is a z-margin
    /// facility; fingerprint identity is core's classification + the affine triangle.
    private static func resolveFingerprint(videos: [ResolverVideo], audios: [ResolverAudio],
                                           engine: Engine, params: PairParams,
                                           prebuiltMatrix: [[PairEvidence]]? = nil) -> [ResolvedRow] {
        let importable = videos.filter { $0.handle != nil }
        // The graph-adjusted matrix under the core default (FingerprintPrimary + profile 2): pass nil
        // params so core uses its default matcher (§16.1). §16.2 fallback (a cell whose fingerprint is
        // unavailable ⇒ EnvelopeV1) is handled INSIDE core; the cell's `method` records it. When the
        // caller supplied a `prebuiltMatrix` (from the determinate matching JOB) we consume it as-is —
        // the resolution policy below is identical to building it synchronously here.
        let matrix: [[PairEvidence]]
        if let prebuiltMatrix {
            matrix = prebuiltMatrix
        } else {
            matrix = importable.isEmpty || audios.isEmpty
                ? []
                : (engine.matchCandidatesGraph(videos: importable.compactMap(\.handle),
                                               audios: audios.map(\.handle)) ?? [])
        }

        // Map importable-indexed rows back onto the full video list (an audio-less video ⇒ no match).
        var perVideoEvidence: [[PairEvidence]?] = Array(repeating: nil, count: videos.count)
        var mi = 0
        for (vi, v) in videos.enumerated() where v.handle != nil {
            if mi < matrix.count { perVideoEvidence[vi] = matrix[mi] }
            mi += 1
        }

        var abMemo: [PairKey: PairEvidence] = [:]
        var rows: [ResolvedRow] = []
        rows.reserveCapacity(videos.count)
        for (vi, v) in videos.enumerated() {
            rows.append(resolveRowFP(video: v, cells: perVideoEvidence[vi],
                                     audios: audios, engine: engine, params: params, abMemo: &abMemo))
        }
        return rows
    }

    /// The effective (lagMs, driftPpm) a `PairEvidence` cell describes — the REFINED mapping when
    /// boundary 7 populated it, else the consensus mapping (§13.1). Both finite (§20).
    private static func evidenceLagDrift(_ e: PairEvidence) -> (lag: Double, drift: Double) {
        (e.refinedLagMs ?? e.lagMs, e.refinedDriftPpm ?? e.driftPpm)
    }

    /// §3.3 resolution for one video from core `PairEvidence` cells (§13.4). Candidacy is core's
    /// `classification == .match`; everything else is the same shell coverage / cluster / triangle /
    /// margin policy as `resolveRow`. A `nil` cells row (stripped-audio video) ⇒ `no match`.
    private static func resolveRowFP(video: ResolverVideo, cells: [PairEvidence]?,
                                     audios: [ResolverAudio], engine: Engine, params: PairParams,
                                     abMemo: inout [PairKey: PairEvidence]) -> ResolvedRow {
        func mk(_ ai: Int, _ e: PairEvidence) -> RowCandidate {
            RowCandidate(audioKey: audios[ai].key, audioName: audios[ai].name, z: e.score,
                         lagMs: evidenceLagDrift(e).lag,
                         coverage: Coverage(durVideo: video.durationS, durAudio: audios[ai].durationS,
                                            lagMs: evidenceLagDrift(e).lag))
        }
        func row(_ state: RowState, _ selected: RowCandidate?, _ alternates: [RowCandidate],
                 _ detail: ResolutionDetail) -> ResolvedRow {
            ResolvedRow(videoKey: video.key, videoName: video.name, state: state,
                        chip: chipText(state, selected, video.durationS),
                        selected: selected, alternates: alternates, detail: detail)
        }

        if !video.hasAudio {
            return row(.noMatch, nil, [], ResolutionDetail(margin: nil, clusters: 0, note: "no audio stream"))
        }
        guard let cells else {
            return row(.noMatch, nil, [], ResolutionDetail(margin: nil, clusters: 0))
        }

        // CANDIDACY = core's identity verdict: keep only `.match` cells (invariant 8 — no re-derived
        // threshold). A deliberate fingerprint rejection (`.noMatch`/`.ambiguous`) is honored (§16.2 —
        // the weaker matcher never overrides it). Order: score desc, ties by audio file name.
        var c: [(Int, PairEvidence)] = cells.enumerated()
            .filter { $0.element.classification == .match }
            .map { ($0.offset, $0.element) }
        c.sort { a, b in a.1.score != b.1.score ? a.1.score > b.1.score : audios[a.0].name < audios[b.0].name }

        if c.isEmpty {
            return row(.noMatch, nil, [], ResolutionDetail(margin: nil, clusters: 0))
        }
        if c.count == 1 {
            let sel = mk(c[0].0, c[0].1)
            let state: RowState = sel.coverage.fraction >= params.coverageFull ? .confident : .partial
            return row(state, sel, [], ResolutionDetail(margin: nil, clusters: 1))
        }

        let covs = c.map { Coverage(durVideo: video.durationS, durAudio: audios[$0.0].durationS, lagMs: evidenceLagDrift($0.1).lag) }
        let clusters = clusterIntervals(covs, tol: params.disjointTolS)

        var triangles: [TriangleCheck] = []
        var marginOut: Double?
        var winners: [(idx: Int, simultaneous: Bool)] = []
        var ambiguous = false
        for cluster in clusters {
            if cluster.count == 1 { winners.append((cluster[0], false)); continue }
            // §3.4 simultaneity over every pair — the SHARED core affine composed-map check (§13.4).
            var allSimultaneous = true
            for i in cluster.indices {
                for j in (i + 1)..<cluster.count {
                    let ci = cluster[i], cj = cluster[j]
                    let (ai, ea) = c[ci], (aj, eb) = (c[cj].0, c[cj].1)
                    let ab = matchAudioPairEvidence(ai, aj, audios: audios, engine: engine, memo: &abMemo)
                    let va = evidenceLagDrift(ea), vb = evidenceLagDrift(eb), abm = evidenceLagDrift(ab)
                    let tri = engine.triangleCloses(
                        vaLagMs: va.lag, vaDriftPpm: va.drift,
                        abLagMs: abm.lag, abDriftPpm: abm.drift,
                        vbLagMs: vb.lag, vbDriftPpm: vb.drift,
                        abClassification: ab.classification,
                        coverageS: video.durationS,
                        offsetTolMs: 100.0, driftTolPpm: 150.0)  // §13.3 graph tolerances (core defaults)
                    allSimultaneous = allSimultaneous && tri.closes
                    triangles.append(TriangleCheck(audioA: audios[ai].name, audioB: audios[aj].name,
                                                   abZ: ab.score, abLagMs: abm.lag,
                                                   residualMs: tri.residualMaxMs, simultaneous: tri.closes))
                }
            }
            if allSimultaneous {
                winners.append((cluster[0], true))
            } else {
                let zBest = c[cluster[0]].1.score
                let zSecond = c[cluster[1]].1.score
                let margin = zBest - zSecond
                marginOut = margin
                if margin >= params.zMargin {
                    winners.append((cluster[0], false))
                } else {
                    ambiguous = true
                }
            }
        }

        if ambiguous {
            let alternates = c.map { mk($0.0, $0.1) }
            return row(.ambiguous, nil, alternates,
                       ResolutionDetail(margin: marginOut, clusters: clusters.count, triangles: triangles))
        }

        let selPos = winners.indices.max { a, b in
            let ca = covs[winners[a].idx], cb = covs[winners[b].idx]
            if ca.seconds != cb.seconds { return ca.seconds < cb.seconds }
            if c[winners[a].idx].1.score != c[winners[b].idx].1.score { return c[winners[a].idx].1.score < c[winners[b].idx].1.score }
            return audios[c[winners[b].idx].0].name > audios[c[winners[a].idx].0].name
        }!
        let selIdx = winners[selPos].idx
        let selSimul = winners[selPos].simultaneous
        let sel = mk(c[selIdx].0, c[selIdx].1)
        let alternates = c.indices.filter { $0 != selIdx }.map { mk(c[$0].0, c[$0].1) }
        let state: RowState
        if clusters.count > 1 {
            state = .split
        } else if selSimul {
            state = .twoSources
        } else {
            state = sel.coverage.fraction >= params.coverageFull ? .confident : .partial
        }
        return row(state, sel, alternates,
                   ResolutionDetail(margin: marginOut, clusters: clusters.count, triangles: triangles))
    }

    /// Audio-audio `PairEvidence` for the §3.4 triangle, memoized with antisymmetric lag/drift
    /// (L(B,A) = −L(A,B)). Uses `matchPair` (the same core path the matrix used, so the A↔B identity
    /// `classification` is consistent with the V→A / V→B cells). nil-core-fault ⇒ an Unavailable cell
    /// (never a fabricated simultaneity — `triangleCloses` then returns false for the non-Match class).
    private static func matchAudioPairEvidence(_ a: Int, _ b: Int, audios: [ResolverAudio],
                                               engine: Engine, memo: inout [PairKey: PairEvidence]) -> PairEvidence {
        let key = PairKey(lo: min(a, b), hi: max(a, b))
        let base: PairEvidence
        if let hit = memo[key] {
            base = hit
        } else {
            base = engine.matchPair(audios[key.lo].handle, audios[key.hi].handle)
                ?? PairEvidence(method: .spectralLandmarkV1, classification: .unavailable,
                                lagMs: 0, driftPpm: 0, score: 0, runnerUpScore: 0, scoreMargin: 0,
                                support: 0, candidateCorrespondences: 0, inlierFraction: 0,
                                temporalCoverage: 0, residualRmsMs: nil, residualP95Ms: nil, overlapS: 0)
            memo[key] = base
        }
        if a > b {
            var e = base
            e.lagMs = -e.lagMs
            e.driftPpm = -e.driftPpm
            if let l = e.refinedLagMs { e.refinedLagMs = -l }
            if let d = e.refinedDriftPpm { e.refinedDriftPpm = -d }
            return e
        }
        return base
    }

    // MARK: - two-pass plumbing (telemetry + escalation)

    /// What pass 1 produced for one row: either a finished row, or a provisional
    /// `ambiguous` row plus the context Phase 2 needs to re-decide it.
    private struct RowOutcome {
        var row: ResolvedRow
        var pending: Pending?
    }

    /// The decision context for a row left provisionally `ambiguous` by an
    /// undecidable contested cluster (carried out of pass 1 so the post-batch
    /// escalation can finish it without re-running stage-1 / triangle checks).
    private struct Pending {
        var c: [(Int, PairCorrelation)]            // candidates, z-desc/name order
        var covs: [Coverage]                       // coverage per candidate (index-aligned to c)
        var clusters: [[Int]]                      // connected components (indices into c)
        var decided: [(idx: Int, simultaneous: Bool)] // winners already settled in pass 1
        var undecided: [[Int]]                     // contested clusters the z-rule couldn't settle
        var triangles: [TriangleCheck]
        var margin: Double?
        var audioKeys: [String]                    // audios[c[i].0].key, index-aligned to c
        var audioNames: [String]
        var durVideo: Double
    }

    /// One contested candidate pair needing a drift-fit RMS: the candidate (by
    /// video+audio key, for stitching) plus the handles + coarse lag the core fit
    /// needs. Collected in pass 1, fitted in one batch.
    private struct FitRequest {
        var videoKey: String
        var audioKey: String
        var videoHandle: TrackHandle
        var audioHandle: TrackHandle
        var lagMs: Double
    }

    /// De-dup key for a contested fit request (a pair shared across rows fits once;
    /// lag bucketed to 0.1 ms — the fit is insensitive below that).
    private struct FitKey: Hashable { var videoKey: String; var audioKey: String; var lagDeci: Int }

    /// Run the batch fit for all collected contested pairs once and return a
    /// (videoKey ⊕ audioKey) → PairFit map. Empty input ⇒ no core call.
    ///
    /// De-dups identical (video, audio, lag~0.1 ms) pairs WITHIN this resolve so a
    /// pair shared by two rows is fitted once.
    /// TODO(R1): memoize fits ACROSS resolves too — `pair_fit_rms` is pure, so a
    /// persistent contested cluster re-fits on every edit. A cross-resolve cache
    /// (`(videoKey, audioKey, round(lagMs*10))` → PairFit, invalidated on pool
    /// import/removal) would bound cost under rapid editing. Deferred: it must live
    /// on AppModel and thread through the off-main `Task.detached` resolve boundary,
    /// which is more surface than the within-resolve dedup warrants for now.
    private static func batchFits(_ requests: [FitRequest], engine: Engine) -> [String: PairFit] {
        guard !requests.isEmpty else { return [:] }
        var order: [FitKey] = []
        var seen: Set<FitKey> = []
        var reqByKey: [FitKey: FitRequest] = [:]
        for r in requests {
            let k = FitKey(videoKey: r.videoKey, audioKey: r.audioKey, lagDeci: Int((r.lagMs * 10).rounded()))
            if seen.insert(k).inserted { order.append(k); reqByKey[k] = r }
        }
        let pairs = order.map { (reqByKey[$0]!.videoHandle, reqByKey[$0]!.audioHandle, reqByKey[$0]!.lagMs) }
        let fits = engine.pairFitBatch(pairs) ?? Array(repeating: nil, count: pairs.count)
        var fitByCand: [String: PairFit] = [:]
        for (i, k) in order.enumerated() where i < fits.count {
            if let f = fits[i] { fitByCand[k.videoKey + "\u{0}" + k.audioKey] = f }
        }
        return fitByCand
    }

    /// Phase-1 fine-verify telemetry over the z-passing candidate set (§4.4): ONE
    /// `verifyBatch` for every `perVideo[vi]` cell, returning VerifyFit per
    /// (videoKey ⊕ audioKey). Purely informational — no decision reads it. Off-main
    /// like `batchFits` (Engine/TrackHandle are Sendable). Empty input ⇒ no core call.
    /// (z-rejected cells are absent here by construction; the full-matrix debug dump
    /// covers them, PLAN §4.1.)
    private static func batchVerify(videos: [ResolverVideo], audios: [ResolverAudio],
                                    perVideo: [[(Int, PairCorrelation)]],
                                    engine: Engine) -> [String: VerifyFit] {
        var order: [(videoKey: String, audioKey: String)] = []
        var pairs: [(TrackHandle, TrackHandle, Double)] = []
        for (vi, v) in videos.enumerated() {
            guard let vh = v.handle else { continue }
            for (ai, m) in perVideo[vi] {
                order.append((v.key, audios[ai].key))
                pairs.append((vh, audios[ai].handle, m.lagMs))
            }
        }
        guard !pairs.isEmpty else { return [:] }
        let fits = engine.verifyBatch(pairs) ?? Array(repeating: nil, count: pairs.count)
        var byCand: [String: VerifyFit] = [:]
        for (i, k) in order.enumerated() where i < fits.count {
            if let f = fits[i] { byCand[k.videoKey + "\u{0}" + k.audioKey] = f }
        }
        return byCand
    }

    /// Boundary-9 OBSERVATIONAL spectral shadow telemetry (PLAN-SPECTRAL-FINGERPRINT-MATCHING
    /// §14.2). Called ONLY when `shadowMatcherActive` is ON. Runs the spectral matcher in
    /// `fingerprintShadow` mode over the SAME importable V×A matrix the legacy path used, then
    /// projects each video's evidence row into ordered `ShadowEvidenceRow`s via the PURE
    /// `ShadowTelemetry.rows` (which surfaces core's `classification` verbatim and orders by `score`
    /// — NO acceptance thresholding; acceptance lives in core). Returns a (videoKey → rows) map; the
    /// resolved rows are NEVER derived from this (display/debug only). Off-main like `batchVerify`
    /// (Engine/TrackHandle are Sendable). On any core fault `matchCandidates` returns nil ⇒ this
    /// yields an empty map (the legacy rows are unaffected — shadow is best-effort observation).
    ///
    /// The matcher selection is core's: we pass the `fingerprintShadow` `matcherMode` override as
    /// JSON; the resolver does NOT re-implement matcher dispatch (mirrors the CLI shadow path).
    private static func shadowEvidence(videos: [ResolverVideo], audios: [ResolverAudio],
                                       engine: Engine) -> [String: [ShadowEvidenceRow]] {
        let importable = videos.filter { $0.handle != nil }
        guard !importable.isEmpty, !audios.isEmpty else { return [:] }
        // Request the spectral SHADOW matcher via a PairingParams override (matcherMode only); all
        // other fields are core defaults. nil-params would select core's default legacyEnvelope.
        let shadowJSON = #"{"matcherMode":"fingerprintShadow"}"#
        guard let matrix = engine.matchCandidates(videos: importable.compactMap(\.handle),
                                                  audios: audios.map(\.handle),
                                                  paramsJSON: shadowJSON) else { return [:] }
        let audioKeys = audios.map(\.key)
        let audioNames = audios.map(\.name)
        // `matrix` rows align to the importable videos in order (audio-less videos are absent),
        // exactly like the CLI re-attaches names by walking the importable list.
        var byVideo: [String: [ShadowEvidenceRow]] = [:]
        for (i, v) in importable.enumerated() where i < matrix.count {
            byVideo[v.key] = ShadowTelemetry.rows(evidence: matrix[i],
                                                  audioKeys: audioKeys, audioNames: audioNames)
        }
        return byVideo
    }

    /// PURE Phase-4 (§9.2) overflow decision — unit-testable headlessly (no engine, no
    /// media), the parity mirror of `cli/src/pair.rs` step 3a. Given the per-video eligible
    /// (`prefilterCandidate`) counts IN INPUT ORDER and the two core-owned resource ceilings,
    /// returns `Some(CandidateOverflow)` for each row that may NOT be verified within budget
    /// and `nil` otherwise. A row overflows when its eligible set exceeds `perVideoLimit`, OR
    /// when adding it would push the running total of VERIFIED pairs past `totalLimit` (only
    /// non-overflowed rows count toward the total — an overflowed row verifies nothing). The
    /// per-resolve total is order-dependent BY THE INPUT ORDER, which is deterministic
    /// (name-sorted upstream), so the result is stable. The caller forces every overflowed
    /// row to honest `ambiguous`; this never drops a candidate silently.
    static func candidateOverflows(eligibleCounts: [Int], perVideoLimit: Int, totalLimit: Int)
        -> [CandidateOverflow?] {
        var overflow: [CandidateOverflow?] = Array(repeating: nil, count: eligibleCounts.count)
        var totalVerified = 0
        for (i, eligible) in eligibleCounts.enumerated() {
            if eligible == 0 { continue }
            if eligible > perVideoLimit {
                overflow[i] = CandidateOverflow(eligible: eligible, limit: perVideoLimit, scope: "perVideo")
            } else if totalVerified + eligible > totalLimit {
                overflow[i] = CandidateOverflow(eligible: eligible, limit: totalLimit, scope: "total")
            } else {
                totalVerified += eligible
            }
        }
        return overflow
    }

    /// Match classifier (PLAN-MATCH-CLASSIFIER §5.2 / Stage B; Phase-4 §9 rework) — flag ON
    /// only. Verifies EVERY `prefilterCandidate` (no top-K truncation — §2.5/§9.1) and keeps
    /// only the cells that pass `isMatch`. The ONLY bound on work is the core-owned
    /// `maxVerifyPairsPerVideo`/`maxVerifyPairsTotal` SAFETY CEILING (read from
    /// `pairingParamsDefault()`): a row whose eligible set exceeds the per-video ceiling, or
    /// whose addition would exceed the per-resolve ceiling, OVERFLOWS — no subset is verified
    /// and a `CandidateOverflow` is returned so `resolveRow` surfaces it as honest `ambiguous`
    /// with all candidates visible (§9.2). Verification runs in chunks to bound transient
    /// allocation. Pure measurement + arithmetic; off-main like `batchVerify` (Engine/
    /// TrackHandle Sendable). The CLI mirror is `cli/src/pair.rs` step 3a.
    private static func classifyMatches(videos: [ResolverVideo], audios: [ResolverAudio],
                                        perVideo: [[(Int, PairCorrelation)]],
                                        engine: Engine, params: PairParams)
        -> (perVideo: [[(Int, PairCorrelation)]], overflow: [CandidateOverflow?]) {
        var out: [[(Int, PairCorrelation)]] = Array(repeating: [], count: videos.count)
        // Resource ceilings come from core's single PairingParams contract (not a copy here).
        let limits = engine.pairingParamsDefault()
        // Chunk size bounds the transient verify request/result allocation; correctness is
        // independent of it (every eligible cell within the ceiling is verified).
        let verifyChunk = 256

        // The overflow decision (pure, unit-tested by `candidateOverflows`) over the per-video
        // eligible counts — a stripped-audio video has no candidates (count 0, never overflows).
        let eligibleCounts = videos.indices.map { videos[$0].handle == nil ? 0 : perVideo[$0].count }
        let overflow = candidateOverflows(eligibleCounts: eligibleCounts,
                                          perVideoLimit: limits.maxVerifyPairsPerVideo,
                                          totalLimit: limits.maxVerifyPairsTotal)

        // Flatten the to-verify cells (NON-overflowed videos only) into a batched verify. An
        // overflowed video keeps `perVideo[vi]` intact (all names visible) and verifies nothing
        // — `resolveRow` surfaces it as honest `ambiguous` with the overflow detail.
        var reqIndex: [(vi: Int, ai: Int, m: PairCorrelation)] = []
        var pairs: [(TrackHandle, TrackHandle, Double)] = []
        for (vi, v) in videos.enumerated() {
            guard let vh = v.handle, overflow[vi] == nil else { continue }
            for (ai, m) in perVideo[vi] {
                reqIndex.append((vi, ai, m))
                pairs.append((vh, audios[ai].handle, m.lagMs))
            }
        }
        guard !pairs.isEmpty else { return (out, overflow) }
        // Verify in chunks; map each is_match survivor back to its video.
        var base = 0
        while base < pairs.count {
            let end = min(base + verifyChunk, pairs.count)
            let slice = Array(pairs[base..<end])
            let fits = engine.verifyBatch(slice) ?? Array(repeating: nil, count: slice.count)
            for k in 0..<slice.count {
                let req = reqIndex[base + k]
                if k < fits.count, let f = fits[k], isMatch(f) {
                    out[req.vi].append((req.ai, req.m))
                }
            }
            base = end
        }
        return (out, overflow)
    }

    /// Pass 2 for one row: attach RMS telemetry, then (for a provisional row) run
    /// the Phase-2 escalation. Phase 1's safety property holds because a row WITHOUT
    /// `pending` is only decorated here — its `state`/`selected` are untouched.
    private static func finalize(_ outcome: RowOutcome, fitByCand: [String: PairFit],
                                 verifyByCand: [String: VerifyFit],
                                 shadowByVideo: [String: [ShadowEvidenceRow]],
                                 params: PairParams) -> ResolvedRow {
        var row = outcome.row
        let vk = row.videoKey
        func fit(_ audioKey: String) -> PairFit? { fitByCand[vk + "\u{0}" + audioKey] }
        func verify(_ audioKey: String) -> VerifyFit? { verifyByCand[vk + "\u{0}" + audioKey] }
        func attach(_ cand: inout RowCandidate) {
            // Tie-breaker RMS (existing) takes precedence for fitRmsMs/fitWindows so the
            // Phase-2 escalation telemetry is unchanged; the §4.4 fine-verify fills the
            // richer fields and, when no contested fit exists, also fitRmsMs/fitWindows
            // from VerifyFit (rms_ms/locked). All informational — no `if` reads them.
            if let f = fit(cand.audioKey) { cand.fitRmsMs = f.rmsMs; cand.fitWindows = Int(f.windows) }
            if let v = verify(cand.audioKey) {
                if cand.fitRmsMs == nil { cand.fitRmsMs = v.rmsMs }
                if cand.fitWindows == nil { cand.fitWindows = Int(v.locked) }
                cand.lockFraction = v.lockFraction
                cand.expected = Int(v.expected)
                cand.ppm = v.ppm
                cand.clamped = v.clamped
            }
        }

        // Phase 2 (§5.2) FIRST: a provisional ambiguous row may be rescued by RMS.
        // Doing this before attaching telemetry means the rescued row's candidates
        // also get decorated below (RMS visible on a rescued/selected row).
        if let p = outcome.pending,
           let rescued = escalate(pending: p, fitByCand: fitByCand, videoKey: vk,
                                  videoName: row.videoName, params: params) {
            row = rescued
        }

        // Telemetry (§4.4/§4.5): decorate selected + alternates wherever a fit exists
        // — on whatever row we ended with (provisional ambiguous OR rescued).
        if row.selected != nil { attach(&row.selected!) }
        for ai in row.alternates.indices { attach(&row.alternates[ai]) }

        // Banner summary (§4.5): the two best contested candidates' RMS, z-order.
        let cands = (row.selected.map { [$0] } ?? []) + row.alternates
        let withFits = cands.filter { $0.fitRmsMs != nil }
        if withFits.count >= 1 { row.detail.fitBestRmsMs = withFits[0].fitRmsMs }
        if withFits.count >= 2 { row.detail.fitSecondRmsMs = withFits[1].fitRmsMs }

        // Boundary-9 (§14.2/§14.4): attach the OBSERVATIONAL spectral shadow telemetry for this
        // video onto the FINAL row (provisional ambiguous OR rescued — the row decision above is
        // already fully formed and is NOT consulted here). Empty when the shadow flag is OFF ⇒
        // `shadowEvidence` stays nil and the row is byte-identical to today. Display-only.
        if let shadow = shadowByVideo[vk] { row.detail.shadowEvidence = shadow }
        return row
    }

    /// Audio keys no resolved row selects — the §3.6 unused tray.
    static func unusedAudio(rows: [ResolvedRow], audios: [ResolverAudio]) -> [ResolverAudio] {
        let used = Set(rows.compactMap { $0.selected?.audioKey })
        return audios.filter { !used.contains($0.key) }
    }

    // MARK: - §3.3 single-row resolution

    private static func resolveRow(video: ResolverVideo, cands: [(Int, PairCorrelation)],
                                   overflow: CandidateOverflow?,
                                   audios: [ResolverAudio], engine: Engine, params: PairParams,
                                   abMemo: inout [PairKey: PairCorrelation],
                                   fitReqs: inout [FitRequest]) -> RowOutcome {
        func mk(_ ai: Int, _ m: PairCorrelation) -> RowCandidate {
            RowCandidate(audioKey: audios[ai].key, audioName: audios[ai].name, z: m.z, lagMs: m.lagMs,
                         coverage: Coverage(durVideo: video.durationS, durAudio: audios[ai].durationS, lagMs: m.lagMs))
        }
        func row(_ state: RowState, _ selected: RowCandidate?, _ alternates: [RowCandidate],
                 _ detail: ResolutionDetail) -> ResolvedRow {
            ResolvedRow(videoKey: video.key, videoName: video.name, state: state,
                        chip: chipText(state, selected, video.durationS),
                        selected: selected, alternates: alternates, detail: detail)
        }
        func done(_ r: ResolvedRow) -> RowOutcome { RowOutcome(row: r, pending: nil) }

        if !video.hasAudio {
            return done(row(.noMatch, nil, [], ResolutionDetail(margin: nil, clusters: 0, note: "no audio stream")))
        }

        // Resource ceiling exceeded (§9.2): NEVER a silent eviction. We verified no subset,
        // so we cannot confidently select — the row is `ambiguous` with all candidate names
        // listed and the overflow detail (eligible count + configured limit) for the user.
        if let ov = overflow {
            let alternates = cands.map { mk($0.0, $0.1) }
            let note = "candidate overflow: \(ov.eligible) eligible exceed the \(ov.limit) \(ov.scope) verify limit"
            return done(row(.ambiguous, nil, alternates,
                            ResolutionDetail(margin: nil, clusters: 0, note: note, candidateOverflow: ov)))
        }

        // Candidates sorted by z desc, ties by audio file name (deterministic,
        // clock-free — SPEC §1.4).
        var c = cands
        c.sort { a, b in a.1.z != b.1.z ? a.1.z > b.1.z : audios[a.0].name < audios[b.0].name }

        if c.isEmpty {
            return done(row(.noMatch, nil, [], ResolutionDetail(margin: nil, clusters: 0)))
        }
        if c.count == 1 {
            let sel = mk(c[0].0, c[0].1)
            let state: RowState = sel.coverage.fraction >= params.coverageFull ? .confident : .partial
            return done(row(state, sel, [], ResolutionDetail(margin: nil, clusters: 1)))
        }

        // Cluster coverage intervals into connected components by overlap
        // (§3.3 disjoint/overlapping discrimination, generalized past 2).
        let covs = c.map { Coverage(durVideo: video.durationS, durAudio: audios[$0.0].durationS, lagMs: $0.1.lagMs) }
        let clusters = clusterIntervals(covs, tol: params.disjointTolS)

        var triangles: [TriangleCheck] = []
        var marginOut: Double?
        var winners: [(idx: Int, simultaneous: Bool)] = []
        var undecided: [[Int]] = []   // contested clusters the z-rule could not settle

        for cluster in clusters {
            if cluster.count == 1 {
                winners.append((cluster[0], false))
                continue
            }
            // §3.4 simultaneity over every pair in the cluster.
            var allSimultaneous = true
            for i in cluster.indices {
                for j in (i + 1)..<cluster.count {
                    let ci = cluster[i], cj = cluster[j]
                    let ai = c[ci].0, aj = c[cj].0
                    let ab = correlateAudioPair(ai, aj, audios: audios, engine: engine, memo: &abMemo)
                    let residual = abs(c[cj].1.lagMs - c[ci].1.lagMs - ab.lagMs)
                    let simultaneous = ab.z >= params.zMin && residual <= params.triangleTolMs
                    allSimultaneous = allSimultaneous && simultaneous
                    triangles.append(TriangleCheck(audioA: audios[ai].name, audioB: audios[aj].name,
                                                   abZ: ab.z, abLagMs: ab.lagMs,
                                                   residualMs: residual, simultaneous: simultaneous))
                }
            }
            if allSimultaneous {
                winners.append((cluster[0], true)) // genuine alternates → best z, `2 sources`
            } else {
                // Conflict (different takes?): §3.5 margin rule, biased to
                // `ambiguous` over a coin-flip (V2-F2). A contested cluster always
                // wants drift-fit telemetry, regardless of whether z settles it.
                for idx in cluster {
                    fitReqs.append(FitRequest(videoKey: video.key, audioKey: audios[c[idx].0].key,
                                              videoHandle: video.handle!, audioHandle: audios[c[idx].0].handle,
                                              lagMs: c[idx].1.lagMs))
                }
                let zBest = c[cluster[0]].1.z
                let zSecond = c[cluster[1]].1.z
                let margin = zBest - zSecond
                marginOut = margin
                if zSecond < params.zMin || margin >= params.zMargin {
                    winners.append((cluster[0], false))
                } else {
                    undecided.append(cluster)   // defer to the post-batch escalation
                }
            }
        }

        // Any unsettled contested cluster ⇒ provisional `ambiguous` carrying the
        // context Phase 2 needs. Pass 2 either rescues it (all undecided clusters
        // pick a take) or keeps the ambiguous row. The ambiguous row's shape (nil
        // selection, all candidates as alternates) is byte-identical to before.
        if !undecided.isEmpty {
            let alternates = c.map { mk($0.0, $0.1) }
            let amb = row(.ambiguous, nil, alternates,
                          ResolutionDetail(margin: marginOut, clusters: clusters.count, triangles: triangles))
            let pending = Pending(c: c, covs: covs, clusters: clusters, decided: winners,
                                  undecided: undecided, triangles: triangles, margin: marginOut,
                                  audioKeys: c.map { audios[$0.0].key }, audioNames: c.map { audios[$0.0].name },
                                  durVideo: video.durationS)
            return RowOutcome(row: amb, pending: pending)
        }

        return done(assembleWinnerRow(videoKey: video.key, videoName: video.name, durVideo: video.durationS,
                                      c: c, covs: covs, clusters: clusters, winners: winners,
                                      audioKeys: c.map { audios[$0.0].key }, audioNames: c.map { audios[$0.0].name },
                                      triangles: triangles, margin: marginOut, params: params))
    }

    /// Build the final selected row from settled cluster winners (§3.3): disjoint
    /// clusters ⇒ recorder restart ⇒ max coverage; ties by z then file name. Shared
    /// by pass-1 (no contest) and the Phase-2 rescue path so both produce identical
    /// selection + state. Byte-identical to the original inline block.
    private static func assembleWinnerRow(videoKey: String, videoName: String, durVideo: Double,
                                          c: [(Int, PairCorrelation)], covs: [Coverage],
                                          clusters: [[Int]], winners: [(idx: Int, simultaneous: Bool)],
                                          audioKeys: [String], audioNames: [String],
                                          triangles: [TriangleCheck], margin: Double?,
                                          params: PairParams) -> ResolvedRow {
        func mk(_ i: Int) -> RowCandidate {
            RowCandidate(audioKey: audioKeys[i], audioName: audioNames[i], z: c[i].1.z, lagMs: c[i].1.lagMs,
                         coverage: covs[i])
        }
        let selPos = winners.indices.max { a, b in
            let wa = winners[a], wb = winners[b]
            let ca = covs[wa.idx], cb = covs[wb.idx]
            if ca.seconds != cb.seconds { return ca.seconds < cb.seconds }
            if c[wa.idx].1.z != c[wb.idx].1.z { return c[wa.idx].1.z < c[wb.idx].1.z }
            return audioNames[wa.idx] > audioNames[wb.idx] // smaller name = greater
        }!
        let selIdx = winners[selPos].idx
        let selSimul = winners[selPos].simultaneous
        let sel = mk(selIdx)
        let alternates = c.indices.filter { $0 != selIdx }.map { mk($0) }
        let state: RowState
        if clusters.count > 1 {
            state = .split
        } else if selSimul {
            state = .twoSources
        } else {
            state = sel.coverage.fraction >= params.coverageFull ? .confident : .partial
        }
        return ResolvedRow(videoKey: videoKey, videoName: videoName, state: state,
                           chip: chipText(state, sel, durVideo),
                           selected: sel, alternates: alternates,
                           detail: ResolutionDetail(margin: margin, clusters: clusters.count, triangles: triangles))
    }

    // MARK: - Phase 2: RMS escalation (§5.2)

    /// Try to rescue a provisional ambiguous row using drift-fit RMS. EVERY
    /// undecided contested cluster must yield a confident pick; if any stays
    /// ambiguous, the whole row stays ambiguous (no partial rescue, no coin-flip).
    /// Returns the finished selected row, or nil to keep the provisional row.
    private static func escalate(pending p: Pending, fitByCand: [String: PairFit],
                                 videoKey: String, videoName: String, params: PairParams) -> ResolvedRow? {
        func fit(_ i: Int) -> PairFit? { fitByCand[videoKey + "\u{0}" + p.audioKeys[i]] }
        var winners = p.decided
        for cluster in p.undecided {
            guard let pick = rmsEscalate(cluster: cluster, c: p.c, covs: p.covs,
                                         fit: fit, params: params) else { return nil }
            winners.append((pick, false))
        }
        return assembleWinnerRow(videoKey: videoKey, videoName: videoName, durVideo: p.durVideo,
                                 c: p.c, covs: p.covs, clusters: p.clusters, winners: winners,
                                 audioKeys: p.audioKeys, audioNames: p.audioNames,
                                 triangles: p.triangles, margin: p.margin, params: params)
    }

    /// PURE Phase-2 tie-breaker predicate (PLAN-RMS-TIEBREAKER §5.2, corrected per
    /// the brief): unit-testable with injected `PairFit`s — no engine, no media.
    /// Returns the index in `c` to SELECT for this contested cluster, or nil to stay
    /// ambiguous. `fit(i)` yields candidate i's drift-fit (nil ⇒ didn't lock).
    ///
    /// ACTIVE form = conservative ("confirm the z-leader"): only confirms the
    /// best-by-z candidate; never flips to a lower-z take on RMS alone. Flip
    /// `rmsStrongForm` to switch to the strong form ("pick the single clean fit").
    static func rmsEscalate(cluster: [Int], c: [(Int, PairCorrelation)], covs: [Coverage],
                            fit: (Int) -> PairFit?, params: PairParams) -> Int? {
        guard cluster.count >= 2 else { return nil }
        return rmsStrongForm
            ? rmsEscalateStrong(cluster: cluster, covs: covs, fit: fit, params: params)
            : rmsEscalateConservative(cluster: cluster, covs: covs, fit: fit, params: params)
    }

    /// THE conservative↔strong switch. One line. Conservative is the shipped
    /// default (§9 R5 / open-Q3); the strong form rescues more rows but trusts RMS
    /// to override the z order.
    static let rmsStrongForm = false

    /// Conservative: confirm `cluster[0]` (best z) is the real take.
    private static func rmsEscalateConservative(cluster: [Int], covs: [Coverage],
                                                fit: (Int) -> PairFit?, params: PairParams) -> Int? {
        let leaderIdx = cluster[0]
        // 1. The leader must itself be a trustworthy clean lock.
        guard let lf = fit(leaderIdx), Int(lf.windows) >= params.minFitWindows,
              lf.rmsMs <= params.rmsTrustFloorMs else { return nil }
        let leaderCov = covs[leaderIdx].seconds
        // 2. Every other candidate must be decisively worse (or not lock at all).
        for o in cluster.dropFirst() {
            // Coverage guard: a non-comparable span isn't a rival (skip it).
            if !comparableCoverage(leaderCov, covs[o].seconds) { continue }
            guard let of = fit(o), Int(of.windows) >= params.minFitWindows else {
                continue // o didn't lock ⇒ decisively worse ⇒ SUPPORTS the pick
            }
            guard of.rmsMs >= lf.rmsMs * params.rmsRatio else { return nil } // o is comparably clean ⇒ ambiguous
        }
        return leaderIdx
    }

    /// Strong: pick the single candidate that is a clean lock AND decisively better
    /// than all others, regardless of z rank. Exactly one such candidate or nil.
    private static func rmsEscalateStrong(cluster: [Int], covs: [Coverage],
                                          fit: (Int) -> PairFit?, params: PairParams) -> Int? {
        let cleans = cluster.filter {
            guard let f = fit($0) else { return false }
            return Int(f.windows) >= params.minFitWindows && f.rmsMs <= params.rmsTrustFloorMs
        }
        guard cleans.count == 1, let winner = cleans.first, let wf = fit(winner) else { return nil }
        let winnerCov = covs[winner].seconds
        for o in cluster where o != winner {
            if !comparableCoverage(winnerCov, covs[o].seconds) { continue }
            guard let of = fit(o), Int(of.windows) >= params.minFitWindows else { continue }
            guard of.rmsMs >= wf.rmsMs * params.rmsRatio else { return nil }
        }
        return winner
    }

    // MARK: - classifier predicate (PLAN-MATCH-CLASSIFIER §2.3) — telemetry only in Phase 1

    /// The fine-verify classifier predicate. Core computes the decision once
    /// (`VerifyFit.matched`, PLAN-AUDIO-MATCHING-PREWORK §6.4); the resolver surfaces it so
    /// CLI, app, and core share ONE predicate with no copied thresholds. Mirror of core
    /// `verify::is_match`.
    static func isMatch(_ fit: VerifyFit) -> Bool {
        fit.matched
    }

    /// TODO-009: does ONE manually-selected video/audio pair independently clear the
    /// SAME confident-match classifier an automatic confident pairing uses? Reuses
    /// the EXISTING per-pair evidence path — no new matching math, no re-derived
    /// threshold (invariant 8): under the fingerprint-primary default the authority is
    /// core's `PairEvidence.classification == .match` (`matchPair`); under the legacy
    /// override it is the fine verifier's `VerifyFit.matched` (`isMatch`), exactly as
    /// `resolveFingerprint`/`classifyMatches` decide candidacy. A core fault (nil) is
    /// NOT confident — a manual pick never greens on absent evidence. Pure measurement;
    /// safe off-main (Engine/TrackHandle Sendable).
    static func manualPairIsConfident(video: TrackHandle, audio: TrackHandle,
                                      engine: Engine, params: PairParams) -> Bool {
        if params.matcherFingerprintPrimary {
            // Fingerprint identity is lag-independent (core builds its own
            // correspondence), so the cell's classification is the whole answer.
            return engine.matchPair(video, audio)?.classification == .match
        }
        // Legacy verifier: it only locks within ±span of the SEED lag (core
        // verify.rs), so seed it with the same Stage-1 coarse lag `classifyMatches`
        // feeds `verifyBatch` (`pairCorrelate.lagMs`) — passing 0 would miss any
        // offset pair. No plausible overlap ⇒ not confident.
        guard let coarse = engine.pairCorrelate(video, audio), !coarse.insufficientOverlap,
              let fit = engine.verifyBatch([(video, audio, coarse.lagMs)])?.first ?? nil else { return false }
        return isMatch(fit)
    }

    /// Two overlap spans are comparable when within 20% (§5.2 coverage guard).
    private static func comparableCoverage(_ a: Double, _ b: Double) -> Bool {
        let hi = max(a, b)
        return hi <= 0 ? true : abs(a - b) / hi <= 0.20
    }

    // MARK: - helpers (ports of pair.rs)

    private struct PairKey: Hashable { var lo: Int; var hi: Int }

    /// A×B stage-1, memoized with antisymmetric lag (L(B,A) = −L(A,B)).
    private static func correlateAudioPair(_ a: Int, _ b: Int, audios: [ResolverAudio],
                                           engine: Engine, memo: inout [PairKey: PairCorrelation]) -> PairCorrelation {
        let key = PairKey(lo: min(a, b), hi: max(a, b))
        let base: PairCorrelation
        if let hit = memo[key] {
            base = hit
        } else {
            base = engine.pairCorrelate(audios[key.lo].handle, audios[key.hi].handle)
                ?? PairCorrelation(z: 0, lagMs: 0, ncc: 0, fineNcc: 0, overlapBins: 0,
                                   runnerUpZ: 0, refined: false, insufficientOverlap: true)
            memo[key] = base
        }
        var m = base
        if a > b { m.lagMs = -m.lagMs }
        return m
    }

    /// Connected components of intervals overlapping by more than `tol` seconds,
    /// each component preserving input (z-descending) order; groups by first index.
    private static func clusterIntervals(_ covs: [Coverage], tol: Double) -> [[Int]] {
        let n = covs.count
        var parent = Array(0..<n)
        func find(_ i: Int) -> Int {
            var r = i
            while parent[r] != r { r = parent[r] }
            var x = i
            while parent[x] != r { let next = parent[x]; parent[x] = r; x = next }
            return r
        }
        for i in 0..<n {
            for j in (i + 1)..<n {
                let inter = min(covs[i].t1, covs[j].t1) - max(covs[i].t0, covs[j].t0)
                if inter > tol {
                    let ri = find(i), rj = find(j)
                    parent[max(ri, rj)] = min(ri, rj)
                }
            }
        }
        var groups: [Int: [Int]] = [:]
        for i in 0..<n { groups[find(i), default: []].append(i) }
        return groups.sorted { ($0.value.first ?? 0) < ($1.value.first ?? 0) }.map(\.value)
    }

    private static func chipText(_ state: RowState, _ selected: RowCandidate?, _ durVideo: Double) -> String {
        switch state {
        case .partial, .split:
            if let s = selected {
                return "\(state.chip) — audio covers \(mmss(s.coverage.t0))–\(mmss(s.coverage.t1)) of \(mmss(durVideo))"
            }
            return state.chip
        default:
            return state.chip
        }
    }

    private static func mmss(_ s: Double) -> String {
        let total = Int(s.rounded())
        return String(format: "%d:%02d", total / 60, total % 60)
    }
}
