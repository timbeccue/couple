// CoupleKit integration tests: exercise the FFI wrapper against the real core
// (ffmpeg generates a small WAV pair; cache goes to a temp dir).

import Foundation
import Testing
@testable import CoupleKit

private func makeTempDir() -> String {
    let dir = NSTemporaryDirectory() + "couplekit-tests-" + UUID().uuidString
    try! FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    return dir
}

/// Generate a noise-burst WAV (deterministic via ffmpeg's anoisesrc seed) and a
/// delayed copy, so auto-sync has real content to correlate.
private func generateFixture(in dir: String) throws -> (external: String, camera: String) {
    let ext = dir + "/ext.wav"
    let cam = dir + "/cam.wav"
    let run = { (args: [String]) throws in
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        p.arguments = ["ffmpeg", "-y", "-v", "error"] + args
        try p.run()
        p.waitUntilExit()
        #expect(p.terminationStatus == 0)
    }
    // 60 s: pink noise amplitude-modulated by ~1.5 Hz-lowpassed noise — an
    // APERIODIC envelope (a periodic gate aliases the envelope NCC, SPEC §9.1).
    try run(["-f", "lavfi", "-i", "anoisesrc=d=60:c=pink:r=48000:a=0.9:seed=7",
             "-f", "lavfi", "-i", "anoisesrc=d=60:c=brown:r=48000:a=1.0:seed=42,lowpass=f=1.5,lowpass=f=1.5,volume=10",
             "-filter_complex", "amultiply,alimiter=limit=0.9:level=false",
             "-ac", "1", ext])
    // Camera = same content starting 2.5 s into the external (u(0)=2.5 ⇒ offset +2500 ms), 40 s long.
    try run(["-ss", "2.5", "-t", "40", "-i", ext, "-c:a", "pcm_s16le", cam])
    return (ext, cam)
}

@Suite(.serialized)
struct CoupleKitTests {
    @Test func abiAndVersion() {
        #expect(Engine.abiVersion == 1)
        #expect(!Engine.coreVersion.isEmpty)
    }

    @Test func fitLS() throws {
        let engine = try Engine(cacheDir: makeTempDir())
        let fit = engine.fitLS([(0, 10), (100, 11), (200, 12)])
        let f = try #require(fit)
        #expect(abs(f.offMs - 10) < 1e-9)
        #expect(abs(f.ppm - 10) < 1e-9) // 1 ms over 100 s = 10 ppm
        #expect(engine.fitLS([(0, 1)]) == nil)
    }

    // §13.4 (boundary 13): the Swift `triangleCloses` binding consumes the SHARED core
    // `match_graph::triangle_closes` predicate (invariant 8 — the resolver does NOT reimplement the
    // affine composition / closure tolerances). Three consistent recorder offsets (V=0, A=0.3, B=1.0 s)
    // close ONLY when the A↔B edge is `.match`; a non-Match A↔B or a large composed residual does not.
    @Test func triangleClosesSharesCorePredicate() throws {
        let engine = try Engine(cacheDir: makeTempDir())
        // V→A lag = (0 - 0.3) s = -300 ms; V→B = -1000 ms; A→B = (0.3 - 1.0) s = -700 ms. Consistent.
        let consistent = engine.triangleCloses(
            vaLagMs: -300, vaDriftPpm: 0, abLagMs: -700, abDriftPpm: 0, vbLagMs: -1000, vbDriftPpm: 0,
            abClassification: .match, coverageS: 600, offsetTolMs: 100, driftTolPpm: 150)
        #expect(consistent.closes, "consistent geometry + A↔B match ⇒ simultaneous")
        #expect(consistent.residualMaxMs < 1.0, "composed residual ~0 for a consistent triangle")

        // The A↔B identity authority is core's classification: a NoMatch A↔B never closes.
        let notMatch = engine.triangleCloses(
            vaLagMs: -300, vaDriftPpm: 0, abLagMs: -700, abDriftPpm: 0, vbLagMs: -1000, vbDriftPpm: 0,
            abClassification: .noMatch, coverageS: 600, offsetTolMs: 100, driftTolPpm: 150)
        #expect(!notMatch.closes, "A↔B noMatch ⇒ NOT simultaneous (identity is core's classification)")

        // A 300 ms composed-residual triangle (A→B off by 300 ms) exceeds the 100 ms offset tol.
        let offset = engine.triangleCloses(
            vaLagMs: -300, vaDriftPpm: 0, abLagMs: -400, abDriftPpm: 0, vbLagMs: -1000, vbDriftPpm: 0,
            abClassification: .match, coverageS: 600, offsetTolMs: 100, driftTolPpm: 150)
        #expect(!offset.closes, "300 ms composed residual does NOT close (> 100 ms tol)")
        #expect(offset.residualMaxMs > 100.0, "the residual read-out surfaces the disagreement")
    }

    // §6.8.5 — Swift decodes core's default PairingParams JSON (the single source of
    // pairing/verifier defaults; the resolver reads these instead of hardcoding copies).
    @Test func pairingParamsDefaultDecodes() throws {
        let engine = try Engine(cacheDir: makeTempDir())
        let p = engine.pairingParamsDefault()
        #expect(p.minOverlapS == 8.0)
        #expect(p.refineNccMin == 0.15)
        #expect(p.legacyZMin == 8.5)
        #expect(p.verifyRmsMaxMs == 12.0)
        #expect(p.verifyLockFractionMin == 0.66)
        #expect(p.verifyMinLocked == 4)
        #expect(p.driftMaxPpm == 600.0)
        #expect(p.stageSpansMs == [500.0, 80.0, 14.0, 3.0])
    }

    // §6.8.5 — VerifyFit decodes the core-computed `matched` decision (the single
    // classification authority Swift surfaces via Resolver.isMatch).
    @Test func verifyFitDecodesMatched() throws {
        let json = """
        {"offMs":1500.0,"ppm":2.0,"rmsMs":3.5,"clamped":false,"locked":8,"expected":9,
         "lockFraction":0.89,"overlapS":20.0,"inliers":8,"residualP95Ms":4.1,
         "temporalCoverage":0.8,"matched":true}
        """
        let fit = try JSONDecoder().decode(VerifyFit.self, from: Data(json.utf8))
        #expect(fit.matched)
        #expect(fit.inliers == 8)
        #expect(fit.lockFraction == 0.89)
        #expect(fit.residualP95Ms == 4.1)
    }

    // §7.2 — PairCorrelation decodes the core-computed candidacy booleans the resolver
    // consumes (legacyCandidate = z>=legacyZMin, prefilterCandidate = ncc>=refineNccMin),
    // so the flag-ON pre-filter reads the core boolean and never re-derives the threshold.
    @Test func pairCorrelationDecodesCandidacyBooleans() throws {
        // A short genuine overlap: z (capped by √n) below legacyZMin but ncc clears the
        // prefilter — exactly the 8–20 s cell Phase 2 makes reachable for the verifier.
        let json = """
        {"z":6.3,"lagMs":1200.0,"ncc":0.97,"fineNcc":0.99,"overlapBins":40,"runnerUpZ":0.4,
         "refined":true,"insufficientOverlap":false,"legacyCandidate":false,"prefilterCandidate":true}
        """
        let m = try JSONDecoder().decode(PairCorrelation.self, from: Data(json.utf8))
        #expect(!m.legacyCandidate)
        #expect(m.prefilterCandidate)
        #expect(!m.insufficientOverlap)
        #expect(m.refined)
    }

