// Dev-only instrumentation (no effect unless COUPLE_* env vars are set):
//   COUPLE_OPEN_CAMERA / COUPLE_OPEN_EXTERNAL  — auto-load a pair at launch
//   COUPLE_SHOT_DIR                            — capture walkthrough PNGs post-sync
//   COUPLE_PROFILE=1                           — synthetic 6 s drag, print FPS-REPORT
//   COUPLE_AUTOQUIT=1                          — terminate after the script
// Supports the §15 Phase-5 verification (screenshots + 60 fps criterion).

import AppKit
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
            || env["COUPLE_PROFILE"] != nil || env["COUPLE_RESTORE_TEST"] != nil else { return }
        Task { await run(env: env) }
    }

    private func run(env: [String: String]) async {
        guard let model else { return }
        try? await Task.sleep(nanoseconds: 800_000_000) // window up

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
            for a in c.alternates { print("      alt: \(a.audioName)") }
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
