// Value types mirroring couple-core's wire JSON (SPEC §4; field names are the
// wire format verbatim, so no key-decoding strategy is needed).

import Foundation

public enum TrackRole: String, Codable, Sendable {
    case camera, external
}

public struct VideoStreamInfo: Codable, Sendable {
    public var codec: String
    public var width: UInt32
    public var height: UInt32
    public var nominalFps: Double
    public var avgFps: Double
    public var startTimeS: Double
    public var bitRate: UInt64?
}

public struct AudioStreamInfo: Codable, Sendable {
    public var codec: String
    public var sampleRate: UInt32
    public var channels: UInt32
    public var bitsPerSample: UInt32?
    public var startTimeS: Double
    public var durationS: Double?
}

public struct BwfInfo: Codable, Sendable {
    public var date: String?
    public var time: String?
    public var timeReferenceSamples: UInt64?
    public var encodedBy: String?
    public var codingHistory: String?
}

public struct MediaInfo: Codable, Sendable {
    public var path: String
    public var container: String
    public var durationS: Double
    public var video: VideoStreamInfo?
    public var audio: [AudioStreamInfo]
    public var creationTimeUtc: String?
    public var bwf: BwfInfo?
    public var tags: [String: String]
}

public struct TrackStats: Codable, Sendable {
    public var absPeak: Double
    public var activeP959600: Double
    public var activeRmsDbfs: Double
}

public struct TrackMeta: Codable, Sendable {
    public var id: UInt64
    public var role: TrackRole
    public var info: MediaInfo
    public var epochS: Double
    public var durationS: Double
    public var sampleRate: UInt32
    public var normalization: Double
    public var stats: TrackStats
    public var contentKey: String
}

// ---------- pairing (SPEC-V2 §3.2 / §4) ----------

/// Role-agnostic stage-1 measurement between any two imported tracks
/// (`cpl_pair_correlate` wire form). Lag convention: content at time t in
/// track_a occurs at t + lagMs/1000 in track_b — so for (video, audio) this is
/// the audio offset. `z` is the stage-1 peak prominence; a cell is a *candidate*
/// when `!insufficientOverlap && z >= Z_MIN` (resolution is shell-side).
public struct PairCorrelation: Codable, Sendable {
    public var z: Double
    public var lagMs: Double
    public var ncc: Double
    /// null (core None) when the peak was below floor and refinement was skipped.
    public var fineNcc: Double?
    public var overlapBins: UInt64
    public var runnerUpZ: Double
    public var refined: Bool
    public var insufficientOverlap: Bool
    /// Core-computed default resolver candidacy: `z >= legacyZMin`
    /// (PLAN-AUDIO-MATCHING-PREWORK §7.2). False whenever `insufficientOverlap`.
    public var legacyCandidate: Bool
    /// Core-computed loose Stage-A candidacy: `ncc >= refineNccMin` (§7.2) — the flag-ON
    /// pre-filter the fine verifier disposes. Read this instead of re-deriving the
    /// threshold (invariant 8). False whenever `insufficientOverlap`.
    public var prefilterCandidate: Bool

    public init(z: Double, lagMs: Double, ncc: Double, fineNcc: Double?, overlapBins: UInt64,
                runnerUpZ: Double, refined: Bool, insufficientOverlap: Bool,
                legacyCandidate: Bool = false, prefilterCandidate: Bool = false) {
        self.z = z
        self.lagMs = lagMs
        self.ncc = ncc
        self.fineNcc = fineNcc
        self.overlapBins = overlapBins
        self.runnerUpZ = runnerUpZ
        self.refined = refined
        self.insufficientOverlap = insufficientOverlap
        self.legacyCandidate = legacyCandidate
        self.prefilterCandidate = prefilterCandidate
    }
}

// ---------- sync (§4.4) ----------

public struct StagePeak: Codable, Sendable {
    public var lagMs: Double
    public var ncc: Double
    public var z: Double
    public var overlapBins: UInt64
}

