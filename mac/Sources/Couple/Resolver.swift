// Pairing resolution (SPEC-V2 §3.3). Phase C of PLAN-CORE-SHELL-BOUNDARY moved the
// DECISION (coverage clustering, the §3.4 simultaneity triangle, the §3.5 margin rule,
// the §3.3 row states, candidate-overflow handling, and the legacy RMS tie-breaker)
// INTO couple-core (`cpl_resolve` / `cpl_resolve_start`), so the CLI and this shell share
// ONE resolver — `AppModel.resolve()` calls `Engine.resolveStart(...)` and gets the same
// `[ResolvedRow]` the CLI does, byte-for-byte.
//
// What stays here is the SHELL's concern only:
//   - the params bridge (`PairParams.resolveParamsJSON`, in PairParams.swift terms) that
//     maps the shell's override switches to a core `ResolveParams` JSON;
//   - the DISPLAY-ONLY decoration (`batchVerify` fine-verify telemetry + the spectral
//     `shadowEvidence`) the UI shows alongside core's decision — no `if` reads it, it is
//     excluded from every stability surface, and it is attached AFTER core decides;
//   - the UI `Verdict`/`MatchQuality` mapping and the manual-pick confidence probe
//     (`manualPairIsConfident`), which reuse the SAME core evidence path (no re-derived
//     threshold, invariant 8).
//
// The wire/UI types (Coverage / RowState / RowCandidate / TriangleCheck / CandidateOverflow /
// ResolutionDetail / ResolvedRow) now live in CoupleKit's Wire.swift so `Engine.resolve*` can
// decode them; their public shape is unchanged.

import CoupleKit
import Foundation

/// Resolver SHELL state (PLAN-CORE-SHELL-BOUNDARY §4). The matcher / classifier / shadow switches
/// the shell decides, plus the two resolution constants the shell READS FOR PRESENTATION. Phase D
/// removed the shell's hardcoded copies of the resolver constants that only fed the resolve
/// (`triangleTolMs`/`coverageFull`/`disjointTolS`/`rmsTrustFloorMs`/`rmsRatio`/`minFitWindows`):
/// they now live in ONE place — core `ResolveParams::default()` — and `resolveParamsJSON()` OMITS
/// them so core's `#[serde(default)]` supplies them. The resolve can no longer drift from core.
struct PairParams: Sendable, Equatable {
    // The two constants the SHELL reads for PRESENTATION (they do NOT drive the resolve — core owns
    // that): `zMin` gates the "forced" badge on a manual sub-floor pair (AppModel), `zMargin` rides
    // the fit line (CenterColumn). Deliberate presentation MIRRORS of core `ResolveParams::default()`
    // (plan §5 acceptable-duplication), kept in lock-step with core's 8.5 / 6.0.
    var zMin = 8.5            // [VALIDATED] mirror of core ResolveParams.z_min
    var zMargin = 6.0         // [TUNE] mirror of core ResolveParams.z_margin

    // ---- resolver-owned classifier policy (PLAN-AUDIO-MATCHING-PREWORK §6.6) ----
    // The fine-verifier THRESHOLDS live in core `PairingParams`. `verifyClassifierActive` is the
    // Phase-1↔Phase-2 boundary switch (AppModel defaults ON); it maps to the core `ResolveParams.
    // verifyActive`. Core now owns the candidacy/overflow logic; the shell only passes the flag.
    var verifyClassifierActive = false // [PHASE-2] flag

    // ---- spectral-shadow telemetry (PLAN-SPECTRAL-FINGERPRINT-MATCHING §14.2 / §14.4) ----
    // OPT-IN, OFF BY DEFAULT. When ON (and the legacy override is active), the spectral `PairEvidence`
    // matrix is computed ALONGSIDE the core decision and attached to each row's
    // `ResolutionDetail.shadowEvidence` as OBSERVATIONAL display telemetry (see `shadowEvidence`).
    // AppModel wires this to the COUPLE_SHADOW_MATCHER env var.
    var shadowMatcherActive = false // [BOUNDARY-9] opt-in observational shadow

