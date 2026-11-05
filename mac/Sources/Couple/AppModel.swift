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
// selection, audition, preview, include/repair verbs, export settings, and persistence,
// and exposes thin forwarding accessors so views / ActivityStrip / DebugDrive reach the
// sub-controller members through `model.` unchanged.

import AppKit
import CoupleKit
import Observation
import SwiftUI

@MainActor
@Observable
final class AppModel {
    let engine: Engine
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

    /// A pending pinned-replacement confirmation (TODO-013, D9). Set when a manual
    /// audio drop targets a PINNED couple (a manual pick) with a different source;
    /// replacing it discards that deliberate choice and clears the current pair
    /// session, so the drop asks first. An included-but-unpinned couple replaces
    /// without asking (default-on inclusion would otherwise prompt on every drop).
    /// Both drop targets (queue row + center workspace) route
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
    /// The keyboard-shortcut reference (KeyboardShortcutsView): footer button,
    /// Help ▸ Keyboard Shortcuts (⌘/) or `?`; Esc closes it (UX §V2.13).
    var shortcutsShown = false

    // MARK: Arrangement (UX §V2.11; PLAN-COMPACT-MODE Phase 4) — see WindowLayout.swift

    /// Compact (session line, fixed preview, couple list, export panel) or full
    /// (three columns). Persisted in UserDefaults; COMPACT when the key is absent —
    /// the first launch after the update opens compact, then last-used (§6.1).
    var layoutMode: LayoutMode = AppModel.storedLayoutMode()
    /// The couple whose candidate list is reopened inline under the player by
    /// "Compare others" / "Change…" after a manual pick (§6.3). Model-owned so it
    /// survives an arrangement switch; selecting a row clears it.
    var compareCoupleID: CoupleRow.ID?
    /// The hosting window, handed over once by `WindowAccessor` (the app had no
    /// NSWindow access before Phase 4).
    @ObservationIgnored weak var window: NSWindow?
    /// Each arrangement's last window frame (mirrored to UserDefaults when
    /// `persistLayout`).
    @ObservationIgnored var layoutFrames: [LayoutMode: NSRect] = [:]
    /// False under the dev harness (DebugDrive), so a harness run never writes the
    /// arrangement or frames into the defaults domain.
    @ObservationIgnored var persistLayout = true
    /// Set while the model itself moves the window, so the frame observers do not
    /// record an intermediate frame as the arrangement's "last" one.
    @ObservationIgnored var applyingLayoutFrame = false
    /// D10: a Pan L/R both-mode is heard as Synced (Overlay) while compact; this
    /// remembers that the user's mode was Pan, restored on returning to full.
    @ObservationIgnored var panHeldForCompact = false
    @ObservationIgnored var windowObservers: [NSObjectProtocol] = []
    /// Phase 5 (WindowLayout.swift): true while the arrangement switch animates —
    /// the root pins its content at the incoming arrangement's size and the window
    /// frame moves alongside.
    var layoutAnimating = false
    /// The incoming arrangement's content size while `layoutAnimating`.
    var layoutTransitionSize: CGSize = .zero
    /// Surfaces unique to one arrangement (overview, loupes, bottom bar, Files list,
    /// media pool, footer) stay transparent until the four blocks settle, then fade in.
    var layoutExtrasHidden = false
    /// FLIP: each block's outgoing frame while the switch plays (empty at rest).
    var layoutFlipFrom: [String: CGRect] = [:]
    /// Split-view panes unclipped for the move: (view, clipsToBounds, masksToBounds).
    @ObservationIgnored var unclippedViews: [(NSView, Bool, Bool)] = []
    /// Harness override for Reduce Motion (nil = follow the system setting).
    @ObservationIgnored var layoutMotionOverride: Bool?
    /// Each block's last frame in window coordinates (harness checks only).
    @ObservationIgnored var layoutBlockFrames: [String: CGRect] = [:]
    /// Bumped by the harness to force a fresh render of the arrangement (the
    /// leftover-offset check compares settled block frames against it).
    var layoutRenderEpoch = 0

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