public struct DriftWindow: Codable, Sendable {
    public var t: Double
    public var lagMs: Double
    public var ncc: Double
    public var accepted: Bool
}

public struct OverlapInfo: Codable, Sendable {
    public var t0: Double
    public var t1: Double
    public var seconds: Double
    public var fractionOfVideo: Double
}

public enum SyncStatus: Sendable, Equatable {
    case synced
    case lowConfidence
    case failed(reason: String, detail: Double)
}

public enum DriftMode: String, Codable, Sendable {
    case resample, none
}

public struct SyncResult: Decodable, Sendable {
    public var offsetMs: Double
    public var driftPpm: Double
    public var driftMeasurable: Bool
    public var coarse: StagePeak
    public var runnerUpZ: Double
    public var fineNcc: Double
    public var windows: [DriftWindow]
    public var fitRmsMs: Double?
    public var endToEndDriftMs: Double
    public var overlap: OverlapInfo
    public var status: SyncStatus
    public var suggestedDriftMode: DriftMode
    public var computeMs: Double

    enum CodingKeys: String, CodingKey {
        case offsetMs, driftPpm, driftMeasurable, coarse, runnerUpZ, fineNcc
        case windows, fitRmsMs, endToEndDriftMs, overlap, status, fail
        case suggestedDriftMode, computeMs
    }

    struct Fail: Decodable {
        var reason: String
        var bestZ: Double?
        var overlapS: Double?
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        offsetMs = try c.decode(Double.self, forKey: .offsetMs)
        driftPpm = try c.decode(Double.self, forKey: .driftPpm)
        driftMeasurable = try c.decode(Bool.self, forKey: .driftMeasurable)
        coarse = try c.decode(StagePeak.self, forKey: .coarse)
        runnerUpZ = try c.decode(Double.self, forKey: .runnerUpZ)
        fineNcc = try c.decode(Double.self, forKey: .fineNcc)
        windows = try c.decode([DriftWindow].self, forKey: .windows)
        fitRmsMs = try c.decodeIfPresent(Double.self, forKey: .fitRmsMs)
        endToEndDriftMs = try c.decode(Double.self, forKey: .endToEndDriftMs)
        overlap = try c.decode(OverlapInfo.self, forKey: .overlap)
        suggestedDriftMode = try c.decode(DriftMode.self, forKey: .suggestedDriftMode)
        computeMs = try c.decode(Double.self, forKey: .computeMs)
        switch try c.decode(String.self, forKey: .status) {
        case "synced": status = .synced
        case "lowConfidence": status = .lowConfidence
        default:
            let f = try c.decodeIfPresent(Fail.self, forKey: .fail)
            status = .failed(reason: f?.reason ?? "unknown",
                             detail: f?.bestZ ?? f?.overlapS ?? 0)
        }
    }
}

// ---------- alignment state (§4.5) ----------

public struct Frame: Codable, Sendable, Equatable {
    public var winMs: Double
    public var auto: Bool
    public init(winMs: Double, auto: Bool) {
        self.winMs = winMs
        self.auto = auto
    }
}

public struct LocalMeasurement: Codable, Sendable, Equatable {
    public var lagMs: Double
    public var ncc: Double
    public init(lagMs: Double, ncc: Double) {
        self.lagMs = lagMs
        self.ncc = ncc
    }
}

public struct DriftWindowState: Codable, Sendable, Equatable {
    public var t: Double
    public var lagMs: Double
    public var ncc: Double
    public init(t: Double, lagMs: Double, ncc: Double) {
        self.t = t
        self.lagMs = lagMs
        self.ncc = ncc
    }
}

public enum LoupeLayout: String, Codable, Sendable {
    case grid, stack
}

public struct AlignmentState: Codable, Sendable {
    public var version: UInt32
    public var offsetMs: Double
    public var driftPpm: Double
    public var driftMode: DriftMode
    public var savedDriftPpm: Double
    public var anchors: [Double]
    public var frames: [Frame]
    public var lockZoom: Bool
    public var perAnchorMeasurements: [LocalMeasurement?]
    public var driftWindows: [DriftWindowState]
    public var verified: Bool
    public var loupeLayout: LoupeLayout
}

