// PairSession: the observable model behind the editor. Core owns canonical
// alignment state (SPEC §4.5); this is the render shadow plus all interaction
// logic from UX §2 (nudges, pivot drift, anchors, auto-framing, debounced
// re-measurement). Deliberately list-ready for v2 (SPEC §11.1).

import AppKit
import CoupleKit
import Foundation
import Observation

enum SlotKind: String {
    case camera, external
}

enum SlotState {
    case empty
    case probing(filename: String)
    case importing(filename: String, phase: String, fraction: Double)
    case loaded(TrackHandle)

    var track: TrackHandle? {
        if case .loaded(let t) = self { return t }
        return nil
    }

    var isBusy: Bool {
        switch self {
        case .probing, .importing: return true
        default: return false
        }
    }
}

enum SyncActivity: Equatable {
    case none                                    // virgin: no sync has run
    case running(label: String, fraction: Double)
    case succeeded
    case failed(message: String)                 // Failed status or job error
}

struct PendingLoad: Identifiable {
    let id = UUID()
    let url: URL
    let role: TrackRole
    let filename: String
}

@MainActor
@Observable
final class PairSession {
    let engine: Engine
    let toasts: ToastCenter

    // Slots
    var camera: SlotState = .empty
    var external: SlotState = .empty
    var session: SessionHandle?
    var loadError: String?
    var pendingReplace: PendingLoad?

    // Alignment shadow (canonical copy lives in core)
    var offsetMs: Double = 0
    var driftPpm: Double = 0
    var driftMode: DriftMode = .resample
    var savedDriftPpm: Double = 0
    var verified = false
    var lockZoom = true
    var loupeLayout: LoupeLayout = .grid
    var anchors: [Double] = []
    var frames: [Frame] = []
    var measurements: [LocalMeasurement?] = []
    /// L(tᵢ) at the time of each measurement attempt (drives the >30 ms staleness rule).
    var measurementCenters: [Double?] = []
    var driftWindows: [DriftWindowState] = []

    // Export options (consumed by the Phase-6 export job)
    enum ExportFormat: String, CaseIterable {
        case auto = "Auto"
        case mov = "MOV"
        case mp4 = "MP4"
        case wav = "WAV"
    }

    var exportFormat: ExportFormat = .auto
    var keepCameraAudio = false

    enum ExportActivity: Equatable {
        case idle
        case running(phase: String, fraction: Double)
    }

    var exportActivity: ExportActivity = .idle
    private var exportJob: Job<ExportReport>?
    /// Saved session found for this pair's content keys (restore offer, §14 6.8).
    private var restorable: SessionFile?

    // Audition / playback (Phase 6)
    let audition = AuditionController()

    // Sync
    var syncActivity: SyncActivity = .none
    var lastSync: SyncResult?
    private var syncJob: Job<SyncResult>?
    private var importJobs: [SlotKind: Job<TrackMeta>] = [:]
    private var remeasureTask: Task<Void, Never>?

    init(engine: Engine, toasts: ToastCenter) {
        self.engine = engine
        self.toasts = toasts
    }

    /// Release core handles when this pair is replaced ("+ add pair").
    func teardown() {
        audition.teardown()
        cancelSync()
        cancelImport(.camera)
        cancelImport(.external)
        session?.free()
        session = nil
        camera.track?.free()
        external.track?.free()
        camera = .empty
        external = .empty
    }

    // MARK: - Derived

    var mapping: Mapping { Mapping(offsetMs: offsetMs, driftPpm: driftPpm) }

    var bothLoaded: Bool { camera.track != nil && external.track != nil }

    var isSyncRunning: Bool {
        if case .running = syncActivity { return true }
        return false
    }

    /// Alignment inputs are disabled while auto-sync runs (UX §3).
    var inputsEnabled: Bool { session != nil && !isSyncRunning }

    var pairName: String {
        let c = camera.track.map { URL(fileURLWithPath: $0.meta.info.path).deletingPathExtension().lastPathComponent }
        let e = external.track.map { URL(fileURLWithPath: $0.meta.info.path).deletingPathExtension().lastPathComponent }
        switch (c, e) {
        case let (c?, e?): return "\(c) × \(e)"
        case let (c?, nil): return c
        case let (nil, e?): return e
        default: return "—"
        }
    }