    // ---- boundary-13 default matcher switch (PLAN-SPECTRAL-FINGERPRINT-MATCHING §16.1) ----
    // The DEFAULT resolver uses the fingerprint matcher (maps to core `ResolveParams.matcher =
    // "fingerprint"`). The DEVELOPER OVERRIDE back to the proven envelope resolution (for ≥ 1 release,
    // §16.1) sets this false — AppModel wires it to COUPLE_MATCHER=legacy (mirrors the CLI
    // `--matcher legacy`). On the legacy path the shell still requests core's RMS escalation
    // (`rmsEscalation = true`) so it stays byte-identical to the old Swift two-pass resolver.
    var matcherFingerprintPrimary = true // [BOUNDARY-13] default ON; COUPLE_MATCHER=legacy forces off
}

extension PairParams {
    /// Map this shell `PairParams` to a core `ResolveParams` JSON string (PLAN-CORE-SHELL-BOUNDARY
    /// §3.1) so `Engine.resolve`/`resolveStart` run the SAME decision the CLI does. The mapping is
    /// behavior-preserving:
    ///   - `matcher` = fingerprint when `matcherFingerprintPrimary`, else legacy (the shadow display
    ///     stays legacy-resolved — it is decorated shell-side, not a core matcher choice here).
    ///   - `verifyActive` = `verifyClassifierActive` (the Phase-1↔Phase-2 classifier switch).
    ///   - `rmsEscalation` = `!matcherFingerprintPrimary`: the Swift LEGACY resolver ALWAYS escalated,
    ///     and the fingerprint path never does, so this keeps legacy byte-identical to today's shell.
    ///   - `rmsStrongForm` is the app `Resolver.rmsStrongForm` static (the conservative↔strong switch).
    /// Phase D (PLAN-CORE-SHELL-BOUNDARY §4): the resolver CONSTANTS (z / margin / triangle / coverage
    /// / cluster + the RMS knobs) and the core-owned `pairing` object are OMITTED — core's
    /// `#[serde(default)]` supplies them from `ResolveParams::default()`, so those defaults live in
    /// exactly one place (core) and the shell can never drift them. The shell sends ONLY its
    /// DECISIONS: which matcher, whether the classifier is on, whether the legacy path escalates, and
    /// the dev strong-form toggle. Built with `JSONSerialization` (finite Doubles only) so the
    /// camelCase wire form is exact.
    func resolveParamsJSON() -> String {
        let obj: [String: Any] = [
            "matcher": matcherFingerprintPrimary ? "fingerprint" : "legacy",
            "verifyActive": verifyClassifierActive,
            "rmsEscalation": !matcherFingerprintPrimary,
            "rmsStrongForm": Resolver.rmsStrongForm,
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: obj),
              let json = String(data: data, encoding: .utf8) else {
            // Unreachable for finite primitives; fall back to core defaults (nil ⇒ ResolveParams::default).
            return "{}"
        }
        return json
    }
}

/// Verdict-model bucket (UX-DECISIONS-V2 §V2.3): good→TO APPROVE,
/// questionable/none→TO FIX. Coverage is never a quality penalty, so a
/// `partial` row with a real match is still good. (`RowState` itself lives in CoupleKit;
/// this UI mapping stays app-side where `Verdict` is defined.)
extension RowState {
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
    // MARK: - display decoration (telemetry + shadow attach)

