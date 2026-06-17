// Engine: the CoupleKit handle over cpl_ctx (SPEC §4.9). Thread-safe; hot reads
// are callable from any thread, jobs report from worker threads.

import CCouple
import Foundation

public struct CoreError: Error, Sendable {
    public let kind: String
    public let message: String
}

public final class Engine: @unchecked Sendable {
    let ctx: UInt64

    public init(ffmpegPath: String? = nil, ffprobePath: String? = nil, cacheDir: String? = nil) throws {
        var config: [String: String] = [:]
        config["ffmpegPath"] = ffmpegPath
        config["ffprobePath"] = ffprobePath
        config["cacheDir"] = cacheDir
        let json = String(data: try JSONEncoder().encode(config), encoding: .utf8)!
        ctx = cpl_ctx_new(json)
        guard ctx != 0 else { throw Engine.staticTakeError() }
    }

    deinit {
        cpl_ctx_free(ctx)
    }

    public static var coreVersion: String {
        takeString(cpl_version()) ?? "?"
    }

    public static var abiVersion: UInt32 {
        cpl_abi_version()
    }

    // ---------- error plumbing ----------

    /// Consume a core-allocated C string.
    static func takeString(_ p: UnsafeMutablePointer<CChar>?) -> String? {
        guard let p else { return nil }
        defer { cpl_string_free(p) }
        return String(cString: p)
    }

    func takeError() -> CoreError {
        Engine.staticTakeError(ctx: ctx)
    }