    var overlap: ClosedRange<Double>? {
        guard let cam = camera.track, let ext = external.track else { return nil }
        return mapping.overlap(camDur: cam.durationS, extDur: ext.durationS)
    }

    enum OverlapChip: Equatable {
        case noAlignment
        case noOverlap
        case overlap(fraction: Double, seconds: Double)
    }

    var overlapChip: OverlapChip {
        guard let cam = camera.track, external.track != nil else { return .noAlignment }
        if syncActivity == .none && offsetMs == 0 && driftPpm == 0 { return .noAlignment }
        guard let ov = overlap else { return .noOverlap }
        let secs = ov.upperBound - ov.lowerBound
        return .overlap(fraction: secs / cam.durationS, seconds: secs)
    }

    /// Displayed residual Δᵢ = λ(tᵢ) − L(tᵢ) (UX §2.1), recomputed against the
    /// *current* mapping every frame.
    func displayedDelta(_ i: Int) -> Double? {
        guard i < measurements.count, let m = measurements[i], i < anchors.count else { return nil }
        return m.lagMs - mapping.localLagMs(anchors[i])
    }

    /// Global fit status vs the current mapping (UX §2.4: one global RMS).
    var fitStatus: (rmsMs: Double, windows: Int)? {
        guard !driftWindows.isEmpty else { return nil }
        let residuals = driftWindows.map { $0.lagMs - mapping.localLagMs($0.t) }
        let rms = (residuals.reduce(0) { $0 + $1 * $1 } / Double(residuals.count)).squareRoot()
        return (rms, driftWindows.count)
    }

    var endToEndDriftMs: Double {
        guard let ov = overlap else { return 0 }
        let ppm = driftMode == .none ? savedDriftPpm : driftPpm
        return abs(ppm) * (ov.upperBound - ov.lowerBound) / 1000
    }

    // MARK: - Loading

    func handleDropped(urls: [URL]) {
        for url in urls {
            loadFile(url)
        }
    }

    func loadFile(_ url: URL) {
        loadError = nil
        let filename = url.lastPathComponent
        Task {
            do {
                let info = try await Task.detached { [engine] in
                    try engine.probe(url.path)
                }.value
                let role: TrackRole = info.video != nil ? .camera : .external
                guard !info.audio.isEmpty || role == .external else {
                    loadError = "\(filename): no audio stream"
                    return
                }
                let slot = role == .camera ? camera : external
                if slot.track != nil || slot.isBusy {
                    // Same-kind replacement needs confirmation (UX §3).
                    pendingReplace = PendingLoad(url: url, role: role, filename: filename)
                } else {
                    startImport(url: url, role: role)
                }
            } catch let e as CoreError {
                loadError = "\(filename): \(e.message)"
            } catch {
                loadError = "\(filename): \(error.localizedDescription)"
            }
        }
    }

    func confirmReplace(_ pending: PendingLoad) {
        pendingReplace = nil
        // Replacing a track invalidates the session.
        resetAlignment()
        session = nil
        startImport(url: pending.url, role: pending.role)
    }

    private func startImport(url: URL, role: TrackRole) {
        let kind: SlotKind = role == .camera ? .camera : .external
        let filename = url.lastPathComponent
        setSlot(kind, .importing(filename: filename, phase: "Decode", fraction: 0))
        do {
            let job = try engine.importStart(url.path, role: role)
            importJobs[kind] = job
            Task {
                for await p in job.progress {
                    if case .importing = self.slot(kind) {
                        self.setSlot(kind, .importing(filename: filename, phase: p.phase, fraction: p.fraction))
                    }
                }
                do {
                    let meta = try await job.value()
                    self.setSlot(kind, .loaded(self.engine.track(meta)))
                    self.importJobs[kind] = nil
                    self.sessionReadyCheck()
                } catch JobError.cancelled {
                    self.setSlot(kind, .empty)
                    self.importJobs[kind] = nil
                } catch {
                    self.setSlot(kind, .empty)
                    self.importJobs[kind] = nil
                    self.loadError = "\(filename): \((error as? JobError).map(String.init(describing:)) ?? error.localizedDescription)"
                }
            }
        } catch {
            setSlot(kind, .empty)
            loadError = "\(filename): \((error as? CoreError)?.message ?? error.localizedDescription)"
        }
    }