public struct Onset: Codable, Sendable {
    public var t: Double
    public var score: Double
}

public struct Fit: Codable, Sendable {
    public var offMs: Double
    public var ppm: Double
    public var rmsMs: Double
}

/// Drift-fit RMS for one contested candidate pair (PLAN-RMS-TIEBREAKER §4.3).
/// Telemetry: `rmsMs` is the per-candidate take-identity signal (clean true take
/// locks ~5 ms end-to-end; a wrong take scatters 20+ ms); `windows` is the ROBUST
/// INLIER count behind the fit (`VerifyFit.inliers` — the §8.2 MAD-threshold survivors,
/// not every locked window; PLAN-AUDIO-MATCHING-PREWORK §8.4). Wire is camelCase verbatim.
public struct PairFit: Codable, Sendable {
    public var offMs: Double
    public var ppm: Double
    public var rmsMs: Double
    public var windows: UInt32

    public init(offMs: Double, ppm: Double, rmsMs: Double, windows: UInt32) {
        self.offMs = offMs
        self.ppm = ppm
        self.rmsMs = rmsMs
        self.windows = windows
    }
}

/// Fine waveform-lock match verification of one candidate pair
/// (PLAN-MATCH-CLASSIFIER §2.3 / Stage B). The classifier signal: a clean true match
/// locks a low-`rmsMs`, high-`lockFraction`, in-bound-drift line end-to-end; a non-match
/// scatters (high `rmsMs` / low `lockFraction`) or `clamped`. `expected` counts only
/// MEASURABLE windows (both tracks non-silent) so quiet passages don't deflate the
/// fraction. Wire is camelCase verbatim. `PairFit` is a projection of this
/// (`windows == inliers`, the §8.4 robust-survivor count); the resolver still reads
/// `PairFit` until Stage C.
public struct VerifyFit: Codable, Sendable {
    public var offMs: Double
    public var ppm: Double
    public var rmsMs: Double
    public var clamped: Bool
    public var locked: UInt32
    public var expected: UInt32
    public var lockFraction: Double
    public var overlapS: Double
    /// Locked windows retained by the robust fit — the §8.2 MAD-threshold survivors (a subset
    /// of `locked`). The match decision's minimum-count condition uses this, not `locked` (§8.4).
    public var inliers: UInt32
    /// Nearest-rank p95 of |residual| of the inliers to the fitted line, ms.
    public var residualP95Ms: Double
    /// `(max_inlier_t − min_inlier_t) / overlapS` — temporal spread of the lock.
    public var temporalCoverage: Double
    /// Core-computed match decision (PLAN-AUDIO-MATCHING-PREWORK §6.4) — the single
    /// classification authority. Swift consumes this instead of re-deriving the predicate.
    public var matched: Bool

    public init(
        offMs: Double, ppm: Double, rmsMs: Double, clamped: Bool,
        locked: UInt32, expected: UInt32, lockFraction: Double, overlapS: Double,
        inliers: UInt32 = 0, residualP95Ms: Double = .infinity,
        temporalCoverage: Double = 0, matched: Bool = false
    ) {
        self.offMs = offMs
        self.ppm = ppm
        self.rmsMs = rmsMs
        self.clamped = clamped
        self.locked = locked
        self.expected = expected
        self.lockFraction = lockFraction
        self.overlapS = overlapS
        self.inliers = inliers
        self.residualP95Ms = residualP95Ms
        self.temporalCoverage = temporalCoverage
        self.matched = matched
    }
}

// ---------- method-neutral pair evidence (PLAN-SPECTRAL-FINGERPRINT-MATCHING §5 / §14.2) ----------