    // §11.2 — cross-stack parity over the committed candidate-matrix fixture
    // (fixtures/matcher-corpus/parity/candidate-matrix.json), the SAME file the Rust CLI parity
    // test (cli/tests/parity_matrix.rs) decodes. CoupleKit decodes EVERY PairCorrelation field,
    // and the flag-ON / flag-OFF filtering reads the CORE-COMPUTED booleans (prefilterCandidate /
    // legacyCandidate) exactly as Resolver.swift does — never a local numeric gate. This runs
    // under `swift test`, giving the parity contract real executable coverage on the Swift side.
    private struct ParityCell: Decodable {
        var audio: String
        var corr: PairCorrelation
        // The cell flattens PairCorrelation's fields alongside video/audio/comment; decode the
        // correlation from the same container (extra keys like video/comment are ignored).
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: AudioKey.self)
            audio = try c.decode(String.self, forKey: .audio)
            corr = try PairCorrelation(from: decoder)
        }
        enum AudioKey: String, CodingKey { case audio }
    }
    private struct ParityFixture: Decodable { var schema: String; var cells: [ParityCell] }

    private func loadParityFixture() throws -> ParityFixture {
        // Walk up from THIS test source file to the repo root, then into fixtures/.
        // mac/Tests/CoupleKitTests/CoupleKitTests.swift ⇒ repo root is four parents up.
        let here = URL(fileURLWithPath: #filePath)
        let repoRoot = here.deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let url = repoRoot.appendingPathComponent("fixtures/matcher-corpus/parity/candidate-matrix.json")
        let data = try Data(contentsOf: url)
        return try JSONDecoder().decode(ParityFixture.self, from: data)
    }

    /// The flag-ON candidacy predicate, IDENTICAL to Resolver.swift (reads prefilterCandidate).
    private func candidateFlagOn(_ m: PairCorrelation) -> Bool { m.prefilterCandidate }
    /// The flag-OFF candidacy predicate, IDENTICAL to Resolver.swift (default z-gate vs zMin).
    private func candidateFlagOff(_ m: PairCorrelation, zMin: Double) -> Bool {
        !m.insufficientOverlap && m.z >= zMin
    }

    @Test func parityCandidateMatrixDecodesEveryField() throws {
        let fx = try loadParityFixture()
        #expect(fx.schema == "couple-pair-matrix/1")
        #expect(fx.cells.count == 4)
        func by(_ name: String) -> PairCorrelation { fx.cells.first { $0.audio == name }!.corr }

        // match.wav: clean match ⇒ both booleans true; fineNcc present.
        let m = by("match.wav")
        #expect(m.legacyCandidate && m.prefilterCandidate)
        #expect(m.refined && !m.insufficientOverlap)
        #expect(m.fineNcc != nil)
        #expect(m.overlapBins == 200)

        // unrelated.wav + short_genuine.wav: the booleans DIVERGE (legacy false, prefilter true).
        for name in ["unrelated.wav", "short_genuine.wav"] {
            let c = by(name)
            #expect(!c.legacyCandidate, "\(name): legacyCandidate false")
            #expect(c.prefilterCandidate, "\(name): prefilterCandidate true")
        }

        // tiny_clip.wav: below the floor ⇒ both false, fineNcc null.
        let t = by("tiny_clip.wav")
        #expect(t.insufficientOverlap && !t.legacyCandidate && !t.prefilterCandidate)
        #expect(t.fineNcc == nil)
    }

    @Test func parityFlagOnOffUseCoreBooleans() throws {
        let fx = try loadParityFixture()
        let zMin = 8.5
        let on = fx.cells.filter { candidateFlagOn($0.corr) }.map(\.audio)
        let off = fx.cells.filter { candidateFlagOff($0.corr, zMin: zMin) }.map(\.audio)
        // Flag ON admits every prefilter candidate; flag OFF only the legacy z-passer.
        #expect(on == ["match.wav", "unrelated.wav", "short_genuine.wav"])
        #expect(off == ["match.wav"])
        #expect(on != off, "flag-ON and flag-OFF select different sets ⇒ the booleans do real work")
        // Flag-ON membership equals each cell's own core prefilterCandidate (consumed, not re-derived).
        for c in fx.cells {
            #expect(candidateFlagOn(c.corr) == c.corr.prefilterCandidate)
        }
    }

    // §11.2 — the match classification authority is VerifyFit.matched (core-computed), decoded
    // straight off the wire. Resolver.isMatch returns exactly this (no local numeric gate), so a
    // matched and an unmatched VerifyFit decode to their `matched` value verbatim.
    @Test func parityVerifyFitMatchedIsTheAuthority() throws {
        let matchedJSON = """
        {"offMs":1500.0,"ppm":2.0,"rmsMs":3.5,"clamped":false,"locked":8,"expected":9,
         "lockFraction":0.89,"overlapS":20.0,"inliers":8,"residualP95Ms":4.1,
         "temporalCoverage":0.8,"matched":true}
        """
        let unmatchedJSON = """
        {"offMs":1500.0,"ppm":2.0,"rmsMs":30.0,"clamped":false,"locked":2,"expected":9,
         "lockFraction":0.22,"overlapS":20.0,"inliers":2,"residualP95Ms":28.0,
         "temporalCoverage":0.1,"matched":false}
        """
        let good = try JSONDecoder().decode(VerifyFit.self, from: Data(matchedJSON.utf8))
        let bad = try JSONDecoder().decode(VerifyFit.self, from: Data(unmatchedJSON.utf8))
        #expect(good.matched)
        #expect(!bad.matched)
    }

    // §14.2 — CoupleKit decodes the method-neutral `PairEvidence` (plumbing for the boundary-9
    // resolver migration). Two cases mirror the core wire JSON exactly:
    //   (1) a SpectralLandmarkV1 shadow `match` with EVERY field populated incl. a nested `legacy`
    //       PairCorrelation, and present refined_* / residual_* numbers;
    //   (2) a `noMatch` whose non-applicable residual_* / refined_* and `legacy` are core `null` ⇒
    //       decode to Swift `nil` (the Option→nil contract). Plus a bare enum-string decode.
    @Test func pairEvidenceDecodesEveryFieldAndOptionalsToNil() throws {
        // (1) Fully-populated SpectralLandmarkV1 shadow Match with an attached legacy correlation.
        let matchJSON = """
        {"method":"spectralLandmarkV1","classification":"match","lagMs":1800.0,"driftPpm":60.0,
         "score":5.0,"runnerUpScore":0.0,"scoreMargin":1.0,"support":50,
         "candidateCorrespondences":60,"inlierFraction":0.9,"temporalCoverage":0.8,
         "residualRmsMs":0.4,"residualP95Ms":0.9,"overlapS":40.0,
         "refinedLagMs":1801.0,"refinedDriftPpm":60.5,"refinedRmsMs":0.3,
         "legacy":{"z":11.9,"lagMs":1799.0,"ncc":0.97,"fineNcc":1.0,"overlapBins":150,
                   "runnerUpZ":10.1,"refined":true,"insufficientOverlap":false,
                   "legacyCandidate":true,"prefilterCandidate":true}}
        """
        let ev = try JSONDecoder().decode(PairEvidence.self, from: Data(matchJSON.utf8))
        #expect(ev.method == .spectralLandmarkV1)
        #expect(ev.classification == .match)
        #expect(ev.lagMs == 1800.0)
        #expect(ev.driftPpm == 60.0)
        #expect(ev.score == 5.0)
        #expect(ev.scoreMargin == 1.0)
        #expect(ev.support == 50)
        #expect(ev.candidateCorrespondences == 60)
        #expect(ev.inlierFraction == 0.9)
        #expect(ev.temporalCoverage == 0.8)
        #expect(ev.residualRmsMs == 0.4)
        #expect(ev.residualP95Ms == 0.9)
        #expect(ev.overlapS == 40.0)
        #expect(ev.refinedLagMs == 1801.0)
        #expect(ev.refinedDriftPpm == 60.5)
        #expect(ev.refinedRmsMs == 0.3)
        // The nested legacy PairCorrelation decodes (shadow telemetry).
        let legacy = try #require(ev.legacy)
        #expect(legacy.legacyCandidate && legacy.prefilterCandidate)
        #expect(abs(legacy.lagMs - 1799.0) < 1e-9)

        // (2) A noMatch with core `null` residual_* / refined_* / legacy ⇒ Swift nil.
        let noMatchJSON = """
        {"method":"envelopeV1","classification":"noMatch","lagMs":0.0,"driftPpm":0.0,
         "score":0.0,"runnerUpScore":0.0,"scoreMargin":0.0,"support":0,
         "candidateCorrespondences":0,"inlierFraction":0.0,"temporalCoverage":0.0,
         "residualRmsMs":null,"residualP95Ms":null,"overlapS":30.0,
         "refinedLagMs":null,"refinedDriftPpm":null,"refinedRmsMs":null,"legacy":null}
        """
        let nm = try JSONDecoder().decode(PairEvidence.self, from: Data(noMatchJSON.utf8))
        #expect(nm.method == .envelopeV1)
        #expect(nm.classification == .noMatch)
        #expect(nm.residualRmsMs == nil)
        #expect(nm.residualP95Ms == nil)
        #expect(nm.refinedLagMs == nil)
        #expect(nm.refinedDriftPpm == nil)
        #expect(nm.refinedRmsMs == nil)
        #expect(nm.legacy == nil)

        // Bare enum-string decode for every variant (the camelCase wire strings).
        func dm(_ s: String) throws -> MatchMethod { try JSONDecoder().decode(MatchMethod.self, from: Data("\"\(s)\"".utf8)) }
        func dc(_ s: String) throws -> MatchClassification { try JSONDecoder().decode(MatchClassification.self, from: Data("\"\(s)\"".utf8)) }
        #expect(try dm("spectralLandmarkV1") == .spectralLandmarkV1)
        #expect(try dm("envelopeV1") == .envelopeV1)
        #expect(try dc("match") == .match)
        #expect(try dc("noMatch") == .noMatch)
        #expect(try dc("ambiguous") == .ambiguous)
        #expect(try dc("insufficientOverlap") == .insufficientOverlap)
        #expect(try dc("unavailable") == .unavailable)
    }

    // §14.2/§14.4 — the boundary-9 PURE shadow-telemetry projection (`ShadowTelemetry.rows`), the
    // decision logic the resolver attaches to its diagnostic disclosure. This is the function's REAL
    // automated coverage (it lives in CoupleKit precisely so `swift test` reaches it). A small evidence
    // builder fabricates `PairEvidence` cells so no engine/media is needed.
    private func ev(_ cls: MatchClassification, score: Double, lag: Double = 0, drift: Double = 0,
                    refinedLag: Double? = nil, refinedDrift: Double? = nil,
                    support: UInt32 = 0, p95: Double? = nil, method: MatchMethod = .spectralLandmarkV1)
        -> PairEvidence {
        PairEvidence(method: method, classification: cls, lagMs: lag, driftPpm: drift,
                     score: score, runnerUpScore: 0, scoreMargin: 0, support: support,
                     candidateCorrespondences: 0, inlierFraction: 0, temporalCoverage: 0,
                     residualRmsMs: nil, residualP95Ms: p95, overlapS: 0,
                     refinedLagMs: refinedLag, refinedDriftPpm: refinedDrift, refinedRmsMs: nil,
                     legacy: nil)
    }

    // (1) Orders by `score` DESCENDING and surfaces classification/score/refined mapping verbatim.
    @Test func shadowTelemetryOrdersByScoreAndProjectsFields() throws {
        // Three cells, deliberately OUT of score order in the input; the projector must sort desc.
        let evidence = [
            ev(.noMatch, score: 1.0, lag: 100, support: 5),                       // audio "b.wav"
            ev(.match, score: 9.0, lag: 1800, refinedLag: 1801, refinedDrift: 60, support: 50, p95: 0.9), // "a.wav"
            ev(.ambiguous, score: 4.0, lag: 500, support: 20),                    // "c.wav"
        ]
        let keys = ["kb", "ka", "kc"]
        let names = ["b.wav", "a.wav", "c.wav"]
        let rows = ShadowTelemetry.rows(evidence: evidence, audioKeys: keys, audioNames: names)
        #expect(rows.count == 3)
        // Score order: 9.0 (a) > 4.0 (c) > 1.0 (b).
        #expect(rows.map(\.audioName) == ["a.wav", "c.wav", "b.wav"])
        #expect(rows.map(\.score) == [9.0, 4.0, 1.0])
        // Classification is surfaced verbatim (core's verdict), not re-decided.
        #expect(rows.map(\.classification) == [.match, .ambiguous, .noMatch])
        // The refined mapping is projected verbatim (boundary-7 fields pass through).
        #expect(rows[0].refinedLagMs == 1801)
        #expect(rows[0].refinedDriftPpm == 60)
        #expect(rows[0].residualP95Ms == 0.9)
        #expect(rows[0].support == 50)
        // The consensus lag/drift are also carried (for cells without refinement).
        #expect(rows[2].lagMs == 100)        // b.wav
        #expect(rows[1].lagMs == 500)        // c.wav
        // Identity carried through (key + name), index-aligned to the input.
        #expect(rows[0].audioKey == "ka")
    }

    // (2) NO acceptance thresholding — a `match` and a `noMatch` are BOTH surfaced, verbatim. The
    // projector never re-decides: it has no threshold, no `matched` recomputation; acceptance is
    // core's `classification`. (The §14.2 invariant: the app must not duplicate the acceptance formula.)
    @Test func shadowTelemetryDoesNoAcceptanceThresholding() throws {
        // A NoMatch with a HIGH score and strong support sits next to a Match with a LOW score and
        // weak support. A re-decider would flip these; the projector must surface both as-is and order
        // PURELY by score (so the high-score no-match even sorts FIRST — proof score is display-only).
        let evidence = [
            ev(.match, score: 0.5, support: 3),     // weak Match
            ev(.noMatch, score: 8.0, support: 99),  // strong-looking NoMatch
        ]
        let rows = ShadowTelemetry.rows(evidence: evidence, audioKeys: ["k0", "k1"],
                                        audioNames: ["m.wav", "n.wav"])
        // Both classifications survive untouched — nothing was promoted or demoted.
        #expect(Set(rows.map(\.classification)) == [.match, .noMatch])
        // The NoMatch (score 8.0) sorts ahead of the Match (score 0.5): ordering is score-only and
        // is NOT an acceptance decision — the function surfaces, it does not gate.
        #expect(rows[0].classification == .noMatch)
        #expect(rows[0].score == 8.0)
        #expect(rows[1].classification == .match)
        // Every classification variant passes through unchanged (no variant is filtered/coalesced).
        let all: [MatchClassification] = [.match, .noMatch, .ambiguous, .insufficientOverlap, .unavailable]
        let proj = ShadowTelemetry.rows(
            evidence: all.enumerated().map { ev($0.element, score: Double(all.count - $0.offset)) },
            audioKeys: all.indices.map { "k\($0)" }, audioNames: all.indices.map { "f\($0).wav" })
        #expect(proj.map(\.classification) == all)  // score desc preserves the input order here
    }

    // (3) The projection is a PASSIVE mirror of core fields — every displayed value equals the source
    // `PairEvidence` field with no transform (beyond ordering). Asserts there is no duplicated numeric
    // gate: the row carries core's numbers, and `classification` is identity, not a recomputation.
    @Test func shadowTelemetryIsPassiveProjectionOfCoreFields() throws {
        let source = ev(.match, score: 5.0, lag: 1800, drift: 60, refinedLag: 1801, refinedDrift: 60.5,
                        support: 50, p95: 0.9)
        let rows = ShadowTelemetry.rows(evidence: [source], audioKeys: ["k"], audioNames: ["a.wav"])
        let r = try #require(rows.first)
        // Each field is the core field verbatim — the projector copies, it does not derive.
        #expect(r.method == source.method)
        #expect(r.classification == source.classification)
        #expect(r.lagMs == source.lagMs)
        #expect(r.driftPpm == source.driftPpm)
        #expect(r.score == source.score)
        #expect(r.scoreMargin == source.scoreMargin)
        #expect(r.support == source.support)
        #expect(r.inlierFraction == source.inlierFraction)
        #expect(r.temporalCoverage == source.temporalCoverage)
        #expect(r.residualP95Ms == source.residualP95Ms)
        #expect(r.refinedLagMs == source.refinedLagMs)
        #expect(r.refinedDriftPpm == source.refinedDriftPpm)
    }

    // Determinism — equal/zero scores never leave the order ambiguous, because the final tiebreak is
    // the unique `audioKey`. Reversing the input order yields the SAME projected order (a total order).
    @Test func shadowTelemetryDeterministicTotalOrderOnTiedScores() throws {
        // All scores equal (incl. 0.0): order must fall back to name asc, then key asc — stable.
        let mk: (String, String) -> PairEvidence = { _, _ in self.ev(.ambiguous, score: 0.0) }
        let keys = ["k3", "k1", "k2"]
        let names = ["c.wav", "a.wav", "a.wav"]  // two share a name ⇒ key breaks the tie
        let forward = ShadowTelemetry.rows(
            evidence: zip(keys, names).map(mk), audioKeys: keys, audioNames: names)
        // name asc: a.wav,a.wav,c.wav; the two a.wav break by key asc: k1 < k2.
        #expect(forward.map { "\($0.audioName)/\($0.audioKey)" } == ["a.wav/k1", "a.wav/k2", "c.wav/k3"])
        // Reversing the inputs produces the identical order (total + deterministic).
        let rKeys = Array(keys.reversed()), rNames = Array(names.reversed())
        let reverse = ShadowTelemetry.rows(
            evidence: zip(rKeys, rNames).map(mk), audioKeys: rKeys, audioNames: rNames)
        #expect(reverse.map(\.audioKey) == forward.map(\.audioKey))
    }

    // Defensive length handling — mismatched evidence/keys/names truncate to the shortest, never crash.
    @Test func shadowTelemetryTruncatesMismatchedLengths() throws {
        let rows = ShadowTelemetry.rows(
            evidence: [ev(.match, score: 1), ev(.noMatch, score: 2)],
            audioKeys: ["k0"], audioNames: ["only.wav"])
        #expect(rows.count == 1)
        #expect(rows[0].audioKey == "k0")
    }

    @Test func importSessionSyncRoundTrip() async throws {
        let dir = makeTempDir()
        let engine = try Engine(cacheDir: dir + "/cache")
        let (extPath, camPath) = try generateFixture(in: dir)

        // Import both with progress.
        let camJob = try engine.importStart(camPath, role: .camera)
        var sawProgress = false
        for await p in camJob.progress {
            #expect(p.fraction >= 0 && p.fraction <= 1)
            sawProgress = true
        }
        let camMeta = try await camJob.value()
        #expect(sawProgress)
        #expect(abs(camMeta.durationS - 40) < 0.1)
        let extMeta = try await engine.importStart(extPath, role: .external).value()
        #expect(abs(extMeta.durationS - 60) < 0.1)

        let cam = engine.track(camMeta)
        let ext = engine.track(extMeta)

        // band() into a reusable buffer.
        let buf = BandBuffer()
        #expect(cam.band(t0: 0, t1: 40, nCols: 600, into: buf))
        #expect(buf.count == 600)
        #expect(buf.levelSpb > 0)
        let peak = (0..<buf.count).map { max(abs(Int(buf.mins[$0])), abs(Int(buf.maxs[$0]))) }.max() ?? 0
        #expect(peak > 50) // content present

        // raw_window.
        var raw = [Float](repeating: 0, count: 48_000)
        let frames = cam.rawWindow(t0: 1.0, t1: 1.5, into: &raw)
        #expect(frames == 24_000)

        // Onsets exist on gated noise.
        #expect(cam.bestOnset(in: 1, 39) != nil)

        // Session: state round trip via cold-path JSON.
        let session = try engine.sessionNew(camera: cam, external: ext)
        session.setOffsetMs(-123.45)
        var state = try session.state()
        #expect(abs(state.offsetMs - -123.45) < 1e-9)
        try session.updateState { s in
            s.anchors = [5.0, 20.0, 35.0]
            s.frames = [Frame(winMs: 240, auto: true), Frame(winMs: 240, auto: true), Frame(winMs: 240, auto: true)]
            s.perAnchorMeasurements = [LocalMeasurement(lagMs: 2500, ncc: 0.9), nil, nil]
        }
        state = try session.state()
        #expect(state.anchors == [5.0, 20.0, 35.0])
        #expect(state.perAnchorMeasurements.count == 3)
        #expect(state.perAnchorMeasurements[1] == nil)

        // None-mode semantics are core-enforced: drift write flips back to resample.
        session.setDriftPpmLive(4.0)
        session.commitDrift()
        session.setDriftMode(.none)
        state = try session.state()
        #expect(state.driftMode == .none)
        #expect(state.driftPpm == 0)
        #expect(abs(state.savedDriftPpm - 4.0) < 1e-9)
        session.setDriftPpmLive(2.0)
        state = try session.state()
        #expect(state.driftMode == .resample)

        // Auto-sync finds the −2500 ms construction offset.
        session.setOffsetMs(0)
        let sync = try await session.autoSyncStart().value()
        #expect(abs(sync.offsetMs - 2500.0) < 2.0)
        #expect(sync.status == .synced || sync.status == .lowConfidence)

        // measure_local agrees near an anchor.
        let m = session.measureLocal(tc: 20, centerMs: sync.offsetMs, spanMs: 80)
        let meas = try #require(m)
        #expect(abs(meas.lagMs - 2500.0) < 2.0)

        // Audition (Phase 6): construction offset is exactly +2500 ms, so in
        // None mode (integer shift) identical content nulls to exact zero.
        session.setDriftMode(.none)
        session.setOffsetMs(2500)
        let audition = try session.auditionOpen(mode: .null, tStart: 10)
        var stereo = [Float](repeating: 1, count: 2048)
        let r = stereo.withUnsafeMutableBufferPointer {
            audition.fill(into: $0.baseAddress!, capacityFrames: 1024)
        }
        #expect(r.frames == 1024)
        #expect(abs(r.streamT - (10 + 1024.0 / 48000)) < 1e-9)
        #expect(stereo.allSatisfy { $0 == 0 })
        // User gains: muting both tracks renders exact silence in overlay too.
        audition.setMode(.overlay)
        audition.setGains(cam: 0, ext: 0)
        _ = stereo.withUnsafeMutableBufferPointer {
            audition.fill(into: $0.baseAddress!, capacityFrames: 1024) // absorb crossfade
        }
        let rg = stereo.withUnsafeMutableBufferPointer {
            audition.fill(into: $0.baseAddress!, capacityFrames: 1024)
        }
        #expect(rg.frames == 1024)
        #expect(stereo.allSatisfy { $0 == 0 })
        audition.setGains(cam: 1, ext: 1)
        // Past the video end: short fill, then zero frames.
        audition.seek(to: 39.99)
        let r2 = stereo.withUnsafeMutableBufferPointer {
            audition.fill(into: $0.baseAddress!, capacityFrames: 1024)
        }
        #expect(r2.frames < 1024)
        audition.close()

        session.free()
        cam.free()
        ext.free()
    }
}