    func cancelImport(_ kind: SlotKind) {
        importJobs[kind]?.cancel()
    }

    func slot(_ kind: SlotKind) -> SlotState {
        kind == .camera ? camera : external
    }

    private func setSlot(_ kind: SlotKind, _ state: SlotState) {
        if kind == .camera { camera = state } else { external = state }
    }

    /// Both imports landed → create the session and auto-sync (UX §2.5, DECIDED).
    private func sessionReadyCheck() {
        guard session == nil, let cam = camera.track, let ext = external.track else { return }
        do {
            let s = try engine.sessionNew(camera: cam, external: ext)
            session = s
            audition.attach(session: s,
                            videoURL: URL(fileURLWithPath: cam.meta.info.path),
                            videoDuration: cam.durationS)
            offerRestoreIfAvailable()
            runAutoSync()
        } catch {
            loadError = "session: \((error as? CoreError)?.message ?? error.localizedDescription)"
        }
    }

    private func resetAlignment() {
        offsetMs = 0
        driftPpm = 0
        driftMode = .resample
        savedDriftPpm = 0
        verified = false
        anchors = []
        frames = []
        measurements = []
        measurementCenters = []
        driftWindows = []
        lastSync = nil
        syncActivity = .none
    }

    // MARK: - Auto-sync

    func runAutoSync() {
        guard let session, !isSyncRunning else { return }
        do {
            let job = try session.autoSyncStart()
            syncJob = job
            syncActivity = .running(label: "Searching…", fraction: 0)
            Task {
                for await p in job.progress {
                    let label: String
                    switch p.phase {
                    case "SyncCoarse": label = "Searching \(Int(p.fraction * 100))%…"
                    case "SyncRefine": label = "Refining…"
                    case "SyncWindows": label = "Measuring drift…"
                    default: label = "Fitting…"
                    }
                    if self.isSyncRunning {
                        self.syncActivity = .running(label: label, fraction: p.fraction)
                    }
                }
                do {
                    let result = try await job.value()
                    self.applySyncResult(result)
                } catch JobError.cancelled {
                    self.syncActivity = .none // back to unaligned unverified (UX §3)
                } catch {
                    self.syncActivity = .failed(message: (error as? JobError).map(String.init(describing:)) ?? error.localizedDescription)
                }
                self.syncJob = nil
            }
        } catch {
            syncActivity = .failed(message: (error as? CoreError)?.message ?? error.localizedDescription)
        }
    }

    func cancelSync() {
        syncJob?.cancel()
    }

    private func applySyncResult(_ result: SyncResult) {
        lastSync = result
        switch result.status {
        case .synced, .lowConfidence:
            // Core wrote mapping + driftWindows + suggested mode into the state (§4.4).
            pullStateFromCore()
            syncActivity = .succeeded
            // Toast with the numbers (UX §2.5); never auto-verifies.
            let accepted = result.windows.filter(\.accepted).count
            let detail = String(
                format: "offset %+.2f ms · %+.2f ppm · end-to-end %.1f ms · %d/%d windows · RMS %@ — %@",
                result.offsetMs, result.driftPpm, result.endToEndDriftMs,
                accepted, result.windows.count,
                result.fitRmsMs.map { String(format: "%.2f ms", $0) } ?? "—",
                result.suggestedDriftMode == .resample ? "suggested: resample" : "suggested: none")
            if case .synced = result.status {
                toasts.show(Toast(style: .success, title: "Auto-sync complete", detail: detail))
            } else {
                toasts.show(Toast(style: .warning,
                                  title: "Low-confidence result — please review",
                                  detail: detail))
            }
        case .failed(let reason, let detail):
            // Mapping untouched — no half-applied guess (UX §3).
            let msg: String
            if reason == "insufficientOverlap" {
                msg = String(format: "No usable overlap (%.0f s)", detail)
            } else {
                msg = String(format: "No plausible correlation peak (best z %.1f)", detail)
            }
            syncActivity = .failed(message: msg)
            toasts.show(Toast(style: .error, title: "Auto-sync failed",
                              detail: msg + " — coarse-drag the external lane to rescue, then re-sync"))
        }
        // Anchors are proposed only when none exist yet; a re-sync must not
        // disturb the view the user has set up (owner decision 2026-06-12,
        // overriding UX §2.2 "after every auto-sync" — SPEC-CHANGE logged).
        let firstProposal = anchors.isEmpty
        if firstProposal {
            proposeAnchors()
        }
        measureAllAnchors()
        if firstProposal {
            autoFrame()
        }
        pushColdState()
    }