/// The matcher that produced a `PairEvidence` (core `MatchMethod`, §5.1). Display only — a resolver
/// gates on `classification`, never on the method. Wire is the camelCase enum string verbatim.
public enum MatchMethod: String, Codable, Sendable {
    /// Spectral-landmark consensus (the boundary 4 correspondences → boundary 5 affine consensus).
    case spectralLandmarkV1
    /// The legacy amplitude-envelope matcher (`pair_correlate`).
    case envelopeV1
}

/// The method-neutral classification of a candidate pair (core `MatchClassification`, §5.1) — the
/// ONLY field a resolver keys on (boundary 9). Wire is the camelCase enum string verbatim.
public enum MatchClassification: String, Codable, Sendable {
    /// A confident same-recording match with an accepted affine/lag mapping.
    case match
    /// Confidently not the same recording (or no fittable affine hypothesis).
    case noMatch
    /// Real evidence that is not confidently unique, or a resource ceiling was hit — honest
    /// uncertainty, preferred over a confident wrong pairing.
    case ambiguous
    /// The two tracks cannot overlap by the pairing floor — no candidate possible.
    case insufficientOverlap
    /// The matcher could not run (e.g. a fingerprint could not be built and no legacy fallback was
    /// available). Distinct from `noMatch`: absence of evidence, not evidence of absence.
    case unavailable
}

/// Method-neutral evidence for one candidate (video, audio) pair (core `PairEvidence`, §5.1).
///
/// THIS BOUNDARY (8) IS PLUMBING: a passive Codable carrier decoded off `cpl_match_pair` /
/// `cpl_match_candidates`. The resolver migration (boundary 9) will read only `classification`, the
/// refined lag/drift when present, `score` (deterministic ordering), and `scoreMargin`/quality for
/// display — and MUST NOT reproduce the fingerprint acceptance formula, which lives solely in core
/// (§14.2). The optional residual/refined fields decode core `null` (a non-applicable measurement)
/// to `nil`; core never serializes NaN/Infinity (§5.1/§20). `legacy` is the shadow-comparison
/// `PairCorrelation` (present in `fingerprintShadow`; `nil` otherwise). Field names are the wire
/// format verbatim, so no key-decoding strategy is needed; the core-only `refineReason` is
/// `#[serde(skip)]` and never appears on the wire.
public struct PairEvidence: Codable, Sendable {
    public var method: MatchMethod
    public var classification: MatchClassification

    // Mapping from A (video) time to B (audio) time (§3.1 convention).
    public var lagMs: Double
    public var driftPpm: Double

    // Method-neutral ranking and uniqueness (higher is better; NOT a probability).
    public var score: Double
    public var runnerUpScore: Double
    public var scoreMargin: Double

    // Consensus quality.
    public var support: UInt32
    public var candidateCorrespondences: UInt32
    public var inlierFraction: Double
    public var temporalCoverage: Double
    /// `null` (core `None`) when not applicable (no fittable hypothesis, or the legacy path).
    public var residualRmsMs: Double?
    public var residualP95Ms: Double?
    public var overlapS: Double

    // Fine refinement of the accepted affine hypothesis (boundary 7, §11) — `null` until refined.
    public var refinedLagMs: Double?
    public var refinedDriftPpm: Double?
    public var refinedRmsMs: Double?

    // Optional legacy telemetry for shadow comparison only (`null` outside `fingerprintShadow`).
    public var legacy: PairCorrelation?

    public init(
        method: MatchMethod, classification: MatchClassification,
        lagMs: Double, driftPpm: Double,
        score: Double, runnerUpScore: Double, scoreMargin: Double,
        support: UInt32, candidateCorrespondences: UInt32,
        inlierFraction: Double, temporalCoverage: Double,
        residualRmsMs: Double?, residualP95Ms: Double?, overlapS: Double,
        refinedLagMs: Double? = nil, refinedDriftPpm: Double? = nil, refinedRmsMs: Double? = nil,
        legacy: PairCorrelation? = nil
    ) {
        self.method = method
        self.classification = classification
        self.lagMs = lagMs
        self.driftPpm = driftPpm
        self.score = score
        self.runnerUpScore = runnerUpScore
        self.scoreMargin = scoreMargin
        self.support = support
        self.candidateCorrespondences = candidateCorrespondences
        self.inlierFraction = inlierFraction
        self.temporalCoverage = temporalCoverage
        self.residualRmsMs = residualRmsMs
        self.residualP95Ms = residualP95Ms
        self.overlapS = overlapS
        self.refinedLagMs = refinedLagMs
        self.refinedDriftPpm = refinedDriftPpm
        self.refinedRmsMs = refinedRmsMs
        self.legacy = legacy
    }
}