    /// DISPLAY-ONLY decoration of core's decision rows (PLAN-CORE-SHELL-BOUNDARY Phase C). Core
    /// returns the DECISION (`ResolvedRow` state/selected/alternates); this attaches the per-candidate
    /// fine-verify telemetry (`batchVerify`) and, when `shadowMatcherActive`, the spectral shadow
    /// evidence (`shadowEvidence`) onto each row's candidates / `ResolutionDetail`. It NEVER changes a
    /// row's `state`/`selected`/`alternates` — the decision is already final; only the informational
    /// fields (`fitRmsMs`/`fitWindows`/`lockFraction`/`expected`/`ppm`/`clamped`, the
    /// `fitBestRmsMs`/`fitSecondRmsMs` banner summary, and `shadowEvidence`) are filled. This is the
    /// telemetry-attach half of the old two-pass `finalize`, re-expressed as a pure decorator (the
    /// ESCALATION half is now in core). Off-main safe (Engine/TrackHandle are Sendable).
    ///
    /// `verifyByCand` is the §4.4 fine-verify map over the SELECTED + ALTERNATE candidates of each row
    /// (the candidates the owner can see). It is keyed by (videoKey ⊕ audioKey).
    static func decorate(_ rows: [ResolvedRow], videos: [ResolverVideo], audios: [ResolverAudio],
                         engine: Engine, params: PairParams) -> [ResolvedRow] {
        // Fine-verify telemetry over each row's visible candidates (selected + alternates). The set is
        // the rows' own candidate audio keys mapped back to handles via the pool.
        let videoByKey = Dictionary(videos.map { ($0.key, $0) }, uniquingKeysWith: { a, _ in a })
        let audioByKey = Dictionary(audios.map { ($0.key, $0) }, uniquingKeysWith: { a, _ in a })
        let verifyByCand = batchVerify(rows: rows, videoByKey: videoByKey, audioByKey: audioByKey,
                                       engine: engine)

        // OBSERVATIONAL spectral shadow telemetry — computed ONLY when the opt-in flag is ON, AFTER
        // the core decision (which it never consults). Flag OFF (the default): no `Engine.matchCandidates`
        // call, an empty map, and the rows are byte-identical to today (§5.2).
        let shadowByVideo = params.shadowMatcherActive
            ? shadowEvidence(videos: videos, audios: audios, engine: engine)
            : [:]

        return rows.map { row in
            var r = row
            let vk = r.videoKey
            func attach(_ cand: inout RowCandidate) {
                guard let v = verifyByCand[vk + "\u{0}" + cand.audioKey] else { return }
                // Fill fitRmsMs/fitWindows from VerifyFit (rms/locked) and the richer fields. All
                // informational — no `if` reads them; excluded from any stability surface.
                cand.fitRmsMs = v.rmsMs
                cand.fitWindows = Int(v.locked)
                cand.lockFraction = v.lockFraction
                cand.expected = Int(v.expected)
                cand.ppm = v.ppm
                cand.clamped = v.clamped
            }
            if r.selected != nil { attach(&r.selected!) }
            for ai in r.alternates.indices { attach(&r.alternates[ai]) }

            // Banner summary (§4.5): the two best candidates' RMS, in the rows' candidate order.
            let cands = (r.selected.map { [$0] } ?? []) + r.alternates
            let withFits = cands.filter { $0.fitRmsMs != nil }
            if withFits.count >= 1 { r.detail.fitBestRmsMs = withFits[0].fitRmsMs }
            if withFits.count >= 2 { r.detail.fitSecondRmsMs = withFits[1].fitRmsMs }

            // Boundary-9 (§14.2/§14.4): attach the OBSERVATIONAL spectral shadow telemetry for this
            // video. Empty when the flag is OFF ⇒ stays nil and the row is byte-identical. Display-only.
            if let shadow = shadowByVideo[vk] { r.detail.shadowEvidence = shadow }
            return r
        }
    }

