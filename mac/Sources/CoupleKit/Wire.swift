// Value types mirroring couple-core's wire JSON (SPEC §4; field names are the
// wire format verbatim, so no key-decoding strategy is needed).

import CCouple
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
        // RELOCATION (PLAN-CORE-SHELL-BOUNDARY Phase E): the PROJECTION (ordering + index-alignment
        // + truncation) now lives in `couple_core::policy::shadow` and is reached over the FFI
        // (`cpl_policy_shadow_rows`). This is a thin WRAPPER: it sends the evidence row + keys/names,
        // and rebuilds each display `ShadowEvidenceRow` from core's verbatim, score-ordered evidence
        // (so the display fields/strings stay Swift-side). On a core error (a malformed/stale
        // request — unreachable in practice) it maps to an EMPTY projection: shadow telemetry simply
        // does not display, never a fabricated row.
        let req: [String: Any] = [
            "evidence": evidence.compactMap(\.wireDict),
            "audioKeys": audioKeys,
            "audioNames": audioNames,
        ]
        return projectedRows(fromEnvelope: callPolicy(req))
    }

    /// Decode core's score-ordered projection rows (`[{audioKey,audioName,evidence}]`) into display
    /// `ShadowEvidenceRow`s. A nil/malformed envelope ⇒ [] (telemetry does not display; never a
    /// fabricated row). Internal test seam (PLAN §3.2).
    @inline(never)
    static func projectedRows(fromEnvelope value: [Any]?) -> [ShadowEvidenceRow] {
        guard let value else { return [] }
        var out: [ShadowEvidenceRow] = []
        out.reserveCapacity(value.count)
        for case let row as [String: Any] in value {
            guard let audioKey = row["audioKey"] as? String,
                  let audioName = row["audioName"] as? String,
                  let evidenceObj = row["evidence"],
                  let evidenceData = try? JSONSerialization.data(withJSONObject: evidenceObj),
                  let evidence = try? JSONDecoder().decode(PairEvidence.self, from: evidenceData)
            else { continue }
            out.append(ShadowEvidenceRow(audioKey: audioKey, audioName: audioName, evidence: evidence))
        }
        return out
    }

    /// Encode the request, call `cpl_policy_shadow_rows`, and return the envelope `value` array
    /// (nil on an encode/call/parse failure or an error envelope — the caller maps nil to []).
    private static func callPolicy(_ req: [String: Any]) -> [Any]? {
        guard let data = try? JSONSerialization.data(withJSONObject: req),
              let json = String(data: data, encoding: .utf8) else { return nil }
        return PolicyEnvelope.decodeValueArray(cpl_policy_shadow_rows(json))
    }
}

extension PairEvidence {
    /// The core `PairEvidence` wire dict — re-encode the Codable value to a JSON object so it can be
    /// embedded in the shadow-projection request. nil only on an (unreachable) encode failure.
    var wireDict: [String: Any]? {
        guard let data = try? JSONEncoder().encode(self),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        return obj
    }
}

// ---------- row resolution wire types (PLAN-CORE-SHELL-BOUNDARY §3.1) ----------
//
// These decode core `cpl_resolve` / `cpl_resolve_start`'s `[ResolvedRow]` JSON (camelCase verbatim).
// Core returns the DECISION (state / selected / alternates with z·lagMs·coverage + the resolution
// honesty trail); the shell DECORATES each row with DISPLAY-ONLY telemetry afterwards — the
// per-candidate fine-verify fields (`fitRmsMs`/`fitWindows`/`lockFraction`/`expected`/`ppm`/`clamped`)
// and the spectral shadow evidence (`shadowEvidence`, `fitBestRmsMs`/`fitSecondRmsMs`). Those decorated
// fields are NOT on core's wire, so they DEFAULT to nil on decode and are filled shell-side. The public
// shape (memberwise inits, field names) is exactly what AppModel / CoupleRow / the Views consumed
// before this layer moved into CoupleKit.

/// Span of the video timeline an audio covers (§3.2; overlap_info with ppm 0). Decodes core's
/// `coverage` object verbatim; the convenience init reproduces the SPEC §5.6 drift-0 arithmetic for
/// the shell's manual-pick construction sites.
public struct Coverage: Sendable, Equatable, Decodable {
    public var t0: Double
    public var t1: Double
    public var seconds: Double
    public var fraction: Double