// ---------- shadow telemetry projection (PLAN-SPECTRAL-FINGERPRINT-MATCHING §14.2 / §14.4) ----------

/// One row of OBSERVATIONAL spectral shadow telemetry for the diagnostic disclosure (boundary 9).
///
/// This is a PASSIVE PROJECTION of a core [`PairEvidence`] for ONE candidate (video, audio) pair —
/// every field is copied verbatim off the wire; NOTHING is re-decided here. In particular
/// `classification` is core's verdict (the single acceptance authority, §14.2): the projector below
/// surfaces it as-is and applies NO threshold, so a `match` and a `noMatch` are both carried through
/// unchanged. The resolver attaches these to its non-persisted `ResolutionDetail` for display in the
/// EXISTING diagnostic disclosure only; per §14.4 they NEVER appear in (or change) the normal
/// queue-row state vocabulary. `lagMs`/`driftPpm` are the consensus mapping; `refinedLagMs`/
/// `refinedDriftPpm` (when core supplied them, boundary 7) are the fine-refined mapping. The quality
/// fields (`support`, `inlierFraction`, `temporalCoverage`, `residualP95Ms`, `scoreMargin`) are for
/// the owner's calibration read-out; `score` is used ONLY for the deterministic display ordering
/// (see [`ShadowTelemetry`]). Not `Codable`/persisted — telemetry, never queue state.
public struct ShadowEvidenceRow: Sendable, Equatable {
    /// Pool content key of the audio this evidence scores (stable identity; not displayed).
    public var audioKey: String
    /// Audio file name (the displayed identity; also the deterministic ordering tiebreak).
    public var audioName: String

    // ---- copied verbatim from core PairEvidence (no re-decision) ----
    public var method: MatchMethod
    /// Core's acceptance verdict — surfaced as-is; the projector NEVER re-thresholds it (§14.2).
    public var classification: MatchClassification
    public var lagMs: Double
    public var driftPpm: Double
    public var score: Double
    public var scoreMargin: Double
    public var support: UInt32
    public var inlierFraction: Double
    public var temporalCoverage: Double
    /// Nearest-rank p95 of inlier |residual| (ms); `nil` when core had no fittable hypothesis.
    public var residualP95Ms: Double?
    /// Fine-refined lag/drift (boundary 7); `nil` until core populates them.
    public var refinedLagMs: Double?
    public var refinedDriftPpm: Double?

    public init(audioKey: String, audioName: String, evidence: PairEvidence) {
        self.audioKey = audioKey
        self.audioName = audioName
        self.method = evidence.method
        self.classification = evidence.classification
        self.lagMs = evidence.lagMs
        self.driftPpm = evidence.driftPpm
        self.score = evidence.score
        self.scoreMargin = evidence.scoreMargin
        self.support = evidence.support
        self.inlierFraction = evidence.inlierFraction
        self.temporalCoverage = evidence.temporalCoverage
        self.residualP95Ms = evidence.residualP95Ms
        self.refinedLagMs = evidence.refinedLagMs
        self.refinedDriftPpm = evidence.refinedDriftPpm
    }
}