// MARK: - Media metadata projection (TODO-003)

/// Pure, media-free coverage of the `MediaMetaCard` projection + its formatters.
/// No engine, no I/O — these assert the display contract directly (clean omission
/// of missing fields, no dangling separators, deterministic formatting).
@Suite struct MediaMetaTests {
    private func audioStream(
        codec: String = "pcm_s16le", sampleRate: UInt32 = 48000,
        channels: UInt32 = 2, bits: UInt32? = 16
    ) -> AudioStreamInfo {
        AudioStreamInfo(codec: codec, sampleRate: sampleRate, channels: channels,
                        bitsPerSample: bits, startTimeS: 0, durationS: nil)
    }

    private func videoStream(
        codec: String = "h264", width: UInt32 = 1920, height: UInt32 = 1080,
        nominalFps: Double = 30, avgFps: Double = 29.97
    ) -> VideoStreamInfo {
        VideoStreamInfo(codec: codec, width: width, height: height,
                        nominalFps: nominalFps, avgFps: avgFps, startTimeS: 0, bitRate: nil)
    }

    private func info(
        path: String = "/Users/me/Audio/take1.wav", container: String = "wav",
        durationS: Double = 95, video: VideoStreamInfo? = nil,
        audio: [AudioStreamInfo] = [], creation: String? = nil
    ) -> MediaInfo {
        MediaInfo(path: path, container: container, durationS: durationS, video: video,
                  audio: audio, creationTimeUtc: creation, bwf: nil, tags: [:])
    }

    private func value(_ card: MediaMetaCard, _ label: String) -> String? {
        for group in [card.common, card.stream, card.embeddedAudio] {
            if let r = group.first(where: { $0.label == label }) { return r.value }
        }
        return nil
    }

    // ---- full audio file: every common + stream field present ----
    @Test func audioFullProjection() {
        let card = MediaMetaCard.make(
            role: .audio, durationS: 95,
            info: info(audio: [audioStream()], creation: "2026-01-15T09:30:00Z"),
            fileSizeBytes: 18_350_080,
            creationTimeUtc: "2026-01-15T09:30:00Z")
        #expect(value(card, "Role") == "Audio")
        #expect(value(card, "Duration") == "1:35")
        // Created renders the UTC instant in the host's local timezone, so assert
        // the stable, timezone-independent parts rather than a fixed hour.
        let created = value(card, "Created")
        #expect(created != nil)
        #expect(created?.contains("2026") == true)
        #expect(created?.contains(" at ") == true)
        #expect(value(card, "Size") != nil)          // ByteCountFormatter output
        #expect(value(card, "Path") == "/Users/me/Audio/take1.wav")
        #expect(value(card, "Format") == "pcm_s16le (wav)")
        #expect(value(card, "Sample rate") == "48 kHz")
        #expect(value(card, "Channels") == "stereo")
        #expect(value(card, "Bit depth") == "16-bit")
        #expect(card.embeddedAudio.isEmpty)          // audio file: no embedded block
    }

    // ---- missing optionals omit cleanly (no blank rows / dangling separators) ----
    @Test func audioMissingOptionalsAreOmitted() {
        // No creation time, unknown bit depth, no file size.
        let card = MediaMetaCard.make(
            role: .audio, durationS: 95,
            info: info(audio: [audioStream(bits: nil)]),
            fileSizeBytes: nil,
            creationTimeUtc: nil)
        #expect(value(card, "Created") == nil)
        #expect(value(card, "Bit depth") == nil)
        #expect(value(card, "Size") == nil)
        // Present fields still appear; no row carries an empty/placeholder value.
        #expect(value(card, "Sample rate") == "48 kHz")
        let allRows = card.common + card.stream + card.embeddedAudio
        #expect(allRows.allSatisfy { !$0.value.isEmpty })
        #expect(allRows.allSatisfy { !$0.value.hasSuffix("·") && !$0.value.hasSuffix(" · ") })
    }

    // ---- unparseable creation timestamp is dropped, not fabricated ----
    @Test func unparseableCreationOmitted() {
        let card = MediaMetaCard.make(
            role: .audio, durationS: 10, info: info(audio: [audioStream()]),
            fileSizeBytes: 1000, creationTimeUtc: "not-a-date")
        #expect(value(card, "Created") == nil)
    }