    public init(t0: Double, t1: Double, seconds: Double, fraction: Double) {
        self.t0 = t0; self.t1 = t1; self.seconds = seconds; self.fraction = fraction
    }

    /// SPEC §5.6 overlap arithmetic at drift 0 (delta = 1).
    public init(durVideo: Double, durAudio: Double, lagMs: Double) {
        let offS = lagMs / 1000
        t0 = max(0, -offS)
        t1 = min(durVideo, durAudio - offS)
        seconds = max(0, t1 - t0)
        fraction = durVideo > 0 ? seconds / durVideo : 0
    }
}

/// §3.3 row states (chip vocabulary mirrors the CLI/UX-DECISIONS-V2). Wire is the camelCase string
/// verbatim. The UI `Verdict` mapping (`verdict`) lives app-side (where `Verdict` is defined).
public enum RowState: String, Sendable, Codable {
    case confident, partial, split, twoSources, ambiguous, noMatch

    public var chip: String {
        switch self {
        case .confident: return "confident"
        case .partial: return "partial"
        case .split: return "split across files"
        case .twoSources: return "2 sources"
        case .ambiguous: return "ambiguous — check me"
        case .noMatch: return "no match"
        }
    }
}

/// One candidate audio source for a video, with its measured fit and coverage. `audioKey` is the pool
/// content key (stable identity); `audioName` drives the CLI-identical name tie-breaks (SPEC §1.4 —
/// clock-free). Core supplies `audioKey`/`audioName`/`z`/`lagMs`/`coverage`; the telemetry fields below
/// are DISPLAY-ONLY (no `if` reads them; excluded from any stability surface — not Equatable/Hashable;
/// QueueStore persists only key/flags, never metrics) and default to nil on decode (decorated shell-side).
public struct RowCandidate: Sendable, Identifiable, Decodable {
    public var id: String { audioKey }
    public var audioKey: String
    public var audioName: String
    public var z: Double
    public var lagMs: Double
    public var coverage: Coverage
    // Phase-1 tie-breaker + fine-verify telemetry (decorated shell-side; not on core's wire).
    public var fitRmsMs: Double? = nil
    public var fitWindows: Int? = nil
    public var lockFraction: Double? = nil
    public var expected: Int? = nil
    public var ppm: Double? = nil
    public var clamped: Bool? = nil

    public init(audioKey: String, audioName: String, z: Double, lagMs: Double, coverage: Coverage,
                fitRmsMs: Double? = nil, fitWindows: Int? = nil, lockFraction: Double? = nil,
                expected: Int? = nil, ppm: Double? = nil, clamped: Bool? = nil) {
        self.audioKey = audioKey; self.audioName = audioName; self.z = z; self.lagMs = lagMs
        self.coverage = coverage
        self.fitRmsMs = fitRmsMs; self.fitWindows = fitWindows; self.lockFraction = lockFraction
        self.expected = expected; self.ppm = ppm; self.clamped = clamped
    }

    // Decode ONLY core's fields; the telemetry fields stay at their nil defaults (decorated later).
    enum CodingKeys: String, CodingKey { case audioKey, audioName, z, lagMs, coverage }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        audioKey = try c.decode(String.self, forKey: .audioKey)
        audioName = try c.decode(String.self, forKey: .audioName)
        z = try c.decode(Double.self, forKey: .z)
        lagMs = try c.decode(Double.self, forKey: .lagMs)
        coverage = try c.decode(Coverage.self, forKey: .coverage)
    }
}

/// §3.4 record for one overlapping audio pair (raw numbers for V2-F2 calibration). Decodes core verbatim.
public struct TriangleCheck: Sendable, Decodable {
    public var audioA: String
    public var audioB: String
    public var abZ: Double
    public var abLagMs: Double
    public var residualMs: Double
    public var simultaneous: Bool

    public init(audioA: String, audioB: String, abZ: Double, abLagMs: Double,
                residualMs: Double, simultaneous: Bool) {
        self.audioA = audioA; self.audioB = audioB; self.abZ = abZ; self.abLagMs = abLagMs
        self.residualMs = residualMs; self.simultaneous = simultaneous
    }
}