    /// Phase-1 fine-verify telemetry over the VISIBLE candidate set (§4.4): ONE `verifyBatch` for
    /// every (row, selected/alternate) cell, returning VerifyFit per (videoKey ⊕ audioKey). Purely
    /// informational — no decision reads it. Off-main safe (Engine/TrackHandle are Sendable). Empty
    /// input ⇒ no core call.
    private static func batchVerify(rows: [ResolvedRow], videoByKey: [String: ResolverVideo],
                                    audioByKey: [String: ResolverAudio],
                                    engine: Engine) -> [String: VerifyFit] {
        var order: [(videoKey: String, audioKey: String)] = []
        var pairs: [(TrackHandle, TrackHandle, Double)] = []
        for row in rows {
            guard let vh = videoByKey[row.videoKey]?.handle else { continue }
            let cands = (row.selected.map { [$0] } ?? []) + row.alternates
            for cand in cands {
                guard let ah = audioByKey[cand.audioKey]?.handle else { continue }
                order.append((row.videoKey, cand.audioKey))
                pairs.append((vh, ah, cand.lagMs))
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
    /// `fingerprintShadow` mode over the SAME importable V×A matrix the resolver scored, then
    /// projects each video's evidence row into ordered `ShadowEvidenceRow`s via the PURE
    /// `ShadowTelemetry.rows` (which surfaces core's `classification` verbatim and orders by `score`
    /// — NO acceptance thresholding; acceptance lives in core). Returns a (videoKey → rows) map; the
    /// resolved rows are NEVER derived from this (display/debug only). Off-main safe. On any core
    /// fault `matchCandidates` returns nil ⇒ this yields an empty map (best-effort observation).
    private static func shadowEvidence(videos: [ResolverVideo], audios: [ResolverAudio],
                                       engine: Engine) -> [String: [ShadowEvidenceRow]] {
        let importable = videos.filter { $0.handle != nil }
        guard !importable.isEmpty, !audios.isEmpty else { return [:] }
        // Request the spectral SHADOW matcher via a PairingParams override (matcherMode only).
        let shadowJSON = #"{"matcherMode":"fingerprintShadow"}"#
        guard let matrix = engine.matchCandidates(videos: importable.compactMap(\.handle),
                                                  audios: audios.map(\.handle),
                                                  paramsJSON: shadowJSON) else { return [:] }
        let audioKeys = audios.map(\.key)
        let audioNames = audios.map(\.name)
        // `matrix` rows align to the importable videos in order (audio-less videos are absent).
        var byVideo: [String: [ShadowEvidenceRow]] = [:]
        for (i, v) in importable.enumerated() where i < matrix.count {
            byVideo[v.key] = ShadowTelemetry.rows(evidence: matrix[i],
                                                  audioKeys: audioKeys, audioNames: audioNames)
        }
        return byVideo
    }

    /// Audio keys no resolved row selects — the §3.6 unused tray.
    static func unusedAudio(rows: [ResolvedRow], audios: [ResolverAudio]) -> [ResolverAudio] {
        let used = Set(rows.compactMap { $0.selected?.audioKey })
        return audios.filter { !used.contains($0.key) }
    }

    // MARK: - classifier predicate (PLAN-MATCH-CLASSIFIER §2.3)

    /// THE conservative↔strong RMS-escalation switch (§9 R5 / open-Q3). One line. The decision itself
    /// now runs in core; this maps to the core `ResolveParams.rmsStrongForm` param (see
    /// `PairParams.resolveParamsJSON`). Conservative is the shipped default.
    static let rmsStrongForm = false

    /// The fine-verify classifier predicate. Core computes the decision once
    /// (`VerifyFit.matched`, PLAN-AUDIO-MATCHING-PREWORK §6.4); the shell surfaces it so
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
    /// the core resolver decides candidacy. A core fault (nil) is NOT confident — a
    /// manual pick never greens on absent evidence. Pure measurement; safe off-main
    /// (Engine/TrackHandle Sendable).
    static func manualPairIsConfident(video: TrackHandle, audio: TrackHandle,
                                      engine: Engine, params: PairParams) -> Bool {
        if params.matcherFingerprintPrimary {
            // Fingerprint identity is lag-independent (core builds its own
            // correspondence), so the cell's classification is the whole answer.
            return engine.matchPair(video, audio)?.classification == .match
        }
        // Legacy verifier: it only locks within ±span of the SEED lag (core
        // verify.rs), so seed it with the same Stage-1 coarse lag (`pairCorrelate.lagMs`)
        // the legacy resolver feeds the verifier — passing 0 would miss any offset pair.
        // No plausible overlap ⇒ not confident.
        guard let coarse = engine.pairCorrelate(video, audio), !coarse.insufficientOverlap,
              let fit = engine.verifyBatch([(video, audio, coarse.lagMs)])?.first ?? nil else { return false }
        return isMatch(fit)
    }
}