    func pullStateFromCore() {
        guard let session, let s = try? session.state() else { return }
        offsetMs = s.offsetMs
        driftPpm = s.driftPpm
        driftMode = s.driftMode
        savedDriftPpm = s.savedDriftPpm
        verified = s.verified
        driftWindows = s.driftWindows
    }

    /// Mirror shell-owned fields into core state (persistence + CLI parity).
    private func pushColdState() {
        guard let session else { return }
        try? session.updateState { s in
            s.anchors = self.anchors
            s.frames = self.frames
            s.perAnchorMeasurements = self.measurements
            s.lockZoom = self.lockZoom
            s.loupeLayout = self.loupeLayout
        }
    }

    // MARK: - Anchors (UX §2.2)

    /// Three anchors at the sharpest attack inside each overlap zone
    /// [1,30] [38,62] [70,99] %, falling back to the camera duration.
    func proposeAnchors() {
        guard let cam = camera.track else { return }
        var span = 0.0...cam.durationS
        if let ov = overlap, ov.upperBound - ov.lowerBound > 30 {
            span = ov
        }
        let len = span.upperBound - span.lowerBound
        var picks: [Double] = []
        for zone in [(0.01, 0.30), (0.38, 0.62), (0.70, 0.99)] {
            let a = span.lowerBound + zone.0 * len
            let b = span.lowerBound + zone.1 * len
            picks.append(cam.bestOnset(in: a, b)?.t ?? (a + b) / 2)
        }
        picks.sort()
        anchors = picks
        frames = picks.map { _ in Frame(winMs: 240, auto: true) }
        measurements = picks.map { _ in nil }
        measurementCenters = picks.map { _ in nil }
    }

    /// "+ anchor": best unused onset keeping ≥ 8%-of-overlap separation, cap 4.
    func addAnchor() {
        guard anchors.count < 4, let cam = camera.track else { return }
        let span = overlap ?? 0...cam.durationS
        let len = span.upperBound - span.lowerBound
        let minSep = 0.08 * len
        let candidates = cam.onsets(t0: span.lowerBound, t1: span.upperBound, limit: 400)
        guard let pick = candidates.first(where: { c in anchors.allSatisfy { abs($0 - c.t) >= minSep } }) else { return }
        insertAnchor(at: pick.t)
    }

    private func insertAnchor(at t: Double) {
        let idx = anchors.firstIndex(where: { $0 > t }) ?? anchors.count
        anchors.insert(t, at: idx)
        frames.insert(Frame(winMs: currentLockedWindow ?? 240, auto: true), at: idx)
        measurements.insert(nil, at: idx)
        measurementCenters.insert(nil, at: idx)
        measureAnchor(idx)
        autoFrame()
        pushColdState()
    }

    func removeAnchor(_ i: Int) {
        guard anchors.count > 2, anchors.indices.contains(i) else { return }
        anchors.remove(at: i)
        frames.remove(at: i)
        measurements.remove(at: i)
        measurementCenters.remove(at: i)
        pushColdState()
    }

    /// Live drag: move without snapping (loupes update live).
    func moveAnchor(_ i: Int, to t: Double) {
        guard anchors.indices.contains(i), let cam = camera.track else { return }
        anchors[i] = min(max(t, 0), cam.durationS)
    }

    /// Explicit ⌖ snap: jump to the strongest transient within ±4 s. Drag
    /// release no longer snaps — the pin stays exactly where the user put it
    /// (owner decision 2026-06-12, overriding UX §2.2 snap-on-release).
    func snapAnchor(_ i: Int) {
        guard anchors.indices.contains(i), let cam = camera.track else { return }
        if let onset = cam.bestOnset(near: anchors[i], halfSpan: 4) {
            anchors[i] = onset.t
        }
        releaseAnchor(i)
    }