    // ---- video: dimensions, fps (avg preferred), embedded audio summary ----
    @Test func videoFullProjection() {
        let card = MediaMetaCard.make(
            role: .video, durationS: 3725,
            info: info(path: "/v/clip.mov", container: "mov", durationS: 3725,
                       video: videoStream(), audio: [audioStream(codec: "aac", bits: nil)]),
            fileSizeBytes: 4_000_000_000,
            creationTimeUtc: nil)
        #expect(value(card, "Role") == "Video")
        #expect(value(card, "Duration") == "1:02:05")          // h:mm:ss past an hour
        #expect(value(card, "Format") == "h264 (mov)")
        #expect(value(card, "Dimensions") == "1920×1080")
        #expect(value(card, "Frame rate") == "29.97 fps")      // avg preferred over nominal
        // Embedded audio is one compact line; aac has no bit depth ⇒ no trailing sep.
        #expect(card.embeddedAudio.count == 1)
        #expect(value(card, "Audio") == "aac · 48 kHz · stereo")
    }

    // ---- avg fps invalid ⇒ falls back to nominal; integer rates have no decimal ----
    @Test func fpsFallbackAndIntegerFormatting() {
        #expect(MediaMetaCard.formatFps(avg: 0, nominal: 30) == "30 fps")
        #expect(MediaMetaCard.formatFps(avg: 23.976, nominal: 24) == "23.976 fps")
        #expect(MediaMetaCard.formatFps(avg: .nan, nominal: 25) == "25 fps")
        #expect(MediaMetaCard.formatFps(avg: 0, nominal: 0) == nil)
    }

    // ---- multiple embedded audio streams ⇒ numbered, each its own compact line ----
    @Test func videoMultipleEmbeddedAudio() {
        let card = MediaMetaCard.make(
            role: .video, durationS: 60,
            info: info(path: "/v/multi.mkv", container: "matroska", durationS: 60,
                       video: videoStream(),
                       audio: [audioStream(codec: "aac", channels: 2, bits: nil),
                               audioStream(codec: "ac3", channels: 6, bits: nil)]),
            fileSizeBytes: nil, creationTimeUtc: nil)
        #expect(card.embeddedAudio.count == 2)
        #expect(value(card, "Audio 1") == "aac · 48 kHz · stereo")
        #expect(value(card, "Audio 2") == "ac3 · 48 kHz · 6 ch")
    }

    // ---- container/codec joining never produces dangling parentheses ----
    @Test func containerCodecJoining() {
        #expect(MediaMetaCard.formatContainerCodec(container: "wav", codec: "pcm_s16le") == "pcm_s16le (wav)")
        #expect(MediaMetaCard.formatContainerCodec(container: nil, codec: "aac") == "aac")
        #expect(MediaMetaCard.formatContainerCodec(container: "mov", codec: nil) == "mov")
        #expect(MediaMetaCard.formatContainerCodec(container: "  ", codec: "") == nil)
    }

    // ---- sample-rate / channel formatting ----
    @Test func sampleRateAndChannels() {
        #expect(MediaMetaCard.formatSampleRate(48000) == "48 kHz")
        #expect(MediaMetaCard.formatSampleRate(44100) == "44.1 kHz")
        #expect(MediaMetaCard.formatSampleRate(96000) == "96 kHz")
        #expect(MediaMetaCard.formatChannels(1) == "mono")
        #expect(MediaMetaCard.formatChannels(2) == "stereo")
        #expect(MediaMetaCard.formatChannels(6) == "6 ch")
    }

    // ---- a probe-less entry still yields the filesystem/duration common rows ----
    @Test func noProbeInfoStillProjectsCommon() {
        let card = MediaMetaCard.make(
            role: .audio, durationS: 42, info: nil,
            fileSizeBytes: 2048, creationTimeUtc: "2026-01-15T09:30:00Z")
        #expect(value(card, "Role") == "Audio")
        #expect(value(card, "Duration") == "0:42")
        #expect(value(card, "Created") != nil)
        #expect(value(card, "Size") != nil)
        #expect(value(card, "Path") == nil)          // no info ⇒ no path row, cleanly
        #expect(card.stream.isEmpty)
        #expect(card.embeddedAudio.isEmpty)
    }
}

// MARK: - Re-resolution merge policy (TODO-007)

/// Deterministic, media-free coverage of `RowMergePolicy.plan` — the pure core of
/// `AppModel.mergeResolution`. These lock the TODO-007 acceptance set: an
/// unapproved/unpinned unresolved row adopts a newly decisive result wholesale
/// (selection + candidates + session/sync); a weak candidate is surfaced without
/// promotion; approved/pinned rows never have their selection changed silently;
/// and repeated re-resolves never duplicate candidates or sync work. AppModel only
/// applies the plan, so testing the plan tests the behavior at the smallest layer
/// (the executable target's DebugDrive `COUPLE_REMERGE_TEST` mirrors these live).
@Suite struct RowMergePolicyTests {
    // (1) Video first, then the decisive correct audio: an unresolved (no-match /
    //     ambiguous, nil selection) UNPINNED/UNAPPROVED row adopts the confident
    //     result whole — the new source is selected, the display is taken fresh,
    //     and the session is (re)built exactly once so auto-sync is queued. Equal
    //     to a couple built fresh from the same resolution.
    @Test func videoFirstThenDecisiveAudioAdoptsConfident() {
        let plan = RowMergePolicy.plan(
            protected: false,
            currentSelectedKey: nil, currentCandidateKeys: [],
            freshSelectedKey: "A", freshCandidateKeys: ["A"])
        #expect(plan.selectedAudioKey == "A")
        #expect(plan.candidateKeys == ["A"])
        #expect(plan.adoptFreshDisplay)        // state/chip/detail become the fresh confident row
        #expect(plan.rebuildSession)           // nil → A ⇒ build session + queue sync, once
        #expect(plan.clearForced)
    }

    // (2) Video first, then a weak/nondecisive audio: the resolver leaves the row
    //     unresolved (no selection), so the candidate is surfaced in the list
    //     WITHOUT promoting the row — no selection, no session rebuild.
    @Test func videoFirstThenWeakAudioStaysUnresolved() {
        let plan = RowMergePolicy.plan(
            protected: false,
            currentSelectedKey: nil, currentCandidateKeys: [],
            freshSelectedKey: nil, freshCandidateKeys: ["W"])
        #expect(plan.selectedAudioKey == nil)  // never a coin-flip
        #expect(plan.candidateKeys == ["W"])   // the weak candidate is still visible
        #expect(plan.adoptFreshDisplay)        // takes the (still ambiguous/no-match) fresh display
        #expect(!plan.rebuildSession)          // nil → nil ⇒ no session, no sync
    }

    // (3) A manually PINNED row, then a stronger audio arrives: the user's chosen
    //     source is preserved; the new audio appears only as an appended alternate;
    //     no session rebuild.
    @Test func pinnedRowKeepsSelectionWhenStrongerAudioArrives() {
        let plan = RowMergePolicy.plan(
            protected: true,                              // pinned
            currentSelectedKey: "chosen", currentCandidateKeys: ["chosen"],
            freshSelectedKey: "stronger", freshCandidateKeys: ["stronger", "chosen"])
        #expect(plan.selectedAudioKey == "chosen")        // selection NOT changed silently
        #expect(plan.candidateKeys == ["chosen", "stronger"]) // existing first, new appended
        #expect(!plan.adoptFreshDisplay)                  // display preserved
        #expect(!plan.rebuildSession)                     // no teardown/reattach
        #expect(!plan.clearForced)                        // a forced chip stays honest
    }

    // (4) An APPROVED row, then a stronger audio arrives: identical protection —
    //     selection preserved, new evidence only adds an alternate.
    @Test func approvedRowKeepsSelectionWhenStrongerAudioArrives() {
        let plan = RowMergePolicy.plan(
            protected: true,                              // approved
            currentSelectedKey: "queued", currentCandidateKeys: ["queued"],
            freshSelectedKey: "stronger", freshCandidateKeys: ["stronger", "queued"])
        #expect(plan.selectedAudioKey == "queued")
        #expect(plan.candidateKeys == ["queued", "stronger"])
        #expect(!plan.rebuildSession)
    }

    // (5) Repeated resolve over the same evidence must not duplicate candidates,
    //     sessions, or sync work. Applying the SAME fresh resolution twice: the
    //     second application changes nothing and asks for NO session rebuild.
    @Test func repeatedResolveIsIdempotentNoDuplicateWork() {
        // First merge: unresolved → confident A (rebuild once).
        let first = RowMergePolicy.plan(
            protected: false,
            currentSelectedKey: nil, currentCandidateKeys: [],
            freshSelectedKey: "A", freshCandidateKeys: ["A"])
        #expect(first.rebuildSession)
        // Second merge with the SAME resolution, now from the post-first state.
        let second = RowMergePolicy.plan(
            protected: false,
            currentSelectedKey: first.selectedAudioKey, currentCandidateKeys: first.candidateKeys,
            freshSelectedKey: "A", freshCandidateKeys: ["A"])
        #expect(second.selectedAudioKey == "A")
        #expect(second.candidateKeys == ["A"])   // no duplicate candidate
        #expect(!second.rebuildSession)          // same source ⇒ no re-teardown / no re-sync

        // Protected idempotency: re-applying a fully-known fresh set appends nothing.
        let pinnedAgain = RowMergePolicy.plan(
            protected: true,
            currentSelectedKey: "chosen", currentCandidateKeys: ["chosen", "stronger"],
            freshSelectedKey: "stronger", freshCandidateKeys: ["stronger", "chosen"])
        #expect(pinnedAgain.candidateKeys == ["chosen", "stronger"]) // unchanged, no dupes
        #expect(!pinnedAgain.rebuildSession)
    }

    // Extra invariants behind the five scenarios.

    // An unprotected re-pick to a DIFFERENT source rebuilds the session once and
    // swaps the selection (e.g. a better take supersedes a provisional auto-pick).
    @Test func unprotectedRepickSwapsSelectionOnce() {
        let plan = RowMergePolicy.plan(
            protected: false,
            currentSelectedKey: "old", currentCandidateKeys: ["old", "new"],
            freshSelectedKey: "new", freshCandidateKeys: ["new", "old"])
        #expect(plan.selectedAudioKey == "new")
        #expect(plan.candidateKeys == ["new", "old"]) // unprotected takes fresh order
        #expect(plan.rebuildSession)
    }

    // A confident unprotected row that re-resolves to AMBIGUOUS (selection lost,
    // e.g. a rival take now muddies it) drops the session exactly once.
    @Test func unprotectedConfidentToAmbiguousTearsDownOnce() {
        let plan = RowMergePolicy.plan(
            protected: false,
            currentSelectedKey: "A", currentCandidateKeys: ["A"],
            freshSelectedKey: nil, freshCandidateKeys: ["A", "B"])
        #expect(plan.selectedAudioKey == nil)
        #expect(plan.candidateKeys == ["A", "B"])
        #expect(plan.rebuildSession)   // A → nil ⇒ tear the session down (once)
    }

    // A protected row whose fresh resolution re-picks the SAME already-known source
    // appends nothing and never rebuilds — the common "re-import unrelated file"
    // case must be free.
    @Test func protectedNoOpWhenNothingNew() {
        let plan = RowMergePolicy.plan(
            protected: true,
            currentSelectedKey: "chosen", currentCandidateKeys: ["chosen", "alt"],
            freshSelectedKey: "chosen", freshCandidateKeys: ["chosen", "alt"])
        #expect(plan.candidateKeys == ["chosen", "alt"])
        #expect(!plan.adoptFreshDisplay)
        #expect(!plan.rebuildSession)
    }
}

// MARK: - Match quality vs. selection (TODO-009)

