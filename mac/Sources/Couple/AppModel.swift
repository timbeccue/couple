// AppModel (SPEC-V2 §5.2) — the queue model. v1's single `pair` becomes a media
// pool + a list of couples + a selection. Each couple owns its PairSession;
// per-couple auto-sync runs in a serialized background queue so verdict dots
// settle progressively without N concurrent syncs. One shared AuditionController
// (§5.3) follows the selection. The v1 editor survives as the drill-down: the
// editor views read `model.pair`, which resolves to the selected couple's
// PairSession (or a harmless empty placeholder).
//
// The model is a FAÇADE over three owned sub-controllers: `ingest` (import→resolve
// orchestration + the serialized sync queue + the activity-strip signal cluster),
// `poolController` (media-pool selection + per-file removal / Clear Session), and
// `input` (NSEvent monitors + key/scroll routing). AppModel keeps the couples list +
// selection, audition, preview, approve/repair verbs, export settings, and persistence,
// and exposes thin forwarding accessors so views / ActivityStrip / DebugDrive reach the
// sub-controller members through `model.` unchanged.

import AppKit
import CoupleKit
import Observation
import SwiftUI

@MainActor
@Observable
final class AppModel {
    let engine: Engine?
    let engineError: String?
    let toasts = ToastCenter()
    let pool: MediaPool
    /// One audio engine / output / fullscreen window across N couples (§5.3),
    /// injected into every PairSession; only the selected couple is attached.
    let audition = AuditionController()
    /// Single source of truth for the media-pool audio preview (§V2.7, TODO-002):
    /// one active entry, play/pause, position, duration; surfaced in the pool's
    /// sticky bottom bar. Owned here so the teardown lifecycle can release it (see
    /// `willTerminate` below).
    let preview = PoolPreviewController()

    /// Import → resolve → couples orchestration + the serialized per-couple sync queue
    /// and the activity-strip signal cluster (IngestController).
    @ObservationIgnored lazy var ingest = IngestController(model: self)
    /// Media-pool Finder-style selection + per-file removal / Clear Session
    /// (PoolController).
    @ObservationIgnored lazy var poolController = PoolController(model: self)
    /// NSEvent key + scroll monitors and their routing (InputRouter).
    @ObservationIgnored lazy var input = InputRouter(model: self)

    var couples: [CoupleRow] = []
    var selectedID: CoupleRow.ID?

    /// The unified long-running activity strip (TODO-005). Owns the debounced summary
    /// of every background job (import / pairing / queued sync / export) shown beneath
    /// the queue header. Driven by `activity.refresh()` whenever a feeding signal
    /// changes; the honest single-line decision is the pure `ActivitySummaryPolicy`.
    @ObservationIgnored lazy var activity = ActivityStrip(model: self)

    /// A pending approved-replacement confirmation (TODO-013). Set when a manual
    /// audio drop targets an APPROVED couple with a different source; replacing it
    /// removes it from the export queue and clears the current pair session, so the
    /// drop asks first. Both drop targets (queue row + center workspace) route
    /// through the SAME `requestRepairDrop`, so they cannot diverge — and a single
    /// `.confirmationDialog` in RootView presents this regardless of which gesture
    /// armed it. `nil` ⇒ no dialog showing. The pure outcome decision is
    /// `RepairDropPolicy.decide`; this struct only carries the confirmed action.
    struct PendingRepairConfirmation: Identifiable {
        let id = UUID()
        let coupleID: CoupleRow.ID
        let audioKey: String
        let videoName: String
        let audioName: String
    }
    var pendingRepairConfirmation: PendingRepairConfirmation?

