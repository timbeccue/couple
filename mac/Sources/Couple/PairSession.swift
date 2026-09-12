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
    case loaded(TrackHandle)

    var track: TrackHandle? {
        if case .loaded(let t) = self { return t }
        return nil
    }
}

enum SyncActivity: Equatable {
    case none                                    // virgin: no sync has run
    case running(label: String, fraction: Double)
    case succeeded
    case failed(message: String)                 // Failed status or job error
}

/// One anchor and everything measured at it (time, loupe frame, last local
/// measurement + the L(tᵢ) it was taken at). Held as a single element so the
/// insert/remove/reorder sites can't drift the four facts out of lockstep;
/// anchors stay in chronological order by `t` (UX §2.2). Persistence keeps the
/// core's parallel-array wire shape (AlignmentState) — this struct is the shell
/// render shadow, mapped to/from that shape at the push/restore boundary.
struct AnchorPoint {
    var t: Double
    var frame: Frame
    var measurement: LocalMeasurement?
    /// L(tᵢ) at the time of the measurement attempt (drives the >30 ms staleness rule).
    var measurementCenter: Double?
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

    // Alignment shadow (canonical copy lives in core)
    var offsetMs: Double = 0
    var driftPpm: Double = 0
    var driftMode: DriftMode = .resample
    var savedDriftPpm: Double = 0
    var verified = false
    var lockZoom = true
    var loupeLayout: LoupeLayout = .grid
    var anchors: [AnchorPoint] = []
    /// Overview strip visible range (TODO-010): independent of loupe window sizes
    /// and of `lockZoom`, and NOT persisted (a fresh fitted view per launch /
    /// couple-switch is the spec'd behavior). `nil` ⇒ fitted to the full union
    /// (`stageDomain`); the viewport helpers in WaveformStage.swift read/mutate this.
    var overviewRange: ClosedRange<Double>?
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

    /// Audition / playback is a single shared controller across N couples (SPEC-V2
    /// §5.3), owned by AppModel and injected here — a PairSession references it
    /// (so the v1 editor's `pair.audition` keeps working) but never owns or frees
    /// it; AppModel attaches it to whichever couple is selected.
    let audition: AuditionController

    // Sync
    var syncActivity: SyncActivity = .none
    var lastSync: SyncResult?
    /// The selected candidate's resolver evidence (lag + acceptance score), captured at
    /// `load` so auto-sync can be SEEDED from it instead of rerunning the §5 stage-1
    /// blind scan just to fail its z gate (PLAN-SPECTRAL-FINGERPRINT-MATCHING §11.3).
    /// nil when the selection carries no evidence; whether/what to seed is
    /// `SyncSeedPolicy.plan`'s call, not this class's.
    @ObservationIgnored private var evidenceLagMs: Double?
    @ObservationIgnored private var evidenceZ: Double?
    @ObservationIgnored private var evidenceDriftPpm: Double?
    private var syncJob: Job<SyncResult>?
    /// The in-flight auto-sync Task, so the per-couple serial queue (§5.2) can
    /// await one couple's sync before starting the next.
    @ObservationIgnored private var syncTask: Task<Void, Never>?
    private var remeasureTask: Task<Void, Never>?

    init(engine: Engine, toasts: ToastCenter, audition: AuditionController) {
        self.engine = engine
        self.toasts = toasts
        self.audition = audition
    }

