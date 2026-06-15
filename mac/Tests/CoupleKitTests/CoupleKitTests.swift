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