    /// The focused window's UndoManager (⌘Z / Edit ▸ Undo), pushed in by RootView
    /// from the SwiftUI environment. Registration is limited to the CHEAPLY
    /// INVERTIBLE queue mutations — approve/unapprove, manual re-pair / candidate
    /// switch, export-name edits. Media removal and Clear Session stay
    /// confirmation-gated instead: they free imported handles and sessions, so
    /// their inverse is a re-import, not a state restore.
    @ObservationIgnored weak var undoManager: UndoManager?
    /// Overview wave-area frame in window coordinates (TODO-010): pointer-over-overview
    /// scrolls route to overview zoom/pan, pointer-over-loupe scrolls to the loupes.
    @ObservationIgnored var overviewRect: CGRect = .zero

    init() {
        let engine: Engine
        do {
            engine = try Engine()
        } catch {
            let engineError = (error as? CoreError)?.message ?? error.localizedDescription
            fatalError("couple-core context failed: \(engineError)")
        }
        self.engine = engine
        self.pool = MediaPool(engine: engine, toasts: toasts)
        self.placeholderPair = PairSession(engine: engine, toasts: toasts, audition: audition)
        // F5d: the resolver-constant presentation mirrors (PairParams.zMin/.zMargin) read
        // CORE's defaults (cpl_resolve_params_default_json) instead of trusting hardcoded
        // copies — the same single source the Electron shell reads (AppModel.ts `zMin`).
        // Failure behavior matches Electron exactly: a core fault or a missing key keeps
        // the struct's fallback literals (8.5 / 6.0).
        if let json = engine.resolveParamsDefaultJSON(),
           let data = json.data(using: .utf8),
           let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] {
            if let z = obj["zMin"] as? Double { params.zMin = z }
            if let m = obj["zMargin"] as? Double { params.zMargin = m }
        }
        input.installMonitors()
        NotificationCenter.default.addObserver(
            forName: NSApplication.willTerminateNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.preview.stop() // release pool-preview playback before exit (TODO-002)
                self?.recordWindowFrame()
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

    /// Visual queue order = the two sections concatenated (To fix above Ready, UX
    /// §V2.12 / D2); drives j/k. Membership is the VERDICT alone — inclusion never
    /// moves a row between sections.
    var orderedCouples: [CoupleRow] { toFix + ready }
    /// Verdict good: includable, in the Ready section.
    var ready: [CoupleRow] { couples.filter { $0.verdict == .good } }
    /// Verdict questionable / none ("pick one", "no audio", forced): the To fix section.
    var toFix: [CoupleRow] { couples.filter { $0.verdict != .good } }

    /// The export batch (D1): every couple the core include rule says goes out next.
    var included: [CoupleRow] { couples.filter(\.included) }
    /// N in "Export N files" / "N to export".
    var includedCount: Int { included.count }
    /// The export rail's Files list: included ∪ already exported (rule in core).
    var filesList: [CoupleRow] { couples.filter(\.inFilesList) }
    var hasContent: Bool { !pool.isEmpty }

    /// The user explicitly chose an export folder — pin it (stop auto-tracking).
    func chooseExportDestination(_ url: URL) {
        exportDestination = url
        destinationAuto = false
        saveQueue()
    }

    // MARK: - Selection + audition (§5.3)

    func select(_ id: CoupleRow.ID?) {
        endCandidateAudition() // choosing another row tears the scratch session down
        compareCoupleID = nil  // a reopened "Compare others" list closes with the row
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
        endCandidateAudition()
        guard let cam = couple.video.handle ?? couple.pair.camera.track else {
            audition.teardown()
            return
        }
        audition.attach(session: couple.pair.session,
                        videoURL: couple.video.url,
                        videoDuration: cam.durationS)
    }

    // MARK: - Include for export (§V2.12, D1) + manual re-pair (§V2.6)

    /// The row checkbox (D1/D4): flip this couple's inclusion in the next export by
    /// recording an EXPLICIT choice — `explicit = !included` — so the choice survives
    /// a re-sync or re-resolve while the verdict stays good (the rule in core derives
    /// `included` from it). Only an includable (verdict good) couple can be toggled;
    /// the checkbox is not shown otherwise. Undoable (TODO-015): the inverse restores
    /// the PREVIOUS stored value (possibly nil = default), not a blind re-toggle, so
    /// ⌘Z after ticking an exported couple back on returns it to "exported, excluded".
    func toggleInclude(_ couple: CoupleRow) {
        guard couple.canInclude else { return }
        setExplicitInclude(!couple.included, for: couple)
    }

    /// The single undoable write path for `CoupleRow.explicit` (nil = back to the
    /// policy default). Symmetric via the old value, so redo is the re-registration
    /// on undo — the same idiom as `setStemOverride`.
    func setExplicitInclude(_ newValue: Bool?, for couple: CoupleRow) {
        let old = couple.explicit
        guard old != newValue else { return }
        couple.explicit = newValue
        let id = couple.id
        undoManager?.registerUndo(withTarget: self) { model in
            MainActor.assumeIsolated {
                guard let c = model.couples.first(where: { $0.id == id }) else { return }
                model.setExplicitInclude(old, for: c)
            }
        }
        undoManager?.setActionName(couple.included ? "Include in Export" : "Exclude from Export")
        saveQueue()
    }

    /// Everything a manual re-pair mutates on a row, captured BEFORE the mutation so
    /// ⌘Z can put the row back (including the include-for-export inputs — `repair`
    /// clears `explicit` and `exportedAt` because the old export is stale). Restoring
    /// re-registers the pre-restore state, so redo is the same mechanism. Candidates
    /// are copied whole: a repair may append a fresh pool-drag candidate that undo
    /// must remove.
    struct RepairSnapshot {
        let selectedAudioKey: String?
        let pinned: Bool
        let forced: Bool
        let chip: String
        let provenance: SelectionProvenance
        let manualMatchConfident: Bool?
        let explicit: Bool?
        let exportedAt: Date?
        let exportStatus: CoupleRow.ExportStatus
        let candidates: [RowCandidate]

        @MainActor
        init(of couple: CoupleRow) {
            selectedAudioKey = couple.selectedAudioKey
            pinned = couple.pinned
            forced = couple.forced
            chip = couple.chip
            provenance = couple.selectionProvenance
            manualMatchConfident = couple.manualMatchConfident
            explicit = couple.explicit
            exportedAt = couple.exportedAt
            exportStatus = couple.exportStatus
            candidates = couple.candidates
        }
    }

    /// Register ⌘Z for a repair about to happen on `couple`, restoring `snapshot`.
    private func registerRepairUndo(_ couple: CoupleRow, restoring snapshot: RepairSnapshot) {
        let id = couple.id
        undoManager?.registerUndo(withTarget: self) { model in
            MainActor.assumeIsolated {
                guard let c = model.couples.first(where: { $0.id == id }) else { return }
                model.applyRepairSnapshot(snapshot, to: c)
            }
        }
        undoManager?.setActionName("Change Audio")
    }

    /// Put a row back to a pre-repair snapshot (undo), registering the pre-restore
    /// state as redo. The session is rebuilt through the same guarded path a repair
    /// uses; if the snapshot's audio has since been removed from the session, the
    /// undo declines with a toast instead of leaving a half-restored row.
    private func applyRepairSnapshot(_ snapshot: RepairSnapshot, to couple: CoupleRow) {
        guard let cam = couple.video.handle else { return }
        // Restoring a selection needs its audio handle back; a removed source
        // cannot be re-selected (same guard as `repair`).
        let audioHandle = snapshot.selectedAudioKey.flatMap { pool.entry(key: $0)?.handle }
        if snapshot.selectedAudioKey != nil && audioHandle == nil {
            toasts.show(Toast(style: .warning, title: "Can't undo",
                              detail: "That audio is no longer in the session."))
            return
        }
        registerRepairUndo(couple, restoring: RepairSnapshot(of: couple)) // redo
        endCandidateAudition()
        // Back to an unpinned row ⇒ the "Audio chosen by you · Undo" line has nothing
        // left to undo; a redo back onto a pick restores nothing here (Undo hides).
        if !snapshot.pinned { prePickSnapshots[couple.id] = nil }
        couple.selectedAudioKey = snapshot.selectedAudioKey
        couple.pinned = snapshot.pinned
        couple.forced = snapshot.forced
        couple.chip = snapshot.chip
        couple.selectionProvenance = snapshot.provenance
        couple.manualMatchConfident = snapshot.manualMatchConfident
        couple.explicit = snapshot.explicit
        couple.exportedAt = snapshot.exportedAt
        couple.exportStatus = snapshot.exportStatus
        couple.candidates = snapshot.candidates
        if couple.id == selectedID { audition.teardown() } // close stream before freeing old session
        if let audioHandle {
            couple.pair.load(cameraTrack: cam, externalTrack: audioHandle,
                             evidence: couple.candidates.first { $0.audioKey == couple.selectedAudioKey })
            couple.pair.runAutoSync(announce: false) // ambient re-derivation, like repair's
        } else {
            couple.pair.teardown() // back to no-selection (ambiguous/no-match row)
        }
        if couple.id == selectedID { attachAudition(couple) }
        saveQueue()
    }

    /// Manual re-pair (§V2.6): point a couple at a chosen audio (candidate pick or
    /// pool drag). Pins the couple; runs a full sync; below-floor keeps the
    /// pairing but marks it `forced` (honest). Candidates are never destroyed —
    /// a brand-new audio is appended to the list. Clears the include-for-export
    /// inputs (`explicit`, `exportedAt`; PLAN-COMPACT-MODE §3): the old export is
    /// stale and the default rule applies to the new pairing once its verdict lands.
    /// `undoSnapshot` lets `performRepairReplace` supply the state from BEFORE its
    /// own un-queue mutation; a direct call captures the row as it stands.
    func repair(_ couple: CoupleRow, audioKey: String, undoSnapshot: RepairSnapshot? = nil) {
        guard let audioEntry = pool.entry(key: audioKey), let audioHandle = audioEntry.handle,
              let cam = couple.video.handle else { return }
        let before = undoSnapshot ?? RepairSnapshot(of: couple)
        registerRepairUndo(couple, restoring: before)
        // The first manual pick of an unpinned row remembers the pre-pick state so the
        // preview's "Audio chosen by you · Undo" can return the couple to To fix
        // (PLAN-COMPACT-MODE Phase 3 step 3); a later re-pick keeps that origin.
        if !before.pinned { prePickSnapshots[couple.id] = before }
        endCandidateAudition() // "use" ends any audition of this couple's candidates
        couple.pinned = true
        couple.explicit = nil     // D1/D8: a new pairing starts from the default rule
        couple.exportedAt = nil   // the previous export (if any) is of the OLD pairing
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
        // A picked existing candidate carries resolver evidence (seeds the sync, §11.3);
        // a fresh pool drag is the z-0 placeholder appended above, which the seed policy
        // rejects — its sync stays a blind search.
        couple.pair.load(cameraTrack: cam, externalTrack: audioHandle,
                         evidence: couple.candidates.first { $0.audioKey == audioKey })
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

    // MARK: - Manual-pick Undo (PLAN-COMPACT-MODE Phase 3 step 3)

    /// Per couple, the row as it stood before its first manual pick (see `repair`).
    /// Drives the preview's "Audio chosen by you · **Undo**" link.
    /// Observed, so the link appears/disappears with it.
    private var prePickSnapshots: [CoupleRow.ID: RepairSnapshot] = [:]

    /// True when the "Undo" link has a pre-pick state to return to (a pick made in
    /// this run; a pin restored from a saved session has no in-memory origin).
    func canUndoManualPick(_ couple: CoupleRow) -> Bool {
        couple.pinned && prePickSnapshots[couple.id] != nil
    }

    /// The preview's Undo link: the SAME inverse ⌘Z applies to a repair
    /// (`applyRepairSnapshot`, which registers its own redo), restoring the row to
    /// its pre-pick state — unpinned, the couple back in To fix.
    func undoManualPick(_ couple: CoupleRow) {
        guard let snap = prePickSnapshots[couple.id] else { return }
        prePickSnapshots[couple.id] = nil
        applyRepairSnapshot(snap, to: couple)
        undoManager?.setActionName("Change Audio")
    }

    // MARK: - Candidate audition (PLAN-COMPACT-MODE Phase 3 step 2; UX §V2.11)

    /// A candidate being auditioned at its best-fit lag in a SCRATCH core session
    /// (row camera + that candidate's audio, offset = the candidate's lag). Nothing
    /// is pinned or re-synced; the couple's own session is untouched. Core reads the
    /// session offset on every audition fill, so no core change is needed.
    struct CandidateAudition {
        let coupleID: CoupleRow.ID
        let candidate: RowCandidate
        let track: TrackHandle
        let session: SessionHandle
        /// What to put back when the audition ends.
        let previousMode: MonitorMode
        let previousPlayhead: Double
    }

    private(set) var candidateAudition: CandidateAudition?

    /// ▶ on a candidate: open the scratch session, swap the shared audition onto it
    /// (video and playhead stay), monitor Overlay, and play from the playhead. Pressing
    /// ▶ on another candidate swaps scratch sessions; ■ is `stopCandidateAudition`.
    func auditionCandidate(_ couple: CoupleRow, _ cand: RowCandidate) {
        guard couple.id == selectedID,
              let cam = couple.video.handle,
              let audio = pool.entry(key: cand.audioKey)?.handle else { return }
        let previous = candidateAudition
        let scratch: SessionHandle
        do {
            scratch = try engine.sessionNew(camera: cam, external: audio)
        } catch {
            toasts.show(Toast(style: .warning, title: "Can't play this candidate",
                              detail: (error as? CoreError)?.message ?? error.localizedDescription))
            return
        }
        scratch.setOffsetMs(cand.lagMs)
        // Swapping closes the open stream (the previous scratch's, if any) first.
        audition.attach(session: scratch)
        previous?.session.free()
        candidateAudition = CandidateAudition(
            coupleID: couple.id, candidate: cand, track: audio, session: scratch,
            previousMode: previous?.previousMode ?? audition.monitorMode,
            previousPlayhead: previous?.previousPlayhead ?? audition.playheadT)
        // A temporary override, not a user choice: set directly so the A/B memory
        // in `policy::monitor` is left exactly as the user had it.
        audition.monitorMode = .overlay
        audition.play(from: audition.playheadT)
    }

    /// ■ / "use" / closing the banner / another row: end the audition and put the
    /// couple's own session, monitor mode and playhead back.
    func stopCandidateAudition() {
        guard let a = candidateAudition else { return }
        candidateAudition = nil
        if let couple = couples.first(where: { $0.id == a.coupleID }), couple.id == selectedID {
            audition.attach(session: couple.pair.session) // closes the scratch stream
            audition.monitorMode = a.previousMode
            audition.seek(to: a.previousPlayhead)
        } else {
            audition.attach(session: nil)
        }
        a.session.free()
    }

    /// End without restoring (the caller is about to re-attach or tear down the
    /// audition itself): close the scratch stream, free the scratch session.
    func endCandidateAudition() {
        guard let a = candidateAudition else { return }
        candidateAudition = nil
        audition.attach(session: nil)
        audition.monitorMode = a.previousMode
        a.session.free()
    }

    /// Set (or clear) a couple's in-place export-name override (§V2.8), undoably.
    /// The single write path for both the in-place edit commit and its reset
    /// button, so ⌘Z restores the previous name either way. Symmetric via the old
    /// value, so redo is the re-registration on undo.
    func setStemOverride(_ newValue: String?, for couple: CoupleRow) {
        let old = couple.stemOverride
        guard old != newValue else { return }
        couple.stemOverride = newValue
        let id = couple.id
        undoManager?.registerUndo(withTarget: self) { model in
            MainActor.assumeIsolated {
                guard let c = model.couples.first(where: { $0.id == id }) else { return }
                model.setStemOverride(old, for: c)
            }
        }
        undoManager?.setActionName("Rename Export")
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
    ///   - `.accept` ⇒ replace now via the same `repair` (unresolved / unpinned);
    ///   - `.needsConfirmation` ⇒ arm the pinned-replacement dialog (RootView shows
    ///     it); the replace happens only when the user confirms. The gate is the PIN
    ///     (D9) — the policy's `approved` wire key carries `pinned` until Phase 6 (D12).
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
            approved: couple.pinned) // D9: carries `pinned` (wire key kept, D12)
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

    /// Apply an accepted replacement (TODO-013): drop the stale per-couple export
    /// result FIRST, then run the SAME `repair` as every other manual pick — which
    /// selects+pins the audio, clears the include inputs (`explicit`/`exportedAt`),
    /// rebuilds the pair session, and runs full sync/classification. The verdict stays
    /// evidence-based (TODO-009): `repair` does NOT green the dot on the drop; only the
    /// re-run classifier can. Centralized so the queue-row and center drops are
    /// byte-identical.
    private func performRepairReplace(_ couple: CoupleRow, audioKey: String) {
        let snapshot = RepairSnapshot(of: couple) // BEFORE any mutation, so ⌘Z restores inclusion/export
        couple.exportStatus = .idle               // drop any stale per-couple export result
        repair(couple, audioKey: audioKey, undoSnapshot: snapshot)
    }

    /// Confirm the pending pinned-replacement (TODO-013): replace the source. The
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

    /// Cancel the pending pinned-replacement (TODO-013): make no change.
    func cancelPendingRepair() {
        pendingRepairConfirmation = nil
    }

    /// TODO-009: (re)evaluate a MANUAL pick's match evidence off-main and update its
    /// dot, reusing the SAME confident classifier as `repair`/the resolver. Used by
    /// restore for a pre-009 manual pin whose evidence wasn't persisted, so a
    /// round-tripped questionable manual pick that DOES match still shows green
    /// without forcing a full re-sync. No-op when the pair's handles are gone.
    private func reevaluateManualMatch(_ couple: CoupleRow) {
        guard let cam = couple.video.handle,
              let key = couple.selectedAudioKey, let audio = pool.entry(key: key)?.handle else { return }
        let engine = self.engine
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
    /// selection (pins included), include-for-export inputs (`explicit`/`exportedAt`,
    /// D8 — absent in a pre-D1 file ⇒ nil ⇒ every good couple included), per-couple
    /// alignment (from the unchanged per-pair autosave), and export settings (§5.4).
    func applyRestore(_ saved: SavedQueue) {
        Task {
            ingest.suppressAutoSync = true
            await ingest.ingest(saved.files.map { URL(fileURLWithPath: $0.path) })
            for sc in saved.couples {
                guard let c = couples.first(where: { $0.id == sc.videoKey }) else { continue }
                c.pinned = sc.pinned
                c.forced = sc.forced
                c.stemOverride = sc.stemOverride
                c.explicit = sc.explicit       // nil in a pre-D1 file ⇒ policy default
                c.exportedAt = sc.exportedAt
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