    var params: PairParams = {
        var p = PairParams()
        // App default: the fine-verify match classifier is ON (the CLI/gates keep
        // PairParams' `false` default so the suite tests the old path explicitly).
        // Override per launch with COUPLE_VERIFY_ACTIVE: =0 disables (A/B against the
        // old z-gate), =1 forces on. See PLAN-MATCH-CLASSIFIER.md.
        p.verifyClassifierActive = true
        if let v = ProcessInfo.processInfo.environment["COUPLE_VERIFY_ACTIVE"] {
            p.verifyClassifierActive = (v == "1")
        }
        // BOUNDARY 13 (§16.1): the app DEFAULT is the FINGERPRINT resolver (rows resolved from the
        // core graph-adjusted `PairEvidence`, classification the identity authority). The DEVELOPER
        // OVERRIDE back to the proven envelope resolver (retained ≥ 1 release) is COUPLE_MATCHER=legacy
        // (mirrors the CLI `--matcher legacy`). Any other value keeps the fingerprint default.
        if let v = ProcessInfo.processInfo.environment["COUPLE_MATCHER"] {
            p.matcherFingerprintPrimary = (v != "legacy")
        }
        // Boundary-9 OBSERVATIONAL spectral shadow telemetry (§14.2 / §14.4): OPT-IN, OFF BY DEFAULT.
        // Under the fingerprint default the rows ARE the spectral evidence; this attaches the shadow
        // `PairEvidence` to the diagnostic disclosure when the LEGACY override is active. The owner
        // turns it on per launch with COUPLE_SHADOW_MATCHER=1.
        if let v = ProcessInfo.processInfo.environment["COUPLE_SHADOW_MATCHER"] {
            p.shadowMatcherActive = (v == "1")
        }
        return p
    }()

    // Bottom-bar toggles (§V2.4): same-place on/off switches, nothing repositions.
    var syncToolsExpanded = false
    var loupesShown = false

    // Export rail (§V2.8 / §6); the batch coordinator lives in BatchExport.swift.
    // Naming is per-file (each couple's filename is edited in place); format is a
    // single global selector — no global template, no per-couple format override
    // (owner 2026-06-13).
    var exportDestination: URL?
    /// True while the destination is the auto-default (the most recent import's
    /// folder); a manual "Choose folder…" pins it and stops the auto-tracking.
    @ObservationIgnored var destinationAuto = true
    var globalFormat: PairSession.ExportFormat = .auto
    var globalKeepCameraAudio = false
    @ObservationIgnored lazy var batch = BatchExport(model: self)

    /// Editor target when no couple is selected — an empty PairSession so the
    /// editor views never see nil.
    @ObservationIgnored let placeholderPair: PairSession

    /// Loupe canvas frames in window coordinates, for scroll-wheel routing.
    @ObservationIgnored var loupeRects: [Int: CGRect] = [:]
    /// Overview wave-area frame in window coordinates (TODO-010): pointer-over-overview
    /// scrolls route to overview zoom/pan, pointer-over-loupe scrolls to the loupes.
    @ObservationIgnored var overviewRect: CGRect = .zero