/// PURE projection of a row of core [`PairEvidence`] (the matrix row for ONE video, one cell per
/// audio) into ordered, displayable [`ShadowEvidenceRow`]s (PLAN-SPECTRAL-FINGERPRINT-MATCHING
/// §14.2). This is the boundary-9 decision logic, kept in `CoupleKit` so `swift test`
/// (`CoupleKitTests`) gives it real automated coverage — mirroring how the prework put the overflow
/// decision in a pure function. No engine, no media, no I/O.
///
/// CONTRACT (the testable invariants):
///   1. PASSIVE — it copies core fields; it does NO acceptance thresholding and does NOT recompute
///      the fingerprint acceptance formula. `classification` is whatever core decided (§14.2): a
///      `match` and a `noMatch` both appear, verbatim, in the output. Acceptance stays in core.
///   2. DETERMINISTIC TOTAL ORDER — rows sort by `score` DESCENDING, with ties broken by `audioName`
///      ascending then `audioKey` ascending. Because the final tiebreak (`audioKey`, the pool
///      content key) is unique per cell, the order is a TOTAL order: equal/zero scores never leave
///      the result run-to-run ambiguous (`score` is used for ordering ONLY, never as a gate).
public enum ShadowTelemetry {
    /// Project + order one video's evidence row. `audioKeys`/`audioNames` are index-aligned to
    /// `evidence` (cell `i` scores audio `i`), exactly the column order `Engine.matchCandidates`
    /// returns. Mismatched lengths defensively truncate to the shortest (no crash, no fabrication).
    public static func rows(
        evidence: [PairEvidence], audioKeys: [String], audioNames: [String]
    ) -> [ShadowEvidenceRow] {
        let n = min(evidence.count, min(audioKeys.count, audioNames.count))
        var out: [ShadowEvidenceRow] = []
        out.reserveCapacity(n)
        for i in 0..<n {
            out.append(ShadowEvidenceRow(audioKey: audioKeys[i], audioName: audioNames[i],
                                         evidence: evidence[i]))
        }
        // Deterministic TOTAL order: score desc, then name asc, then key asc (key is unique).
        // `score` orders the display; it is NEVER an acceptance gate (that is core's classification).
        out.sort { a, b in
            if a.score != b.score { return a.score > b.score }
            if a.audioName != b.audioName { return a.audioName < b.audioName }
            return a.audioKey < b.audioKey
        }
        return out
    }
}

/// Swift mirror of core `PairingParams` (PLAN-AUDIO-MATCHING-PREWORK §6) — the single
/// pairing/verifier parameter contract. The app decodes the DEFAULTS from core
/// (`Engine.pairingParamsDefault()` → `cpl_pairing_params_default_json`) instead of keeping
/// its own copies, so a verifier threshold lives in exactly one place. The memberwise
/// defaults here are only a typed fallback if that FFI read ever fails; the live values
/// always come from core. Wire is camelCase verbatim.
public struct PairingParams: Codable, Sendable, Equatable {
    public var minOverlapS: Double = 8.0
    public var legacyZMin: Double = 8.5
    public var refineNccMin: Double = 0.15
    public var stageSpansMs: [Double] = [500.0, 80.0, 14.0, 3.0]
    public var parabolicClamp: Double = 0.6
    public var verifyWinFrac: Double = 0.5
    public var verifyHalfWinMinS: Double = 2.5
    public var verifyHalfWinMaxS: Double = 9.0
    public var verifyDensityPerS: Double = 0.5
    public var verifyMinWindowsFloor: Int = 3
    public var verifyMaxWindows: Int = 24
    public var verifyBaseSpanMs: Double = 120.0
    public var verifySpanScalePpm: Double = 150.0
    public var verifySpanCapMs: Double = 500.0
    public var verifyNccWindowMin: Double = 0.25
    public var verifyMinLocked: UInt32 = 4
    public var verifyLockFractionMin: Double = 0.66
    public var verifyRmsMaxMs: Double = 12.0
    public var verifyOutlierFloorMs: Double = 2.0
    public var driftLambdaS2: Double = 50.0
    public var driftMaxPpm: Double = 600.0
    public var maxVerifyPairsPerVideo: Int = 32
    public var maxVerifyPairsTotal: Int = 4096

    public init() {}
}