    /// Release the core session when this couple's selection changes or the app
    /// tears down. Tracks are owned by the MediaPool (§5.1) — never freed here.
    func teardown() {
        cancelSync()
        session?.free()
        session = nil
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
        guard anchors.indices.contains(i), let m = anchors[i].measurement else { return nil }
        return m.lagMs - mapping.localLagMs(anchors[i].t)
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

    // MARK: - Loading (SPEC-V2 §5.1 two-handles init; tracks owned by the pool)

    /// Build a couple's core session from two pool-owned tracks. Replaces v1's
    /// probe→import→sessionReadyCheck path (the pool now owns that). Does NOT
    /// auto-run sync — the per-couple serial queue (§5.2) drives that, so N
    /// couples don't fire N concurrent syncs. A single video+audio drop ends up
    /// as one couple whose sync runs immediately, behaving exactly like v1.
    ///
    /// `evidence` is the selected candidate the caller paired on (nil when there is
    /// none) — its lag/z/ppm seed auto-sync via `SyncSeedPolicy` (§11.3). A manual
    /// placeholder candidate (z 0) is safe to pass; the policy rejects it as evidence.
    func load(cameraTrack: TrackHandle, externalTrack: TrackHandle,
              evidence: RowCandidate? = nil) {
        teardown()
        resetAlignment()
        evidenceLagMs = evidence?.lagMs
        evidenceZ = evidence?.z
        evidenceDriftPpm = evidence?.ppm
        camera = .loaded(cameraTrack)
        external = .loaded(externalTrack)
        do {
            session = try engine.sessionNew(camera: cameraTrack, external: externalTrack)
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
        driftWindows = []
        overviewRange = nil // fresh fitted overview per load (TODO-010)
        lastSync = nil
        syncActivity = .none
    }

    // MARK: - Auto-sync

    /// `announce` toasts the result. v2 auto-sync is ambient (§V2.4): the per-
    /// couple serial queue runs silent (the verdict dot + fit-status line carry
    /// the result), so N couples don't stack N toasts. An explicit user re-sync
    /// passes announce: true.
    func runAutoSync(announce: Bool = true) {
        guard let session, !isSyncRunning else { return }
        do {
            // §11.3 seeding: core decides whether this run begins from the row's
            // accepted evidence lag, from the user's coarse-dragged placement (explicit
            // re-sync after a failure — the rescue the failure toast promises), or from
            // nothing (the blind stage-1 scan, unchanged). announce ≡ explicit: only a
            // user-invoked re-sync toasts, and only it may adopt manual placement.
            var lastFailed = false
            if let last = lastSync, case .failed = last.status { lastFailed = true }
            let seed = SyncSeedPolicy.plan(
                evidenceLagMs: evidenceLagMs, evidenceZ: evidenceZ,
                evidenceDriftPpm: evidenceDriftPpm,
                explicit: announce, lastSyncFailed: lastFailed, currentOffsetMs: offsetMs)
            let job = try session.autoSyncStart(seed: seed)
            syncJob = job
            syncActivity = .running(label: "Searching…", fraction: 0)
            syncTask = Task {
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
                    self.applySyncResult(result, announce: announce)
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

    /// Await the in-flight auto-sync (for the §5.2 serial per-couple queue). The
    /// Task self-cleans on completion; awaiting a finished/absent sync is a no-op.
    func awaitSync() async {
        await syncTask?.value
    }

    private func applySyncResult(_ result: SyncResult, announce: Bool = true) {
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
            if announce {
                if case .synced = result.status {
                    toasts.show(Toast(style: .success, title: "Auto-sync complete", detail: detail))
                } else {
                    toasts.show(Toast(style: .warning,
                                      title: "Low-confidence result — please review",
                                      detail: detail))
                }
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
            if announce {
                toasts.show(Toast(style: .error, title: "Auto-sync failed",
                                  detail: msg + " — coarse-drag the external lane to rescue, then re-sync"))
            }
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
            // Core's persisted shape stays parallel arrays (AlignmentState); the
            // shell holds them as one struct per anchor and splits them here.
            s.anchors = self.anchors.map(\.t)
            s.frames = self.anchors.map(\.frame)
            s.perAnchorMeasurements = self.anchors.map(\.measurement)
            s.lockZoom = self.lockZoom
            s.loupeLayout = self.loupeLayout
        }
    }

    // MARK: - Anchors (UX §2.2)

    /// Automatic anchor proposal (TODO-011): anchors must ALWAYS lie inside the
    /// current video/external overlap. The count is decided on USABLE OVERLAP
    /// duration by the pure `AnchorProposal.propose` — overlap ≥ 30 s ⇒ three
    /// zoned anchors; 0 < overlap < 30 s ⇒ two well-separated non-degenerate
    /// onsets; no usable overlap (or no two usable candidates) ⇒ ZERO (the
    /// manual-rescue state), never a misleading midpoint and never exactly one.
    /// (Removes the old fall-back to the full camera duration, which placed
    /// anchors where the external source had no content.)
    func proposeAnchors() {
        let picks = AnchorProposal.propose(overlap: overlap,
                                           candidates: anchorCandidates())
        anchors = picks.map { t in
            AnchorPoint(t: t, frame: Frame(winMs: Self.defaultWinMs, auto: true),
                        measurement: nil, measurementCenter: nil)
        }
    }

    /// `true` when automatic proposal could not place any anchor inside the
    /// overlap (no usable overlap, or no two non-degenerate candidates). The UI
    /// surfaces this as the manual-rescue state (anchors must be placed by hand).
    var anchorsUnavailable: Bool { bothLoaded && anchors.isEmpty }

    /// Automatic "+ anchor" (TODO-011 / UX §2.2): the strongest usable onset
    /// inside the overlap, ≥ 8%-of-overlap from existing anchors, cap 4. NEVER
    /// chooses an onset outside the overlap; adds nothing when none qualifies.
    func addAnchor() {
        guard anchors.count < 4 else { return }
        guard let t = AnchorProposal.addAnchorCandidate(overlap: overlap,
                                                        existing: anchors.map(\.t),
                                                        candidates: anchorCandidates())
        else { return }
        insertAnchor(at: t)
    }

    /// Build the ranked onset candidates for the proposer, marking each `usable`
    /// iff it has non-silent structure in BOTH mapped lanes — the camera lane at
    /// the onset time and the external lane at the mapped time `u(t)` (TODO-011:
    /// reject flat silence / one-sided content). Reuses the engine's existing
    /// score-ranked onset list (UX §4.5); no new matching math. With no overlap
    /// or no camera track there are no candidates (⇒ the proposer returns []).
    private func anchorCandidates() -> [AnchorProposal.Candidate] {
        guard let cam = camera.track, let ov = overlap else { return [] }
        let ext = external.track
        let onsets = cam.onsets(t0: ov.lowerBound, t1: ov.upperBound, limit: 400)
        return onsets.map { o in
            AnchorProposal.Candidate(t: o.t, score: o.score,
                                     usable: laneHasContent(cam, at: o.t)
                                         && (ext.map { laneHasContent($0, at: mapping.u(o.t)) } ?? false))
        }
    }

    /// A lane has judgeable content around a candidate when its short-window RMS
    /// is above a silence floor (a held/periodic passage still has level, so this
    /// is the cheap "not flat silence / not one-sided" gate; the onset score
    /// already favors sharp, isolated transients over sustained wiggles).
    private func laneHasContent(_ track: TrackHandle, at t: Double) -> Bool {
        guard t >= 0, t <= track.durationS else { return false }
        let half = 0.25 // 0.5 s window centered on the candidate
        guard let dbfs = track.rms(t0: max(0, t - half), t1: min(track.durationS, t + half))
        else { return false }
        return dbfs > Self.anchorSilenceFloorDb
    }

    /// RMS floor (dBFS) below which a lane window is treated as silence for anchor
    /// usability. −55 dBFS clears noise floors while rejecting genuine silence.
    static let anchorSilenceFloorDb = -55.0

    private func insertAnchor(at t: Double) {
        let idx = anchors.firstIndex(where: { $0.t > t }) ?? anchors.count
        anchors.insert(AnchorPoint(t: t,
                                   frame: Frame(winMs: currentLockedWindow ?? Self.defaultWinMs, auto: true),
                                   measurement: nil, measurementCenter: nil), at: idx)
        measureAnchor(idx)
        autoFrame()
        pushColdState()
    }

    func removeAnchor(_ i: Int) {
        guard anchors.count > 2, anchors.indices.contains(i) else { return }
        anchors.remove(at: i)
        pushColdState()
    }

    /// Live drag: move without snapping (loupes update live).
    func moveAnchor(_ i: Int, to t: Double) {
        guard anchors.indices.contains(i), let cam = camera.track else { return }
        anchors[i].t = min(max(t, 0), cam.durationS)
    }

    /// Explicit ⌖ snap: jump to the strongest transient within ±4 s. Drag
    /// release no longer snaps — the pin stays exactly where the user put it
    /// (owner decision 2026-06-12, overriding UX §2.2 snap-on-release).
    func snapAnchor(_ i: Int) {
        guard anchors.indices.contains(i), let cam = camera.track else { return }
        if let onset = cam.bestOnset(near: anchors[i].t, halfSpan: 4) {
            anchors[i].t = onset.t
        }
        releaseAnchor(i)
    }

    /// Drag release: keep the user's placement; re-measure and re-frame
    /// (anchor move invalidates its measurement).
    func releaseAnchor(_ i: Int) {
        guard anchors.indices.contains(i) else { return }
        // Keep chronological order (UX §2.2).
        let t = anchors[i].t
        let sortedIndex = anchors.map(\.t).sorted().firstIndex(of: t)!
        if sortedIndex != i {
            // The move invalidates the measurement; the loupe frame is kept.
            var moved = anchors.remove(at: i)
            moved.measurement = nil
            moved.measurementCenter = nil
            anchors.insert(moved, at: sortedIndex)
            measureAnchor(sortedIndex)
        } else {
            anchors[i].measurement = nil
            measureAnchor(i)
        }
        autoFrame()
        pushColdState()
    }

    // MARK: - Measurement (UX §2.4 / §4.3)

    func measureAnchor(_ i: Int) {
        guard let session, anchors.indices.contains(i) else { return }
        let center = mapping.localLagMs(anchors[i].t)
        let m = session.measureLocal(tc: anchors[i].t, centerMs: center, spanMs: 80)
        anchors[i].measurement = m
        anchors[i].measurementCenter = center
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
                let center = self.anchors[i].measurementCenter
                let lNow = self.mapping.localLagMs(self.anchors[i].t)
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
        let dt = anchors[i].t - anchors[r].t
        guard abs(dt) >= 5 else { return } // guard |tᵢ−t_r| < 5 s
        let slope = delta / dt                 // ms per second
        if driftMode == .none { driftMode = .resample }
        driftPpm += slope * 1000
        offsetMs -= anchors[r].t * slope
        session?.setDriftPpmLive(driftPpm)
        session?.setOffsetMs(offsetMs)
        mappingChanged()
    }

    /// ⇥ align: zero this anchor's measured Δ (pivot math; plain offset when it
    /// *is* the reference). Re-applies auto-framing (UX §2.3).
    func alignAnchor(_ i: Int) {
        guard inputsEnabled, let delta = displayedDelta(i) else { return }
        let r = pivotReference(dragging: i)
        if i == r || anchors.count < 2 || abs(anchors[i].t - anchors[r].t) < 5 {
            setOffsetLive(offsetMs + delta)
        } else {
            pivotDrag(anchorIndex: i, deltaLMs: delta)
            commitDrift()
        }
        mappingChanged(immediate: true)
        autoFrame()
    }

    // MARK: - Loupe windows: auto-framing + zoom lock (UX §2.3)

    /// Default loupe window. 500 ms (was the UX §2.3 240 ms): the owner found
    /// the tighter default too zoomed-in to judge alignment at a glance
    /// (2026-06-12, SPEC-CHANGE logged in PROGRESS.md).
    static let defaultWinMs = 500.0

    var currentLockedWindow: Double? {
        lockZoom ? anchors.map(\.frame.winMs).max() : nil
    }

    /// Auto-frame: winᵢ = max(default, 5·|Δᵢ|) rounded to 10 ms, cap 2 s.
    /// Applied on load, auto-sync, anchor move, ⇥ align — never on plain nudges.
    func autoFrame() {
        guard !anchors.isEmpty else { return }
        func desired(_ i: Int) -> Double {
            let d = displayedDelta(i).map(abs) ?? 0
            let w = max(Self.defaultWinMs, 5 * d)
            return min((w / 10).rounded() * 10, 2000)
        }
        if lockZoom {
            let autos = anchors.indices.filter { anchors[$0].frame.auto }
            guard !autos.isEmpty else { return }
            let w = autos.map(desired).max()!
            for i in anchors.indices {
                anchors[i].frame.winMs = w
            }
        } else {
            for i in anchors.indices where anchors[i].frame.auto {
                anchors[i].frame.winMs = desired(i)
            }
        }
    }

    /// Manual scroll-zoom: exits auto mode (all loupes under lock; one when unlocked).
    func manualZoom(loupe i: Int, factor: Double) {
        guard anchors.indices.contains(i) else { return }
        let clamp = { (w: Double) in min(max(w, 1.0), 8000.0) }
        if lockZoom {
            let w = clamp(anchors[i].frame.winMs * factor)
            for j in anchors.indices {
                anchors[j].frame.winMs = w
                anchors[j].frame.auto = false
            }
        } else {
            anchors[i].frame.winMs = clamp(anchors[i].frame.winMs * factor)
            anchors[i].frame.auto = false
        }
    }

    /// Clicking the auto chip re-arms (all loupes under lock).
    func armAuto(loupe i: Int) {
        guard anchors.indices.contains(i) else { return }
        if lockZoom {
            for j in anchors.indices {
                anchors[j].frame.auto = true
            }
        } else {
            anchors[i].frame.auto = true
        }
        autoFrame()
    }

    func toggleLockZoom() {
        lockZoom.toggle()
        if lockZoom, let w = anchors.map(\.frame.winMs).max() {
            for i in anchors.indices {
                anchors[i].frame.winMs = w // unify to current maximum (UX §2.3)
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

    struct ExportError: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    /// Awaited export for the batch coordinator (§6). Identical options and
    /// pipeline to the single export above — so a batch is byte-identical to the
    /// Phase-4 CLI export, couple by couple — but serialized via `await`.
    /// format/keepCameraAudio come from the couple's effective settings; drift
    /// mode lives in the core session and is honored unchanged.
    func exportAwait(to url: URL, format: ExportFormat, keepCameraAudio: Bool,
                     onProgress: @escaping (Double) -> Void) async throws {
        guard let session, verified else { throw ExportError(message: "not approved") }
        let opts: [String: Any] = [
            "kind": format == .wav ? "wav" : "mux",
            "outputPath": url.path,
            "keepCameraAudio": keepCameraAudio && format != .wav,
            "container": format == .mov ? "mov" : format == .mp4 ? "mp4" : "auto",
            "faststart": false,
            "overwrite": true,
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: opts),
              let json = String(data: data, encoding: .utf8) else {
            throw ExportError(message: "bad export options")
        }
        let job = try session.exportStart(optionsJSON: json)
        exportJob = job
        exportActivity = .running(phase: "ExportRender", fraction: 0)
        let progressTask = Task {
            for await p in job.progress {
                self.exportActivity = .running(phase: p.phase, fraction: p.fraction)
                onProgress(p.fraction)
            }
        }
        defer { progressTask.cancel(); exportActivity = .idle; exportJob = nil }
        _ = try await job.value()
        autosave()
    }

    // MARK: - Autosave / restore (§11.5, §14 item 6.8)

    func autosave() {
        guard let session, let cam = camera.track, let ext = external.track,
              let state = try? session.state() else { return }
        SessionStore.save(camera: cam.meta, external: ext.meta, alignment: state)
    }

    /// Apply this pair's saved per-pair alignment (SessionStore) if present — the
    /// §5.4 restore path, used by the queue-level restore so manual edits and the
    /// approved state come back, not just a fresh re-sync. Returns false when no
    /// saved file exists (the caller then auto-syncs instead).
    @discardableResult
    func restoreSavedAlignment() -> Bool {
        guard let cam = camera.track, let ext = external.track,
              let saved = SessionStore.load(cameraKey: cam.meta.contentKey,
                                            externalKey: ext.meta.contentKey) else { return false }
        restorable = saved
        applyRestored(announce: false)
        return true
    }

    func applyRestored(announce: Bool = true) {
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
        // Reassemble the shell's per-anchor struct from core's parallel arrays;
        // measurement centers are recomputed against the just-restored mapping.
        anchors = a.anchors.enumerated().map { i, t in
            AnchorPoint(t: t,
                        frame: i < a.frames.count ? a.frames[i] : Frame(winMs: Self.defaultWinMs, auto: true),
                        measurement: i < a.perAnchorMeasurements.count ? a.perAnchorMeasurements[i] : nil,
                        measurementCenter: mapping.localLagMs(t))
        }
        driftWindows = a.driftWindows
        syncActivity = driftWindows.isEmpty ? .none : .succeeded
        if announce { toasts.show(Toast(style: .success, title: "Session restored")) }
    }
}