    init() {
        var engine: Engine?
        var engineError: String?
        do {
            engine = try Engine()
        } catch {
            engineError = (error as? CoreError)?.message ?? error.localizedDescription
        }
        self.engine = engine
        self.engineError = engineError
        guard let engine else {
            fatalError("couple-core context failed: \(engineError ?? "?")")
        }
        self.pool = MediaPool(engine: engine, toasts: toasts)
        self.placeholderPair = PairSession(engine: engine, toasts: toasts, audition: audition)
        input.installMonitors()
        NotificationCenter.default.addObserver(
            forName: NSApplication.willTerminateNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.preview.stop() // release pool-preview playback before exit (TODO-002)
                self?.saveAll()
            }
        }
        DebugDrive.shared.configure(model: self)
        // Feed the unified activity strip from the pool's import progress (TODO-005);
        // the other signals refresh it from their own change points.
        pool.onImportChange = { [weak self] in self?.activity.refresh() }
        offerRestoreOnLaunch()
    }

    // MARK: - Façade forwarding (sub-controller members reached via `model.`)

    // IngestController (import → resolve → couples + sync queue + strip signals).
    func importDropped(_ urls: [URL]) { ingest.importDropped(urls) }
    var resolving: Bool { ingest.resolving }
    var matchingActivity: ActivitySignals.Matching? { ingest.matchingActivity }
    var ingestInFlight: Bool { ingest.ingestInFlight }
    var ingestStages: Stages { ingest.ingestStages }
    var resolveIsUpdateForStrip: Bool { ingest.resolveIsUpdateForStrip }
    var resolveIsImportTailForStrip: Bool { ingest.resolveIsImportTailForStrip }
    var syncIndex: Int { ingest.syncIndex }
    var syncTotal: Int { ingest.syncTotal }
    var syncVideoName: String? { ingest.syncVideoName }

    // PoolController (media-pool selection + removal / Clear Session).
    typealias PendingMediaRemoval = PoolController.PendingMediaRemoval
    typealias PendingClearSession = PoolController.PendingClearSession
    var poolSelection: Set<String> {
        get { poolController.poolSelection }
        set { poolController.poolSelection = newValue }
    }
    var poolPrimary: String? {
        get { poolController.poolPrimary }
        set { poolController.poolPrimary = newValue }
    }
    var poolPrimaryEntry: MediaPool.Entry? { poolController.poolPrimaryEntry }
    var poolDraggingKey: String? {
        get { poolController.poolDraggingKey }
        set { poolController.poolDraggingKey = newValue }
    }
    var poolDraggingName: String? { poolController.poolDraggingName }
    func selectPool(key: String, modifiers: NSEvent.ModifierFlags) {
        poolController.selectPool(key: key, modifiers: modifiers)
    }
    var pendingMediaRemoval: PendingMediaRemoval? { poolController.pendingMediaRemoval }
    func removeSelectedMedia() { poolController.removeSelectedMedia() }
    func confirmMediaRemoval() { poolController.confirmMediaRemoval() }
    func cancelMediaRemoval() { poolController.cancelMediaRemoval() }
    var pendingClearSession: PendingClearSession? { poolController.pendingClearSession }
    func requestClearSession() { poolController.requestClearSession() }
    func confirmClearSession() { poolController.confirmClearSession() }
    func cancelClearSession() { poolController.cancelClearSession() }

    // InputRouter (scroll-wheel zoom routing, also driven directly by tests).
    @discardableResult
    func zoom(at point: CGPoint, deltaY: CGFloat, precise: Bool) -> Bool {
        input.zoom(at: point, deltaY: deltaY, precise: precise)
    }

    // MARK: - Derived: selection + workflow sections (§V2.2)

    var selectedCouple: CoupleRow? { couples.first { $0.id == selectedID } }
    /// The editor's target (selected couple's pair, or the empty placeholder).
    var pair: PairSession { selectedCouple?.pair ?? placeholderPair }

    /// Visual queue order = the three sections concatenated; drives j/k.
    var orderedCouples: [CoupleRow] { queued + toApprove + toFix }
    var queued: [CoupleRow] { couples.filter(\.approved) }
    var toApprove: [CoupleRow] { couples.filter { !$0.approved && $0.verdict == .good } }
    var toFix: [CoupleRow] { couples.filter { !$0.approved && $0.verdict != .good } }

    var approvedCount: Int { queued.count }
    var hasContent: Bool { !pool.isEmpty }

    /// The user explicitly chose an export folder — pin it (stop auto-tracking).
    func chooseExportDestination(_ url: URL) {
        exportDestination = url
        destinationAuto = false
        saveQueue()
    }

    // MARK: - Selection + audition (§5.3)

    func select(_ id: CoupleRow.ID?) {
        selectedID = id
        loupeRects = [:]
        overviewRect = .zero
        guard let couple = selectedCouple else {
            audition.teardown()
            return
        }
        attachAudition(couple)
    }

    /// Detach/attach/swap (§5.3): exactly one audition handle open at a time. A
    /// couple with no session attaches video-only (camera-audio poster).
    func attachAudition(_ couple: CoupleRow) {
        guard let cam = couple.video.handle ?? couple.pair.camera.track else {
            audition.teardown()
            return
        }
        audition.attach(session: couple.pair.session,
                        videoURL: couple.video.url,
                        videoDuration: cam.durationS)
    }

    // MARK: - Approve (§V2.5) + manual re-pair (§V2.6)

    /// Approve ≡ queue for export. No audition gate (§11.4). Only a couple with a
    /// session can be approved.
    func toggleApprove(_ couple: CoupleRow) {
        guard couple.pair.session != nil else { return }
        couple.pair.toggleVerified()
        saveQueue()
    }

    /// Manual re-pair (§V2.6): point a couple at a chosen audio (candidate pick or
    /// pool drag). Pins the couple; runs a full sync; below-floor keeps the
    /// pairing but marks it `forced` (honest). Candidates are never destroyed —
    /// a brand-new audio is appended to the list.
    func repair(_ couple: CoupleRow, audioKey: String) {
        guard let audioEntry = pool.entry(key: audioKey), let audioHandle = audioEntry.handle,
              let cam = couple.video.handle else { return }
        couple.pinned = true
        couple.selectedAudioKey = audioKey
        // TODO-009: a deliberate pick is a MANUAL selection. Its match quality is NOT
        // assumed from the click — it stays questionable until this pair's evidence is
        // re-evaluated below (nil ⇒ not-yet-confident, so the dot does not flip green).
        couple.selectionProvenance = .manual
        couple.manualMatchConfident = nil
        if !couple.candidates.contains(where: { $0.audioKey == audioKey }) {
            couple.candidates.append(RowCandidate(
                audioKey: audioKey, audioName: audioEntry.name, z: 0, lagMs: 0,
                coverage: Coverage(durVideo: couple.video.durationS,
                                   durAudio: audioEntry.durationS, lagMs: 0)))
        }
        if couple.id == selectedID { audition.teardown() } // close stream before freeing old session
        couple.pair.load(cameraTrack: cam, externalTrack: audioHandle)
        if couple.id == selectedID { attachAudition(couple) }
        // Forced verdict if this pairing is below the floor (a candidate carries
        // its z; a fresh pool drag correlates on sync — checked on completion).
        // Decided by the core policy (SPEC-V2 §3.6); the app owns only the chip.
        let z = couple.candidates.first { $0.audioKey == audioKey }?.z ?? 0
        couple.forced = RepairDropPolicy.forced(candidateZ: z, zMin: params.zMin,
                                                syncFailed: false)
        couple.chip = couple.forced ? "forced — below floor" : couple.state.chip
        let engine = self.engine
        let params = self.params
        Task {
            // EVIDENCE for the dot (TODO-009): run the SAME confident classifier the
            // resolver uses on THIS exact pair, off-main. Only this can green a manual
            // pick — never the click, the sync, or approval. Independent of the
            // alignment fit below (a clean RMS does not imply a match).
            let confident = await Task.detached(priority: .userInitiated) { () -> Bool in
                guard let engine else { return false }
                return Resolver.manualPairIsConfident(video: cam, audio: audioHandle,
                                                       engine: engine, params: params)
            }.value
            couple.manualMatchConfident = confident
            couple.pair.runAutoSync(announce: false) // ambient; the dot/chip carry it
            await couple.pair.awaitSync()
            // A below-floor pool drag (z unknown until now) surfaces as forced
            // when the sync can't lock — the same core decision, re-run with the
            // sync outcome (never un-forces a below-floor pick).
            var syncFailed = false
            if case .failed = couple.pair.syncActivity { syncFailed = true }
            couple.forced = RepairDropPolicy.forced(candidateZ: z, zMin: params.zMin,
                                                    syncFailed: syncFailed)
            couple.chip = couple.forced ? "forced — below floor" : couple.state.chip
            saveQueue()
        }
        saveQueue()
    }

    /// Whether a dragged content key names an AUDIO pool entry (TODO-013). The pool
    /// publishes audio entries only once importable (a handle is the precondition),
    /// so this is the single source of truth both drop targets ask before pairing —
    /// a video key, the empty sentinel a non-audio `.draggable` carries, or junk all
    /// answer false.
    func payloadIsAudio(_ key: String) -> Bool {
        pool.audios.contains { $0.id == key }
    }

    /// The SHARED entry point for a manual audio drop onto a couple (TODO-013), used
    /// by BOTH the queue-row drop and the center-workspace drop so the two gestures
    /// cannot diverge. Decides the outcome with the pure `RepairDropPolicy`, then:
    ///   - `.reject`/`.noOp` ⇒ no state change (a non-audio/no-video/no-couple drop,
    ///     or re-dropping the already-selected source);
    ///   - `.accept` ⇒ replace now via the same `repair` (unresolved / unapproved);
    ///   - `.needsConfirmation` ⇒ arm the approved-replacement dialog (RootView shows
    ///     it); the replace happens only when the user confirms.
    /// Returns whether the drop was consumed (so `.dropDestination` reports the right
    /// value): true for accept/no-op/confirmation-armed, false for a rejected drop.
    @discardableResult
    func requestRepairDrop(_ couple: CoupleRow, audioKey: String) -> Bool {
        let outcome = RepairDropPolicy.decide(
            hasSelectedCouple: true,
            payloadIsAudio: payloadIsAudio(audioKey),
            videoAvailable: couple.video.handle != nil,
            currentSelectedKey: couple.selectedAudioKey,
            droppedKey: audioKey,
            approved: couple.approved)
        switch outcome {
        case .reject:
            return false
        case .noOp:
            return true
        case .accept:
            performRepairReplace(couple, audioKey: audioKey)
            return true
        case .needsConfirmation:
            let audioName = pool.entry(key: audioKey)?.name ?? audioKey
            pendingRepairConfirmation = PendingRepairConfirmation(
                coupleID: couple.id, audioKey: audioKey,
                videoName: couple.video.name, audioName: audioName)
            return true
        }
    }

    /// Apply an accepted replacement (TODO-013): clear stale approval/export status
    /// FIRST (replacing an approved couple removes it from the export queue), then run
    /// the SAME `repair` as every other manual pick — which selects+pins the audio,
    /// rebuilds the pair session, and runs full sync/classification. The verdict stays
    /// evidence-based (TODO-009): `repair` does NOT green the dot on the drop; only the
    /// re-run classifier can. Centralized so the queue-row and center drops are
    /// byte-identical.
    private func performRepairReplace(_ couple: CoupleRow, audioKey: String) {
        if couple.approved { couple.pair.toggleVerified() } // un-queue: clears export approval
        couple.exportStatus = .idle                          // drop any stale per-couple export result
        repair(couple, audioKey: audioKey)
    }

    /// Confirm the pending approved-replacement (TODO-013): replace the source. The
    /// couple is re-looked-up by id (it never moves, but this keeps the model the
    /// source of truth) and re-validated through the SAME pure policy, so a state
    /// change between arming and confirming can still cancel cleanly.
    func confirmPendingRepair() {
        guard let pending = pendingRepairConfirmation else { return }
        pendingRepairConfirmation = nil
        guard let couple = couples.first(where: { $0.id == pending.coupleID }),
              couple.video.handle != nil, payloadIsAudio(pending.audioKey),
              pending.audioKey != couple.selectedAudioKey else { return }
        performRepairReplace(couple, audioKey: pending.audioKey)
    }

    /// Cancel the pending approved-replacement (TODO-013): make no change.
    func cancelPendingRepair() {
        pendingRepairConfirmation = nil
    }

    /// TODO-009: (re)evaluate a MANUAL pick's match evidence off-main and update its
    /// dot, reusing the SAME confident classifier as `repair`/the resolver. Used by
    /// restore for a pre-009 manual pin whose evidence wasn't persisted, so a
    /// round-tripped questionable manual pick that DOES match still shows green
    /// without forcing a full re-sync. No-op when the pair's handles are gone.
    private func reevaluateManualMatch(_ couple: CoupleRow) {
        guard let engine, let cam = couple.video.handle,
              let key = couple.selectedAudioKey, let audio = pool.entry(key: key)?.handle else { return }
        let params = self.params
        Task {
            let confident = await Task.detached(priority: .userInitiated) {
                Resolver.manualPairIsConfident(video: cam, audio: audio, engine: engine, params: params)
            }.value
            guard couple.selectionProvenance == .manual, couple.selectedAudioKey == key else { return }
            couple.manualMatchConfident = confident
            saveQueue()
        }
    }

    // MARK: - Export settings

    /// Reset session-level export state on Clear Session (TODO-001). Per-couple
    /// `exportStatus` lives on the couples (already dropped by the caller); this resets
    /// the session destination back to auto-tracking so a fresh import re-defaults it.
    func resetExportSettings() {
        exportDestination = nil
        destinationAuto = true
        globalFormat = .auto
        globalKeepCameraAudio = false
    }

    // MARK: - Persistence (Task 9 fills couple-queue/1)

    func saveAll() {
        for couple in couples where couple.pair.session != nil { couple.pair.autosave() }
        saveQueue()
    }

    func saveQueue() {
        QueueStore.save(model: self)
    }

    /// On launch, offer to restore the previous queue (§5.4) — unless a debug
    /// drive is loading content explicitly. The offer is a non-blocking toast.
    func offerRestoreOnLaunch() {
        let env = ProcessInfo.processInfo.environment
        guard env["COUPLE_PAIR_DIR"] == nil, env["COUPLE_OPEN_CAMERA"] == nil,
              let saved = QueueStore.load() else { return }
        let when = saved.savedAt.prefix(16).replacingOccurrences(of: "T", with: " ")
        toasts.show(Toast(
            style: .info, title: "Restore previous session?",
            detail: "\(saved.couples.count) couple\(saved.couples.count == 1 ? "" : "s") · \(when)",
            actionLabel: "Restore",
            action: { [weak self] in self?.applyRestore(saved) }), seconds: 20)
    }

    /// Re-import the saved files, re-resolve, then overlay the saved per-couple
    /// selection (pins included), approved flags, per-couple alignment (from the
    /// unchanged per-pair autosave), and export settings (§5.4).
    func applyRestore(_ saved: SavedQueue) {
        Task {
            ingest.suppressAutoSync = true
            await ingest.ingest(saved.files.map { URL(fileURLWithPath: $0.path) })
            for sc in saved.couples {
                guard let c = couples.first(where: { $0.id == sc.videoKey }) else { continue }
                c.pinned = sc.pinned
                c.forced = sc.forced
                c.stemOverride = sc.stemOverride
                // Saved selection (may be a manual pin different from the fresh auto-pick).
                if sc.selectedAudioKey != c.selectedAudioKey {
                    c.selectedAudioKey = sc.selectedAudioKey
                    if let key = sc.selectedAudioKey,
                       !c.candidates.contains(where: { $0.audioKey == key }),
                       let e = pool.entry(key: key) {
                        c.candidates.append(RowCandidate(
                            audioKey: key, audioName: e.name, z: 0, lagMs: 0,
                            coverage: Coverage(durVideo: c.video.durationS, durAudio: e.durationS, lagMs: 0)))
                    }
                    ingest.ensureSession(for: c)
                }
                // TODO-009: restore selection provenance + the manual pair's match
                // evidence so the dot color round-trips. A pre-009 file lacks
                // provenance → infer it from `pinned` (a pinned pick is manual). The
                // saved evidence is trusted when present; an unknown manual evidence
                // (nil) is re-evaluated off-main below so the dot is never stale.
                let provenance = sc.selectionProvenance
                    ?? (c.selectedAudioKey == nil ? .none : (c.pinned ? .manual : .automatic))
                c.selectionProvenance = provenance
                c.manualMatchConfident = provenance == .manual ? sc.manualMatchConfident : nil
                // Prefer the user's saved per-pair alignment; else sync this couple.
                if !c.pair.restoreSavedAlignment(), c.pair.session != nil {
                    ingest.enqueueSync(c)
                }
                if sc.approved, c.pair.session != nil, !c.approved { c.pair.toggleVerified() }
                // Recompute a manual pick's evidence verdict only when it wasn't saved
                // (a back-compat restore), reusing the SAME classifier as `repair`.
                if provenance == .manual, c.manualMatchConfident == nil { reevaluateManualMatch(c) }
            }
            if let dest = saved.export.destination {
                exportDestination = URL(fileURLWithPath: dest)
                destinationAuto = false // a saved destination is treated as pinned
            }
            globalFormat = PairSession.ExportFormat(rawValue: saved.export.globalFormat) ?? .auto
            globalKeepCameraAudio = saved.export.keepCameraAudio
            ingest.suppressAutoSync = false
            if let first = orderedCouples.first { select(first.id) }
            toasts.show(Toast(style: .success, title: "Session restored",
                              detail: "\(couples.count) couples"))
        }
    }
}
