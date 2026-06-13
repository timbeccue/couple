// AppModel (SPEC-V2 §5.2) — the queue model. v1's single `pair` becomes a media
// pool + a list of couples + a selection. Each couple owns its PairSession;
// per-couple auto-sync runs in a serialized background queue so verdict dots
// settle progressively without N concurrent syncs. One shared AuditionController
// (§5.3) follows the selection. The v1 editor survives as the drill-down: the
// editor views read `model.pair`, which resolves to the selected couple's
// PairSession (or a harmless empty placeholder).

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

    var couples: [CoupleRow] = []
    var selectedID: CoupleRow.ID?
    /// True while the candidate matrix + resolution runs (off-main).
    var resolving = false
    var params = PairParams()

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
    @ObservationIgnored private var destinationAuto = true
    var globalFormat: PairSession.ExportFormat = .auto
    var globalKeepCameraAudio = false
    @ObservationIgnored lazy var batch = BatchExport(model: self)

    /// Editor target when no couple is selected — an empty PairSession so the
    /// editor views never see nil.
    @ObservationIgnored let placeholderPair: PairSession

    /// Loupe canvas frames in window coordinates, for scroll-wheel routing.
    @ObservationIgnored var loupeRects: [Int: CGRect] = [:]
    @ObservationIgnored private var keyMonitor: Any?
    @ObservationIgnored private var scrollMonitor: Any?

    // Per-couple serialized sync queue (§5.2).
    @ObservationIgnored private var syncPending: [CoupleRow] = []
    @ObservationIgnored private var syncRunning = false
    /// Set during the restore path so couples take their saved alignment instead
    /// of a fresh auto-sync (§5.4).
    @ObservationIgnored private var suppressAutoSync = false

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
        installMonitors()
        NotificationCenter.default.addObserver(
            forName: NSApplication.willTerminateNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.saveAll() }
        }
        DebugDrive.shared.configure(model: self)
        offerRestoreOnLaunch()
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

    // MARK: - Import → resolve → couples

    func importDropped(_ urls: [URL]) {
        Task { await ingest(urls) }
    }

    private func ingest(_ urls: [URL]) async {
        let added = await pool.importDropped(urls)
        guard !added.isEmpty else { return }
        // Default the export folder to where the most recent import came from,
        // until the user picks one explicitly.
        if destinationAuto, let dir = added.last?.url.deletingLastPathComponent() {
            exportDestination = dir
        }
        await resolveQueue()
    }

    /// The user explicitly chose an export folder — pin it (stop auto-tracking).
    func chooseExportDestination(_ url: URL) {
        exportDestination = url
        destinationAuto = false
        saveQueue()
    }

    /// Run the §3 candidate matrix + §3.3 resolution over the pool (off-main),
    /// then build/update couples — pinned/approved selections survive (§3.6).
    func resolveQueue() async {
        guard let engine, !pool.videos.isEmpty || !pool.audios.isEmpty else { return }
        resolving = true
        let videos = pool.videos.map {
            ResolverVideo(key: $0.id, name: $0.name, durationS: $0.durationS,
                          handle: $0.handle, hasAudio: $0.handle != nil)
        }
        let audios = pool.audios.map {
            ResolverAudio(key: $0.id, name: $0.name, durationS: $0.durationS, handle: $0.handle!)
        }
        let params = params
        let rows = await Task.detached(priority: .userInitiated) {
            Resolver.resolve(videos: videos, audios: audios, engine: engine, params: params)
        }.value
        applyResolution(rows)
        resolving = false
    }

    private func applyResolution(_ rows: [ResolvedRow]) {
        var order: [CoupleRow] = []
        for row in rows {
            guard let videoEntry = pool.entry(key: row.videoKey) else { continue }
            if let existing = couples.first(where: { $0.id == row.videoKey }) {
                mergeResolution(into: existing, row: row)
                order.append(existing)
            } else {
                let couple = CoupleRow(video: videoEntry, row: row, engine: engine!,
                                    toasts: toasts, audition: audition)
                order.append(couple)
                ensureSession(for: couple)
            }
        }
        // Keep any couples whose video left the pool? Files only ever get added,
        // so `rows` always covers every video; `order` is the authoritative list.
        couples = order
        if selectedID == nil { select(orderedCouples.first?.id) }
    }

    /// §3.6 stability: a pinned/approved couple keeps its selection and state;
    /// new files only append genuinely-new alternates. An unresolved couple takes
    /// the fresh row wholesale.
    private func mergeResolution(into couple: CoupleRow, row: ResolvedRow) {
        let fresh = (row.selected.map { [$0] } ?? []) + row.alternates
        if couple.pinned || couple.approved {
            let known = Set(couple.candidates.map(\.audioKey))
            couple.candidates.append(contentsOf: fresh.filter { !known.contains($0.audioKey) })
        } else {
            couple.state = row.state
            couple.chip = row.chip
            couple.detail = row.detail
            couple.candidates = fresh
            if couple.selectedAudioKey != row.selected?.audioKey {
                couple.selectedAudioKey = row.selected?.audioKey
                ensureSession(for: couple)
            }
        }
    }

    /// Ensure the couple's PairSession reflects its current selection, and queue
    /// its auto-sync. No selection ⇒ tear the session down (ambiguous/no-match
    /// carry no session, §V2.6).
    private func ensureSession(for couple: CoupleRow) {
        // Close the shared audition's stream before freeing this couple's old
        // core session (the stream was opened from it — Phase-6 audio-graph
        // ordering, which v1's teardown enforced before audition moved out).
        if couple.id == selectedID { audition.teardown() }
        guard let key = couple.selectedAudioKey,
              let cam = couple.video.handle,
              let audio = pool.entry(key: key)?.handle else {
            couple.pair.teardown()
            return
        }
        couple.pair.load(cameraTrack: cam, externalTrack: audio)
        if !suppressAutoSync { enqueueSync(couple) }
        if couple.id == selectedID { attachAudition(couple) }
    }

    // MARK: - Serialized per-couple sync (§5.2)

    private func enqueueSync(_ couple: CoupleRow) {
        if !syncPending.contains(where: { $0 === couple }) { syncPending.append(couple) }
        pumpSync()
    }

    private func pumpSync() {
        guard !syncRunning, !syncPending.isEmpty else { return }
        syncRunning = true
        Task {
            while let next = syncPending.first {
                syncPending.removeFirst()
                guard next.pair.session != nil else { continue }
                next.pair.runAutoSync(announce: false) // ambient (§V2.4); dots carry the result
                await next.pair.awaitSync()
            }
            syncRunning = false
        }
    }

    // MARK: - Selection + audition (§5.3)

    func select(_ id: CoupleRow.ID?) {
        selectedID = id
        loupeRects = [:]
        guard let couple = selectedCouple else {
            audition.teardown()
            return
        }
        attachAudition(couple)
    }

    /// Detach/attach/swap (§5.3): exactly one audition handle open at a time. A
    /// couple with no session attaches video-only (camera-audio poster).
    private func attachAudition(_ couple: CoupleRow) {
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
        let z = couple.candidates.first { $0.audioKey == audioKey }?.z ?? 0
        couple.forced = z > 0 && z < params.zMin
        couple.chip = couple.forced ? "forced — below floor" : couple.state.chip
        Task {
            couple.pair.runAutoSync(announce: false) // ambient; the dot/chip carry it
            await couple.pair.awaitSync()
            // A below-floor pool drag (z unknown until now) surfaces as forced
            // when the sync can't lock.
            if case .failed = couple.pair.syncActivity { couple.forced = true }
            couple.chip = couple.forced ? "forced — below floor" : couple.state.chip
            saveQueue()
        }
        saveQueue()
    }

    // MARK: - Keyboard (UX §2.9 + j/k/v/Enter queue triage)

    private func installMonitors() {
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self else { return event }
            return self.handleKey(event) ? nil : event
        }
        scrollMonitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self] event in
            guard let self else { return event }
            return self.handleScroll(event) ? nil : event
        }
    }

    private var isEditingText: Bool {
        NSApp.keyWindow?.firstResponder is NSTextView
    }

    private func handleKey(_ event: NSEvent) -> Bool {
        // Esc: while editing a text field (e.g. an inline filename rename), let
        // it through so the field can cancel/revert. Otherwise cancel a running
        // sync, else collapse the bottom-bar tools / dismiss loupes.
        if event.keyCode == 53 {
            if isEditingText { return false }
            if pair.isSyncRunning { pair.cancelSync(); return true }
            if loupesShown { loupesShown = false; return true }
            if syncToolsExpanded { syncToolsExpanded = false; return true }
            return false
        }
        guard !isEditingText else { return false }
        let mods = event.modifierFlags
        let chars = event.charactersIgnoringModifiers ?? ""

        switch event.keyCode {
        case 123, 124: // ← → — move the EXTERNAL content in the arrow's direction
            guard pair.inputsEnabled else { return true }
            var step = 1.0
            if mods.contains(.shift) { step = 10 }
            if mods.contains(.option) { step = 0.1 }
            pair.nudgeOffset(event.keyCode == 123 ? step : -step)
            return true
        case 38: // j — next couple
            moveSelection(1)
            return true
        case 40: // k — previous couple
            moveSelection(-1)
            return true
        case 36, 76: // Return / keypad Enter — open/close the editor (sync tools)
            guard selectedCouple != nil else { return true }
            syncToolsExpanded.toggle()
            return true
        case 49: // space: play/pause audition + preview
            guard pair.session != nil else { return true }
            let start = max(0, (pair.anchors.first ?? 0.8) - 0.8)
            audition.toggle(defaultStart: start)
            return true
        default:
            break
        }

        switch chars {
        case "[", "]":
            guard pair.inputsEnabled else { return true }
            let step = mods.contains(.option) ? 0.1 : 1.0
            pair.nudgeDrift(chars == "[" ? -step : step)
            return true
        case "v": // approve + advance to next unapproved (§V2.5)
            approveAndAdvance()
            return true
        case "l": // loupes (§V2.4)
            loupesShown.toggle()
            return true
        default:
            return false
        }
    }

    private func moveSelection(_ delta: Int) {
        let order = orderedCouples
        guard !order.isEmpty else { return }
        guard let cur = order.firstIndex(where: { $0.id == selectedID }) else {
            select(order.first?.id); return
        }
        let next = max(0, min(order.count - 1, cur + delta))
        select(order[next].id)
    }

    private func approveAndAdvance() {
        guard let couple = selectedCouple else { return }
        toggleApprove(couple)
        // Advance to the next couple still needing approval (TO APPROVE / TO FIX).
        let pendingAfter = (toApprove + toFix)
        if let next = pendingAfter.first(where: { $0.id != couple.id }) {
            select(next.id)
        }
    }

    // MARK: - Scroll-wheel zoom routing (editor loupes; UX §2.9)

    private func handleScroll(_ event: NSEvent) -> Bool {
        guard let window = event.window, let contentView = window.contentView else { return false }
        let p = contentView.convert(event.locationInWindow, from: nil)
        let point = contentView.isFlipped ? p : CGPoint(x: p.x, y: contentView.bounds.height - p.y)
        return zoom(at: point, deltaY: event.scrollingDeltaY, precise: event.hasPreciseScrollingDeltas)
    }

    @discardableResult
    func zoom(at point: CGPoint, deltaY: CGFloat, precise: Bool) -> Bool {
        guard pair.inputsEnabled,
              let (index, _) = loupeRects.first(where: { $0.value.contains(point) }) else {
            return false
        }
        guard deltaY != 0 else { return true }
        let factor = exp(deltaY * (precise ? 0.004 : 0.06))
        pair.manualZoom(loupe: index, factor: 1 / factor)
        return true
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
            suppressAutoSync = true
            await ingest(saved.files.map { URL(fileURLWithPath: $0.path) })
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
                    ensureSession(for: c)
                }
                // Prefer the user's saved per-pair alignment; else sync this couple.
                if !c.pair.restoreSavedAlignment(), c.pair.session != nil {
                    enqueueSync(c)
                }
                if sc.approved, c.pair.session != nil, !c.approved { c.pair.toggleVerified() }
            }
            if let dest = saved.export.destination {
                exportDestination = URL(fileURLWithPath: dest)
                destinationAuto = false // a saved destination is treated as pinned
            }
            globalFormat = PairSession.ExportFormat(rawValue: saved.export.globalFormat) ?? .auto
            globalKeepCameraAudio = saved.export.keepCameraAudio
            suppressAutoSync = false
            if let first = orderedCouples.first { select(first.id) }
            toasts.show(Toast(style: .success, title: "Session restored",
                              detail: "\(couples.count) couples"))
        }
    }
}