/// Deterministic, media-free coverage of `MatchQualityPolicy.decide` — the pure core
/// behind `CoupleRow.verdict`. These lock the TODO-009 acceptance set: the verdict dot
/// is EVIDENCE for the selected pair, never the act of selecting. So a manual pick
/// alone never goes green (it needs the SAME confident classifier an automatic
/// confident match clears); a confident AUTOMATIC selection is green; a below-floor
/// `forced` pairing is questionable; and a manual pick is PROMOTED to green only after
/// its pair's evidence (recomputed by the app from `matchPair`/`verify`) clears the
/// bar. AppModel/CoupleRow only obtain the inputs and map the result to the UI
/// `Verdict`, so testing the decision tests the behavior at the smallest layer (the
/// executable target's DebugDrive `COUPLE_QUALITY_TEST` mirrors these live).
@Suite struct MatchQualityTests {
    // (A) AMBIGUOUS MANUAL SELECTION STAYS QUESTIONABLE — the bug: clicking a
    //     candidate on an ambiguous row must NOT flip the dot green. A manual pick
    //     whose pair has not cleared the classifier (false), or hasn't been evaluated
    //     yet (nil), is questionable regardless of the resolver's row verdict.
    @Test func ambiguousManualSelectionStaysQuestionable() {
        // The resolver row is ambiguous; the user picked a candidate. Pair evidence
        // says NOT a confident match.
        let q = MatchQualityPolicy.decide(forced: false, selection: .manual,
                                          autoVerdict: .questionable, manualMatchConfident: false)
        #expect(q == .questionable)
        // Not-yet-evaluated (the instant after the click, before evidence lands): also
        // questionable — never green on absent evidence.
        let pending = MatchQualityPolicy.decide(forced: false, selection: .manual,
                                                autoVerdict: .questionable, manualMatchConfident: nil)
        #expect(pending == .questionable)
        // Even if the underlying row's auto verdict were "good", a manual pick whose
        // OWN pair didn't pass is still questionable (selection never promotes).
        let goodRowButManualFails = MatchQualityPolicy.decide(
            forced: false, selection: .manual, autoVerdict: .confident, manualMatchConfident: false)
        #expect(goodRowButManualFails == .questionable)
    }

    // (B) CONFIDENT AUTOMATIC SELECTION IS GREEN — an auto-resolved confident/partial
    //     row takes the resolver verdict directly (the resolver already classified
    //     that pair); no separate manual evidence is needed or consulted.
    @Test func confidentAutomaticSelectionIsGreen() {
        let confident = MatchQualityPolicy.decide(forced: false, selection: .automatic,
                                                  autoVerdict: .confident, manualMatchConfident: nil)
        #expect(confident == .confident)
        // `partial` (coverage is never a quality penalty, §V2.3) is also good.
        let partial = MatchQualityPolicy.decide(forced: false, selection: .automatic,
                                                autoVerdict: .confident, manualMatchConfident: nil)
        #expect(partial == .confident)
        // An automatic selection ignores any stale manualMatchConfident value.
        let ignoresManualFlag = MatchQualityPolicy.decide(
            forced: false, selection: .automatic, autoVerdict: .confident, manualMatchConfident: false)
        #expect(ignoresManualFlag == .confident)
    }

    // (C) BELOW-FLOOR FORCING IS QUESTIONABLE — a kept below-floor manual pairing is
    //     always questionable, overriding every other signal (honest, §3.6).
    @Test func belowFloorForcedIsQuestionable() {
        // Forced wins even if the pair somehow reported confident, and for any
        // selection provenance.
        #expect(MatchQualityPolicy.decide(forced: true, selection: .manual,
                                          autoVerdict: .confident, manualMatchConfident: true) == .questionable)
        #expect(MatchQualityPolicy.decide(forced: true, selection: .automatic,
                                          autoVerdict: .confident, manualMatchConfident: nil) == .questionable)
        #expect(MatchQualityPolicy.decide(forced: true, selection: .none,
                                          autoVerdict: .none, manualMatchConfident: nil) == .questionable)
    }

    // (D) EVIDENCE-BASED PROMOTION AFTER RE-ANALYSIS — a manual pick on an ambiguous
    //     row that, once its pair is run through the SAME classifier, DOES pass is
    //     promoted to green. This is the only path that greens a manual pick.
    @Test func manualSelectionPromotedWhenPairPasses() {
        // Before evidence: questionable. After re-analysis says the pair is a match:
        // green — on the same standard an automatic confident match uses.
        let before = MatchQualityPolicy.decide(forced: false, selection: .manual,
                                               autoVerdict: .questionable, manualMatchConfident: nil)
        #expect(before == .questionable)
        let after = MatchQualityPolicy.decide(forced: false, selection: .manual,
                                              autoVerdict: .questionable, manualMatchConfident: true)
        #expect(after == .confident)
    }

    // (E) NO SELECTION FALLS BACK TO THE RESOLVER VERDICT (never a coin-flip): an
    //     unselected ambiguous row is questionable, an unselected no-match is none.
    @Test func noSelectionUsesResolverVerdict() {
        #expect(MatchQualityPolicy.decide(forced: false, selection: .none,
                                          autoVerdict: .questionable, manualMatchConfident: nil) == .questionable)
        #expect(MatchQualityPolicy.decide(forced: false, selection: .none,
                                          autoVerdict: .none, manualMatchConfident: nil) == .none)
        // A manualMatchConfident value left over from a prior manual pick is ignored
        // once nothing is selected.
        #expect(MatchQualityPolicy.decide(forced: false, selection: .none,
                                          autoVerdict: .none, manualMatchConfident: true) == .none)
    }

    // (F) APPROVAL AND ALIGNMENT FIT ARE NOT INPUTS — neither appears in the decision
    //     signature, so they cannot promote quality. This test documents the
    //     invariant structurally: the same inputs always yield the same verdict
    //     regardless of any approval/RMS state the caller might hold.
    @Test func qualityIsIndependentOfApprovalAndFit() {
        // Two identical decisions (a questionable manual pick) — there is no parameter
        // by which "approved" or "low RMS" could change the answer.
        let a = MatchQualityPolicy.decide(forced: false, selection: .manual,
                                          autoVerdict: .confident, manualMatchConfident: false)
        let b = MatchQualityPolicy.decide(forced: false, selection: .manual,
                                          autoVerdict: .confident, manualMatchConfident: false)
        #expect(a == b)
        #expect(a == .questionable)
    }
}

// MARK: - Media-pool selection (TODO-001 foundation; redesign 2026-06-15)

/// Deterministic, media-free coverage of `PoolSelection.select` — the pure core of
/// `AppModel.selectPool`. These lock the owner-decision-3 acceptance set: a plain
/// click replaces; ⌘ toggles one without disturbing the rest (and re-homes the
/// primary when the removed key WAS the primary, or empties to nil); ⇧ selects the
/// contiguous range over the DISPLAYED order with the anchor preserved; ⇧ with no
/// anchor degrades to a plain click; and an off-list click is inert. AppModel only
/// applies the result and drives the preview (load-paused on primary change), so
/// testing the transition tests the behavior at the smallest layer. The app
/// target's DebugDrive `COUPLE_POOLSEL_TEST` mirrors these live and additionally
/// covers the preview load-paused path (`PoolPreviewState` lives in the app target,
/// so it cannot be reached from this CoupleKit-only test target — exactly as the
/// existing `COUPLE_PREVIEW_TEST` state-machine coverage is DebugDrive-only).
@Suite struct PoolSelectionTests {
    private let order = ["a", "b", "c", "d"] // a stable displayed pool order

    // (1) PLAIN replaces the whole selection; the clicked key is selection, primary,
    //     and the anchor (start of any subsequent ⇧-range).
    @Test func plainReplacesAndSetsPrimaryAndAnchor() {
        let r = PoolSelection.select(current: ["a", "b"], primary: "a", anchor: "a",
                                     clicked: "c", ordered: order, modifier: .plain)
        #expect(r.selection == ["c"])
        #expect(r.primary == "c")
        #expect(r.anchor == "c")
    }

    // (2) ⌘ toggle ON adds without disturbing the rest; the freshly-added key leads.
    @Test func commandTogglesOnAddsAndLeads() {
        let r = PoolSelection.select(current: ["a"], primary: "a", anchor: "a",
                                     clicked: "c", ordered: order, modifier: .command)
        #expect(r.selection == ["a", "c"])
        #expect(r.primary == "c")
        #expect(r.anchor == "c")
    }

    // (3) ⌘ toggle OFF a NON-primary keeps the current primary intact.
    @Test func commandTogglesOffNonPrimaryKeepsPrimary() {
        let r = PoolSelection.select(current: ["a", "b", "c"], primary: "c", anchor: "c",
                                     clicked: "a", ordered: order, modifier: .command)
        #expect(r.selection == ["b", "c"])
        #expect(r.primary == "c")
    }

    // (4) ⌘ toggle OFF the PRIMARY re-homes the primary to the nearest remaining
    //     member in display order (b is adjacent to the removed c).
    @Test func commandRemovingPrimaryReHomesToNearest() {
        let r = PoolSelection.select(current: ["a", "b", "c"], primary: "c", anchor: "c",
                                     clicked: "c", ordered: order, modifier: .command)
        #expect(r.selection == ["a", "b"])
        #expect(r.primary == "b")     // nearest remaining (distance 1) wins over a (2)
        #expect(r.anchor == "b")
    }

    // (4b) Re-homing prefers the closer neighbor on either side, and ties resolve to
    //      the earlier one (deterministic).
    @Test func commandRemovingPrimaryPrefersCloserThenEarlier() {
        // Remove primary "b" from {a,b,d}; a is distance 1, d is distance 2 ⇒ a.
        let near = PoolSelection.select(current: ["a", "b", "d"], primary: "b", anchor: "b",
                                        clicked: "b", ordered: order, modifier: .command)
        #expect(near.primary == "a")
        // Remove primary "b" from {a,c} after removing b: a (1) vs c (1) tie ⇒ earlier a.
        let tie = PoolSelection.select(current: ["a", "b", "c"], primary: "b", anchor: "b",
                                       clicked: "b", ordered: order, modifier: .command)
        #expect(tie.selection == ["a", "c"])
        #expect(tie.primary == "a")
    }

    // (5) ⌘ removing the LAST selected key empties everything (the caller then stops
    //     the preview — primary nil).
    @Test func commandRemovingLastEmptiesToNil() {
        let r = PoolSelection.select(current: ["a"], primary: "a", anchor: "a",
                                     clicked: "a", ordered: order, modifier: .command)
        #expect(r.selection.isEmpty)
        #expect(r.primary == nil)
        #expect(r.anchor == nil)
    }

    // (6) ⇧ selects the inclusive contiguous range over DISPLAYED order from the
    //     anchor to the clicked key; the anchor is preserved and the clicked end leads.
    @Test func shiftSelectsContiguousRangePreservingAnchor() {
        let r = PoolSelection.select(current: ["b"], primary: "b", anchor: "b",
                                     clicked: "d", ordered: order, modifier: .shift)
        #expect(r.selection == ["b", "c", "d"])
        #expect(r.primary == "d")
        #expect(r.anchor == "b")      // sticky, so the range can be re-extended
    }

    // (6b) ⇧ the other direction over the SAME anchor re-extends (range is replaced,
    //      anchor unchanged) — the Finder behavior where ⇧ always measures from the
    //      anchor, not from the previous range.
    @Test func shiftReExtendsFromStickyAnchor() {
        let r = PoolSelection.select(current: ["b", "c", "d"], primary: "d", anchor: "b",
                                     clicked: "a", ordered: order, modifier: .shift)
        #expect(r.selection == ["a", "b"])
        #expect(r.primary == "a")
        #expect(r.anchor == "b")
    }

    // (6c) ⇧ onto the anchor itself collapses the range to the single anchor.
    @Test func shiftOntoAnchorIsSingleton() {
        let r = PoolSelection.select(current: ["a", "b", "c"], primary: "c", anchor: "a",
                                     clicked: "a", ordered: order, modifier: .shift)
        #expect(r.selection == ["a"])
        #expect(r.primary == "a")
        #expect(r.anchor == "a")
    }

