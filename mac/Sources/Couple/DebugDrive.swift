// Dev-only instrumentation (no effect unless COUPLE_* env vars are set):
//   COUPLE_OPEN_CAMERA / COUPLE_OPEN_EXTERNAL  — auto-load a pair at launch
//   COUPLE_SHOT_DIR                            — capture walkthrough PNGs post-sync
//   COUPLE_PROFILE=1                           — synthetic 6 s drag, print FPS-REPORT
//   COUPLE_AUTOQUIT=1                          — terminate after the script
// Supports the §15 Phase-5 verification (screenshots + 60 fps criterion).

import AppKit
import CoupleKit
import Foundation
import QuartzCore

/// Per-surface draw counters, recorded from Canvas draw closures.
final class FrameStats: @unchecked Sendable {
    static let shared = FrameStats()
    private let lock = NSLock()
    private var counts: [String: Int] = [:]
    private var totals: [String: CFTimeInterval] = [:]
    private var maxes: [String: CFTimeInterval] = [:]

    func record(_ surface: String, seconds: CFTimeInterval) {
        lock.lock()
        counts[surface, default: 0] += 1
        totals[surface, default: 0] += seconds
        maxes[surface] = max(maxes[surface, default: 0], seconds)
        lock.unlock()
    }

    func reset() {
        lock.lock()
        counts = [:]
        totals = [:]
        maxes = [:]
        lock.unlock()
    }

    func report(window: CFTimeInterval) -> String {
        lock.lock()
        defer { lock.unlock() }
        return counts.keys.sorted().map { k in
            let n = counts[k]!
            let mean = totals[k]! / Double(n) * 1000
            let mx = maxes[k]! * 1000
            return String(format: "%@: %d draws (%.1f/s), mean %.3f ms, max %.3f ms",
                          k, n, Double(n) / window, mean, mx)
        }.joined(separator: "\n")
    }
}

final class DisplayDriver: NSObject {
    var tick: (() -> Void)?
    @objc func fire() {
        tick?()
    }
}

@MainActor
final class DebugDrive {
    static let shared = DebugDrive()
    private var model: AppModel?
    private var started = false

    func configure(model: AppModel) {
        self.model = model
        guard !started else { return }
        started = true
        let env = ProcessInfo.processInfo.environment
        guard env["COUPLE_OPEN_CAMERA"] != nil || env["COUPLE_PAIR_DIR"] != nil
            || env["COUPLE_PROFILE"] != nil || env["COUPLE_RESTORE_TEST"] != nil
            || env["COUPLE_RMS_TEST"] != nil || env["COUPLE_VERIFY_DUMP"] != nil else { return }
        Task { await run(env: env) }
    }