    static func staticTakeError(ctx: UInt64 = 0) -> CoreError {
        guard let json = takeString(cpl_take_error_json(ctx)),
              let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return CoreError(kind: "internal", message: "unknown core error")
        }
        var kind = "internal"
        if let e = obj["error"] as? String {
            kind = e
        } else if let e = obj["error"] as? [String: Any], let k = e.keys.first {
            kind = k
        }
        return CoreError(kind: kind, message: (obj["message"] as? String) ?? json)
    }

    func decodeJSON<T: Decodable>(_ p: UnsafeMutablePointer<CChar>?, as type: T.Type) throws -> T {
        guard let s = Engine.takeString(p) else { throw takeError() }
        guard let data = s.data(using: .utf8) else {
            throw CoreError(kind: "internal", message: "non-UTF8 core JSON")
        }
        return try JSONDecoder().decode(T.self, from: data)
    }

    /// Decode a JSON payload that may be the literal `null` (e.g. measure_local misses).
    func decodeOptionalJSON<T: Decodable>(_ p: UnsafeMutablePointer<CChar>?, as type: T.Type) throws -> T? {
        guard let s = Engine.takeString(p) else { throw takeError() }
        if s == "null" { return nil }
        guard let data = s.data(using: .utf8) else { return nil }
        return try JSONDecoder().decode(T.self, from: data)
    }

    // ---------- media ----------

    public func probe(_ path: String) throws -> MediaInfo {
        try decodeJSON(cpl_probe_json(ctx, path), as: MediaInfo.self)
    }

    /// Start an import job (§4.3); the job's value is the imported TrackHandle.
    public func importStart(_ path: String, role: TrackRole) throws -> Job<TrackMeta> {
        let box = JobBox()
        let user = Unmanaged.passRetained(box).toOpaque()
        let id = cpl_import_start(ctx, path, role == .camera ? 0 : 1, jobTrampoline, user)
        guard id != 0 else {
            Unmanaged<JobBox>.fromOpaque(user).release()
            throw takeError()
        }
        return Job(engine: self, box: box, id: id)
    }

    public func track(_ meta: TrackMeta) -> TrackHandle {
        TrackHandle(engine: self, meta: meta)
    }

    public func sessionNew(camera: TrackHandle, external: TrackHandle) throws -> SessionHandle {
        let id = cpl_session_new(ctx, camera.id, external.id)
        guard id != 0 else { throw takeError() }
        return SessionHandle(engine: self, id: id)
    }

    /// fit_ls over (t_s, lag_ms) points; nil when < 2 points (§4.4).
    public func fitLS(_ points: [(Double, Double)]) -> Fit? {
        let arr = points.map { [$0.0, $0.1] }
        guard let json = try? JSONEncoder().encode(arr),
              let s = String(data: json, encoding: .utf8) else { return nil }
        return try? decodeOptionalJSON(cpl_fit_ls_json(s), as: Fit.self)
    }

    public func cacheGC(maxBytes: UInt64) -> UInt64 {
        cpl_cache_gc(ctx, maxBytes)
    }

    // ---------- pairing (SPEC-V2 §4) ----------

    /// Core's default `PairingParams` (PLAN-AUDIO-MATCHING-PREWORK §6) — the single source of
    /// pairing/verifier defaults. The resolver reads these instead of keeping its own copies,
    /// so a verifier threshold lives in exactly one place. Falls back to the Swift mirror
    /// defaults only if the core read ever fails (never in normal operation).
    public func pairingParamsDefault() -> PairingParams {
        (try? decodeJSON(cpl_pairing_params_default_json(), as: PairingParams.self)) ?? PairingParams()
    }

    /// Role-agnostic stage-1 between any two imported tracks. Never errors on a
    /// poor match (below-floor cells keep the coarse lag); nil only on a real
    /// core fault.
    public func pairCorrelate(_ a: TrackHandle, _ b: TrackHandle) -> PairCorrelation? {
        (try? decodeOptionalJSON(cpl_pair_correlate(ctx, a.id, b.id), as: PairCorrelation.self)) ?? nil
    }

    /// Batch candidate matrix: result[i] is video i's row of cells, one per
    /// audio (same wire form as `pairCorrelate`). Returns one empty row per
    /// video when there are no audios; nil only on a core fault.
    public func pairCandidates(videos: [TrackHandle], audios: [TrackHandle]) -> [[PairCorrelation]]? {
        guard !videos.isEmpty else { return [] }
        guard !audios.isEmpty else { return videos.map { _ in [] } }
        let vs = videos.map(\.id)
        let aus = audios.map(\.id)
        return vs.withUnsafeBufferPointer { vp in
            aus.withUnsafeBufferPointer { ap in
                try? decodeJSON(
                    cpl_pair_candidates(ctx, vp.baseAddress, UInt(vp.count), ap.baseAddress, UInt(ap.count)),
                    as: [[PairCorrelation]].self)
            }
        }
    }

    /// Drift-fit RMS for contested candidate pairs (PLAN-RMS-TIEBREAKER §4.3
    /// tie-breaker telemetry). `pairs`: (video, audio, coarseLagMs), where
    /// `coarseLagMs` is a candidate's `PairCorrelation.lagMs`. The result aligns to
    /// input order; an element is nil when too few windows locked. Returns `[]` for
    /// empty input; nil only on a core fault. Pure/deterministic in core (§5.8).
    public func pairFitBatch(_ pairs: [(TrackHandle, TrackHandle, Double)]) -> [PairFit?]? {
        guard !pairs.isEmpty else { return [] }
        // Encode [[videoId, audioId, lagMs], …]; ids are UInt64 (kept integral so
        // large handles survive), lag is Double.
        let rows: [[Any]] = pairs.map { [$0.0.id, $0.1.id, $0.2] }
        guard let data = try? JSONSerialization.data(withJSONObject: rows),
              let json = String(data: data, encoding: .utf8) else { return nil }
        return try? decodeJSON(cpl_pair_fit_batch(ctx, json), as: [PairFit?].self)
    }

    /// Fine match-verification for candidate pairs (PLAN-MATCH-CLASSIFIER §2.4 / Stage B —
    /// the classifier contract Stage C consumes). `pairs`: (video, audio, coarseLagMs),
    /// where `coarseLagMs` is a candidate's `PairCorrelation.lagMs`. The result aligns to
    /// input order; an element is nil only when the overlap is below the absolute hard
    /// floor. Returns `[]` for empty input; nil only on a core fault. Pure/deterministic
    /// in core (§5.8).
    public func verifyBatch(_ pairs: [(TrackHandle, TrackHandle, Double)]) -> [VerifyFit?]? {
        guard !pairs.isEmpty else { return [] }
        // Encode [[videoId, audioId, lagMs], …]; ids are UInt64 (kept integral so large
        // handles survive), lag is Double.
        let rows: [[Any]] = pairs.map { [$0.0.id, $0.1.id, $0.2] }
        guard let data = try? JSONSerialization.data(withJSONObject: rows),
              let json = String(data: data, encoding: .utf8) else { return nil }
        return try? decodeJSON(cpl_verify_batch(ctx, json), as: [VerifyFit?].self)
    }

    // ---------- method-neutral pair evidence (PLAN-SPECTRAL-FINGERPRINT-MATCHING §14.2) ----------

    /// Method-neutral evidence for one candidate (`video`, `audio`) pair (`cpl_match_pair`, §14.2).
    /// `paramsJSON` is an optional `PairingParams` override selecting the matcher via `matcherMode`
    /// (nil ⇒ core default `legacyEnvelope` ⇒ `EnvelopeV1` evidence; pass
    /// `{"matcherMode":"fingerprintShadow"}` to run the spectral pipeline with the legacy correlation
    /// attached for comparison). nil only on a real core fault or invalid params.
    ///
    /// PLUMBING ONLY (boundary 8): this returns the decoded carrier. The resolver migration
    /// (boundary 9) will consume `classification` / refined lag-drift / `score` / `scoreMargin`; it
    /// MUST NOT reproduce the fingerprint acceptance formula — all acceptance logic stays in core.
    public func matchPair(_ video: TrackHandle, _ audio: TrackHandle, paramsJSON: String? = nil)
        -> PairEvidence?
    {
        (try? decodeOptionalJSON(
            cpl_match_pair(ctx, video.id, audio.id, paramsJSON), as: PairEvidence.self)) ?? nil
    }

    /// Batch method-neutral evidence (`cpl_match_candidates`, §14.2): `result[i]` is video `i`'s row
    /// of cells, one per audio (same wire form as `matchPair`). In the fingerprint modes core builds
    /// ONE audio-side index for the whole batch (§5.3). `paramsJSON` as in `matchPair` (nil ⇒ core
    /// default `legacyEnvelope`). Returns one empty row per video when there are no audios; nil only
    /// on a core fault or invalid params. PLUMBING ONLY — see `matchPair`.
    public func matchCandidates(
        videos: [TrackHandle], audios: [TrackHandle], paramsJSON: String? = nil
    ) -> [[PairEvidence]]? {
        guard !videos.isEmpty else { return [] }
        guard !audios.isEmpty else { return videos.map { _ in [] } }
        let vs = videos.map(\.id)
        let aus = audios.map(\.id)
        return vs.withUnsafeBufferPointer { vp in
            aus.withUnsafeBufferPointer { ap in
                try? decodeJSON(
                    cpl_match_candidates(
                        ctx, vp.baseAddress, UInt(vp.count), ap.baseAddress, UInt(ap.count),
                        paramsJSON),
                    as: [[PairEvidence]].self)
            }
        }
    }

    /// GRAPH-ADJUSTED batch evidence (`cpl_match_candidates_graph`, §13.4): same shape and wire form
    /// as `matchCandidates`, but core applies the §13 cross-device graph-consistency pass (an
    /// `Ambiguous` edge may be promoted to `Match`, or a `Match` demoted to `Ambiguous`, when
    /// independent composed affine paths close per §13.3). Boundary 13: this graph-adjusted matrix is
    /// the DEFAULT resolver's input — core's `resolve` keys on each cell's `classification` (the
    /// identity authority) and orders by `score`; acceptance logic stays in core (the Swift shell now
    /// consumes that decision via `resolve`/`resolveStart` rather than re-deriving it). In
    /// `legacyEnvelope` mode the graph pass is a no-op and this equals `matchCandidates`. nil only on a
    /// core fault or invalid params.
    public func matchCandidatesGraph(
        videos: [TrackHandle], audios: [TrackHandle], paramsJSON: String? = nil
    ) -> [[PairEvidence]]? {
        guard !videos.isEmpty else { return [] }
        guard !audios.isEmpty else { return videos.map { _ in [] } }
        let vs = videos.map(\.id)
        let aus = audios.map(\.id)
        return vs.withUnsafeBufferPointer { vp in
            aus.withUnsafeBufferPointer { ap in
                try? decodeJSON(
                    cpl_match_candidates_graph(
                        ctx, vp.baseAddress, UInt(vp.count), ap.baseAddress, UInt(ap.count),
                        paramsJSON),
                    as: [[PairEvidence]].self)
            }
        }
    }

    /// JOB form of `matchCandidatesGraph` (`cpl_match_candidates_graph_start`, incremental-matching
    /// boundary): the asynchronous twin that reports a determinate fraction while the graph-adjusted
    /// matrix is built. The job's value is the SAME `[[PairEvidence]]` matrix the synchronous call
    /// returns (resolution byte-identical; progress observational). `progress` yields phase names
    /// "MatchFingerprint" / "MatchCorrelate" / "MatchGraph" with a 0…1 fraction; cancellation throws
    /// `JobError.cancelled` and never yields a partial matrix. Mirrors `importStart`: core copies the
    /// id slices synchronously inside the `withUnsafeBufferPointer` closures, so the buffers need not
    /// outlive this call. The empty-input shapes (`videos.isEmpty ⇒ []`; `audios.isEmpty ⇒ one empty
    /// row per video`) of the synchronous form are NOT reproduced here — the caller short-circuits
    /// those before starting a job (there is no work, hence no job).
    public func matchCandidatesGraphStart(
        videos: [TrackHandle], audios: [TrackHandle], paramsJSON: String? = nil
    ) throws -> Job<[[PairEvidence]]> {
        let box = JobBox()
        let user = Unmanaged.passRetained(box).toOpaque()
        let vs = videos.map(\.id)
        let aus = audios.map(\.id)
        let id = vs.withUnsafeBufferPointer { vp in
            aus.withUnsafeBufferPointer { ap in
                cpl_match_candidates_graph_start(
                    ctx, vp.baseAddress, UInt(vp.count), ap.baseAddress, UInt(ap.count),
                    paramsJSON, jobTrampoline, user)
            }
        }
        guard id != 0 else {
            Unmanaged<JobBox>.fromOpaque(user).release()
            throw takeError()
        }
        return Job(engine: self, box: box, id: id)
    }

    // ---------- row resolution (PLAN-CORE-SHELL-BOUNDARY §3.1) ----------

    /// Core's default `ResolveParams` as JSON (`cpl_resolve_params_default_json`) — the single source
    /// of the row-resolver defaults (matcher choice, z gate/margin, triangle tol, coverage-full,
    /// cluster tol, the verify/RMS-escalation knobs), exactly as `pairingParamsDefault()` is for the
    /// pairing layer. Returns nil only on a core fault (never in normal operation).
    public func resolveParamsDefaultJSON() -> String? {
        Engine.takeString(cpl_resolve_params_default_json())
    }

    /// Full §3.3 ROW RESOLUTION over imported tracks (`cpl_resolve`, PLAN-CORE-SHELL-BOUNDARY §3.1) —
    /// the single cross-platform home for coverage clustering, the §3.4 audio↔audio simultaneity
    /// (triangle) check, the §3.5 margin rule, the §3.3 row states, candidate-overflow handling, and
    /// the legacy RMS tie-breaker. Rows are `videos` (IMPORTED-TRACK ONLY: the caller excludes
    /// audio-less videos and wraps those as inert no-match rows shell-side); the resolver picks each
    /// row's audio from `audios`. `paramsJSON` is an optional `ResolveParams` override (camelCase;
    /// nil ⇒ core defaults; partial ⇒ serde fills the rest). Decodes the `[ResolvedRow]` array; the
    /// empty-input shapes mirror the other batch wrappers (no videos ⇒ `[]`); nil only on a core
    /// fault or invalid params.
    public func resolve(videos: [TrackHandle], audios: [TrackHandle], paramsJSON: String?) -> [ResolvedRow]? {
        guard !videos.isEmpty else { return [] }
        let vs = videos.map(\.id)
        let aus = audios.map(\.id)
        return vs.withUnsafeBufferPointer { vp in
            aus.withUnsafeBufferPointer { ap in
                try? decodeJSON(
                    cpl_resolve(ctx, vp.baseAddress, UInt(vp.count), ap.baseAddress, UInt(ap.count),
                                paramsJSON),
                    as: [ResolvedRow].self)
            }
        }
    }

    /// JOB form of `resolve` (`cpl_resolve_start`, PLAN-CORE-SHELL-BOUNDARY §3.1): the asynchronous
    /// twin that reports a determinate fraction while the matrix build + row pass run. The job's value
    /// is the SAME `[ResolvedRow]` array `resolve` returns (resolution byte-identical; progress
    /// observational). `progress` yields phase names "MatchFingerprint"/"MatchCorrelate"/"MatchGraph"
    /// (the matrix build) then "Verify"/"Resolve" (the row pass / any flag-ON classifier + RMS
    /// escalation), each with a 0…1 fraction; cancellation throws `JobError.cancelled` and never
    /// yields a partial result. Mirrors `matchCandidatesGraphStart`: core copies the id slices
    /// synchronously inside the `withUnsafeBufferPointer` closures, so the buffers need not outlive
    /// this call.
    public func resolveStart(videos: [TrackHandle], audios: [TrackHandle], paramsJSON: String?) throws -> Job<[ResolvedRow]> {
        let box = JobBox()
        let user = Unmanaged.passRetained(box).toOpaque()
        let vs = videos.map(\.id)
        let aus = audios.map(\.id)
        let id = vs.withUnsafeBufferPointer { vp in
            aus.withUnsafeBufferPointer { ap in
                cpl_resolve_start(
                    ctx, vp.baseAddress, UInt(vp.count), ap.baseAddress, UInt(ap.count),
                    paramsJSON, jobTrampoline, user)
            }
        }
        guard id != 0 else {
            Unmanaged<JobBox>.fromOpaque(user).release()
            throw takeError()
        }
        return Job(engine: self, box: box, id: id)
    }

    /// §13.4 audio-audio SIMULTANEITY check over affine evidence (`cpl_triangle_closes`) — the SAME
    /// core predicate the CLI resolver uses, so neither shell reimplements the affine composition or
    /// the closure tolerances (invariant 8). Given the three legs' effective maps (V→A, A→B, V→B as
    /// `(lagMs, driftPpm)`), the A↔B core `classification`, the video coverage window, and the §13.3
    /// graph tolerances, returns whether the two recorders are simultaneous: the A↔B edge is a `Match`
    /// AND composing V→A then A→B reproduces V→B at start/mid/end within tolerance. `residualMaxMs` is
    /// the composed start/mid/end residual MAX for the calibration read-out. Defaults (closes=false)
    /// on any core fault — a fault never fabricates a simultaneity.
    public func triangleCloses(
        vaLagMs: Double, vaDriftPpm: Double,
        abLagMs: Double, abDriftPpm: Double,
        vbLagMs: Double, vbDriftPpm: Double,
        abClassification: MatchClassification,
        coverageS: Double, offsetTolMs: Double, driftTolPpm: Double
    ) -> (closes: Bool, residualMaxMs: Double) {
        let req: [String: Any] = [
            "va": ["lagMs": vaLagMs, "driftPpm": vaDriftPpm],
            "ab": ["lagMs": abLagMs, "driftPpm": abDriftPpm],
            "vb": ["lagMs": vbLagMs, "driftPpm": vbDriftPpm],
            "abClassification": abClassification.rawValue,
            "coverageS": coverageS,
            "offsetTolMs": offsetTolMs,
            "driftTolPpm": driftTolPpm,
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: req),
            let json = String(data: data, encoding: .utf8),
            let result = try? decodeJSON(cpl_triangle_closes(json), as: TriangleResult.self)
        else {
            return (false, 0.0)
        }
        return (result.closes, result.residualMaxMs)
    }
}

/// Decoded result of `cpl_triangle_closes` (§13.4): whether the triangle closes + the composed
/// start/mid/end residual MAX (ms) for calibration read-out.
struct TriangleResult: Decodable {
    var closes: Bool
    var residualMaxMs: Double
}