    // (7) ⇧ with no usable anchor degrades to a plain click (anchor falls back to the
    //     clicked key).
    @Test func shiftWithNoAnchorDegradesToPlain() {
        let r = PoolSelection.select(current: [], primary: nil, anchor: nil,
                                     clicked: "c", ordered: order, modifier: .shift)
        #expect(r.selection == ["c"])
        #expect(r.primary == "c")
        #expect(r.anchor == "c")
    }

    // (8) A click on a key NOT in the displayed order is inert — selection, primary,
    //     and anchor are preserved verbatim (a stale row can't poison the selection).
    @Test func offListClickIsInert() {
        for mod: PoolSelectModifier in [.plain, .command, .shift] {
            let r = PoolSelection.select(current: ["a", "b"], primary: "b", anchor: "a",
                                         clicked: "zzz", ordered: order, modifier: mod)
            #expect(r.selection == ["a", "b"])
            #expect(r.primary == "b")
            #expect(r.anchor == "a")
        }
    }

    // (9) Edge: a plain click into an EMPTY pool selection state still selects the
    //     clicked key cleanly (the first click after import).
    @Test func plainFromEmptySelects() {
        let r = PoolSelection.select(current: [], primary: nil, anchor: nil,
                                     clicked: "a", ordered: order, modifier: .plain)
        #expect(r.selection == ["a"])
        #expect(r.primary == "a")
        #expect(r.anchor == "a")
    }
}

// MARK: - Drag-to-pair drop decision (TODO-013)

/// Deterministic, media-free coverage of `RepairDropPolicy.decide` — the pure core
/// shared by BOTH manual-pairing drop targets (the queue row and the center
/// workspace). Because both call sites consume this single decision, locking it here
/// guarantees they cannot diverge: the same (selected-couple / audio-payload /
/// video-available / current-vs-dropped key / approved) inputs always yield the same
/// outcome. The app target owns only the side effects (present the confirmation, then
/// the shared `repair`); the app-target DebugDrive `COUPLE_REPAIRDROP_TEST` mirrors
/// these live AND asserts both gestures route through the same `requestRepairDrop`
/// (the routing + AppModel state live in the app target, unreachable from this
/// CoupleKit-only suite — exactly as `COUPLE_QUALITY_TEST`/`COUPLE_POOLSEL_TEST` are).
@Suite struct RepairDropPolicyTests {
    // Defaults for a well-formed, droppable couple; each test overrides only the
    // dimension it exercises (keeps the intent of each case obvious).
    private func decide(hasSelectedCouple: Bool = true,
                        payloadIsAudio: Bool = true,
                        videoAvailable: Bool = true,
                        currentSelectedKey: String? = "current",
                        droppedKey: String = "dropped",
                        approved: Bool = false) -> RepairDropOutcome {
        RepairDropPolicy.decide(hasSelectedCouple: hasSelectedCouple,
                                payloadIsAudio: payloadIsAudio,
                                videoAvailable: videoAvailable,
                                currentSelectedKey: currentSelectedKey,
                                droppedKey: droppedKey, approved: approved)
    }

    // (1) DROP THE ALREADY-SELECTED AUDIO ⇒ NO-OP — re-dropping the current source
    //     changes nothing, regardless of approval (so it never clears an approved
    //     couple's export or rebuilds its session).
    @Test func droppingCurrentSourceIsNoOp() {
        #expect(decide(currentSelectedKey: "x", droppedKey: "x", approved: false) == .noOp)
        // Even an approved couple: re-dropping its own source must NOT ask to confirm
        // or clear anything — nothing is changing.
        #expect(decide(currentSelectedKey: "x", droppedKey: "x", approved: true) == .noOp)
    }

    // (2) AN UNRESOLVED COUPLE ACCEPTS IMMEDIATELY — no current selection (and a real
    //     dropped key) ⇒ accept; there is no approved export to protect.
    @Test func unresolvedCoupleAcceptsImmediately() {
        #expect(decide(currentSelectedKey: nil, droppedKey: "new", approved: false) == .accept)
    }

    // (3) AN UNAPPROVED COUPLE WITH A DIFFERENT SOURCE REPLACES IMMEDIATELY — no
    //     confirmation dialog (the old audio simply stays in the pool).
    @Test func unapprovedDifferentSourceAcceptsImmediately() {
        #expect(decide(currentSelectedKey: "old", droppedKey: "new", approved: false) == .accept)
    }

    // (4) AN APPROVED COUPLE WITH A DIFFERENT SOURCE NEEDS CONFIRMATION — replacing it
    //     removes it from the export queue, so it asks first.
    @Test func approvedDifferentSourceNeedsConfirmation() {
        #expect(decide(currentSelectedKey: "old", droppedKey: "new", approved: true) == .needsConfirmation)
        // Also when the approved couple was unresolved-then-approved (no current key):
        // a different (non-nil) source still confirms because approval is at stake.
        #expect(decide(currentSelectedKey: nil, droppedKey: "new", approved: true) == .needsConfirmation)
    }

    // (5) NO SELECTED COUPLE ⇒ REJECT (no change) — a center-workspace drop with
    //     nothing selected, or a queue-row drop with a nil row, can't pair anything.
    @Test func noSelectedCoupleRejects() {
        #expect(decide(hasSelectedCouple: false) == .reject)
        // Reject wins even if every other signal would otherwise accept.
        #expect(decide(hasSelectedCouple: false, currentSelectedKey: nil,
                       droppedKey: "new", approved: false) == .reject)
    }

    // (6) A NON-AUDIO PAYLOAD ⇒ REJECT (no change) — a video key, the empty sentinel a
    //     non-audio `.draggable` carries, or junk never mutates state.
    @Test func nonAudioPayloadRejects() {
        #expect(decide(payloadIsAudio: false) == .reject)
        // Reject precedes the same-key no-op check too (a non-audio "current" key drop
        // is still a reject, never a silent no-op).
        #expect(decide(payloadIsAudio: false, currentSelectedKey: "x", droppedKey: "x") == .reject)
    }

    // (7) AN UNAVAILABLE VIDEO ⇒ REJECT (no change) — a stripped/handle-less video has
    //     nothing to pair the audio WITH.
    @Test func unavailableVideoRejects() {
        #expect(decide(videoAvailable: false) == .reject)
        #expect(decide(videoAvailable: false, currentSelectedKey: "old",
                       droppedKey: "new", approved: true) == .reject)
    }

    // (8) REJECT IS CHECKED BEFORE REPLACEMENT REASONING — when multiple guards fail
    //     the result is still a single clean reject (no partial decision leaks).
    @Test func rejectGuardsTakePrecedence() {
        #expect(decide(hasSelectedCouple: false, payloadIsAudio: false,
                       videoAvailable: false) == .reject)
    }

    // (9) THE TWO GESTURES CANNOT DIVERGE — the decision is a pure function of its
    //     inputs alone, so identical inputs (whatever the call site) give an identical
    //     outcome. This documents the shared-path guarantee structurally: there is no
    //     parameter that distinguishes "queue row" from "center workspace".
    @Test func sameInputsYieldSameOutcomeAcrossCallSites() {
        let cases: [(String?, String, Bool)] = [
            (nil, "a", false), ("a", "a", true), ("a", "b", false), ("a", "b", true),
        ]
        for (cur, dropped, approved) in cases {
            let a = decide(currentSelectedKey: cur, droppedKey: dropped, approved: approved)
            let b = decide(currentSelectedKey: cur, droppedKey: dropped, approved: approved)
            #expect(a == b)
        }
    }
}

// TODO-001: the PURE removal/clear decisions (`MediaRemovalPolicy`) behind
// `AppModel.removeSelectedMedia` (trash button / Delete key) and `clearSession`.
// No engine, no media: the plan is a function of the selection set, the live pool
// keys, and the couples' (video, selectedAudio, approved) — so the Delete key and
// the trash button (and the same selection) decide identically. Covers full /
// partial / zero reference; immediate-vs-confirm; affected-couple/approved counts;
// stale-key dropping; and the clear-session counts.
@Suite struct MediaRemovalPolicyTests {
    private func ref(_ v: String, _ a: String?, approved: Bool = false) -> CoupleRef {
        CoupleRef(videoKey: v, selectedAudioKey: a, approved: approved)
    }

    // (1) ZERO reference — removing UNUSED audio (no couple selects it) needs NO
    //     confirmation and reports zero affected: it is removed immediately.
    @Test func unusedRemovalIsImmediate() {
        let couples = [ref("v1", "a1"), ref("v2", "a2")]
        let plan = MediaRemovalPolicy.plan(selectedKeys: ["unused"],
                                           poolKeys: ["v1", "v2", "a1", "a2", "unused"],
                                           couples: couples)
        #expect(plan.removableKeys == ["unused"])
        #expect(plan.needsConfirmation == false)
        #expect(plan.affectedCouples == 0)
        #expect(plan.affectedApproved == 0)
        #expect(!plan.isEmpty)
    }

    // (1b) A removed key that is only a NON-SELECTED ALTERNATE is still "unused" for the
    //      consequence count — the selected pairing is untouched, so no confirmation.
    //      (The plan never sees alternates; it counts a couple affected ONLY by its video
    //      or its SELECTED audio — this asserts that contract directly.)
    @Test func removedAlternateIsNotAnAffectedCouple() {
        // v1 selects a1; a2 is merely an alternate the resolver also found. Removing a2
        // changes nothing about v1's selected pairing.
        let couples = [ref("v1", "a1")]
        let plan = MediaRemovalPolicy.plan(selectedKeys: ["a2"],
                                           poolKeys: ["v1", "a1", "a2"], couples: couples)
        #expect(plan.needsConfirmation == false)
        #expect(plan.affectedCouples == 0)
    }

    // (2) PARTIAL/FULL reference — removing a SELECTED audio affects its couple (the row
    //     will re-resolve) ⇒ confirmation with the affected count.
    @Test func removingSelectedAudioNeedsConfirmation() {
        let couples = [ref("v1", "a1"), ref("v2", "a2")]
        let plan = MediaRemovalPolicy.plan(selectedKeys: ["a1"],
                                           poolKeys: ["v1", "v2", "a1", "a2"], couples: couples)
        #expect(plan.needsConfirmation)
        #expect(plan.affectedCouples == 1)
        #expect(plan.affectedApproved == 0)
    }

    // (3) Removing a VIDEO affects its couple (the row is removed) ⇒ confirmation.
    @Test func removingVideoNeedsConfirmation() {
        let couples = [ref("v1", "a1")]
        let plan = MediaRemovalPolicy.plan(selectedKeys: ["v1"],
                                           poolKeys: ["v1", "a1"], couples: couples)
        #expect(plan.needsConfirmation)
        #expect(plan.affectedCouples == 1)
    }

    // (4) APPROVED rows are counted: removing audio used by two couples, one approved.
    @Test func affectedApprovedCount() {
        // a1 is shared as the selected source by v1 (approved) and v2 (not). Removing a1
        // affects both; one is approved.
        let couples = [ref("v1", "a1", approved: true), ref("v2", "a1", approved: false),
                       ref("v3", "a3", approved: true)]
        let plan = MediaRemovalPolicy.plan(selectedKeys: ["a1"],
                                           poolKeys: ["v1", "v2", "v3", "a1", "a3"], couples: couples)
        #expect(plan.affectedCouples == 2)
        #expect(plan.affectedApproved == 1)
    }

