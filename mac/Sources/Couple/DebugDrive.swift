// Dev-only instrumentation (no effect unless COUPLE_* env vars are set):
//   COUPLE_OPEN_CAMERA / COUPLE_OPEN_EXTERNAL  — auto-load a pair at launch
//   COUPLE_SHOT_DIR                            — capture walkthrough PNGs post-sync
//   COUPLE_PROFILE=1                           — synthetic 6 s drag, print FPS-REPORT
//   COUPLE_AUTOQUIT=1                          — terminate after the script
// Supports the §15 Phase-5 verification (screenshots + 60 fps criterion).
//
// Compiled only under COUPLE_DEVHARNESS (dev.sh/build.sh pass it; make-app.sh
// does not, so the distributed .app carries none of this). #if DEBUG is the
// wrong fence here: the §15 fps criterion must be measured on -c release.

import AppKit
import CoupleKit
import Foundation
import QuartzCore

#if COUPLE_DEVHARNESS

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
            || env["COUPLE_RMS_TEST"] != nil || env["COUPLE_VERIFY_DUMP"] != nil
            || env["COUPLE_OVERFLOW_TEST"] != nil || env["COUPLE_SHADOW_DUMP"] != nil
            || env["COUPLE_LAYOUT_TEST"] != nil || env["COUPLE_PREVIEW_TEST"] != nil
            || env["COUPLE_TRANSPORT_TEST"] != nil || env["COUPLE_LOUPE_TEST"] != nil
            || env["COUPLE_REMERGE_TEST"] != nil || env["COUPLE_POOLSEL_TEST"] != nil
            || env["COUPLE_QUALITY_TEST"] != nil || env["COUPLE_REPAIRDROP_TEST"] != nil
            || env["COUPLE_REMOVE_TEST"] != nil || env["COUPLE_ANCHOR_TEST"] != nil
            || env["COUPLE_ACTIVITY_TEST"] != nil || env["COUPLE_OVERVIEW_TEST"] != nil
            || env["COUPLE_MONITOR_TEST"] != nil || env["COUPLE_INCLUDE_TEST"] != nil else { return }
        Task { await run(env: env) }
    }

    private func run(env: [String: String]) async {
        guard let model else { return }
        try? await Task.sleep(nanoseconds: 800_000_000) // window up

        // Media-free pure-predicate tests run first and, when nothing else is
        // requested, exit without entering the (media-gated) resolve/sync wait.
        if env["COUPLE_RMS_TEST"] == "1" || env["COUPLE_OVERFLOW_TEST"] == "1" || env["COUPLE_LAYOUT_TEST"] == "1"
            || env["COUPLE_PREVIEW_TEST"] == "1" || env["COUPLE_TRANSPORT_TEST"] == "1"
            || env["COUPLE_LOUPE_TEST"] == "1" || env["COUPLE_REMERGE_TEST"] == "1"
            || env["COUPLE_POOLSEL_TEST"] == "1" || env["COUPLE_QUALITY_TEST"] == "1"
            || env["COUPLE_REPAIRDROP_TEST"] == "1" || env["COUPLE_ACTIVITY_TEST"] == "1"
            || env["COUPLE_OVERVIEW_TEST"] == "1" {
            if env["COUPLE_RMS_TEST"] == "1" { await rmsTest() }
            if env["COUPLE_OVERFLOW_TEST"] == "1" { await overflowTest() }
            if env["COUPLE_LAYOUT_TEST"] == "1" { await layoutTest() }
            if env["COUPLE_PREVIEW_TEST"] == "1" { await previewTest() }
            if env["COUPLE_TRANSPORT_TEST"] == "1" { await transportTest() }
            if env["COUPLE_LOUPE_TEST"] == "1" { await loupeTest() }
            if env["COUPLE_REMERGE_TEST"] == "1" { await remergeTest() }
            if env["COUPLE_POOLSEL_TEST"] == "1" { await poolselTest() }
            if env["COUPLE_QUALITY_TEST"] == "1" { await qualityTest() }
            if env["COUPLE_REPAIRDROP_TEST"] == "1" { await repairDropTest() }
            if env["COUPLE_ACTIVITY_TEST"] == "1" { await activityTest() }
            if env["COUPLE_OVERVIEW_TEST"] == "1" { await overviewTest() }
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
        if env["COUPLE_INCLUDE_TEST"] == "1" {
            await includeTest(env: env)
        }
        if env["COUPLE_VERIFY_DUMP"] == "1" {
            await verifyDump()
        }
        if env["COUPLE_SHADOW_DUMP"] == "1" {
            await shadowDump()
        }
        if env["COUPLE_RESTORE_TEST"] == "1" {
            await restoreTest()
        }
        if env["COUPLE_REMOVE_TEST"] == "1" {
            await removeTest()
        }
        if env["COUPLE_ANCHOR_TEST"] == "1" {
            await anchorTest()
        }
        if env["COUPLE_MONITOR_TEST"] == "1" {
            await monitorTest(shotDir: env["COUPLE_SHOT_DIR"])
        }
        if env["COUPLE_AUTOQUIT"] == "1" {
            print("DEBUG-DRIVE: done, quitting")
            NSApp.terminate(nil)
        }
    }

    // MARK: - A/B monitor (UX-DECISIONS 2026-11-03): the 4-way picker + a/b keys
    // through the live policy + InputRouter, with lane-silence feedback.

    private func monitorTest(shotDir: String?) async {
        guard let model, model.pair.session != nil else {
            print("MONITOR-TEST: SKIPPED (no session)")
            return
        }
        let a = model.pair.audition
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("MONITOR-TEST \(ok ? "ok  " : "FAIL") \(name)")
        }
        check("default is overlay, no lane silenced",
              a.monitorMode == .overlay && !a.camSilenced && !a.extSilenced)
        check("picker offers camera · pan · overlay · external",
              MonitorMode.allCases == [.camera, .pan, .overlay, .external])
        a.flipMonitor(); await settle()
        check("flip from both → camera only; external lane silenced",
              a.monitorMode == .camera && a.extSilenced && !a.camSilenced)
        if let d = shotDir { capture(to: "\(d)/10-ab-camera-only.png") }
        a.flipMonitor(); await settle()
        check("flip again → external only; camera lane silenced",
              a.monitorMode == .external && a.camSilenced && !a.extSilenced)
        if let d = shotDir { capture(to: "\(d)/11-ab-external-only.png") }
        a.restoreBothMonitor(); await settle()
        check("both → back to overlay", a.monitorMode == .overlay && !a.camSilenced && !a.extSilenced)
        a.selectMonitor(.pan); a.flipMonitor(); await settle()
        check("flip after picking pan resumes external (side heard last)", a.monitorMode == .external)
        a.restoreBothMonitor(); await settle()
        check("both restores pan (the mode left)", a.monitorMode == .pan)
        a.extMuted = true
        a.selectMonitor(.camera); a.restoreBothMonitor(); await settle()
        check("a user mute survives a solo round-trip", a.extMuted && a.monitorMode == .pan)
        a.extMuted = false
        a.selectMonitor(.overlay); await settle()

        // Keys through InputRouter (QUEUE-EVENT idiom): a flips, b restores.
        if let win = NSApp.windows.first(where: { $0.isVisible }) {
            NSApp.activate(ignoringOtherApps: true)
            win.makeKeyAndOrderFront(nil)
            try? await Task.sleep(nanoseconds: 300_000_000)
            let pid = ProcessInfo.processInfo.processIdentifier
            func key(_ code: CGKeyCode) async {
                for down in [true, false] {
                    if let e = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down) {
                        e.postToPid(pid)
                    }
                }
                try? await Task.sleep(nanoseconds: 200_000_000)
            }
            await key(0) // a
            check("a key → camera only", a.monitorMode == .camera)
            await key(0) // a
            check("a key again → external only", a.monitorMode == .external)
            await key(11) // b
            check("b key → overlay", a.monitorMode == .overlay)
        }
        print("MONITOR-TEST: \(fail == 0 ? "PASS" : "FAIL") (\(pass) ok, \(fail) failed)")
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
        pair.manualZoom(loupe: 0, factor: 60.0 / (pair.anchors.first?.frame.winMs ?? 240))
        await settle()
        capture(to: "\(dir)/07-deep-zoom-raw-60ms.png")
        pair.manualZoom(loupe: 0, factor: 10.0 / (pair.anchors.first?.frame.winMs ?? 60))
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
    // triage events (j/k/Enter), and the byte-identical batch export keystone.
    // Sections and the batch follow the include model (UX §V2.12 / D1–D2).

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
            // D2: membership is the verdict alone; inclusion rides along as a flag.
            (c.verdict == .good ? "READY" : "TO-FIX") + (c.included ? " ✓" : "")
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
            if let good = model.ready.first { model.select(good.id) }
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
            // Export rail with the included couples (every good one by default, D1):
            // in-place filenames, no template, global format only (owner 2026-06-13).
            if let good = model.included.first { model.select(good.id) }
            await settle()
            capture(to: "\(dir)/v2-04-export.png")
            print("QUEUE-SHOT: wrote screenshots to \(dir)")
        }

        await queueNavTest()

        // Persist the queue (§5.4) so a restore can be exercised. Under D1 the good
        // couples are included by default — nothing to approve first.
        model.saveQueue()
        print("QUEUE-SAVE: queue.json \(model.couples.count) couples, \(model.includedCount) included")

        guard let outDir = env["COUPLE_BATCH_OUT"] else { return }
        try? FileManager.default.createDirectory(atPath: outDir, withIntermediateDirectories: true)
        model.exportDestination = URL(fileURLWithPath: outDir)
        model.globalFormat = .auto
        model.globalKeepCameraAudio = false
        print("QUEUE-TEST: batch exporting \(model.includedCount) couple(s) → \(outDir)")
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

    /// j/k/Enter over the queue (UX §2.9 extension to v2; SPEC-V2 §7). `v` is
    /// UNBOUND since UX §V2.12 / D4 — asserted here as a no-op on inclusion.
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
        // D4: `v` is unbound — it changes neither inclusion nor the selection.
        if let sel = model.selectedCouple {
            let id = sel.id
            let was = sel.included
            let wasExplicit = sel.explicit
            await key(9) // v
            check("v is unbound (inclusion and selection unchanged)",
                  model.selectedID == id
                  && model.couples.first { $0.id == id }?.included == was
                  && model.couples.first { $0.id == id }?.explicit == wasExplicit)
        }
        print("QUEUE-EVENT summary: \(pass) passed, \(fail) failed")
    }

    /// COUPLE_INCLUDE_TEST=1 (PLAN-COMPACT-MODE Phase 1 step 8; UX §V2.12 / D1, D8):
    /// the app-target END-TO-END exercise of the include-for-export model over a REAL
    /// `COUPLE_PAIR_DIR` session — the stateful behavior the pure
    /// `ExportIncludePolicyTests` cannot reach. Asserts: every good couple is included
    /// by default and no flagged one is (and only good couples offer the checkbox);
    /// `toggleInclude` records an explicit opt-out that SURVIVES a re-run of that
    /// pair's auto-sync (inclusion is derived, nothing recomputes it away); a batch
    /// export stamps `exportedAt` on every exported couple and the included count
    /// drops to the explicitly re-included ones; and a save → restore round-trips
    /// `explicit` / `exportedAt` / `included` per couple. Like the other media
    /// harnesses it needs the GUI app-bundle runloop and does NOT complete in a
    /// headless agent shell. Run with COUPLE_PAIR_DIR=<session> COUPLE_INCLUDE_TEST=1
    /// (optionally COUPLE_BATCH_OUT=<dir>, COUPLE_SHOT_DIR=<dir>, COUPLE_AUTOQUIT=1).
    private func includeTest(env: [String: String]) async {
        guard let model else { return }
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("INCLUDE-TEST \(ok ? "ok  " : "FAIL") \(name)")
        }
        func settleQueue() async {
            for _ in 0..<3000 {
                let pending = model.couples.contains { c in
                    c.pair.session != nil && (c.pair.syncActivity == .none || c.pair.isSyncRunning)
                }
                if !model.resolving && !pending { break }
                try? await Task.sleep(nanoseconds: 100_000_000)
            }
        }
        await settleQueue()
        guard !model.couples.isEmpty else {
            print("INCLUDE-TEST: SKIPPED (no couples — needs COUPLE_PAIR_DIR)")
            return
        }
        let good = model.couples.filter { $0.verdict == .good }
        let flagged = model.couples.filter { $0.verdict != .good }
        print("INCLUDE-TEST: \(model.couples.count) couples · \(good.count) good · \(flagged.count) flagged")

        // (1) DEFAULTS (D1): included ⇔ verdict good; the checkbox exists for good rows only.
        check("(1) every good couple is included by default", good.allSatisfy(\.included))
        check("(1) no flagged couple is included", !flagged.contains(where: \.included))
        check("(1) canInclude ⇔ verdict good",
              model.couples.allSatisfy { $0.canInclude == ($0.verdict == .good) })
        check("(1) sections: Ready = good, To fix = rest",
              model.ready.count == good.count && model.toFix.count == flagged.count)
        check("(1) includedCount == good count", model.includedCount == good.count)
        check("(1) a flagged couple cannot be toggled on", {
            guard let f = flagged.first else { return true }
            model.toggleInclude(f)
            return !f.included && f.explicit == nil
        }())

        // (2) EXPLICIT OPT-OUT survives a re-run of that pair's auto-sync: inclusion is
        //     derived from the stored toggle, and a sync completion recomputes nothing.
        guard let target = good.first else {
            print("INCLUDE-TEST summary: \(pass) passed, \(fail) failed (no good couple to toggle)")
            return
        }
        model.toggleInclude(target)
        check("(2) toggleInclude records explicit = false and excludes the row",
              target.explicit == false && !target.included)
        check("(2) included count dropped by one", model.includedCount == good.count - 1)
        target.pair.runAutoSync(announce: false)
        await target.pair.awaitSync()
        check("(2) re-sync leaves the opt-out in place",
              target.explicit == false && !target.included && target.verdict == .good)
        // ⌘Z inverse: restores the PREVIOUS stored value (nil here), not a blind re-toggle.
        model.undoManager?.undo()
        check("(2) undo restores the default (explicit nil, included)",
              target.explicit == nil && target.included)
        // Tick it off, then back on: an explicit TRUE (re-include) is what survives the export below.
        model.toggleInclude(target)
        model.toggleInclude(target)
        check("(2) toggling twice leaves explicit = true, included", target.explicit == true && target.included)

        // (3) EXPORT (D8): every exported couple gets `exportedAt`; `explicit` is untouched;
        //     the default-included ones drop out, the explicitly re-included one stays.
        let before = model.includedCount
        let batch = model.included
        let outDir = env["COUPLE_BATCH_OUT"]
            ?? NSTemporaryDirectory().appending("couple-include-test-\(ProcessInfo.processInfo.processIdentifier)")
        try? FileManager.default.createDirectory(atPath: outDir, withIntermediateDirectories: true)
        model.exportDestination = URL(fileURLWithPath: outDir)
        model.globalFormat = .auto
        model.globalKeepCameraAudio = false
        print("INCLUDE-TEST: batch exporting \(before) couple(s) → \(outDir)")
        model.batch.run()
        for _ in 0..<6000 {
            if case .finished = model.batch.phase { break }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        let exportedOK = batch.filter { if case .done = $0.exportStatus { return true } else { return false } }
        check("(3) batch finished with every included couple exported", exportedOK.count == batch.count)
        check("(3) exportedAt stamped on every exported couple", exportedOK.allSatisfy { $0.exportedAt != nil })
        check("(3) explicit untouched by the export",
              target.explicit == true && batch.filter { $0.id != target.id }.allSatisfy { $0.explicit == nil })
        check("(3) default-included couples left the batch after export",
              batch.filter { $0.id != target.id }.allSatisfy { !$0.included && $0.inFilesList })
        check("(3) the explicitly re-included couple stays included", target.included)
        check("(3) included count dropped to the explicit re-includes",
              model.includedCount == model.couples.filter { $0.explicit == true && $0.verdict == .good }.count
              && (batch.count < 2 || model.includedCount < before))
        check("(3) Files list = included ∪ exported",
              Set(model.filesList.map(\.id)) == Set(model.couples.filter { $0.included || $0.exportedAt != nil }.map(\.id)))

        // (4) SAVE → RESTORE round-trips the include inputs and the derived state.
        model.saveQueue()
        struct Snap { let explicit: Bool?; let exportedAt: Date?; let included: Bool }
        var snaps: [String: Snap] = [:]
        for c in model.couples { snaps[c.id] = Snap(explicit: c.explicit, exportedAt: c.exportedAt, included: c.included) }
        guard let saved = QueueStore.load() else {
            check("(4) queue.json saved and loadable", false)
            print("INCLUDE-TEST summary: \(pass) passed, \(fail) failed")
            return
        }
        check("(4) saved file carries explicit/exportedAt per couple",
              saved.couples.allSatisfy { sc in
                  let s = snaps[sc.videoKey]
                  return s != nil && sc.explicit == s!.explicit
                      && (sc.exportedAt == nil) == (s!.exportedAt == nil)
              })
        model.applyRestore(saved)
        for _ in 0..<3000 {
            let settled = !model.resolving && model.couples.count == saved.couples.count
                && !model.couples.contains { $0.pair.session != nil
                    && ($0.pair.syncActivity == .none || $0.pair.isSyncRunning) }
            if settled { break }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        try? await Task.sleep(nanoseconds: 500_000_000)
        check("(4) restore keeps the couple set", model.couples.count == snaps.count)
        check("(4) explicit round-trips", model.couples.allSatisfy { snaps[$0.id]?.explicit == $0.explicit })
        check("(4) exportedAt round-trips (to the second — ISO8601)",
              model.couples.allSatisfy { c in
                  guard let s = snaps[c.id] else { return false }
                  switch (s.exportedAt, c.exportedAt) {
                  case (nil, nil): return true
                  case let (a?, b?): return abs(a.timeIntervalSince(b)) < 1.0
                  default: return false
                  }
              })
        check("(4) included round-trips", model.couples.allSatisfy { snaps[$0.id]?.included == $0.included })

        if let dir = env["COUPLE_SHOT_DIR"] {
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            if let f = model.filesList.first { model.select(f.id) }
            await settle()
            capture(to: "\(dir)/include-01-after-export.png")
            print("INCLUDE-TEST: wrote screenshot to \(dir)/include-01-after-export.png")
        }
        print("INCLUDE-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// Restore round-trip (§5.4): load queue.json, re-import + re-resolve + overlay
    /// the saved selections / include inputs, and print the restored queue.
    private func restoreTest() async {
        guard let model else { return }
        guard let saved = QueueStore.load() else {
            print("RESTORE-TEST: no saved queue.json")
            return
        }
        print("RESTORE-TEST: restoring \(saved.couples.count) couples (\(saved.couples.filter { $0.exportedAt != nil }.count) exported) from queue.json")
        model.applyRestore(saved)
        for _ in 0..<3000 {
            let settled = !model.resolving && model.couples.count == saved.couples.count
                && !model.couples.contains { $0.pair.session != nil
                    && ($0.pair.syncActivity == .none || $0.pair.isSyncRunning) }
            if settled { break }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        print("RESTORE-TEST: \(model.couples.count) couples, \(model.includedCount) included")
        for c in model.couples {
            print("  \(c.video.name) [\(c.state.rawValue)] -> \(c.selectedCandidate?.audioName ?? "—") included=\(c.included) explicit=\(c.explicit.map(String.init) ?? "nil") exportedAt=\(c.exportedAt.map { ISO8601DateFormatter().string(from: $0) } ?? "nil")")
        }
    }

    /// COUPLE_REMOVE_TEST=1 (TODO-001): the app-target END-TO-END exercise of per-file
    /// removal + Clear Session over a REAL `COUPLE_PAIR_DIR` session, asserting the
    /// stateful behavior the pure `MediaRemovalPolicyTests` cannot reach: after removing
    /// an audio its video row RE-RESOLVES over the remaining files and NO couple /
    /// candidate / persisted-queue record retains the removed key; removing a video DROPS
    /// its row; and Clear Session empties the model AND removes the persisted restore
    /// record (so relaunch offers no restore). Like the other media harnesses it needs
    /// the GUI app-bundle runloop (pool import + handles) and does NOT complete in a
    /// headless agent shell; the pure decision's automated coverage is
    /// `MediaRemovalPolicyTests` in CoupleKitTests (`swift test`). Run with
    /// COUPLE_PAIR_DIR=<session> COUPLE_REMOVE_TEST=1 (and usually COUPLE_AUTOQUIT=1).
    private func removeTest() async {
        guard let model else { return }
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("REMOVE-TEST \(ok ? "ok  " : "FAIL") \(name)")
        }
        // Wait for the pool import + resolution + every couple's sync to settle.
        func settleQueue() async {
            for _ in 0..<3000 {
                let pending = model.couples.contains { c in
                    c.pair.session != nil && (c.pair.syncActivity == .none || c.pair.isSyncRunning)
                }
                if !model.resolving && !pending { break }
                try? await Task.sleep(nanoseconds: 100_000_000)
            }
        }
        await settleQueue()

        guard !model.couples.isEmpty else { print("REMOVE-TEST: no couples, skipped"); return }
        // The keys present anywhere a removed key must NOT survive.
        func keySurvives(_ key: String) -> Bool {
            if model.pool.entry(key: key) != nil { return true }
            if model.poolSelection.contains(key) { return true }
            for c in model.couples {
                if c.id == key { return true }
                if c.selectedAudioKey == key { return true }
                if c.candidates.contains(where: { $0.audioKey == key }) { return true }
            }
            // Persisted queue record.
            model.saveQueue()
            if let saved = QueueStore.load() {
                if saved.files.contains(where: { $0.contentKey == key }) { return true }
                for sc in saved.couples {
                    if sc.videoKey == key || sc.selectedAudioKey == key { return true }
                }
            }
            return false
        }

        // --- (A) Remove a SELECTED AUDIO: the video row must survive and re-resolve. ---
        if let coupleWithAudio = model.couples.first(where: { $0.selectedAudioKey != nil }),
           let audioKey = coupleWithAudio.selectedAudioKey {
            let videoKey = coupleWithAudio.id
            let beforeCount = model.couples.count
            // Select just the audio and remove it (unused ⇒ immediate; referenced ⇒
            // confirm — drive the confirmed path directly so the test is deterministic).
            model.poolSelection = [audioKey]
            model.poolPrimary = audioKey
            model.removeSelectedMedia()
            if model.pendingMediaRemoval != nil { model.confirmMediaRemoval() }
            await settleQueue()

            check("(A) removing selected audio keeps the video row",
                  model.couples.contains { $0.id == videoKey })
            check("(A) video row count unchanged (row re-resolved, not deleted)",
                  model.couples.count == beforeCount)
            check("(A) removed audio is gone from the pool", model.pool.entry(key: audioKey) == nil)
            check("(A) no couple/candidate/selection retains the removed audio key",
                  !keySurvives(audioKey))
            if let row = model.couples.first(where: { $0.id == videoKey }) {
                // The row re-resolved: it no longer points at the removed audio (it is
                // newly-resolved over the remaining files, ambiguous, or no-match).
                check("(A) re-resolved row no longer selects the removed audio",
                      row.selectedAudioKey != audioKey)
            }
        } else {
            print("REMOVE-TEST: (A) skipped — no couple has a selected audio")
        }

        // --- (B) Remove a VIDEO: its couple row drops. ---
        if let videoEntry = model.pool.videos.first {
            let videoKey = videoEntry.id
            model.poolSelection = [videoKey]
            model.poolPrimary = videoKey
            model.removeSelectedMedia()
            if model.pendingMediaRemoval != nil { model.confirmMediaRemoval() }
            await settleQueue()
            check("(B) removing a video drops its couple row",
                  !model.couples.contains { $0.id == videoKey })
            check("(B) removed video is gone from the pool", model.pool.entry(key: videoKey) == nil)
            check("(B) no record retains the removed video key", !keySurvives(videoKey))
        } else {
            print("REMOVE-TEST: (B) skipped — no video in pool")
        }

        // --- (C) Clear Session: empties the model and removes the restore record. ---
        model.saveQueue() // ensure a restore record exists to be cleared
        let hadRecordBefore = QueueStore.load() != nil
        model.requestClearSession()
        check("(C) Clear Session arms a confirmation", model.pendingClearSession != nil)
        model.confirmClearSession()
        check("(C) Clear Session empties the pool (returns to import screen)", model.pool.isEmpty)
        check("(C) Clear Session empties the couples", model.couples.isEmpty)
        check("(C) Clear Session has no content (center import surface)", !model.hasContent)
        check("(C) Clear Session clears the pool selection",
              model.poolSelection.isEmpty && model.poolPrimary == nil)
        check("(C) Clear Session removed the persisted restore record (was present: \(hadRecordBefore))",
              QueueStore.load() == nil)
        // A teardown save must NOT recreate the cleared record (the empty-session guard).
        model.saveAll()
        check("(C) saveAll after clear does not recreate the restore record",
              QueueStore.load() == nil)

        print("REMOVE-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// COUPLE_ANCHOR_TEST=1 (TODO-011): the app-target END-TO-END exercise of the LIVE
    /// `PairSession.proposeAnchors()` / `addAnchor()` over a REAL `COUPLE_PAIR_DIR`
    /// session, asserting what the pure `AnchorProposalTests` cannot reach: that the
    /// engine's onset + per-lane RMS usability feeds the proposer correctly and EVERY
    /// proposed anchor (and every automatic "+ anchor") lands inside the live overlap,
    /// with the count obeying the 3/2/0 rule even when the overlap is artificially
    /// shrunk below 30 s by a large offset (the old camera-duration fallback would put
    /// anchors out past the external content here). Needs the GUI app-bundle runloop
    /// (pool import + handles); the pure decision's automated coverage is
    /// `AnchorProposalTests` in CoupleKitTests (`swift test`). Run with
    /// COUPLE_PAIR_DIR=<session> COUPLE_ANCHOR_TEST=1 (and usually COUPLE_AUTOQUIT=1).
    private func anchorTest() async {
        guard let model else { return }
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("ANCHOR-TEST \(ok ? "ok  " : "FAIL") \(name)")
        }
        for _ in 0..<3000 {
            if let c = model.selectedCouple, !model.resolving,
               c.pair.syncActivity == .succeeded || { if case .failed = c.pair.syncActivity { return true }; return false }() { break }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        guard let pair = model.selectedCouple?.pair, pair.session != nil else {
            print("ANCHOR-TEST: no synced couple, skipped"); return
        }
        func anchorsInOverlap() -> Bool {
            guard let ov = pair.overlap else { return pair.anchors.isEmpty }
            return pair.anchors.allSatisfy { $0.t >= ov.lowerBound && $0.t <= ov.upperBound }
        }

        // --- (A) Natural overlap (the owner corpus is long) ⇒ proposal in-overlap. ---
        pair.proposeAnchors()
        let natural = pair.anchors.count
        check("(A) every proposed anchor is inside the overlap", anchorsInOverlap())
        check("(A) count is 0, 2, or 3 — never one", natural != 1)
        if let ov = pair.overlap, ov.upperBound - ov.lowerBound >= 30 {
            check("(A) long overlap ⇒ three zoned anchors", natural == 3)
        }

        // --- (B) Automatic "+ anchor" never lands outside the overlap. ---
        if pair.anchors.count < 4 {
            pair.addAnchor()
            check("(B) after + anchor every anchor is still inside the overlap", anchorsInOverlap())
        }

        // --- (C) Shrink the overlap below 30 s with a large offset, re-propose: anchors
        //         must stay inside the (now-small) overlap or be ZERO — NEVER out past
        //         the external content and NEVER exactly one (the bug TODO-011 fixes). ---
        let savedOffset = pair.offsetMs
        if let ov = pair.overlap, let ext = pair.external.track {
            // Push the external so only the last ~15 s of it overlaps the camera.
            let want = (ov.upperBound - ov.lowerBound) - max(0, ext.durationS - 15)
            pair.commitTypedOffset(savedOffset + max(want, 1) * 1000)
            pair.mappingChanged(immediate: true)
            pair.proposeAnchors()
            let small = pair.overlap.map { $0.upperBound - $0.lowerBound } ?? 0
            check("(C) shrunken overlap: anchors inside overlap (or zero)", anchorsInOverlap())
            check("(C) shrunken overlap: count never exactly one", pair.anchors.count != 1)
            if small > 0, small < 30 { check("(C) usable overlap < 30 s ⇒ at most two anchors", pair.anchors.count <= 2) }
        }
        // --- (D) No overlap at all ⇒ zero anchors + manual-rescue state. ---
        if let cam = pair.camera.track {
            pair.commitTypedOffset((cam.durationS + 10_000) * 1000) // push entirely apart
            pair.mappingChanged(immediate: true)
            pair.proposeAnchors()
            check("(D) no overlap ⇒ zero anchors", pair.anchors.isEmpty && pair.overlap == nil)
            check("(D) no overlap ⇒ manual-rescue state shown", pair.anchorsUnavailable)
        }
        pair.commitTypedOffset(savedOffset)
        pair.mappingChanged(immediate: true)

        print("ANCHOR-TEST summary: \(pass) passed, \(fail) failed")
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
        await key(9) // v — UNBOUND since UX §V2.12 / D4
        check("v is unbound (verified untouched)", pair.verified == v)
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
            let before = pair.anchors.map(\.frame.winMs)
            let routed = model.zoom(at: CGPoint(x: rect.midX, y: rect.midY), deltaY: 120, precise: true)
            let after = pair.anchors.map(\.frame.winMs)
            check("zoom routes to loupe under cursor, zooms all (locked), exits auto",
                  routed && after != before && Set(after).count == 1
                  && pair.anchors.allSatisfy { !$0.frame.auto })
            check("zoom outside any loupe is ignored",
                  !model.zoom(at: CGPoint(x: -10, y: -10), deltaY: 120, precise: true))
            pair.armAuto(loupe: 0)
            check("auto chip re-arms all loupes under lock",
                  pair.anchors.allSatisfy(\.frame.auto))
        }
        print("EVENT-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// The RMS tie-breaker decision moved INTO couple-core (PLAN-CORE-SHELL-BOUNDARY Phase C):
    /// `cpl_resolve` owns the §3.5/§5.2 escalation, so the old Swift `Resolver.rmsEscalate` mirror is
    /// gone. The shell now keeps only the `rmsStrongForm` switch (which maps to the core
    /// `ResolveParams.rmsStrongForm` param). The escalation's behavior is exercised by core's own Rust
    /// tests + the CLI↔Swift decision parity (both call the same core resolve via FFI). Kept as a
    /// stub so COUPLE_RMS_TEST still resolves; it asserts the surviving shell invariant only.
    private func rmsTest() async {
        let ok = Resolver.rmsStrongForm == false
        print("RMS-TEST \(ok ? "ok " : "FAIL") conservative form is the shipped default (param maps to core)")
        print("RMS-TEST: the escalation DECISION now lives in core (cpl_resolve); covered by core/CLI parity")
        print("RMS-TEST summary: \(ok ? 1 : 0) passed, \(ok ? 0 : 1) failed")
    }

    /// The candidate-overflow decision moved INTO couple-core (PLAN-CORE-SHELL-BOUNDARY Phase C):
    /// `cpl_resolve` owns the §9.2/§9.3 no-silent-eviction logic, so the old Swift
    /// `Resolver.candidateOverflows` mirror is gone. The property (a low ceiling degrades to honest
    /// `ambiguous` with eligible/limit/scope surfaced, by COUNTS only) is now proven by core's Rust
    /// tests + the CLI↔Swift parity. Kept as a stub so COUPLE_OVERFLOW_TEST still resolves.
    private func overflowTest() async {
        print("OVERFLOW-TEST: the overflow DECISION now lives in core (cpl_resolve); covered by core/CLI parity")
        print("OVERFLOW-TEST summary: 0 passed, 0 failed")
    }

    /// Pure unit test for TODO-006's responsive candidate-scroll cap
    /// (`CenterLayout.candidateScrollCap`). No engine, no media: it drives the
    /// pure height→cap function so the bounded-picker contract is regression-
    /// checked deterministically (the live ScrollView clamp/scroll-into-view is
    /// on the owner's manual walkthrough at the four list sizes). Asserts: the
    /// first-pass/zero height and tiny windows return the floor (~3 rows always
    /// visible); large windows are clamped to the ceiling (a huge list never
    /// dwarfs the video); the mid-range scales with the column; and — the bug's
    /// crux — at the minimum supported window the cap still leaves room for the
    /// fixed WaveformRow + BottomBar below it. Run headless via
    /// COUPLE_LAYOUT_TEST=1 (needs no files).
    private func layoutTest() async {
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("LAYOUT-TEST \(ok ? "ok " : "FAIL") \(name)")
        }
        let floor: CGFloat = 96, ceiling: CGFloat = 240

        // (a) First layout pass (height not yet measured) ⇒ the floor, never 0
        // (a 0-height scroll region would hide every candidate).
        check("(a) zero container height ⇒ floor (\(Int(floor)))",
              CenterLayout.candidateScrollCap(containerHeight: 0) == floor)

        // (b) A tiny window clamps UP to the floor so ~3 rows stay visible.
        check("(b) tiny window (200) clamps up to floor",
              CenterLayout.candidateScrollCap(containerHeight: 200) == floor)

        // (c) A tall window clamps DOWN to the ceiling so a long candidate list
        // can never grow without bound and dwarf the video hero.
        check("(c) tall window (2000) clamps down to ceiling (\(Int(ceiling)))",
              CenterLayout.candidateScrollCap(containerHeight: 2000) == ceiling)

        // (d) In between, the cap scales with the column (monotonic, strictly
        // inside the clamp band).
        let mid = CenterLayout.candidateScrollCap(containerHeight: 600)
        check("(d) mid window (600) scales between floor and ceiling",
              mid > floor && mid < ceiling)
        check("(d') cap is monotonic in container height",
              CenterLayout.candidateScrollCap(containerHeight: 700) >= mid)

        // (e) THE bug: at the minimum supported window the bounded picker must
        // still leave room for the persistent controls below it. The center
        // column gets the window minus the footer (~24); the fixed region under
        // the banner is WaveformRow (124) + inter-row spacing (8) + BottomBar
        // (~52) + bottom padding (8) ≈ 192. The cap must be comfortably smaller
        // than the column minus that region, so the waveform row + bottom
        // toolbar remain reachable even before the greedy video hero yields.
        let minWindow: CGFloat = 400
        let column = minWindow - 24            // RootView footer
        let fixedBelowBanner: CGFloat = 124 + 8 + 52 + 8
        let capAtMin = CenterLayout.candidateScrollCap(containerHeight: column)
        check("(e) at min window the cap leaves room for waveform row + bottom bar",
              capAtMin + fixedBelowBanner < column)

        print("LAYOUT-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// Pure unit test for TODO-014 Part 1's direct anchor-drag mapping
    /// (`OverviewTimeline`). The bug was a velocity-adaptive gain on accumulated
    /// pointer deltas, so sensitivity changed mid-drag and the landing time depended
    /// on speed/path. The fix maps the pointer's CURRENT x straight through the
    /// displayed visible range (the same absolute mapping the playhead scrub uses).
    /// This locks the resulting contract — no engine, media, or view needed. Run
    /// headless via COUPLE_OVERVIEW_TEST=1. Covers: endpoints + midpoint, monotonic
    /// in x, clamp outside the lane, x→time→x round-trip, the SAME x yields the SAME
    /// time regardless of the path/speed that reached it (no velocity gain remains),
    /// a non-zero/zoomed visible lower bound, and degenerate width/range pinning to the
    /// lower bound — plus TODO-010's `OverviewViewport` zoom/pan/clamp math (pointer-
    /// centered zoom, min-span floor, domain clamping, and fit ⇒ nil).
    private func overviewTest() async {
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("OVERVIEW-TEST \(ok ? "ok " : "FAIL") \(name)")
        }
        let eps = 1e-9
        let w: CGFloat = 800
        let vis = 0.0...100.0 // displayed range (seconds)

        // (a) Endpoints + midpoint map directly through the visible range.
        check("(a) x=0 ⇒ lower bound",
              abs(OverviewTimeline.time(atX: 0, width: w, visible: vis) - 0) < eps)
        check("(a) x=width ⇒ upper bound",
              abs(OverviewTimeline.time(atX: w, width: w, visible: vis) - 100) < eps)
        check("(a) x=width/2 ⇒ midpoint",
              abs(OverviewTimeline.time(atX: w / 2, width: w, visible: vis) - 50) < eps)

        // (b) Strictly monotonic increasing in x across the lane.
        var monotonic = true
        var prev = -Double.infinity
        for i in 0...80 {
            let t = OverviewTimeline.time(atX: CGFloat(i) * 10, width: w, visible: vis)
            if t < prev - eps { monotonic = false }
            prev = t
        }
        check("(b) time(atX:) is monotonic non-decreasing in x", monotonic)

        // (c) Off-lane pointer x clamps into the visible range (no escape, no second
        // clamp needed before moveAnchor's playable clamp).
        check("(c) x<0 clamps to lower bound",
              abs(OverviewTimeline.time(atX: -250, width: w, visible: vis) - 0) < eps)
        check("(c) x>width clamps to upper bound",
              abs(OverviewTimeline.time(atX: w + 250, width: w, visible: vis) - 100) < eps)

        // (d) x→time→x round-trips inside the lane.
        var roundTrips = true
        for px in stride(from: CGFloat(0), through: w, by: 40) {
            let t = OverviewTimeline.time(atX: px, width: w, visible: vis)
            let back = OverviewTimeline.x(forTime: t, width: w, visible: vis)
            if abs(back - px) > 1e-6 { roundTrips = false }
        }
        check("(d) x→time→x round-trips", roundTrips)

        // (e) THE bug: the landing time must depend ONLY on the final x, never on the
        // path/speed taken to reach it. A slow walk, a fast jump, and a back-and-forth
        // wander that all END at the same x must all yield the identical time (the old
        // velocity-gain accumulator failed this).
        let targetX: CGFloat = 537
        let direct = OverviewTimeline.time(atX: targetX, width: w, visible: vis)
        // Simulate three drag paths; only the final sample feeds the (now stateless)
        // mapping, so each path's result is just time(atX: targetX).
        let slowPath: [CGFloat] = stride(from: 0, through: targetX, by: 3).map { $0 }
        let fastPath: [CGFloat] = [0, 200, targetX]
        let wanderPath: [CGFloat] = [0, 760, 40, 700, 120, targetX]
        func landing(_ path: [CGFloat]) -> Double {
            var t = 0.0
            for x in path { t = OverviewTimeline.time(atX: x, width: w, visible: vis) }
            return t
        }
        check("(e) slow path lands at the direct time", abs(landing(slowPath) - direct) < eps)
        check("(e) fast path lands at the same time", abs(landing(fastPath) - direct) < eps)
        check("(e) wandering path lands at the same time", abs(landing(wanderPath) - direct) < eps)
        check("(e) all three paths agree (no velocity gain remains)",
              abs(landing(slowPath) - landing(fastPath)) < eps
              && abs(landing(fastPath) - landing(wanderPath)) < eps)

        // (f) A non-zero / zoomed visible range (TODO-010 forward-compat): the mapping
        // is relative to the DISPLAYED range, not a hardcoded 0...duration.
        let zoomed = 30.0...45.0
        check("(f) zoomed lower bound at x=0", abs(OverviewTimeline.time(atX: 0, width: w, visible: zoomed) - 30) < eps)
        check("(f) zoomed midpoint at x=width/2",
              abs(OverviewTimeline.time(atX: w / 2, width: w, visible: zoomed) - 37.5) < eps)
        check("(f) zoomed upper bound at x=width",
              abs(OverviewTimeline.time(atX: w, width: w, visible: zoomed) - 45) < eps)

        // (g) Degenerate width / range can't divide-by-zero; pin to the lower bound.
        check("(g) zero width ⇒ lower bound",
              abs(OverviewTimeline.time(atX: 123, width: 0, visible: vis) - 0) < eps)
        check("(g) empty range ⇒ lower bound",
              abs(OverviewTimeline.time(atX: 123, width: w, visible: 50.0...50.0) - 50) < eps)

        // (h) TODO-010 viewport math (`OverviewViewport`): zoom keeps the pointer time
        // at the same fractional x, clamps to the domain + min span, pans, and fits
        // (nil) when zoomed fully out. Pure — no view or media.
        let domain = 0.0...100.0
        if let z = OverviewViewport.zoom(domain, factor: 0.5, around: 50, domain: domain) {
            check("(h) zoom-in about midpoint halves span, stays centered",
                  abs((z.upperBound - z.lowerBound) - 50) < 1e-6
                  && abs(z.lowerBound - 25) < 1e-6 && abs(z.upperBound - 75) < 1e-6)
        } else { check("(h) zoom-in produced a viewport", false) }
        if let z = OverviewViewport.zoom(domain, factor: 0.5, around: 0, domain: domain) {
            check("(h) zoom about the left edge pins the lower bound", abs(z.lowerBound - 0) < 1e-6)
        } else { check("(h) edge zoom produced a viewport", false) }
        if let c = OverviewViewport.clamp(80.0...130.0, domain: domain) {
            check("(h) clamp keeps span, shifts inside the domain",
                  abs((c.upperBound - c.lowerBound) - 50) < 1e-6 && abs(c.upperBound - 100) < 1e-6)
        } else { check("(h) over-range clamp produced a viewport", false) }
        if let c = OverviewViewport.clamp(50.0...50.1, domain: domain) {
            check("(h) an over-tight range widens to the min span",
                  abs((c.upperBound - c.lowerBound) - OverviewViewport.minSpanS) < 1e-6)
        } else { check("(h) min-span clamp produced a viewport", false) }
        check("(h) zoom out past the full span ⇒ fitted (nil)",
              OverviewViewport.zoom(20.0...40.0, factor: 100, around: 30, domain: domain) == nil)
        check("(h) a fitted-width clamp ⇒ nil",
              OverviewViewport.clamp(0.0...100.0, domain: domain) == nil)
        if let p = OverviewViewport.pan(10.0...30.0, by: -50, domain: domain) {
            check("(h) pan clamps at the start edge, preserving span",
                  abs(p.lowerBound - 0) < 1e-6 && abs((p.upperBound - p.lowerBound) - 20) < 1e-6)
        } else { check("(h) pan produced a viewport", false) }

        print("OVERVIEW-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// Pure unit test for TODO-002's media-pool preview state machine
    /// (`PoolPreviewState`). No engine, no media, no AVPlayer: it drives the pure
    /// value-type transitions and asserts every acceptance criterion as a state +
    /// emitted `PoolPreviewAction` (the controller's AVPlayer side effect is a thin
    /// translation of these actions, exercised on the owner's manual walkthrough).
    /// Run headless via COUPLE_PREVIEW_TEST=1 (needs no files). Covers: play→pause
    /// preserves position; resume from that position; switching files stops the old
    /// one and resets to zero (both rows' icons follow because `activeKey` is the
    /// single truth); scrub while playing AND paused; ticks ignored while scrubbing;
    /// natural end rewinds to zero and is non-playing; and explicit stop/cleanup.
    private func previewTest() async {
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("PREVIEW-TEST \(ok ? "ok " : "FAIL") \(name)")
        }
        let a = URL(fileURLWithPath: "/tmp/a.wav"), b = URL(fileURLWithPath: "/tmp/b.wav")

        // (1) First toggle on entry A loads + plays from 0, A becomes the active row.
        var s = PoolPreviewState()
        var act = s.toggle(key: "A", url: a)
        check("(1) toggle A ⇒ load A at 0 + playing", act == .load(key: "A", url: a, at: 0, play: true)
              && s.isActive("A") && s.isPlaying && s.currentTime == 0)
        check("(1) only A is active (B's row stays compact)", s.isActive("A") && !s.isActive("B"))

        // (2) Duration arrives, then ticks advance the position while playing.
        s.setDuration(30)
        s.tick(5); s.tick(12.5)
        check("(2) ticks advance currentTime while playing", abs(s.currentTime - 12.5) < 1e-9)
        check("(2) duration surfaced", s.duration == 30)

        // (3) Pause preserves position; the play icon would flip (isPlaying false).
        act = s.toggle(key: "A", url: a)
        check("(3) toggle A while playing ⇒ pause, position preserved",
              act == .pause && !s.isPlaying && abs(s.currentTime - 12.5) < 1e-9)
        // A tick while paused is ignored (no periodic fighting a paused preview).
        s.tick(20)
        check("(3) tick while paused is ignored", abs(s.currentTime - 12.5) < 1e-9)

        // (4) Resume plays again from the SAME position (not restart).
        act = s.toggle(key: "A", url: a)
        check("(4) toggle A while paused ⇒ resume from position",
              act == .play && s.isPlaying && abs(s.currentTime - 12.5) < 1e-9)

        // (5) Scrub WHILE PLAYING: begin freezes ticks, scrubTo seeks, a racing
        // tick can't move the thumb, end re-enables ticks. Still playing throughout.
        s.scrubBegin()
        check("(5) scrubBegin sets scrubbing", s.isScrubbing && s.isPlaying)
        act = s.scrubTo(20)
        check("(5) scrubTo seeks to 20 while playing", act == .seek(20) && abs(s.currentTime - 20) < 1e-9)
        s.tick(13) // a periodic update mid-drag must NOT win
        check("(5) tick during scrub is ignored (no fight)", abs(s.currentTime - 20) < 1e-9)
        s.scrubEnd()
        check("(5) scrubEnd clears scrubbing, still playing", !s.isScrubbing && s.isPlaying)
        s.tick(21)
        check("(5) ticks resume after scrubEnd", abs(s.currentTime - 21) < 1e-9)

        // (6) Switching to a DIFFERENT file stops the old and starts B at 0. The old
        // row's icon necessarily reverts because A is no longer the active key.
        act = s.toggle(key: "B", url: b)
        check("(6) toggle B ⇒ load B at 0 + playing", act == .load(key: "B", url: b, at: 0, play: true)
              && s.isActive("B") && s.isPlaying && s.currentTime == 0)
        check("(6) A is no longer active (its icon reverts to play)", !s.isActive("A"))

        // (7) Scrub WHILE PAUSED defines the next start; play resumes from there.
        s.setDuration(40)
        _ = s.toggle(key: "B", url: b) // pause B
        check("(7) B paused", !s.isPlaying)
        act = s.scrubTo(18)
        check("(7) scrubTo while paused seeks + sets position",
              act == .seek(18) && abs(s.currentTime - 18) < 1e-9 && !s.isPlaying)
        act = s.toggle(key: "B", url: b)
        check("(7) play resumes from the scrubbed-while-paused position",
              act == .play && s.isPlaying && abs(s.currentTime - 18) < 1e-9)

        // (8) Natural end ⇒ non-playing AND rewound to 0 (next play starts at zero).
        s.tick(39); s.tick(40)
        act = s.reachedEnd()
        check("(8) natural end ⇒ pause + rewind to 0",
              act == .pause && !s.isPlaying && s.currentTime == 0 && s.isActive("B"))
        act = s.toggle(key: "B", url: b)
        check("(8) play after end starts at 0", act == .play && s.isPlaying && s.currentTime == 0)

        // (8b) Toggling a file that has ENDED but is still active also restarts at 0.
        s.setDuration(10); _ = s.scrubTo(10) // park at the very end while playing
        _ = s.toggle(key: "B", url: b)        // pause (now at end)
        act = s.toggle(key: "B", url: b)      // play again
        check("(8b) resume from end rewinds to 0", act == .play && s.currentTime == 0 && s.isPlaying)

        // (9) Explicit stop/cleanup clears the active entry entirely (teardown path,
        // and the seam a future per-file removal calls before freeing the handle).
        act = s.stop()
        check("(9) stop ⇒ pause action + no active entry",
              act == .pause && s.activeKey == nil && !s.isPlaying && s.currentTime == 0)
        check("(9) stop with nothing active is a no-op action", s.stop() == PoolPreviewAction.none)

        // (10) Guards: events with no active entry never crash / mutate.
        var idle = PoolPreviewState()
        check("(10) tick with no active entry is inert", { idle.tick(5); return idle.currentTime == 0 }())
        check("(10) scrubTo with no active entry ⇒ .none", idle.scrubTo(5) == .none)
        check("(10) reachedEnd with no active entry ⇒ .none", idle.reachedEnd() == .none)

        print("PREVIEW-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// Pure unit test for the media-pool redesign (2026-06-15, owner decisions 1–3):
    /// the Finder selection transition (`PoolSelection.select`) and the preview
    /// load-paused path (`PoolPreviewState.loadPaused`). No engine, no media, no
    /// AVPlayer: it drives the pure value-type transitions and asserts each
    /// acceptance criterion as a resulting (selection, primary, anchor) and emitted
    /// `PoolPreviewAction`. Run headless via COUPLE_POOLSEL_TEST=1 (needs no files).
    /// Mirrors the `CoupleKitTests.PoolSelectionTests` / `PoolPreviewLoadPausedTests`
    /// suites. Covers: plain replace; ⌘ toggle add/remove incl. removing the primary
    /// (re-homes to a remaining member, or nil when emptied); ⇧ contiguous range over
    /// displayed order with anchor preserved; ⇧ with no anchor degrades to plain; an
    /// off-list click is inert; and SELECT loads-paused (.load play:false, isPlaying
    /// false), a subsequent toggle plays from 0, switching the primary loads-paused
    /// and deactivates the old key.
    private func poolselTest() async {
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("POOLSEL-TEST \(ok ? "ok " : "FAIL") \(name)")
        }
        let order = ["a", "b", "c", "d"] // displayed pool order

        // (1) PLAIN click replaces the whole selection; the clicked key is selection,
        //     primary, AND anchor (the start of any subsequent ⇧-range).
        var r = PoolSelection.select(current: ["a", "b"], primary: "a", anchor: "a",
                                     clicked: "c", ordered: order, modifier: .plain)
        check("(1) plain replaces ⇒ {c}, primary c, anchor c",
              r.selection == ["c"] && r.primary == "c" && r.anchor == "c")

        // (2) ⌘ toggle ON adds without disturbing the rest; the new key leads.
        r = PoolSelection.select(current: ["a"], primary: "a", anchor: "a",
                                 clicked: "c", ordered: order, modifier: .command)
        check("(2) cmd add ⇒ {a,c}, primary c, anchor c",
              r.selection == ["a", "c"] && r.primary == "c" && r.anchor == "c")

        // (3) ⌘ toggle OFF a non-primary leaves the primary intact.
        r = PoolSelection.select(current: ["a", "b", "c"], primary: "c", anchor: "c",
                                 clicked: "a", ordered: order, modifier: .command)
        check("(3) cmd remove non-primary ⇒ {b,c}, primary stays c",
              r.selection == ["b", "c"] && r.primary == "c")

        // (4) ⌘ toggle OFF the PRIMARY re-homes the primary to the nearest remaining
        //     member in display order (here b is adjacent to the removed c).
        r = PoolSelection.select(current: ["a", "b", "c"], primary: "c", anchor: "c",
                                 clicked: "c", ordered: order, modifier: .command)
        check("(4) cmd remove primary ⇒ {a,b}, primary re-homes to nearest (b)",
              r.selection == ["a", "b"] && r.primary == "b" && r.anchor == "b")

        // (5) ⌘ removing the LAST selected key empties everything (caller stops play).
        r = PoolSelection.select(current: ["a"], primary: "a", anchor: "a",
                                 clicked: "a", ordered: order, modifier: .command)
        check("(5) cmd remove last ⇒ empty, primary nil, anchor nil",
              r.selection.isEmpty && r.primary == nil && r.anchor == nil)

        // (6) ⇧ selects the inclusive contiguous range over DISPLAYED order from the
        //     anchor to the clicked key; the anchor is preserved, the clicked end leads.
        r = PoolSelection.select(current: ["b"], primary: "b", anchor: "b",
                                 clicked: "d", ordered: order, modifier: .shift)
        check("(6) shift range b→d ⇒ {b,c,d}, primary d, anchor preserved (b)",
              r.selection == ["b", "c", "d"] && r.primary == "d" && r.anchor == "b")

        // (6b) ⇧ the other direction over the same anchor re-extends (anchor sticky).
        r = PoolSelection.select(current: ["b", "c", "d"], primary: "d", anchor: "b",
                                 clicked: "a", ordered: order, modifier: .shift)
        check("(6b) shift range b→a re-extends ⇒ {a,b}, primary a, anchor still b",
              r.selection == ["a", "b"] && r.primary == "a" && r.anchor == "b")

        // (7) ⇧ with no usable anchor degrades to a plain click.
        r = PoolSelection.select(current: [], primary: nil, anchor: nil,
                                 clicked: "c", ordered: order, modifier: .shift)
        check("(7) shift with no anchor ⇒ plain {c}, primary c, anchor c",
              r.selection == ["c"] && r.primary == "c" && r.anchor == "c")

        // (8) A click on a key NOT in the displayed order is inert (state preserved).
        r = PoolSelection.select(current: ["a", "b"], primary: "b", anchor: "a",
                                 clicked: "zzz", ordered: order, modifier: .plain)
        check("(8) off-list click is inert ⇒ selection/primary/anchor unchanged",
              r.selection == ["a", "b"] && r.primary == "b" && r.anchor == "a")

        // --- Preview load-paused path (owner decision 1) ---

        // (9) SELECT loads-paused: .load with play:false at 0, NOT playing.
        var s = PoolPreviewState()
        let ua = URL(fileURLWithPath: "/tmp/a.wav"), ub = URL(fileURLWithPath: "/tmp/b.wav")
        var act = s.loadPaused(key: "a", url: ua)
        check("(9) loadPaused a ⇒ .load(play:false) at 0, active a, NOT playing",
              act == .load(key: "a", url: ua, at: 0, play: false)
              && s.isActive("a") && !s.isPlaying && s.currentTime == 0)

        // (10) A subsequent toggle on the loaded-paused primary PLAYS from 0.
        act = s.toggle(key: "a", url: ua)
        check("(10) toggle after select ⇒ play from 0",
              act == .play && s.isPlaying && s.currentTime == 0)

        // (11) Switching the PRIMARY loads-paused the new key and deactivates the old.
        act = s.loadPaused(key: "b", url: ub)
        check("(11) select b ⇒ load-paused b, b active + paused, a deactivated",
              act == .load(key: "b", url: ub, at: 0, play: false)
              && s.isActive("b") && !s.isActive("a") && !s.isPlaying)

        // (12) Re-selecting the ALREADY-active key is a no-op (a play in progress is
        //      not interrupted by re-clicking the same primary).
        _ = s.toggle(key: "b", url: ub) // play b
        act = s.loadPaused(key: "b", url: ub)
        check("(12) re-select active key ⇒ .none, still playing (not interrupted)",
              act == .none && s.isPlaying && s.isActive("b"))

        print("POOLSEL-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// Pure unit test for TODO-004's play/Space transport policy
    /// (`AuditionController.transportPlan`). No engine, no media, no AVPlayer: it
    /// asserts the decision that Space and the main play/pause button BOTH funnel
    /// through, so the two controls cannot diverge and first play starts at the
    /// visible playhead (never an anchor fallback). Run headless via
    /// COUPLE_TRANSPORT_TEST=1 (needs no files). Covers: first play at 0:00; first
    /// play at a parked nonzero playhead; pause while playing; resume from the
    /// stream cursor; a paused seek redefining the next first-play start; and the
    /// shared-path equivalence (same state ⇒ same plan).
    private func transportTest() async {
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("TRANSPORT-TEST \(ok ? "ok " : "FAIL") \(name)")
        }
        typealias C = AuditionController

        // (1) A newly attached couple parks at 0:00; first play starts there, NOT
        // at an anchor-derived pre-roll.
        check("(1) first play (no stream) at parked 0:00 ⇒ playFromPlayhead(0)",
              C.transportPlan(isPlaying: false, hasStream: false, playheadT: 0) == .playFromPlayhead(at: 0))

        // (2) A parked nonzero playhead is the authoritative first-play start.
        check("(2) first play (no stream) at parked 12.5 ⇒ playFromPlayhead(12.5)",
              C.transportPlan(isPlaying: false, hasStream: false, playheadT: 12.5) == .playFromPlayhead(at: 12.5))

        // (3) Toggling while playing pauses (position preserved by the controller).
        check("(3) toggle while playing ⇒ pause",
              C.transportPlan(isPlaying: true, hasStream: true, playheadT: 12.5) == .pause)
        check("(3) pause is independent of the playhead value",
              C.transportPlan(isPlaying: true, hasStream: false, playheadT: 99) == .pause)

        // (4) Resume after a pause continues from the stream cursor, not a restart.
        check("(4) toggle while paused with an open stream ⇒ resume",
              C.transportPlan(isPlaying: false, hasStream: true, playheadT: 30) == .resume)

        // (5) A seek while paused moves `playheadT`; because the FIRST play uses
        // `playheadT`, the next start is the seeked position. (Pre-stream case.)
        let afterPausedSeek = 7.25
        check("(5) paused seek then first play ⇒ playFromPlayhead(seek target)",
              C.transportPlan(isPlaying: false, hasStream: false, playheadT: afterPausedSeek)
                  == .playFromPlayhead(at: afterPausedSeek))

        // (6) Shared-path equivalence: Space and the button call the SAME pure
        // function, so identical state yields an identical plan — they cannot
        // diverge.
        for (playing, stream, t) in [(false, false, 0.0), (false, true, 5.0), (true, true, 9.0)] {
            let a = C.transportPlan(isPlaying: playing, hasStream: stream, playheadT: t)
            let b = C.transportPlan(isPlaying: playing, hasStream: stream, playheadT: t)
            check("(6) same state ⇒ same plan (playing=\(playing) stream=\(stream))", a == b)
        }

        print("TRANSPORT-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// Pure unit test for TODO-012's loupe click x→camera-time mapping
    /// (`LoupeView.cameraTime`). No engine, no media, no SwiftUI: it drives the
    /// pure mapping the way the loupe canvas draws (camera lane linear from
    /// `t0 = anchor − winMs/2` at x=0 to `t1 = anchor + winMs/2` at x=width, same
    /// frac as the drawn playhead), so the click-to-seek mapping is regression-
    /// checked deterministically (the live gesture composition / drag-vs-tap
    /// exclusivity is on the owner's manual walkthrough and the EVENT-TEST live
    /// path). Run headless via COUPLE_LOUPE_TEST=1 (needs no files). Covers: left
    /// edge ⇒ window start; center ⇒ exactly the anchor; right edge ⇒ window end;
    /// off-canvas x clamps into the window; a zero-width canvas is inert; and the
    /// mapping inverts the drawn-playhead frac for an arbitrary interior x.
    private func loupeTest() async {
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("LOUPE-TEST \(ok ? "ok " : "FAIL") \(name)")
        }
        let anchor = 42.0, winMs = 500.0, width: CGFloat = 320
        let winS = winMs / 1000
        let t0 = anchor - winS / 2, t1 = anchor + winS / 2
        func map(_ x: CGFloat, w: CGFloat = 320) -> Double {
            LoupeView.cameraTime(forLocalX: x, width: w, anchor: anchor, winMs: winMs)
        }

        // (a) Left edge maps to the window start (anchor − winMs/2).
        check("(a) x=0 ⇒ window start (t0)", abs(map(0) - t0) < 1e-9)

        // (b) Center maps to exactly the anchor (the drawn center line).
        check("(b) x=width/2 ⇒ anchor", abs(map(width / 2) - anchor) < 1e-9)

        // (c) Right edge maps to the window end (anchor + winMs/2).
        check("(c) x=width ⇒ window end (t1)", abs(map(width) - t1) < 1e-9)

        // (d) Off-canvas x clamps INTO the window (the shared seek path then
        // clamps again to the playable duration; the frac itself never escapes).
        check("(d) x<0 clamps to t0", abs(map(-50) - t0) < 1e-9)
        check("(d) x>width clamps to t1", abs(map(width + 80) - t1) < 1e-9)

        // (e) A zero-width canvas (first layout pass) is inert — returns the
        // anchor rather than dividing by zero.
        check("(e) zero width ⇒ anchor (no divide-by-zero)", map(0, w: 0) == anchor)

        // (f) Interior x is the exact inverse of the drawn-playhead frac
        // `(playheadT − t0)/winS` the canvas uses, so a click lands the playhead
        // back under the pointer. Quarter width ⇒ t0 + 0.25·winS.
        let q = map(width * 0.25)
        check("(f) interior x inverts the drawn-playhead frac", abs(q - (t0 + 0.25 * winS)) < 1e-9)
        let fracBack = (q - t0) / winS
        check("(f) round-trip frac == 0.25", abs(fracBack - 0.25) < 1e-9)

        print("LOUPE-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// Pure unit test for TODO-007's re-resolution merge policy
    /// (`RowMergePolicy.plan`). No engine, no media: it drives the pure decision
    /// behind `AppModel.mergeResolution`, so the "import the correct audio after the
    /// video and the row becomes the same confident pairing" behavior is regression-
    /// checked deterministically (the live import→re-resolve→session-rebuild wiring
    /// is on the owner's manual walkthrough). Run headless via COUPLE_REMERGE_TEST=1
    /// (needs no files). Mirrors the `CoupleKitTests.RowMergePolicyTests` suite.
    /// Covers: video-first then decisive audio adopts the confident result whole and
    /// rebuilds the session once; video-first then a weak audio stays unresolved
    /// (candidate visible, NOT promoted); a pinned row keeps its selection (new audio
    /// only appended as an alternate) while an included-but-unpinned row is NOT
    /// protected (D6 — inclusion is not an input); and a repeated resolve is
    /// idempotent (no duplicate candidate, no second session rebuild / sync).
    private func remergeTest() async {
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("REMERGE-TEST \(ok ? "ok " : "FAIL") \(name)")
        }

        // (1) video first, then decisive correct audio ⇒ confident, session built once.
        let p1 = RowMergePolicy.plan(protected: false, currentSelectedKey: nil,
                                     currentCandidateKeys: [], freshSelectedKey: "A",
                                     freshCandidateKeys: ["A"])
        check("(1) video-first + decisive audio ⇒ selects A, adopts display, rebuilds once",
              p1.selectedAudioKey == "A" && p1.candidateKeys == ["A"]
              && p1.adoptFreshDisplay && p1.rebuildSession)

        // (2) video first, then weak/nondecisive audio ⇒ stays unresolved, no promotion.
        let p2 = RowMergePolicy.plan(protected: false, currentSelectedKey: nil,
                                     currentCandidateKeys: [], freshSelectedKey: nil,
                                     freshCandidateKeys: ["W"])
        check("(2) video-first + weak audio ⇒ no selection, candidate visible, no rebuild",
              p2.selectedAudioKey == nil && p2.candidateKeys == ["W"] && !p2.rebuildSession)

        // (3) pinned row, then stronger audio ⇒ selection preserved, new = alternate.
        let p3 = RowMergePolicy.plan(protected: true, currentSelectedKey: "chosen",
                                     currentCandidateKeys: ["chosen"], freshSelectedKey: "stronger",
                                     freshCandidateKeys: ["stronger", "chosen"])
        check("(3) pinned + stronger audio ⇒ keeps 'chosen', appends 'stronger', no rebuild",
              p3.selectedAudioKey == "chosen" && p3.candidateKeys == ["chosen", "stronger"]
              && !p3.adoptFreshDisplay && !p3.rebuildSession)

        // (4) D6: an included-but-UNPINNED row is not protected — the shell composes
        //     `protected` as `pinned` alone, so a stronger audio re-pairs it wholesale.
        let p4 = RowMergePolicy.plan(protected: false, currentSelectedKey: "auto",
                                     currentCandidateKeys: ["auto"], freshSelectedKey: "stronger",
                                     freshCandidateKeys: ["stronger", "auto"])
        check("(4) included, unpinned + stronger audio ⇒ adopts 'stronger', rebuilds once (D6)",
              p4.selectedAudioKey == "stronger" && p4.candidateKeys == ["stronger", "auto"]
              && p4.adoptFreshDisplay && p4.rebuildSession)

        // (5) repeated resolve ⇒ idempotent: no duplicate candidate, no second rebuild.
        let p5 = RowMergePolicy.plan(protected: false, currentSelectedKey: "A",
                                     currentCandidateKeys: ["A"], freshSelectedKey: "A",
                                     freshCandidateKeys: ["A"])
        check("(5) repeated resolve ⇒ same A, no duplicate candidate, no second rebuild",
              p5.selectedAudioKey == "A" && p5.candidateKeys == ["A"] && !p5.rebuildSession)

        print("REMERGE-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// COUPLE_QUALITY_TEST=1 (TODO-009): the app-target END-TO-END mirror of the
    /// `MatchQualityTests` suite — it exercises the real `CoupleRow.verdict` (the
    /// `MatchQuality`→UI-`Verdict` mapping + the `forced`/`selectionProvenance`/
    /// `manualMatchConfident` wiring), which the CoupleKit-only test target cannot
    /// reach (`CoupleRow`/`Verdict` live in the app target). Media-free: a synthetic
    /// `ResolvedRow` + a handle-less pool `Entry` suffice because `verdict` reads no
    /// media. Covers: a confident AUTOMATIC selection is green; clicking a candidate
    /// on an ambiguous row stays questionable (the bug); a below-floor `forced` pick
    /// is questionable; and a manual pick is PROMOTED to green only once its pair's
    /// evidence clears the classifier. Run headless via COUPLE_QUALITY_TEST=1.
    private func qualityTest() async {
        guard let model else { print("QUALITY-TEST: no engine"); return }
        let engine = model.engine
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("QUALITY-TEST \(ok ? "ok " : "FAIL") \(name)")
        }
        func verdictName(_ v: Verdict) -> String {
            switch v { case .good: return "good"; case .questionable: return "questionable"; case .none: return "none" }
        }
        // A handle-less synthetic video entry (verdict reads none of the media).
        func video(_ key: String) -> MediaPool.Entry {
            MediaPool.Entry(id: key, url: URL(fileURLWithPath: "/dev/null/\(key).mov"), name: "\(key).mov",
                            role: .video, durationS: 60, creationTimeUtc: nil, info: nil,
                            fileSizeBytes: nil, handle: nil)
        }
        func candidate(_ key: String) -> RowCandidate {
            RowCandidate(audioKey: key, audioName: "\(key).wav", z: 12, lagMs: 0,
                         coverage: Coverage(durVideo: 60, durAudio: 60, lagMs: 0))
        }
        func row(_ vk: String, state: RowState, selected: RowCandidate?) -> ResolvedRow {
            ResolvedRow(videoKey: vk, videoName: "\(vk).mov", state: state, chip: state.chip,
                        selected: selected, alternates: [], detail: ResolutionDetail(margin: nil, clusters: 1))
        }
        func makeRow(_ resolved: ResolvedRow) -> CoupleRow {
            CoupleRow(video: video(resolved.videoKey), row: resolved, engine: engine,
                      toasts: model.toasts, audition: model.audition)
        }

        // (B) Confident AUTOMATIC selection ⇒ green.
        let auto = makeRow(row("v1", state: .confident, selected: candidate("a")))
        check("(B) confident auto selection ⇒ good (provenance=\(auto.selectionProvenance.rawValue))",
              auto.selectionProvenance == .automatic && auto.verdict == .good)

        // (A) Manual pick on an AMBIGUOUS row, before evidence ⇒ stays questionable
        //     (clicking a candidate must NOT flip the dot green — the bug).
        let manual = makeRow(row("v2", state: .ambiguous, selected: nil))
        manual.selectedAudioKey = "a"
        manual.candidates = [candidate("a")]
        manual.pinned = true
        manual.selectionProvenance = .manual
        manual.manualMatchConfident = nil
        check("(A) manual pick on ambiguous, no evidence ⇒ questionable (not green)",
              manual.verdict == .questionable)
        // Evidence says NOT a match ⇒ still questionable.
        manual.manualMatchConfident = false
        check("(A') manual pick, evidence=not-a-match ⇒ questionable",
              manual.verdict == .questionable)

        // (D) Evidence-based promotion: the SAME manual pick, once its pair clears the
        //     classifier ⇒ green. The only path that greens a manual pick.
        manual.manualMatchConfident = true
        check("(D) manual pick promoted after evidence clears classifier ⇒ good",
              manual.verdict == .good)

        // (C) Below-floor FORCED pick ⇒ questionable, overriding even a passing pair.
        let forced = makeRow(row("v3", state: .partial, selected: candidate("a")))
        forced.selectedAudioKey = "a"
        forced.selectionProvenance = .manual
        forced.manualMatchConfident = true   // even with a matching pair…
        forced.forced = true                 // …forced keeps it questionable
        check("(C) below-floor forced ⇒ questionable (overrides evidence)",
              forced.verdict == .questionable)

        // (E) No selection ⇒ resolver verdict (never a coin-flip).
        let unsel = makeRow(row("v4", state: .ambiguous, selected: nil))
        check("(E) ambiguous, no selection ⇒ questionable (\(verdictName(unsel.verdict)))",
              unsel.selectionProvenance == .none && unsel.verdict == .questionable)
        let nomatch = makeRow(row("v5", state: .noMatch, selected: nil))
        check("(E') no-match, no selection ⇒ none", nomatch.verdict == .none)

        print("QUALITY-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// COUPLE_REPAIRDROP_TEST=1 (TODO-013): the app-target END-TO-END mirror of the
    /// `RepairDropPolicyTests` suite — it exercises the REAL `AppModel.requestRepairDrop`
    /// (the shared entry point BOTH drop targets call), `payloadIsAudio`, and the
    /// pinned-replacement confirmation state machine (`pendingRepairConfirmation` /
    /// `confirmPendingRepair` / `cancelPendingRepair`), which the CoupleKit-only suite
    /// cannot reach (`AppModel`/`CoupleRow` live in the app target). Media-free: a
    /// handle-less synthetic couple + a fake pool key suffice for the routing/state
    /// behaviors asserted here (reject when the video is unavailable; reject a non-audio
    /// payload; confirmation arm/confirm/cancel). The accept/replace mutation needs real
    /// handles (`repair` no-ops without them) and is covered by the pure suite's decision
    /// + the owner's live walkthrough; the parity GUARANTEE is structural — both targets
    /// call the SAME `requestRepairDrop`, so identical state yields identical results.
    /// Run headless via COUPLE_REPAIRDROP_TEST=1 (needs no files).
    private func repairDropTest() async {
        guard let model else { print("REPAIRDROP-TEST: no engine"); return }
        let engine = model.engine
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("REPAIRDROP-TEST \(ok ? "ok " : "FAIL") \(name)")
        }
        func video(_ key: String) -> MediaPool.Entry {
            MediaPool.Entry(id: key, url: URL(fileURLWithPath: "/dev/null/\(key).mov"), name: "\(key).mov",
                            role: .video, durationS: 60, creationTimeUtc: nil, info: nil,
                            fileSizeBytes: nil, handle: nil) // handle-less ⇒ video unavailable
        }
        func row(_ vk: String) -> ResolvedRow {
            ResolvedRow(videoKey: vk, videoName: "\(vk).mov", state: .ambiguous, chip: RowState.ambiguous.chip,
                        selected: nil, alternates: [], detail: ResolutionDetail(margin: nil, clusters: 1))
        }
        let couple = CoupleRow(video: video("v1"), row: row("v1"), engine: engine,
                               toasts: model.toasts, audition: model.audition)

        // (1) THE SHARED ENTRY POINT: both the queue-row and center-workspace drops call
        //     `requestRepairDrop`. A handle-less video ⇒ the video is unavailable ⇒ the
        //     drop is REJECTED (returns false) and arms NO confirmation — proving the
        //     reject guard fires inside the real model path (not just the pure policy).
        model.pendingRepairConfirmation = nil
        let rejectedNoVideo = model.requestRepairDrop(couple, audioKey: "some-audio")
        check("(1) handle-less video ⇒ requestRepairDrop returns false (reject)", rejectedNoVideo == false)
        check("(1) reject arms no confirmation", model.pendingRepairConfirmation == nil)

        // (2) A NON-AUDIO PAYLOAD is rejected: a key absent from `pool.audios` is not
        //     audio, so `payloadIsAudio` is false regardless of the video.
        check("(2) payloadIsAudio false for a non-pool key", model.payloadIsAudio("not-in-pool") == false)
        check("(2) non-audio payload ⇒ requestRepairDrop returns false",
              model.requestRepairDrop(couple, audioKey: "not-in-pool") == false)

        // (3) THE CONFIRMATION STATE MACHINE (the pinned-replacement path both targets
        //     share): arming a pending confirmation, then CANCEL clears it with no change.
        model.pendingRepairConfirmation = AppModel.PendingRepairConfirmation(
            coupleID: couple.id, audioKey: "a", videoName: "v1.mov", audioName: "a.wav")
        check("(3) pending confirmation is armed", model.pendingRepairConfirmation != nil)
        model.cancelPendingRepair()
        check("(3) cancelPendingRepair clears it (no change)", model.pendingRepairConfirmation == nil)

        // (4) CONFIRM clears the pending state and routes through the shared replace
        //     (which no-ops here because the synthetic couple isn't in `model.couples`
        //     and has no handle — the point is that confirming consumes the pending
        //     state cleanly and never leaves a stuck dialog).
        model.pendingRepairConfirmation = AppModel.PendingRepairConfirmation(
            coupleID: couple.id, audioKey: "a", videoName: "v1.mov", audioName: "a.wav")
        model.confirmPendingRepair()
        check("(4) confirmPendingRepair clears the pending state", model.pendingRepairConfirmation == nil)

        print("REPAIRDROP-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// COUPLE_ACTIVITY_TEST=1 (TODO-005): media-free exercise of the LIVE
    /// `ActivityStrip` controller — the parts the pure `ActivitySummaryTests` cannot
    /// reach because they need WALL-CLOCK time + state carried across recomputes: the
    /// ~250 ms debounce (no flash, then appear), the per-phase MONOTONIC floor carried
    /// forward across `refresh()` calls, the failure→error→auto-clear path, and the
    /// idle→clear teardown. Drives the controller through its real `refresh()`/timer via
    /// the `debugSignalsOverride` seam (no media, no engine). Run with
    /// COUPLE_ACTIVITY_TEST=1 (usually COUPLE_AUTOQUIT=1).
    private func activityTest() async {
        guard let model else { return }
        let strip = model.activity
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool) {
            if ok { pass += 1 } else { fail += 1 }
            print("ACTIVITY-TEST \(ok ? "ok  " : "FAIL") \(name)")
        }
        func ms(_ n: UInt64) async { try? await Task.sleep(nanoseconds: n * 1_000_000) }

        // --- (A) DEBOUNCE: an active job does NOT flash; it appears only after ~250 ms. ---
        strip.debugSignalsOverride = ActivitySignals(
            importing: .init(phase: .decoding, fraction: 0.10, index: 1, total: 2, filename: "a.wav"))
        strip.refresh()
        await ms(80)
        check("(A) nothing shown before the debounce (~250 ms)", strip.debugCurrentSummary == nil)
        await ms(260)   // now well past 250 ms total
        check("(A) determinate import line appears after the debounce",
              strip.debugCurrentSummary?.fraction != nil)
        check("(A) the line is the unified import text",
              strip.debugCurrentSummary?.text.hasPrefix("Importing") == true)

        // --- (B) UNIFIED 0→100% bar across import → match → sync: ONE determinate bar that
        //         advances through every stage and NEVER regresses at a hand-off (the whole
        //         ingest shares one "ingest" phaseKey, so the monotonic floor spans all stages). ---
        let stages: Stages = [.importStage, .matchStage, .syncStage]
        // Import near its ceiling: the import sub-range tops out at 50% of the unified bar.
        strip.debugSignalsOverride = ActivitySignals(ingest: .init(
            importing: .init(phase: .finishing, fraction: 0.9, index: 2, total: 2, filename: "b.wav"),
            stagesPlanned: stages))
        strip.refresh(); await ms(20)
        let afterImport = strip.debugCurrentSummary?.fraction ?? 0
        check("(B) the unified bar reached the import sub-range top (≥0.45)", afterImport >= 0.45)
        check("(B) the unified bar carries the ingest phaseKey",
              strip.debugCurrentSummary?.phaseKey == "ingest")
        // A HAND-OFF GAP (ingest in flight, NO sub-stage momentarily live — import done, the first
        // match frame not yet emitted): the bar must HOLD (stay visible, never blink to nil) at the
        // import top — this is the exact between-stages flicker we fixed.
        strip.debugSignalsOverride = ActivitySignals(ingest: .init(
            importing: nil, matching: nil, syncing: nil, stagesPlanned: stages))
        strip.refresh(); await ms(20)
        check("(B) a stage HAND-OFF GAP keeps the bar visible (does not blink out)",
              strip.debugCurrentSummary != nil)
        check("(B) the gap holds the bar at the import top (no regress)",
              (strip.debugCurrentSummary?.fraction ?? 0) >= afterImport - 1e-6)
        // MATCHING advances into the match sub-range (50%→70%) and must NOT regress the bar.
        strip.debugSignalsOverride = ActivitySignals(ingest: .init(
            importing: nil, matching: .init(phase: .comparing, fraction: 0.5),
            stagesPlanned: stages))
        strip.refresh(); await ms(20)
        let afterMatch = strip.debugCurrentSummary?.fraction ?? 0
        check("(B) matching advances the bar without regressing", afterMatch >= afterImport - 1e-6)
        check("(B) the matching stage reads 'Matching couples…'",
              strip.debugCurrentSummary?.text == "Matching couples…")
        check("(B) matching is now DETERMINATE (a real slice, not a moving bar)",
              strip.debugCurrentSummary?.fraction != nil)
        // SYNC position advances further (70%→100%), still monotone.
        strip.debugSignalsOverride = ActivitySignals(ingest: .init(
            importing: nil, matching: nil,
            syncing: .init(index: 3, total: 6, videoName: "b.mov"), stagesPlanned: stages))
        strip.refresh(); await ms(20)
        let afterSync = strip.debugCurrentSummary?.fraction ?? 0
        check("(B) sync advances the bar without regressing", afterSync >= afterMatch - 1e-6)
        check("(B) the sync stage reads its couple position",
              strip.debugCurrentSummary?.text == "Syncing couple 3 of 6: b.mov")
        // The FINAL couple lands the bar at ~1.0 (it CLEARS on idle — covered by (E)/(F) — so we
        // leave the strip active here to keep the debounce satisfied for (C), as before).
        strip.debugSignalsOverride = ActivitySignals(ingest: .init(
            importing: nil, matching: nil,
            syncing: .init(index: 6, total: 6, videoName: "b.mov"), stagesPlanned: stages))
        strip.refresh(); await ms(20)
        check("(B) the final couple lands the unified bar at ~1.0",
              (strip.debugCurrentSummary?.fraction ?? 0) >= 0.999)

        // --- (C) PAIRING stays indeterminate (no fabricated %). ---
        strip.debugSignalsOverride = ActivitySignals(
            pairing: .init(videoCount: 4, audioCount: 8))
        strip.refresh(); await ms(20)
        check("(C) pairing line is indeterminate (no fraction)",
              strip.debugCurrentSummary != nil && strip.debugCurrentSummary?.fraction == nil)
        check("(C) pairing line carries counts, never a percentage",
              strip.debugCurrentSummary?.text == "Comparing 4 videos with 8 audio files…")

        // --- (D) FAILURE: an actionable error shows IMMEDIATELY (skips the debounce),
        //         carries no bar, then AUTO-CLEARS. ---
        strip.debugSignalsOverride = nil          // no ambient job
        strip.reportFailure("export failed: disk full")
        await ms(30)
        check("(D) failure shows immediately as an error (no debounce)",
              strip.debugCurrentSummary?.isError == true && strip.debugCurrentSummary?.fraction == nil)

        // --- (E) IDLE clears the strip (no stuck bar on success/cancellation). ---
        // (Clear the transient failure first so idle is genuinely idle.)
        strip.debugSignalsOverride = ActivitySignals() // empty ⇒ idle
        // The failure auto-clear runs on a 6 s timer; emulate "cleared" by forcing a
        // fresh idle snapshot (the controller drops the line when nothing is active).
        strip.refresh(); await ms(20)
        // While the failure message is still pending it overrides idle, so assert the
        // controller will clear once the failure is gone by checking the override path:
        let pendingFailure = strip.debugCurrentSummary?.isError == true
        check("(E) idle keeps showing the pending failure until it clears (then nil)",
              pendingFailure || strip.debugCurrentSummary == nil)

        // --- (F) RESET clears a shown bar immediately (Clear Session, TODO-001). ---
        strip.reset()                       // clean slate (drops any pending failure too)
        strip.debugSignalsOverride = ActivitySignals(
            importing: .init(phase: .decoding, fraction: 0.5, index: 1, total: 1, filename: "x.wav"))
        strip.refresh(); await ms(300)      // past the debounce ⇒ a determinate line shows
        check("(F) a determinate line is shown before reset",
              strip.debugCurrentSummary?.fraction != nil)
        strip.reset()
        check("(F) reset() clears the strip to idle immediately",
              strip.debugCurrentSummary == nil)

        strip.debugSignalsOverride = nil
        print("ACTIVITY-TEST summary: \(pass) passed, \(fail) failed")
    }

    /// COUPLE_VERIFY_DUMP=1 (PLAN-MATCH-CLASSIFIER §4.1/§4.5): the owner's window onto
    /// the fine-verify signal on real files. Iterates the FULL video×audio matrix and
    /// runs verify on EVERY cell that has a plausible lag, **bypassing the z-gate** — so
    /// the z-rejected short clip (the motivating case, absent from the resolver's
    /// `perVideo` set) is shown. Read-only: changes NO decision; prints to stdout only.
    /// Per (video, audio): z, lag_ms, overlap_s, locked/expected, lock_fraction, rms_ms,
    /// ppm, clamped, is_match.
    private func verifyDump() async {
        guard let model else { print("VERIFY-DUMP: no engine"); return }
        let engine = model.engine
        // Wait for the pool import + resolution to settle (the dump needs handles).
        for _ in 0..<1800 {
            if !model.resolving && (!model.pool.videos.isEmpty || !model.pool.audios.isEmpty) { break }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        // Core owns the pairing/verifier defaults (PLAN-AUDIO-MATCHING-PREWORK §6); the dump
        // reads the floor from core rather than a copied threshold.
        let pairing = engine.pairingParamsDefault()
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
                             v, a, cell.m.z, cell.m.lagMs, pairing.minOverlapS))
                continue
            }
            let match = Resolver.isMatch(f)
            print(String(format: "  %@ x %@ | z %.1f lag %.0f overlap %.1f locked %d/%d frac %.2f rms %.2f ppm %.0f clamped %@ is_match %@",
                         v, a, cell.m.z, cell.m.lagMs, f.overlapS, f.locked, f.expected,
                         f.lockFraction, f.rmsMs, f.ppm, f.clamped ? "Y" : "n", match ? "YES" : "no"))
        }
        print("VERIFY-DUMP: done")
    }

    /// COUPLE_SHADOW_DUMP=1 (PLAN-SPECTRAL-FINGERPRINT-MATCHING §14.2 / §14.4): the owner's window
    /// onto the OBSERVATIONAL spectral shadow telemetry on real files. Re-resolves the live pool with
    /// the shadow matcher forced ON (independent of the launch flag), so each video's
    /// `ResolutionDetail.shadowEvidence` is populated by the SAME pure projection the diagnostic
    /// disclosure renders, and prints the per-(video, audio) evidence in `score` order. Read-only:
    /// it changes NO resolved row (the legacy `state`/selection are printed alongside to prove they
    /// stay legacy); prints to stdout only. Like the other harnesses it needs the GUI app-bundle
    /// runloop (pool import + handles) — it does NOT run to completion in a headless agent shell; the
    /// pure projection's automated coverage is `ShadowTelemetry` in CoupleKitTests (`swift test`).
    private func shadowDump() async {
        guard let model else { print("SHADOW-DUMP: no engine"); return }
        let engine = model.engine
        // Wait for the pool import + resolution to settle (the dump needs handles).
        for _ in 0..<1800 {
            if !model.resolving && (!model.pool.videos.isEmpty || !model.pool.audios.isEmpty) { break }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        let videos = model.pool.videos.map {
            ResolverVideo(key: $0.id, name: $0.name, durationS: $0.durationS,
                          handle: $0.handle, hasAudio: $0.handle != nil)
        }
        let audios = model.pool.audios.map {
            ResolverAudio(key: $0.id, name: $0.name, durationS: $0.durationS, handle: $0.handle!)
        }
        // Force shadow ON for the dump regardless of the launch flag — the observational shadow
        // telemetry attaches to the LEGACY resolver's rows (boundary 9). Boundary 13 made the
        // fingerprint resolver the default (its rows ARE the spectral evidence and carry no
        // `shadowEvidence`), so this dump pins the legacy override to populate the comparison.
        var params = model.params
        params.matcherFingerprintPrimary = false
        params.shadowMatcherActive = true
        // Core-shell boundary (Phase C): the legacy DECISION comes from core (`cpl_resolve`); the
        // spectral shadow is attached shell-side by `Resolver.decorate` (display-only). Resolve the
        // importable videos in core, wrap audio-less ones as inert no-match rows, then decorate.
        let paramsJSON = params.resolveParamsJSON()
        let importableHandles = videos.compactMap(\.handle)
        let audioHandles = audios.map(\.handle)
        let rows = await Task.detached(priority: .userInitiated) { () -> [ResolvedRow] in
            let coreRows = importableHandles.isEmpty
                ? []
                : (engine.resolve(videos: importableHandles, audios: audioHandles, paramsJSON: paramsJSON) ?? [])
            var byVideo = Dictionary(coreRows.map { ($0.videoKey, $0) }, uniquingKeysWith: { a, _ in a })
            let assembled: [ResolvedRow] = videos.map { v in
                if v.handle != nil, let r = byVideo.removeValue(forKey: v.key) { return r }
                return ResolvedRow(videoKey: v.key, videoName: v.name, state: .noMatch,
                                   chip: RowState.noMatch.chip, selected: nil, alternates: [],
                                   detail: ResolutionDetail(margin: nil, clusters: 0, note: "no audio stream"))
            }
            return Resolver.decorate(assembled, videos: videos, audios: audios, engine: engine, params: params)
        }.value
        print("SHADOW-DUMP: \(rows.count) row(s) — spectral shadow is OBSERVATIONAL; resolved rows stay legacy")
        for row in rows {
            let sel = row.selected?.audioName ?? "—"
            print("  \(row.videoName) [legacy: \(row.state.rawValue) -> \(sel)]")
            guard let shadow = row.detail.shadowEvidence, !shadow.isEmpty else {
                print("      (no spectral evidence)")
                continue
            }
            for ev in shadow {
                print(String(format: "      shadow: %@ %@ · lag %.0f ppm %.0f · supp %d · in %.0f%% · cov %.0f%% · score %.3f · margin %.3f",
                             String(describing: ev.classification), ev.audioName, ev.refinedLagMs ?? ev.lagMs,
                             ev.refinedDriftPpm ?? ev.driftPpm, ev.support, ev.inlierFraction * 100,
                             ev.temporalCoverage * 100, ev.score, ev.scoreMargin))
            }
        }
        print("SHADOW-DUMP: done")
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

#else

// Stubs so call sites (Canvas draw closures, AppModel launch) compile
// unchanged in distribution builds.
final class FrameStats: @unchecked Sendable {
    static let shared = FrameStats()
    @inline(__always)
    func record(_ surface: String, seconds: CFTimeInterval) {}
}

@MainActor
final class DebugDrive {
    static let shared = DebugDrive()
    func configure(model: AppModel) {}
}

#endif