    /// Drag release: keep the user's placement; re-measure and re-frame
    /// (anchor move invalidates its measurement).
    func releaseAnchor(_ i: Int) {
        guard anchors.indices.contains(i) else { return }
        // Keep chronological order (UX §2.2).
        let t = anchors[i]
        let sortedIndex = anchors.sorted().firstIndex(of: t)!
        if sortedIndex != i {
            let frame = frames[i]
            anchors.remove(at: i)
            frames.remove(at: i)
            measurements.remove(at: i)
            measurementCenters.remove(at: i)
            anchors.insert(t, at: sortedIndex)
            frames.insert(frame, at: sortedIndex)
            measurements.insert(nil, at: sortedIndex)
            measurementCenters.insert(nil, at: sortedIndex)
            measureAnchor(sortedIndex)
        } else {
            measurements[i] = nil
            measureAnchor(i)
        }
        autoFrame()
        pushColdState()
    }

    // MARK: - Measurement (UX §2.4 / §4.3)

    func measureAnchor(_ i: Int) {
        guard let session, anchors.indices.contains(i) else { return }
        let center = mapping.localLagMs(anchors[i])
        let m = session.measureLocal(tc: anchors[i], centerMs: center, spanMs: 80)
        measurements[i] = m
        measurementCenters[i] = center
    }

    func measureAllAnchors() {
        for i in anchors.indices {
            measureAnchor(i)
        }
    }

    /// Called after every mapping mutation: re-measure stale anchors on a 450 ms
    /// debounce once the mapping has moved > 30 ms from a measurement's center.
    func mappingChanged(immediate: Bool = false) {
        remeasureTask?.cancel()
        guard session != nil else { return }
        let work = { @MainActor in
            var any = false
            for i in self.anchors.indices {
                let center = self.measurementCenters.indices.contains(i) ? self.measurementCenters[i] : nil
                let lNow = self.mapping.localLagMs(self.anchors[i])
                if center == nil || abs(lNow - center!) > 30 {
                    self.measureAnchor(i)
                    any = true
                }
            }
            if any { self.pushColdState() }
        }
        if immediate {
            work()
        } else {
            remeasureTask = Task {
                try? await Task.sleep(nanoseconds: 450_000_000)
                guard !Task.isCancelled else { return }
                work()
            }
        }
    }

    // MARK: - Mapping mutations (UX §2.4)

    func setOffsetLive(_ v: Double) {
        guard let session else { return }
        offsetMs = v
        session.setOffsetMs(v)
        mappingChanged()
    }

    func nudgeOffset(_ deltaMs: Double) {
        guard inputsEnabled else { return }
        setOffsetLive(offsetMs + deltaMs)
    }

    func commitTypedOffset(_ v: Double) {
        guard inputsEnabled else { return }
        offsetMs = v
        session?.setOffsetMs(v)
        mappingChanged(immediate: true) // re-measure on commit of typed values
    }

    func setDriftLive(_ v: Double) {
        guard let session else { return }
        if driftMode == .none {
            driftMode = .resample // mirror of the core-enforced UX §1.5 rule
        }
        driftPpm = v
        session.setDriftPpmLive(v)
        mappingChanged()
    }

    func nudgeDrift(_ deltaPpm: Double) {
        guard inputsEnabled else { return }
        setDriftLive(driftPpm + deltaPpm)
        commitDrift()
    }

    func commitTypedDrift(_ v: Double) {
        guard inputsEnabled else { return }
        setDriftLive(v)
        commitDrift()
        mappingChanged(immediate: true)
    }

    func commitDrift() {
        session?.commitDrift()
    }

    func setDriftMode(_ mode: DriftMode) {
        guard let session else { return }
        session.setDriftMode(mode)
        pullStateFromCore() // None zeroes live drift / Resample restores saved (core-enforced)
        mappingChanged()
    }

    func toggleVerified() {
        guard session != nil else { return }
        verified.toggle()
        session?.setVerified(verified)
        autosave()
    }

    // MARK: - Pivot drift drag and align (UX §2.4, DECIDED math)

