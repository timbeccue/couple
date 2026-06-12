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

        session.free()
        cam.free()
        ext.free()
    }
}