    // (5) ONE plan for the WHOLE batch — a mixed selection (one video + one unused audio
    //     + one used audio) is a single decision with the combined counts (never per file).
    @Test func batchMixesUnusedAndReferencedIntoOnePlan() {
        let couples = [ref("v1", "a1", approved: true), ref("v2", "a2")]
        let plan = MediaRemovalPolicy.plan(selectedKeys: ["v2", "a1", "unused"],
                                           poolKeys: ["v1", "v2", "a1", "a2", "unused"],
                                           couples: couples)
        #expect(plan.removableKeys == ["v2", "a1", "unused"])
        #expect(plan.needsConfirmation)
        // v2 (its row removed) + v1 (its selected a1 removed) = 2 affected; v1 approved.
        #expect(plan.affectedCouples == 2)
        #expect(plan.affectedApproved == 1)
    }

    // (6) STALE/JUNK keys are dropped — a selection naming a key absent from the pool is
    //     intersected away; an all-stale selection is an empty (no-op) plan.
    @Test func staleKeysAreDroppedAndEmptyPlanNoOps() {
        let couples = [ref("v1", "a1")]
        let mixed = MediaRemovalPolicy.plan(selectedKeys: ["a1", "ghost"],
                                            poolKeys: ["v1", "a1"], couples: couples)
        #expect(mixed.removableKeys == ["a1"]) // ghost dropped
        #expect(mixed.needsConfirmation)

        let allStale = MediaRemovalPolicy.plan(selectedKeys: ["ghost", "phantom"],
                                               poolKeys: ["v1", "a1"], couples: couples)
        #expect(allStale.isEmpty)
        #expect(allStale.needsConfirmation == false)
        #expect(allStale.affectedCouples == 0)
    }

    // (7) THE TWO REMOVAL GESTURES CANNOT DIVERGE — the plan is a pure function of its
    //     inputs alone (no call-site flavor), so the Delete key and the trash button on
    //     the same selection yield an identical plan.
    @Test func sameSelectionYieldsSamePlanAcrossCallSites() {
        let couples = [ref("v1", "a1", approved: true), ref("v2", "a2")]
        let pool: Set<String> = ["v1", "v2", "a1", "a2"]
        for sel: Set<String> in [["a1"], ["v2"], ["a1", "v2"], ["a2"], []] {
            let a = MediaRemovalPolicy.plan(selectedKeys: sel, poolKeys: pool, couples: couples)
            let b = MediaRemovalPolicy.plan(selectedKeys: sel, poolKeys: pool, couples: couples)
            #expect(a == b)
        }
    }

    // (8) CLEAR SESSION counts — media files, couples, and approved (queued) couples.
    @Test func clearSessionCounts() {
        let couples = [ref("v1", "a1", approved: true), ref("v2", "a2", approved: false),
                       ref("v3", nil, approved: false)]
        let counts = MediaRemovalPolicy.clearCounts(mediaCount: 5, couples: couples)
        #expect(counts == ClearSessionCounts(media: 5, couples: 3, approved: 1))
        // Empty session ⇒ all zero.
        #expect(MediaRemovalPolicy.clearCounts(mediaCount: 0, couples: []) ==
                ClearSessionCounts(media: 0, couples: 0, approved: 0))
    }
}

// MARK: - TODO-011 anchor proposal (pure: overlap-bounded 3/2/0 count + "+ anchor")

@Suite struct AnchorProposalTests {
    typealias Cand = AnchorProposal.Candidate

    /// A dense, evenly-spaced, all-usable onset list across [t0, t1] so each zone/half
    /// has a candidate to pick. Score rises with index so picks are deterministic.
    private func usableOnsets(_ t0: Double, _ t1: Double, step: Double = 1) -> [Cand] {
        var out: [Cand] = []
        var t = t0
        var i = 0.0
        while t <= t1 {
            out.append(Cand(t: t, score: 1 + i, usable: true))
            t += step; i += 1
        }
        return out
    }

    private func inside(_ ts: [Double], _ ov: ClosedRange<Double>) -> Bool {
        ts.allSatisfy { $0 >= ov.lowerBound && $0 <= ov.upperBound }
    }

    // (1) FULL overlap (≥30 s, == camera span) ⇒ THREE anchors, all in the overlap, one
    //     per zone, sorted.
    @Test func fullOverlapProposesThreeInsideOverlap() {
        let ov = 0.0...120.0
        let picks = AnchorProposal.propose(overlap: ov, candidates: usableOnsets(0, 120))
        #expect(picks.count == 3)
        #expect(inside(picks, ov))
        #expect(picks == picks.sorted())
        // One pick lands in each of the three zones [1,30]/[38,62]/[70,99]% of 120 s.
        #expect(picks[0] <= 36)            // zone 1 ≈ [1.2, 36]
        #expect(picks[1] >= 45 && picks[1] <= 74) // zone 2 ≈ [45.6, 74.4]
        #expect(picks[2] >= 84)            // zone 3 ≈ [84, 118.8]
    }

    // (2) PARTIAL overlap (≥30 s but SHORTER than the camera clip) ⇒ still THREE, and NONE
    //     past the external content: every pick stays inside the (shifted) overlap even
    //     though onsets exist out to the full camera duration.
    @Test func partialOverlapKeepsThreeInsideTheOverlapNotCameraDuration() {
        // Camera is 0..200 s but the external only overlaps 40..100 s (60 s window).
        let ov = 40.0...100.0
        let camOnsets = usableOnsets(0, 200) // onsets exist well past the overlap
        let picks = AnchorProposal.propose(overlap: ov, candidates: camOnsets)
        #expect(picks.count == 3)
        #expect(inside(picks, ov))
        #expect(picks.allSatisfy { $0 >= 40 && $0 <= 100 }) // never out at 100..200
    }

    // (3) SHORT overlap (<30 s) ⇒ exactly TWO well-separated, non-degenerate anchors,
    //     ≥ ~25% of the overlap apart, both inside.
    @Test func shortOverlapProposesTwoSeparatedInsideOverlap() {
        let ov = 0.0...20.0 // 20 s < 30 s threshold
        let picks = AnchorProposal.propose(overlap: ov, candidates: usableOnsets(0, 20, step: 0.5))
        #expect(picks.count == 2)
        #expect(inside(picks, ov))
        #expect(abs(picks[1] - picks[0]) >= 0.25 * 20) // ≥ 25% of overlap apart
    }

    // (4) ZERO / degenerate overlap ⇒ ZERO anchors (manual rescue), NEVER one.
    @Test func noOverlapProposesZeroNeverOne() {
        #expect(AnchorProposal.propose(overlap: nil, candidates: usableOnsets(0, 60)).isEmpty)
        // Inside-out / empty range is also zero.
        let empty: ClosedRange<Double> = 10.0...10.0
        #expect(AnchorProposal.propose(overlap: empty, candidates: usableOnsets(0, 60)).isEmpty)
    }

    // (4b) SHORT overlap with only ONE usable candidate ⇒ ZERO, not one, not a midpoint
    //      pair (a degenerate window can't be rescued by faking a second anchor).
    @Test func shortOverlapWithOneUsableCandidateProposesZero() {
        let ov = 0.0...20.0
        // Many onsets but only one is usable (rest are silent/degenerate).
        var cands = usableOnsets(0, 20, step: 0.5).map { Cand(t: $0.t, score: $0.score, usable: false) }
        cands[5] = Cand(t: cands[5].t, score: 99, usable: true)
        let picks = AnchorProposal.propose(overlap: ov, candidates: cands)
        #expect(picks.isEmpty) // never one, never a midpoint pair
    }

    // (4c) SHORT overlap whose only usable candidates are bunched together (cannot meet the
    //      ≥25% separation) ⇒ ZERO rather than a too-close pair.
    @Test func shortOverlapWithBunchedCandidatesProposesZero() {
        let ov = 0.0...20.0
        // Two usable onsets 1 s apart — far below 25% of 20 s (= 5 s); rest unusable.
        let cands = [Cand(t: 9.0, score: 5, usable: true), Cand(t: 10.0, score: 6, usable: true),
                     Cand(t: 2.0, score: 1, usable: false), Cand(t: 18.0, score: 1, usable: false)]
        #expect(AnchorProposal.propose(overlap: ov, candidates: cands).isEmpty)
    }

    // (5) Long overlap where EVERYTHING is silent/degenerate ⇒ ZERO (no fabricated
    //     midpoints even on a long overlap); the count is never one.
    @Test func longOverlapAllDegenerateProposesZero() {
        let ov = 0.0...120.0
        let cands = usableOnsets(0, 120).map { Cand(t: $0.t, score: $0.score, usable: false) }
        #expect(AnchorProposal.propose(overlap: ov, candidates: cands).isEmpty)
    }

    // (6) Out-of-overlap onsets are never chosen even when they are the strongest — only
    //     in-overlap usable candidates are eligible.
    @Test func strongestOutsideOverlapIsIgnored() {
        let ov = 40.0...100.0
        var cands = usableOnsets(40, 100)
        cands.append(Cand(t: 150, score: 9999, usable: true)) // strongest, but outside
        let picks = AnchorProposal.propose(overlap: ov, candidates: cands)
        #expect(inside(picks, ov))
        #expect(!picks.contains(150))
    }

    // (7) "+ anchor" stays inside the overlap, respects the ≥8% separation, and returns the
    //     STRONGEST qualifying in-overlap onset.
    @Test func addAnchorPicksStrongestInsideSeparatedFromExisting() {
        let ov = 0.0...100.0
        let cands = [Cand(t: 10, score: 5, usable: true), Cand(t: 50, score: 9, usable: true),
                     Cand(t: 52, score: 10, usable: true)] // 52 is strongest but ~2 s from 50
        let t = AnchorProposal.addAnchorCandidate(overlap: ov, existing: [50], candidates: cands)
        // 52 is < 8% of 100 (= 8 s) from the existing 50, so it's excluded ⇒ 10 stays.
        #expect(t == 10)
    }

    // (8) "+ anchor" NEVER chooses an onset outside the overlap, even the strongest.
    @Test func addAnchorNeverChoosesOutsideOverlap() {
        let ov = 0.0...60.0
        let cands = [Cand(t: 30, score: 5, usable: true), Cand(t: 90, score: 9999, usable: true)]
        #expect(AnchorProposal.addAnchorCandidate(overlap: ov, existing: [], candidates: cands) == 30)
        // With no overlap, adds nothing.
        #expect(AnchorProposal.addAnchorCandidate(overlap: nil, existing: [], candidates: cands) == nil)
    }

    // (9) "+ anchor" respects the cap and returns nil when nothing qualifies (degenerate
    //     candidates, or all too close) — never an invalid add.
    @Test func addAnchorCapAndNoQualifyingReturnsNil() {
        let ov = 0.0...100.0
        let usable = [Cand(t: 50, score: 9, usable: true)]
        #expect(AnchorProposal.addAnchorCandidate(overlap: ov, existing: [10, 30, 60, 90],
                                                  candidates: usable) == nil) // at cap (4)
        // Only candidate is unusable ⇒ nil.
        let unusable = [Cand(t: 50, score: 9, usable: false)]
        #expect(AnchorProposal.addAnchorCandidate(overlap: ov, existing: [], candidates: unusable) == nil)
    }

    // (10) The count is ALWAYS 0, 2, or 3 — never exactly one — across a sweep of overlap
    //      lengths and candidate densities (the core invariant).
    @Test func proposalCountIsNeverExactlyOne() {
        for len in stride(from: 0.0, through: 130.0, by: 2.5) {
            let ov = 0.0...max(len, 0.0001)
            for step in [0.5, 3.0, 17.0] {
                let picks = AnchorProposal.propose(overlap: len == 0 ? nil : ov,
                                                   candidates: usableOnsets(0, len, step: step))
                #expect(picks.count != 1)
                #expect(inside(picks, ov))
            }
        }
    }
}