    /// Reference anchor: anchor 1 (leftmost), or the rightmost when dragging anchor 1.
    func pivotReference(dragging i: Int) -> Int {
        i == 0 ? anchors.count - 1 : 0
    }

    /// δ = desired change of L(tᵢ) in ms. Rotates the mapping line about the reference.
    func pivotDrag(anchorIndex i: Int, deltaLMs delta: Double) {
        guard inputsEnabled, anchors.indices.contains(i) else { return }
        let r = pivotReference(dragging: i)
        let dt = anchors[i] - anchors[r]
        guard abs(dt) >= 5 else { return } // guard |tᵢ−t_r| < 5 s
        let slope = delta / dt                 // ms per second
        if driftMode == .none { driftMode = .resample }
        driftPpm += slope * 1000
        offsetMs -= anchors[r] * slope
        session?.setDriftPpmLive(driftPpm)
        session?.setOffsetMs(offsetMs)
        mappingChanged()
    }

    /// ⇥ align: zero this anchor's measured Δ (pivot math; plain offset when it
    /// *is* the reference). Re-applies auto-framing (UX §2.3).
    func alignAnchor(_ i: Int) {
        guard inputsEnabled, let delta = displayedDelta(i) else { return }
        let r = pivotReference(dragging: i)
        if i == r || anchors.count < 2 || abs(anchors[i] - anchors[r]) < 5 {
            setOffsetLive(offsetMs + delta)
        } else {
            pivotDrag(anchorIndex: i, deltaLMs: delta)
            commitDrift()
        }
        mappingChanged(immediate: true)
        autoFrame()
    }

    // MARK: - Loupe windows: auto-framing + zoom lock (UX §2.3)

    var currentLockedWindow: Double? {
        lockZoom ? frames.map(\.winMs).max() : nil
    }

    /// Auto-frame: winᵢ = max(240, 5·|Δᵢ|) rounded to 10 ms, cap 2 s. Applied on
    /// load, auto-sync, anchor move, ⇥ align — never on plain nudges.
    func autoFrame() {
        guard !frames.isEmpty else { return }
        func desired(_ i: Int) -> Double {
            let d = displayedDelta(i).map(abs) ?? 0
            let w = max(240.0, 5 * d)
            return min((w / 10).rounded() * 10, 2000)
        }
        if lockZoom {
            let autos = frames.indices.filter { frames[$0].auto }
            guard !autos.isEmpty else { return }
            let w = autos.map(desired).max()!
            for i in frames.indices {
                frames[i].winMs = w
            }
        } else {
            for i in frames.indices where frames[i].auto {
                frames[i].winMs = desired(i)
            }
        }
    }

    /// Manual scroll-zoom: exits auto mode (all loupes under lock; one when unlocked).
    func manualZoom(loupe i: Int, factor: Double) {
        guard frames.indices.contains(i) else { return }
        let clamp = { (w: Double) in min(max(w, 1.0), 8000.0) }
        if lockZoom {
            let w = clamp((frames[i].winMs) * factor)
            for j in frames.indices {
                frames[j].winMs = w
                frames[j].auto = false
            }
        } else {
            frames[i].winMs = clamp(frames[i].winMs * factor)
            frames[i].auto = false
        }
    }

    /// Clicking the auto chip re-arms (all loupes under lock).
    func armAuto(loupe i: Int) {
        guard frames.indices.contains(i) else { return }
        if lockZoom {
            for j in frames.indices {
                frames[j].auto = true
            }
        } else {
            frames[i].auto = true
        }
        autoFrame()
    }

    func toggleLockZoom() {
        lockZoom.toggle()
        if lockZoom, let w = frames.map(\.winMs).max() {
            for i in frames.indices {
                frames[i].winMs = w // unify to current maximum (UX §2.3)
            }
        }
        pushColdState()
    }

    func setLoupeLayout(_ layout: LoupeLayout) {
        loupeLayout = layout
        pushColdState()
    }

    // MARK: - Export (§8 via core job; gated on Verified, UX §2.8)

    var isExporting: Bool {
        if case .running = exportActivity { return true }
        return false
    }

    var suggestedExportFilename: String {
        guard let cam = camera.track else { return "synced" }
        let base = URL(fileURLWithPath: cam.meta.info.path).deletingPathExtension().lastPathComponent
        return "\(base)-synced.\(exportFileExtension)"
    }