/// Why a row's candidates could not be verified within the configured resource ceiling
/// (PLAN-AUDIO-MATCHING-PREWORK §9.2). Decodes core's `candidateOverflow` object verbatim.
public struct CandidateOverflow: Sendable, Decodable {
    /// Number of eligible (`prefilterCandidate`) cells for this row.
    public var eligible: Int
    /// The resource ceiling that was exceeded.
    public var limit: Int
    /// Which ceiling: "perVideo" (`maxVerifyPairsPerVideo`) or "total" (`maxVerifyPairsTotal`).
    public var scope: String

    public init(eligible: Int, limit: Int, scope: String) {
        self.eligible = eligible; self.limit = limit; self.scope = scope
    }
}

/// The honesty trail behind a row's state — surfaced raw so the owner can tune. Core supplies
/// `margin`/`clusters`/`triangles`/`note`/`candidateOverflow`; the fit-summary and shadow fields are
/// DISPLAY-ONLY (decorated shell-side) and default to nil on decode.
public struct ResolutionDetail: Sendable, Decodable {
    public var margin: Double?
    public var clusters: Int
    public var triangles: [TriangleCheck] = []
    public var note: String?
    public var candidateOverflow: CandidateOverflow? = nil
    // Decorated shell-side (telemetry, not core's decision; non-persisted).
    public var fitBestRmsMs: Double? = nil
    public var fitSecondRmsMs: Double? = nil
    public var shadowEvidence: [ShadowEvidenceRow]? = nil

    public init(margin: Double?, clusters: Int, triangles: [TriangleCheck] = [], note: String? = nil,
                candidateOverflow: CandidateOverflow? = nil,
                fitBestRmsMs: Double? = nil, fitSecondRmsMs: Double? = nil,
                shadowEvidence: [ShadowEvidenceRow]? = nil) {
        self.margin = margin; self.clusters = clusters; self.triangles = triangles; self.note = note
        self.candidateOverflow = candidateOverflow
        self.fitBestRmsMs = fitBestRmsMs; self.fitSecondRmsMs = fitSecondRmsMs
        self.shadowEvidence = shadowEvidence
    }

    // Decode only core's fields (triangles/note/candidateOverflow optional); telemetry stays nil.
    enum CodingKeys: String, CodingKey { case margin, clusters, triangles, note, candidateOverflow }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        margin = try c.decodeIfPresent(Double.self, forKey: .margin)
        clusters = try c.decode(Int.self, forKey: .clusters)
        triangles = try c.decodeIfPresent([TriangleCheck].self, forKey: .triangles) ?? []
        note = try c.decodeIfPresent(String.self, forKey: .note)
        candidateOverflow = try c.decodeIfPresent(CandidateOverflow.self, forKey: .candidateOverflow)
    }
}

/// A resolved queue row = one video + its selected audio (nil for ambiguous / no-match: never a
/// coin-flip) + the alternates. Mirrors `couple pair`'s Row. Core's row field is `resolution`; the
/// Swift field is `detail` (mapped via CodingKeys), keeping the shape the UI consumed.
public struct ResolvedRow: Sendable, Decodable {
    public var videoKey: String
    public var videoName: String
    public var state: RowState
    public var chip: String
    public var selected: RowCandidate?
    public var alternates: [RowCandidate]
    public var detail: ResolutionDetail

    public init(videoKey: String, videoName: String, state: RowState, chip: String,
                selected: RowCandidate?, alternates: [RowCandidate], detail: ResolutionDetail) {
        self.videoKey = videoKey; self.videoName = videoName; self.state = state; self.chip = chip
        self.selected = selected; self.alternates = alternates; self.detail = detail
    }

    // Map core's `resolution` → the Swift `detail` field; everything else is the wire name verbatim.
    enum CodingKeys: String, CodingKey {
        case videoKey, videoName, state, chip, selected, alternates
        case detail = "resolution"
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        videoKey = try c.decode(String.self, forKey: .videoKey)
        videoName = try c.decode(String.self, forKey: .videoName)
        state = try c.decode(RowState.self, forKey: .state)
        chip = try c.decode(String.self, forKey: .chip)
        selected = try c.decodeIfPresent(RowCandidate.self, forKey: .selected)
        alternates = try c.decode([RowCandidate].self, forKey: .alternates)
        detail = try c.decode(ResolutionDetail.self, forKey: .detail)
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