// MARK: - TODO-005 unified activity strip (pure summary policy)

/// `ActivitySummaryPolicy.summarize` is the PURE core behind the unified activity
/// strip: given a snapshot of the existing job signals + how long the foreground
/// operation has run, it returns the single honest line (text + optional fraction) the
/// strip shows, or nil for "show nothing". These media-free tests lock down its honesty
/// contract — pairing is always percentage-free, import/export are determinate, queued
/// sync shows a position, idle shows nothing, a failure is an error (no bar), the
/// ~250 ms debounce suppresses flashes, and a determinate fraction is monotonic within
/// a phase. The live wiring (timer + per-phase floor in ActivityStrip) is exercised in
/// the app via these same decisions.
@Suite struct ActivitySummaryTests {
    // Past the debounce so non-error lines appear.
    private let shown = ActivitySummaryPolicy.debounceMs + 10

    // (idle) No active signal ⇒ no strip, regardless of elapsed time.
    @Test func idleShowsNothing() {
        #expect(ActivitySummaryPolicy.summarize(ActivitySignals(), elapsedMs: 99999) == nil)
    }

    // (debounce) An active job younger than the debounce flashes nothing; at/after the
    // debounce it appears. (200–300 ms window; midpoint 250.)
    @Test func debounceSuppressesQuickWork() {
        let s = ActivitySignals(importing: .init(phase: .decoding, fraction: 0.4))
        #expect(ActivitySummaryPolicy.summarize(s, elapsedMs: 100) == nil)         // too quick
        #expect(ActivitySummaryPolicy.summarize(s, elapsedMs: 249) == nil)         // just under
        #expect(ActivitySummaryPolicy.summarize(s, elapsedMs: 250) != nil)         // at boundary
        #expect(ActivitySummaryPolicy.summarize(s, elapsedMs: 5000) != nil)        // well after
    }

    // (pairing ⇒ indeterminate, the central honesty rule) Counts, NEVER a percentage.
    @Test func pairingIsIndeterminateWithCounts() {
        // First analysis with both kinds present ⇒ "Comparing V videos with A audio files".
        let comparing = ActivitySignals(pairing: .init(videoCount: 4, audioCount: 8))
        let c = ActivitySummaryPolicy.summarize(comparing, elapsedMs: shown)
        #expect(c?.fraction == nil)                                  // no fabricated %
        #expect(c?.isError == false)
        #expect(c?.text == "Comparing 4 videos with 8 audio files…")
        #expect(c?.text.contains("%") == false)

        // Singular grammar.
        let one = ActivitySignals(pairing: .init(videoCount: 1, audioCount: 1))
        #expect(ActivitySummaryPolicy.summarize(one, elapsedMs: shown)?.text
                == "Comparing 1 video with 1 audio file…")

        // Only one role present ⇒ the generic "Analyzing N files for matches".
        let analyzing = ActivitySignals(pairing: .init(videoCount: 0, audioCount: 12))
        #expect(ActivitySummaryPolicy.summarize(analyzing, elapsedMs: shown)?.text
                == "Analyzing 12 files for matches…")

        // Resolve sub-phase ⇒ "Resolving matches…", still no percentage.
        let resolving = ActivitySignals(pairing: .init(videoCount: 4, audioCount: 8, resolving: true))
        let r = ActivitySummaryPolicy.summarize(resolving, elapsedMs: shown)
        #expect(r?.text == "Resolving matches…")
        #expect(r?.fraction == nil)

        // A re-resolve after a later import reads the "Updating…" line (no %).
        let update = ActivitySignals(pairing: .init(videoCount: 2, audioCount: 3, afterImport: true))
        let u = ActivitySummaryPolicy.summarize(update, elapsedMs: shown)
        #expect(u?.text == "Updating matches after import…")
        #expect(u?.fraction == nil)
    }

    // (import ⇒ determinate fraction + phase text) honest phase verbs + the %.
    @Test func importIsDeterminateWithPhaseText() {
        let decoding = ActivitySignals(importing:
            .init(phase: .decoding, fraction: 0.42, index: 3, total: 12, filename: "clip.mov"))
        let d = ActivitySummaryPolicy.summarize(decoding, elapsedMs: shown)
        #expect(d?.fraction == 0.42)                                 // trustworthy fraction passes through
        #expect(d?.text == "Decoding audio 3 of 12: clip.mov — 42%")
        #expect(d?.phaseKey == "import.decoding")

        // Waveform phase verb.
        let wave = ActivitySignals(importing: .init(phase: .waveform, fraction: 0.68))
        #expect(ActivitySummaryPolicy.summarize(wave, elapsedMs: shown)?.text == "Building waveform — 68%")

        // Finishing phase verb.
        let fin = ActivitySignals(importing: .init(phase: .finishing, fraction: 0.95))
        #expect(ActivitySummaryPolicy.summarize(fin, elapsedMs: shown)?.text == "Finishing import — 95%")

        // Probe sub-phase has no trustworthy fraction ⇒ shown INDETERMINATE (no %).
        let probe = ActivitySignals(importing:
            .init(phase: .probing, fraction: 0, index: 1, total: 1, filename: "a.wav"))
        let p = ActivitySummaryPolicy.summarize(probe, elapsedMs: shown)
        #expect(p?.fraction == nil)
        #expect(p?.text == "Inspecting a.wav…")
    }

    // (queued sync ⇒ "couple X of N" position text) indeterminate at the summary level.
    @Test func queuedSyncShowsPosition() {
        let s = ActivitySignals(syncing: .init(index: 2, total: 7, videoName: "video.mov"))
        let y = ActivitySummaryPolicy.summarize(s, elapsedMs: shown)
        #expect(y?.text == "Syncing couple 2 of 7: video.mov")
        #expect(y?.fraction == nil)                                  // summary-level indeterminate
        #expect(y?.isError == false)
        // No name available ⇒ still a valid position line.
        let noName = ActivitySignals(syncing: .init(index: 1, total: 3))
        #expect(ActivitySummaryPolicy.summarize(noName, elapsedMs: shown)?.text == "Syncing couple 1 of 3")
    }

    // (export ⇒ determinate) row position + per-row % ; preflight + finishing edges.
    @Test func exportIsDeterminate() {
        let e = ActivitySignals(exporting: .init(index: 2, total: 5, rowFraction: 0.61))
        let x = ActivitySummaryPolicy.summarize(e, elapsedMs: shown)
        #expect(x?.fraction == 0.61)
        #expect(x?.text == "Exporting 2 of 5 — 61%")
        #expect(x?.phaseKey == "export.2")

        // With a filename.
        let named = ActivitySignals(exporting: .init(index: 1, total: 2, rowFraction: 0.1, filename: "a-synced.mov"))
        #expect(ActivitySummaryPolicy.summarize(named, elapsedMs: shown)?.text == "Exporting 1 of 2: a-synced.mov — 10%")

        // Preflight before the first row ⇒ "Preparing export…", indeterminate.
        let pre = ActivitySignals(exporting: .init(index: 0, total: 3, rowFraction: 0, preflight: true))
        let pf = ActivitySummaryPolicy.summarize(pre, elapsedMs: shown)
        #expect(pf?.text == "Preparing export…")
        #expect(pf?.fraction == nil)

        // A row at 100% reads "Finishing export…" rather than a stuck full bar.
        let done = ActivitySignals(exporting: .init(index: 3, total: 3, rowFraction: 1.0))
        #expect(ActivitySummaryPolicy.summarize(done, elapsedMs: shown)?.text == "Finishing export…")
    }

    // (failure ⇒ error state, NOT a stuck bar) shown immediately (skips the debounce),
    // carries no fraction, and flags isError so the view drops the bar.
    @Test func failureIsErrorNotStuckBar() {
        let s = ActivitySignals(failure: "2 exported, 1 failed — disk full")
        // Immediate: even at elapsed 0 (well under the debounce) the error shows.
        let f = ActivitySummaryPolicy.summarize(s, elapsedMs: 0)
        #expect(f?.isError == true)
        #expect(f?.fraction == nil)
        #expect(f?.text == "2 exported, 1 failed — disk full")
        // A failure wins even when other work is concurrently active (act on it first).
        let concurrent = ActivitySignals(
            importing: .init(phase: .decoding, fraction: 0.5),
            failure: "import failed: bad file")
        #expect(ActivitySummaryPolicy.summarize(concurrent, elapsedMs: shown)?.isError == true)
    }

    // (monotonic within a phase) a regressing fraction is clamped UP to the floor the
    // app already showed for the SAME phase; a phase change is free to drop.
    @Test func determinateFractionIsMonotonicWithinPhase() {
        let s = ActivitySignals(importing: .init(phase: .decoding, fraction: 0.30))
        // Floor 0.55 from earlier in the same phase ⇒ never regress below it.
        #expect(ActivitySummaryPolicy.summarize(s, elapsedMs: shown, phaseFloor: 0.55)?.fraction == 0.55)
        // A higher reported fraction wins over the floor.
        let ahead = ActivitySignals(importing: .init(phase: .decoding, fraction: 0.80))
        #expect(ActivitySummaryPolicy.summarize(ahead, elapsedMs: shown, phaseFloor: 0.55)?.fraction == 0.80)
        // No floor (phase just changed) ⇒ the raw fraction, free to be small.
        #expect(ActivitySummaryPolicy.summarize(s, elapsedMs: shown, phaseFloor: nil)?.fraction == 0.30)
    }

    // (out-of-range fractions clamp to 0…1 so the bar can never exceed 100% / go negative.
    @Test func fractionsClampToUnitRange() {
        let over = ActivitySignals(exporting: .init(index: 1, total: 1, rowFraction: 1.4))
        // 1.4 clamps to 1.0 ⇒ the "Finishing export…" end state.
        #expect(ActivitySummaryPolicy.summarize(over, elapsedMs: shown)?.fraction == 1.0)
        let under = ActivitySignals(importing: .init(phase: .decoding, fraction: -0.2))
        #expect(ActivitySummaryPolicy.summarize(under, elapsedMs: shown)?.fraction == 0.0)
    }

    // (priority when several run) the SUMMARY shows exactly one line, most user-relevant
    // first: export ▸ import ▸ pairing ▸ sync (a failure, tested above, precedes all).
    @Test func priorityPicksTheMostRelevantSingleLine() {
        let all = ActivitySignals(
            importing: .init(phase: .decoding, fraction: 0.5),
            pairing: .init(videoCount: 2, audioCount: 2),
            syncing: .init(index: 1, total: 2),
            exporting: .init(index: 1, total: 1, rowFraction: 0.3))
        #expect(ActivitySummaryPolicy.summarize(all, elapsedMs: shown)?.text == "Exporting 1 of 1 — 30%")

        let noExport = ActivitySignals(
            importing: .init(phase: .decoding, fraction: 0.5),
            pairing: .init(videoCount: 2, audioCount: 2),
            syncing: .init(index: 1, total: 2))
        #expect(ActivitySummaryPolicy.summarize(noExport, elapsedMs: shown)?.text == "Decoding audio — 50%")

        let pairThenSync = ActivitySignals(
            pairing: .init(videoCount: 2, audioCount: 2),
            syncing: .init(index: 1, total: 2))
        #expect(ActivitySummaryPolicy.summarize(pairThenSync, elapsedMs: shown)?.text
                == "Comparing 2 videos with 2 audio files…")
    }

    // hasActiveWork mirrors "is there something to show?" so the controller arms/clears
    // its debounce correctly.
    @Test func hasActiveWorkReflectsAnySignal() {
        #expect(ActivitySignals().hasActiveWork == false)
        #expect(ActivitySignals(pairing: .init(videoCount: 1, audioCount: 1)).hasActiveWork == true)
        #expect(ActivitySignals(failure: "x").hasActiveWork == true)
    }
}