    var exportFileExtension: String {
        switch exportFormat {
        case .wav: return "wav"
        case .mov: return "mov"
        case .mp4: return "mp4"
        case .auto:
            let container = camera.track.map { URL(fileURLWithPath: $0.meta.info.path).pathExtension.lowercased() }
            return container == "mov" ? "mov" : "mp4"
        }
    }

    func runExport(to url: URL) {
        guard let session, verified, !isExporting else { return }
        let opts: [String: Any] = [
            "kind": exportFormat == .wav ? "wav" : "mux",
            "outputPath": url.path,
            "keepCameraAudio": keepCameraAudio && exportFormat != .wav,
            "container": exportFormat == .mov ? "mov" : exportFormat == .mp4 ? "mp4" : "auto",
            "faststart": false,
            "overwrite": true, // the save panel already confirmed replacement
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: opts),
              let json = String(data: data, encoding: .utf8) else { return }
        do {
            let job = try session.exportStart(optionsJSON: json)
            exportJob = job
            exportActivity = .running(phase: "ExportRender", fraction: 0)
            Task {
                for await p in job.progress {
                    if self.isExporting {
                        self.exportActivity = .running(phase: p.phase, fraction: p.fraction)
                    }
                }
                do {
                    _ = try await job.value()
                    self.exportActivity = .idle
                    self.autosave()
                    self.toasts.show(Toast(
                        style: .success, title: "Export complete",
                        detail: url.lastPathComponent,
                        actionLabel: "Reveal",
                        action: { NSWorkspace.shared.activateFileViewerSelecting([url]) }
                    ), seconds: 10)
                } catch JobError.cancelled {
                    self.exportActivity = .idle
                } catch {
                    self.exportActivity = .idle
                    let msg = (error as? JobError).map(String.init(describing:)) ?? error.localizedDescription
                    self.toasts.show(Toast(style: .error, title: "Export failed", detail: msg), seconds: 10)
                }
                self.exportJob = nil
            }
        } catch {
            toasts.show(Toast(style: .error, title: "Export failed",
                              detail: (error as? CoreError)?.message ?? error.localizedDescription))
        }
    }

    func cancelExport() {
        exportJob?.cancel()
    }

    // MARK: - Autosave / restore (§11.5, §14 item 6.8)

    func autosave() {
        guard let session, let cam = camera.track, let ext = external.track,
              let state = try? session.state() else { return }
        SessionStore.save(camera: cam.meta, external: ext.meta, alignment: state)
    }

    private func offerRestoreIfAvailable() {
        guard let cam = camera.track, let ext = external.track,
              let saved = SessionStore.load(cameraKey: cam.meta.contentKey,
                                            externalKey: ext.meta.contentKey) else { return }
        restorable = saved
        let when = saved.savedAt.prefix(16).replacingOccurrences(of: "T", with: " ")
        toasts.show(Toast(
            style: .info, title: "Saved session found for this pair",
            detail: "\(when) · offset \(String(format: "%+.2f", saved.alignment.offsetMs)) ms"
                + (saved.alignment.verified ? " · verified" : ""),
            actionLabel: "Restore",
            action: { [weak self] in self?.applyRestored() }
        ), seconds: 15)
    }

    func applyRestored() {
        guard let session, let saved = restorable else { return }
        cancelSync()
        do {
            try session.setState(saved.alignment)
        } catch {
            toasts.show(Toast(style: .error, title: "Restore failed",
                              detail: (error as? CoreError)?.message ?? error.localizedDescription))
            return
        }
        let a = saved.alignment
        offsetMs = a.offsetMs
        driftPpm = a.driftPpm
        driftMode = a.driftMode
        savedDriftPpm = a.savedDriftPpm
        verified = a.verified
        lockZoom = a.lockZoom
        loupeLayout = a.loupeLayout
        anchors = a.anchors
        frames = a.frames
        measurements = a.perAnchorMeasurements
        measurementCenters = a.anchors.map { mapping.localLagMs($0) }
        driftWindows = a.driftWindows
        syncActivity = driftWindows.isEmpty ? .none : .succeeded
        toasts.show(Toast(style: .success, title: "Session restored"))
    }
}