    private func run(env: [String: String]) async {
        guard let model else { return }
        try? await Task.sleep(nanoseconds: 800_000_000) // window up

        // Media-free pure-predicate tests run first and, when nothing else is
        // requested, exit without entering the (media-gated) resolve/sync wait.
        if env["COUPLE_RMS_TEST"] == "1" {
            await rmsTest()
            let mediaRequested = env["COUPLE_OPEN_CAMERA"] != nil || env["COUPLE_PAIR_DIR"] != nil
                || env["COUPLE_PROFILE"] != nil || env["COUPLE_RESTORE_TEST"] != nil
            if !mediaRequested {
                if env["COUPLE_AUTOQUIT"] == "1" { print("DEBUG-DRIVE: done, quitting"); NSApp.terminate(nil) }
                return
            }
        }

        if let dir = env["COUPLE_SHOT_DIR"] {
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            capture(to: "\(dir)/00-launch-empty.png")
        }
        // v2: everything imports into the media pool, then resolves into couples.
        // COUPLE_PAIR_DIR drives the whole-session queue gates; the single
        // camera+external pair behaves like v1 (one couple, auto-paired, opened).
        var dropURLs: [URL] = []
        if let dir = env["COUPLE_PAIR_DIR"] { dropURLs.append(URL(fileURLWithPath: dir)) }
        if let cam = env["COUPLE_OPEN_CAMERA"] { dropURLs.append(URL(fileURLWithPath: cam)) }
        if let ext = env["COUPLE_OPEN_EXTERNAL"] { dropURLs.append(URL(fileURLWithPath: ext)) }
        if !dropURLs.isEmpty { model.importDropped(dropURLs) }

        // Wait for resolution + the selected couple's sync (success or failure),
        // up to 3 min.
        for _ in 0..<1800 {
            if let c = model.selectedCouple, !model.resolving {
                if c.pair.syncActivity == .succeeded { break }
                if case .failed = c.pair.syncActivity { break }
            }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        try? await Task.sleep(nanoseconds: 700_000_000) // settle/render

        if let dir = env["COUPLE_SHOT_DIR"] {
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            await shoot(dir: dir)
        }
        if env["COUPLE_PROFILE"] == "1" {
            await profile()
        }
        if env["COUPLE_EVENT_TEST"] == "1" {
            await eventTest()
        }
        if env["COUPLE_E2E"] == "1" {
            await endToEnd(env: env)
        }
        if env["COUPLE_QUEUE_TEST"] == "1" {
            await queueTest(env: env)
        }
        if env["COUPLE_VERIFY_DUMP"] == "1" {
            await verifyDump()
        }
        if env["COUPLE_RESTORE_TEST"] == "1" {
            await restoreTest()
        }
        if env["COUPLE_AUTOQUIT"] == "1" {
            print("DEBUG-DRIVE: done, quitting")
            NSApp.terminate(nil)
        }
    }

    private func shoot(dir: String) async {
        guard let model else { return }
        let pair = model.pair

        capture(to: "\(dir)/01-post-sync-side-by-side.png")
        await settle()

        pair.setLoupeLayout(.stack)
        await settle()
        capture(to: "\(dir)/02-post-sync-stacked.png")

        pair.setLoupeLayout(.grid)
        await settle()

        // Misaligned state: nudge far enough for guidance arrows.
        let savedOffset = pair.offsetMs
        let savedDrift = pair.driftPpm
        pair.commitTypedOffset(savedOffset + 60)
        await settle()
        capture(to: "\(dir)/03-nudged-guidance-arrow.png")

        // ⇥ align rescue: align anchor 1 (plain offset), then the far anchor
        // (pivot math) — should restore the mapping to ≈ the synced values.
        pair.alignAnchor(0)
        await settle()
        if !pair.anchors.isEmpty {
            pair.alignAnchor(pair.anchors.count - 1)
        }
        await settle()
        print(String(format: "DEBUG-DRIVE: align rescue offset %.3f (was %.3f), drift %.3f (was %.3f)",
                     pair.offsetMs, savedOffset, pair.driftPpm, savedDrift))
        capture(to: "\(dir)/06-after-align-rescue.png")
        pair.commitTypedOffset(savedOffset)
        pair.commitTypedDrift(savedDrift)
        await settle()

        // Deep zoom exercises the raw-sample loupe paths (§14 6.5): per-pixel
        // min/max bars at 60 ms, connected sample line at 10 ms.
        pair.manualZoom(loupe: 0, factor: 60.0 / (pair.frames.first?.winMs ?? 240))
        await settle()
        capture(to: "\(dir)/07-deep-zoom-raw-60ms.png")
        pair.manualZoom(loupe: 0, factor: 10.0 / (pair.frames.first?.winMs ?? 60))
        await settle()
        capture(to: "\(dir)/08-deep-zoom-line-10ms.png")
        pair.armAuto(loupe: 0)
        await settle()

        // Tall window: extra height should grow the strip / video preview.
        if let win = NSApp.windows.first(where: { $0.isVisible }) {
            let saved = win.frame
            win.setContentSize(NSSize(width: 1280, height: 1000))
            await settle()
            capture(to: "\(dir)/09-tall-window.png")
            win.setFrame(saved, display: true)
            await settle()
        }

        // Drift mode None (greyed drift group, uncorrected summary).
        pair.setDriftMode(.none)
        await settle()
        capture(to: "\(dir)/04-drift-mode-none.png")
        pair.setDriftMode(.resample)
        await settle()
        capture(to: "\(dir)/05-final.png")
        print("DEBUG-DRIVE: screenshots written to \(dir)")
    }

    private func settle() async {
        try? await Task.sleep(nanoseconds: 600_000_000)
    }

    // MARK: - Phase-6 end-to-end: audition playback + skew, then shell export

    private func endToEnd(env: [String: String]) async {
        guard let model, model.pair.session != nil else {
            print("E2E: no session, skipped")
            return
        }
        let pair = model.pair
        print("E2E: begin (sync state: \(pair.syncActivity))")

        // --- Shell export of the real pair first (deterministic gate); hash
        // must match the Phase-4 CLI export.
        if !pair.verified {
            pair.toggleVerified()
        }
        pair.exportFormat = .auto
        pair.keepCameraAudio = false
        let out = URL(fileURLWithPath: "/tmp/couple-shell-export.\(pair.exportFileExtension)")
        try? FileManager.default.removeItem(at: out)
        print("E2E: export starting → \(out.path)")
        pair.runExport(to: out)
        for _ in 0..<1200 {
            if pair.exportActivity == .idle {
                break
            }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        if FileManager.default.fileExists(atPath: out.path) {
            let sha = Process()
            sha.executableURL = URL(fileURLWithPath: "/usr/bin/shasum")
            sha.arguments = ["-a", "256", out.path]
            let pipe = Pipe()
            sha.standardOutput = pipe
            try? sha.run()
            sha.waitUntilExit()
            let hashLine = String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
            let hash = hashLine.split(separator: " ").first.map(String.init) ?? "?"
            let expected = env["COUPLE_E2E_EXPECTED_SHA"] ?? ""
            let verdict = expected.isEmpty ? "(no expected hash provided)"
                : (hash == expected ? "MATCHES Phase-4 CLI hash — PASS" : "DIFFERS from Phase-4 CLI hash — FAIL")
            print("E2E-EXPORT: \(out.lastPathComponent) sha256 \(hash) \(verdict)")
        } else {
            print("E2E-EXPORT: FAIL (no output file)")
        }

        // --- Audition: play from t=60, measure A/V skew at 1 Hz (§11.4 budget 33 ms).
        pair.audition.monitorMode = .overlay
        print("E2E: audition opening…")
        pair.audition.play(from: 60)
        for _ in 0..<100 { // engine bring-up is async with a 5 s HAL timeout
            if pair.audition.isPlaying || pair.audition.audioUnavailable {
                break
            }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        print("E2E: audition playing=\(pair.audition.isPlaying) unavailable=\(pair.audition.audioUnavailable)")
        if pair.audition.audioUnavailable {
            print("E2E-AUDITION: SKIPPED — audio output unavailable in this session; owner: run the skew test live")
            return
        }
        var skews: [Double] = []
        var playheads: [Double] = []
        for _ in 0..<16 {
            try? await Task.sleep(nanoseconds: 1_000_000_000)
            playheads.append(pair.audition.playheadT)
            print(String(format: "E2E: t=%.2f skew=%.1f ms", pair.audition.playheadT, pair.audition.lastSkewMs))
            skews.append(abs(pair.audition.lastSkewMs))
        }
        // §11.4 acceptance is SUSTAINED skew ≤ 1 frame; evaluate the tail
        // after the startup transient has converged.
        skews = Array(skews.suffix(8))
        // Mode/gain switches mid-playback must not stall the stream.
        pair.audition.monitorMode = .pan
        try? await Task.sleep(nanoseconds: 700_000_000)
        pair.audition.monitorMode = .overlay
        pair.audition.camGain = 1.5
        pair.audition.extMuted = true
        try? await Task.sleep(nanoseconds: 700_000_000)
        pair.audition.camGain = 1.0
        pair.audition.extMuted = false
        let advancing = zip(playheads, playheads.dropFirst()).allSatisfy { $1 > $0 }
        let endT = pair.audition.playheadT
        pair.audition.pause()
        let maxSkew = skews.max() ?? .nan
        print(String(format: "E2E-AUDITION: playhead advanced %@ (60.0 → %.2f s); max |skew| after convergence %.1f ms (budget 33) — %@",
                     advancing ? "ok" : "STALLED", endT, maxSkew,
                     maxSkew <= 33 ? "PASS" : "FAIL"))
    }

    // MARK: - Queue gates (SPEC-V2 §7 shell phase): resolution parity, queue
    // triage events (j/k/v/Enter), and the byte-identical batch export keystone.

    private func queueTest(env: [String: String]) async {
        guard let model else { return }
        // Wait for resolution + every couple's background sync to settle.
        for _ in 0..<3000 {
            let pending = model.couples.contains { c in
                c.pair.session != nil && (c.pair.syncActivity == .none || c.pair.isSyncRunning)
            }
            if !model.resolving && !pending { break }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }

        func verdictName(_ v: Verdict) -> String {
            switch v {
            case .good: return "good"
            case .questionable: return "questionable"
            case .none: return "none"
            }
        }
        func section(_ c: CoupleRow) -> String {
            if c.approved { return "QUEUED" }
            return c.verdict == .good ? "TO-APPROVE" : "TO-FIX"
        }
        print("QUEUE-TEST: \(model.couples.count) couples · \(model.pool.videos.count) video · \(model.pool.audios.count) audio")
        for c in model.couples {
            let sel = c.selectedCandidate?.audioName ?? "—"
            let cov = c.selectedCandidate.map { String(format: "%.1f%%", $0.coverage.fraction * 100) } ?? "—"
            print("  \(c.video.name) [\(c.state.rawValue)] -> \(sel) (cov \(cov)) verdict=\(verdictName(c.verdict)) §\(section(c))")
            for a in c.alternates {
                let fit = a.fitRmsMs.map { String(format: " · fit %.1f ms (%d win)", $0, a.fitWindows ?? 0) } ?? ""
                print("      alt: \(a.audioName)\(fit)")
            }
        }
        let used = Set(model.couples.compactMap { $0.selectedAudioKey })
        let unused = model.pool.audios.filter { !used.contains($0.id) }.map(\.name)
        print("QUEUE-TEST unused: \(unused.joined(separator: ", "))")
        print("QUEUE-DEST: \(model.exportDestination?.lastPathComponent ?? "nil")")

        if let dir = env["COUPLE_QUEUE_SHOT_DIR"] {
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            if let good = model.toApprove.first { model.select(good.id) }
            model.syncToolsExpanded = true
            await settle()
            capture(to: "\(dir)/v2-01-queue.png")
            model.loupesShown = true
            await settle()
            capture(to: "\(dir)/v2-02-loupes.png")
            model.loupesShown = false
            if let amb = model.toFix.first(where: { $0.needsResolve }) { model.select(amb.id) }
            await settle()
            capture(to: "\(dir)/v2-03-resolve-banner.png")
            model.syncToolsExpanded = false
            // Export rail with approved couples: in-place filenames, no template,
            // global format only (owner 2026-06-13).
            for c in model.couples where c.verdict == .good && c.pair.session != nil && !c.approved {
                c.pair.toggleVerified()
            }
            if let good = model.queued.first { model.select(good.id) }
            await settle()
            capture(to: "\(dir)/v2-04-export.png")
            print("QUEUE-SHOT: wrote screenshots to \(dir)")
        }

        await queueNavTest()

        // Persist the queue (§5.4) so a restore can be exercised: approve the
        // good couples through the model path (which saves), then write the file.
        for c in model.couples where c.verdict == .good && c.pair.session != nil && !c.approved {
            model.toggleApprove(c)
        }
        model.saveQueue()
        print("QUEUE-SAVE: queue.json \(model.couples.count) couples, \(model.approvedCount) approved")

        guard let outDir = env["COUPLE_BATCH_OUT"] else { return }
        try? FileManager.default.createDirectory(atPath: outDir, withIntermediateDirectories: true)
        for c in model.couples where c.pair.session != nil && !c.approved { c.pair.toggleVerified() }
        model.exportDestination = URL(fileURLWithPath: outDir)
        model.globalFormat = .auto
        model.globalKeepCameraAudio = false
        print("QUEUE-TEST: batch exporting \(model.approvedCount) couple(s) → \(outDir)")
        model.batch.run()
        for _ in 0..<6000 {
            if case .finished = model.batch.phase { break }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        let expected = env["COUPLE_E2E_EXPECTED_SHA"] ?? ""
        for c in model.couples {
            if case .done(let url) = c.exportStatus {
                let hash = sha256(url)
                let verdict = (expected.isEmpty || model.couples.count != 1) ? ""
                    : (hash == expected ? "  MATCHES Phase-4 CLI — PASS" : "  DIFFERS — FAIL")
                print("QUEUE-EXPORT: \(c.video.name) -> \(url.lastPathComponent) sha256 \(hash)\(verdict)")
            } else if case .failed(let msg) = c.exportStatus {
                print("QUEUE-EXPORT: \(c.video.name) FAILED: \(msg)")
            }
        }
    }

    /// j/k/v/Enter over the queue (UX §2.9 extension to v2; SPEC-V2 §7).
    private func queueNavTest() async {
        guard let model, model.couples.count >= 2,
              let win = NSApp.windows.first(where: { $0.isVisible }) else { return }
        NSApp.activate(ignoringOtherApps: true)
        win.makeKeyAndOrderFront(nil)
        try? await Task.sleep(nanoseconds: 300_000_000)
        let pid = ProcessInfo.processInfo.processIdentifier
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("QUEUE-EVENT \(ok ? "ok  " : "FAIL") \(name)")
        }
        func key(_ code: CGKeyCode) async {
            for down in [true, false] {
                if let e = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down) {
                    e.postToPid(pid)
                }
            }
            try? await Task.sleep(nanoseconds: 160_000_000)
        }
        model.select(model.orderedCouples.first?.id)
        try? await Task.sleep(nanoseconds: 120_000_000)
        let first = model.selectedID
        await key(38) // j
        check("j → next couple", model.selectedID != nil && model.selectedID != first)
        await key(40) // k
        check("k → previous couple", model.selectedID == first)
        let expanded = model.syncToolsExpanded
        await key(36) // Return
        check("Return toggles sync tools", model.syncToolsExpanded != expanded)
        await key(36)
        // v approves the selected couple (if it can be) and advances.
        if let sel = model.selectedCouple, sel.pair.session != nil {
            let id = sel.id
            let was = sel.approved
            await key(9) // v
            check("v approves (= queue) the selected couple",
                  model.couples.first { $0.id == id }?.approved != was)
            // un-approve it again so the batch below starts clean.
            if let c = model.couples.first(where: { $0.id == id }), c.approved { c.pair.toggleVerified() }
        }
        print("QUEUE-EVENT summary: \(pass) passed, \(fail) failed")
    }

    /// Restore round-trip (§5.4): load queue.json, re-import + re-resolve + overlay
    /// the saved selections/approvals, and print the restored queue.
    private func restoreTest() async {
        guard let model else { return }
        guard let saved = QueueStore.load() else {
            print("RESTORE-TEST: no saved queue.json")
            return
        }
        print("RESTORE-TEST: restoring \(saved.couples.count) couples (\(saved.couples.filter(\.approved).count) approved) from queue.json")
        model.applyRestore(saved)
        for _ in 0..<3000 {
            let settled = !model.resolving && model.couples.count == saved.couples.count
                && !model.couples.contains { $0.pair.session != nil
                    && ($0.pair.syncActivity == .none || $0.pair.isSyncRunning) }
            if settled { break }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        print("RESTORE-TEST: \(model.couples.count) couples, \(model.approvedCount) approved")
        for c in model.couples {
            print("  \(c.video.name) [\(c.state.rawValue)] -> \(c.selectedCandidate?.audioName ?? "—") approved=\(c.approved)")
        }
    }

    private func sha256(_ url: URL) -> String {
        let sha = Process()
        sha.executableURL = URL(fileURLWithPath: "/usr/bin/shasum")
        sha.arguments = ["-a", "256", url.path]
        let pipe = Pipe()
        sha.standardOutput = pipe
        try? sha.run()
        sha.waitUntilExit()
        let line = String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        return line.split(separator: " ").first.map(String.init) ?? "?"
    }

    // MARK: - Synthetic input events (verifies the UX §2.9 map end to end)

    private func eventTest() async {
        guard let model, let win = NSApp.windows.first(where: { $0.isVisible }) else { return }
        let pair = model.pair
        NSApp.activate(ignoringOtherApps: true)
        win.makeKeyAndOrderFront(nil)
        try? await Task.sleep(nanoseconds: 400_000_000)
        let pid = ProcessInfo.processInfo.processIdentifier
        var pass = 0
        var fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("EVENT-TEST \(ok ? "ok " : "FAIL") \(name)")
        }
        func key(_ code: CGKeyCode, _ flags: CGEventFlags = []) async {
            for down in [true, false] {
                guard let e = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down) else { continue }
                e.flags = flags
                e.postToPid(pid)
            }
            try? await Task.sleep(nanoseconds: 150_000_000)
        }

        var o = pair.offsetMs
        await key(124) // → moves external content right = −offset
        check("arrow right −1 ms (content follows arrow)", abs(pair.offsetMs - (o - 1)) < 1e-9)
        o = pair.offsetMs
        await key(123, .maskShift) // ⇧←
        check("shift-left +10 ms", abs(pair.offsetMs - (o + 10)) < 1e-9)
        o = pair.offsetMs
        await key(124, .maskAlternate) // ⌥→
        check("option-right −0.1 ms", abs(pair.offsetMs - (o - 0.1)) < 1e-9)
        var d = pair.driftPpm
        await key(30) // ]
        check("] drift +1 ppm", abs(pair.driftPpm - (d + 1)) < 1e-9)
        d = pair.driftPpm
        await key(33, .maskAlternate) // ⌥[
        check("option-[ drift −0.1 ppm", abs(pair.driftPpm - (d - 0.1)) < 1e-9)
        let v = pair.verified
        await key(9) // v
        check("v toggles verified", pair.verified != v)
        await key(9)
        o = pair.offsetMs
        d = pair.driftPpm
        await key(38) // j
        await key(40) // k
        await key(36) // return
        await key(49) // space
        check("j/k/return/space are no-ops", pair.offsetMs == o && pair.driftPpm == d)

        // Zoom routing (position-routed CGEvents aren't deliverable to a
        // background session, so the routing function is exercised directly;
        // the raw scroll/drag wiring is on the owner's manual walkthrough).
        if let rect = model.loupeRects[0] {
            let before = pair.frames.map(\.winMs)
            let routed = model.zoom(at: CGPoint(x: rect.midX, y: rect.midY), deltaY: 120, precise: true)
            let after = pair.frames.map(\.winMs)
            check("zoom routes to loupe under cursor, zooms all (locked), exits auto",
                  routed && after != before && Set(after).count == 1
                  && pair.frames.allSatisfy { !$0.auto })
            check("zoom outside any loupe is ignored",
                  !model.zoom(at: CGPoint(x: -10, y: -10), deltaY: 120, precise: true))
            pair.armAuto(loupe: 0)
            check("auto chip re-arms all loupes under lock",
                  pair.frames.allSatisfy(\.auto))
        }
        print("EVENT-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// Pure unit tests for the Phase-2 RMS escalation predicate
    /// (PLAN-RMS-TIEBREAKER §6.3 R3). No engine, no media: `rmsEscalate` takes a
    /// fit-lookup closure, so injected `PairFit`s drive every case. Run headless via
    /// COUPLE_RMS_TEST=1 (needs no files).
    private func rmsTest() async {
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("RMS-TEST \(ok ? "ok " : "FAIL") \(name)")
        }
        let params = PairParams() // floor 9 ms, ratio 3, minWindows 6

        // Synthetic candidate + coverage builders. z only sets the cluster order
        // (cluster[0] = best z); coverage seconds drive the comparability guard.
        func corr(_ z: Double, lag: Double = 0) -> PairCorrelation {
            PairCorrelation(z: z, lagMs: lag, ncc: 0, fineNcc: 0, overlapBins: 0,
                            runnerUpZ: 0, refined: true, insufficientOverlap: false)
        }
        func cov(_ seconds: Double) -> Coverage {
            // durVideo == durAudio, lag 0 ⇒ coverage seconds == min(dur, dur).
            Coverage(durVideo: seconds, durAudio: seconds, lagMs: 0)
        }
        func fitOf(_ rms: Double, windows: UInt32) -> PairFit {
            PairFit(offMs: 0, ppm: 0, rmsMs: rms, windows: windows)
        }
        // A two-candidate contested cluster (indices 0,1 = best z, runner-up).
        let c = [(0, corr(20.0)), (1, corr(18.0))]
        let covs = [cov(100), cov(100)] // comparable spans
        let cluster = [0, 1]
        // Lookup factory: fits keyed by candidate index; missing ⇒ nil (didn't lock).
        func lookup(_ m: [Int: PairFit]) -> (Int) -> PairFit? { { m[$0] } }

        // (a) rescue: leader rms 4 / runner-up 22, both ≥6 win, margin<zMargin.
        check("(a) rescue: leader 4 vs runner-up 22 → selects leader",
              Resolver.rmsEscalate(cluster: cluster, c: c, covs: covs,
                  fit: lookup([0: fitOf(4, windows: 12), 1: fitOf(22, windows: 12)]),
                  params: params) == 0)

        // (b) rescue (THE corrected case): runner-up didn't lock (fit nil) → leader.
        check("(b) rescue: runner-up didn't lock (nil fit) → selects leader",
              Resolver.rmsEscalate(cluster: cluster, c: c, covs: covs,
                  fit: lookup([0: fitOf(4, windows: 12)]),
                  params: params) == 0)

        // (b') same, but runner-up locked too few windows ⇒ also supports the pick.
        check("(b') rescue: runner-up windows<min → selects leader",
              Resolver.rmsEscalate(cluster: cluster, c: c, covs: covs,
                  fit: lookup([0: fitOf(4, windows: 12), 1: fitOf(2, windows: 3)]),
                  params: params) == 0)

        // (c) hold: leader 6 / runner-up 9 (ratio 1.5 < 3) → nil (comparably clean).
        check("(c) hold: ratio 1.5 < 3 → nil",
              Resolver.rmsEscalate(cluster: cluster, c: c, covs: covs,
                  fit: lookup([0: fitOf(6, windows: 12), 1: fitOf(9, windows: 12)]),
                  params: params) == nil)

        // (d) hold: leader 12 ms (> floor 9) even if runner-up 40 → nil.
        check("(d) hold: leader 12 ms > floor → nil",
              Resolver.rmsEscalate(cluster: cluster, c: c, covs: covs,
                  fit: lookup([0: fitOf(12, windows: 12), 1: fitOf(40, windows: 12)]),
                  params: params) == nil)

        // (e) guard: leader windows < min → nil (can't trust the leader).
        check("(e) guard: leader windows<min → nil",
              Resolver.rmsEscalate(cluster: cluster, c: c, covs: covs,
                  fit: lookup([0: fitOf(4, windows: 4), 1: fitOf(40, windows: 12)]),
                  params: params) == nil)

        // (e') coverage guard: a non-comparable rival is skipped, so a clean
        // leader with an otherwise-comparable-clean rival on a tiny span still wins.
        check("(e') coverage guard: non-comparable rival skipped → selects leader",
              Resolver.rmsEscalate(cluster: cluster, c: c, covs: [cov(100), cov(50)],
                  fit: lookup([0: fitOf(4, windows: 12), 1: fitOf(5, windows: 12)]),
                  params: params) == 0)

        // (f) never on simultaneous path: a 1-element "cluster" never escalates;
        // and the resolver only calls rmsEscalate from the non-simultaneous branch.
        check("(f) single-element cluster → nil (never fires on simultaneous path)",
              Resolver.rmsEscalate(cluster: [0], c: c, covs: covs,
                  fit: lookup([0: fitOf(4, windows: 12)]),
                  params: params) == nil)

        check("conservative form is the shipped default",
              Resolver.rmsStrongForm == false)

        print("RMS-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// COUPLE_VERIFY_DUMP=1 (PLAN-MATCH-CLASSIFIER §4.1/§4.5): the owner's window onto
    /// the fine-verify signal on real files. Iterates the FULL video×audio matrix and
    /// runs verify on EVERY cell that has a plausible lag, **bypassing the z-gate** — so
    /// the z-rejected short clip (the motivating case, absent from the resolver's
    /// `perVideo` set) is shown. Read-only: changes NO decision; prints to stdout only.
    /// Per (video, audio): z, lag_ms, overlap_s, locked/expected, lock_fraction, rms_ms,
    /// ppm, clamped, is_match.
    private func verifyDump() async {
        guard let model, let engine = model.engine else { print("VERIFY-DUMP: no engine"); return }
        // Wait for the pool import + resolution to settle (the dump needs handles).
        for _ in 0..<1800 {
            if !model.resolving && (!model.pool.videos.isEmpty || !model.pool.audios.isEmpty) { break }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        let params = model.params
        let videos = model.pool.videos
        let audios = model.pool.audios
        print("VERIFY-DUMP: \(videos.count) video(s) × \(audios.count) audio file(s) — FULL matrix, z-gate bypassed")
        guard !audios.isEmpty else { print("VERIFY-DUMP: no audio in pool"); return }

        // FULL matrix over the importable videos (a stripped-audio video has no handle).
        let importable = videos.filter { $0.handle != nil }
        let matrix = engine.pairCandidates(videos: importable.compactMap(\.handle),
                                           audios: audios.compactMap(\.handle)) ?? []

        // Build a verify request for EVERY cell with a plausible lag — NO z-gate. The
        // matrix already holds each cell's coarse z/lag, so the lag is in hand.
        struct Cell { let vi: Int; let ai: Int; let m: PairCorrelation }
        var cells: [Cell] = []
        var pairs: [(TrackHandle, TrackHandle, Double)] = []
        var mrow = 0
        for (vi, v) in videos.enumerated() where v.handle != nil {
            let row = mrow < matrix.count ? matrix[mrow] : []
            mrow += 1
            for (ai, m) in row.enumerated() where !m.insufficientOverlap {
                cells.append(Cell(vi: vi, ai: ai, m: m))
                pairs.append((v.handle!, audios[ai].handle!, m.lagMs))
            }
        }
        guard !pairs.isEmpty else { print("VERIFY-DUMP: no plausibly-overlapping cells"); return }
        let fits = engine.verifyBatch(pairs) ?? Array(repeating: nil, count: pairs.count)

        print("VERIFY-DUMP rows (video x audio | z lag_ms overlap_s locked/expected lock_fraction rms_ms ppm clamped is_match):")
        for (i, cell) in cells.enumerated() {
            let v = videos[cell.vi].name
            let a = audios[cell.ai].name
            guard i < fits.count, let f = fits[i] else {
                // None ⇒ overlap below the absolute hard floor (no candidate possible).
                print(String(format: "  %@ x %@ | z %.1f lag %.0f — below verify_min_overlap_s (%.0f s), no fit",
                             v, a, cell.m.z, cell.m.lagMs, params.verifyMinOverlapS))
                continue
            }
            let match = Resolver.isMatch(f, params: params)
            print(String(format: "  %@ x %@ | z %.1f lag %.0f overlap %.1f locked %d/%d frac %.2f rms %.2f ppm %.0f clamped %@ is_match %@",
                         v, a, cell.m.z, cell.m.lagMs, f.overlapS, f.locked, f.expected,
                         f.lockFraction, f.rmsMs, f.ppm, f.clamped ? "Y" : "n", match ? "YES" : "no"))
        }
        print("VERIFY-DUMP: done")
    }

    private func capture(to path: String) {
        guard let window = NSApp.windows.first(where: { $0.isVisible }),
              let view = window.contentView,
              let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
            print("DEBUG-DRIVE: capture failed for \(path)")
            return
        }
        view.cacheDisplay(in: view.bounds, to: rep)
        if let png = rep.representation(using: .png, properties: [:]) {
            try? png.write(to: URL(fileURLWithPath: path))
        }
    }

    /// Synthetic offset drag: ±15 ms sine at 1.2 Hz for 6 s, mutated at display
    /// rate; reports per-surface draw rate and cost (SPEC §11.2 acceptance).
    private func profile() async {
        guard let model, model.pair.session != nil else {
            print("FPS-REPORT: no session, skipped")
            return
        }
        let pair = model.pair
        let base = pair.offsetMs
        let activity = ProcessInfo.processInfo.beginActivity(
            options: [.userInitiated, .latencyCritical], reason: "drag profiling")
        defer { ProcessInfo.processInfo.endActivity(activity) }
        NSApp.activate(ignoringOtherApps: true)
        if let win = NSApp.windows.first(where: { $0.isVisible }) {
            win.orderFrontRegardless()
            win.makeKey()
            try? await Task.sleep(nanoseconds: 400_000_000)
            let display = CGMainDisplayID()
            let mode = CGDisplayCopyDisplayMode(display)
            print("DEBUG-DRIVE: window visible=\(win.occlusionState.contains(.visible)) key=\(win.isKeyWindow) displayAsleep=\(CGDisplayIsAsleep(display) != 0) nominalHz=\(mode?.refreshRate ?? -1)")
        }
        FrameStats.shared.reset()
        let t0 = CACurrentMediaTime()
        var mutations = 0
        // One mutation per main-runloop pass: SwiftUI coalesces to display rate,
        // so the resulting draws/s is the achievable drag frame rate.
        while CACurrentMediaTime() - t0 < 6 {
            let t = CACurrentMediaTime() - t0
            pair.setOffsetLive(base + 15 * sin(2 * .pi * 1.2 * t))
            mutations += 1
            await Task.yield()
            await MainActor.run {} // let the runloop turn
        }
        let window = CACurrentMediaTime() - t0
        print("FPS-REPORT (\(String(format: "%.1f", window)) s synthetic drag, \(mutations) mutations):")
        print(FrameStats.shared.report(window: window))

        // Display-link drive: mutate once per display refresh — sustained
        // commits make adaptive-sync ramp up, so draws/s is the real drag rate.
        if let win = NSApp.windows.first(where: { $0.isVisible }), let view = win.contentView {
            FrameStats.shared.reset()
            let driver = DisplayDriver()
            let c0 = CACurrentMediaTime()
            var ticks = 0
            driver.tick = {
                let t = CACurrentMediaTime() - c0
                pair.setOffsetLive(base + 15 * sin(2 * .pi * 1.2 * t))
                ticks += 1
            }
            let link = view.displayLink(target: driver, selector: #selector(DisplayDriver.fire))
            link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
            link.add(to: .main, forMode: .common)
            try? await Task.sleep(nanoseconds: 6_000_000_000)
            link.invalidate()
            let cw = CACurrentMediaTime() - c0
            print(String(format: "DISPLAYLINK-REPORT: %d ticks in %.2f s = %.1f Hz", ticks, cw, Double(ticks) / cw))
            print(FrameStats.shared.report(window: cw))
        }
        pair.setOffsetLive(base)
    }
}
