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
    /// Single source of truth for the media-pool audio preview (§V2.7, TODO-002):
    /// one active entry, play/pause, position, duration; surfaced in the pool's
    /// sticky bottom bar. Owned here so the teardown lifecycle can release it (see
    /// `willTerminate` below).
    let preview = PoolPreviewController()

    /// Media-pool Finder-style selection (owner decision 3; TODO-001 foundation).
    /// `poolSelection` is the multi-select highlight set (reused verbatim by the
    /// future remove-media feature); `poolPrimary` is the single last-clicked entry
    /// the sticky player follows; `poolAnchor` is the ⇧-range start. The pure
    /// transition lives in CoupleKit.PoolSelection; `selectPool` applies it and
    /// drives the preview (load-paused on primary change, stop when nil).
    var poolSelection: Set<String> = []
    var poolPrimary: String?
    @ObservationIgnored private var poolAnchor: String?
    /// The primary entry as a pool Entry (sticky bar reads name/url/duration off it).
    var poolPrimaryEntry: MediaPool.Entry? { poolPrimary.flatMap { pool.entry(key: $0) } }

    var couples: [CoupleRow] = []
    var selectedID: CoupleRow.ID?
    /// True while the candidate matrix + resolution runs (off-main).
    var resolving = false {
        didSet { if resolving != oldValue { activity.refresh() } }
    }
    /// Determinate MATCHING progress (PLAN-CORE-SHELL-BOUNDARY §3.1): the live phase+fraction of the
    /// core ROW-RESOLUTION JOB (`Engine.resolveStart`), mapped from each
    /// `ProgressUpdate`. Drives the MATCH stage of the unified ingest bar. Set on the main actor as
    /// the job reports; CLEARED SYNCHRONOUSLY alongside `resolving = false` in `resolveQueue` (the
    /// same late-final-frame gate as import — the matching signal is only ever read while
    /// `model.resolving`, so a stale post-resolve frame cannot stick).
    @ObservationIgnored private(set) var matchingActivity: ActivitySignals.Matching?
    /// True from the start of an `ingest()` (a dropped batch) until its post-import sync queue
    /// drains — the window in which import→match→sync is ONE unified 0→100% bar (unified-bar
    /// boundary). A standalone re-sync / re-resolve (no preceding import) leaves it false and keeps
    /// the per-kind lines (Open Decision 6). Set in `ingest`, cleared in the `pumpSync` drain tail.
    @ObservationIgnored private(set) var ingestInFlight = false
    /// Which stages the in-flight ingest will run, for the unified bar's renormalization. Set when
    /// the batch resolution is known (an all-duplicates batch drops match+sync ⇒ import-only).
    @ObservationIgnored private(set) var ingestStages: Stages =
        [.importStage, .matchStage, .syncStage]
    /// True for a resolve that runs AFTER an earlier import already produced couples
    /// (so the activity strip reads `Updating matches after import...` rather than the
    /// first-analysis count line, TODO-005). Set per `resolveQueue` call.
    @ObservationIgnored private var resolveIsUpdate = false
    /// Strip-facing read of `resolveIsUpdate` (TODO-005).
    var resolveIsUpdateForStrip: Bool { resolveIsUpdate }
    /// True for a resolve that is the MATCHING TAIL of an import — it ran inside `ingest`,
    /// right after the dropped batch finished importing. The activity strip then continues
    /// the unified import→match bar rather than an indeterminate pairing line (TODO). A
    /// standalone re-resolve (e.g. after a removal) leaves it false.
    @ObservationIgnored private var resolveIsImportTail = false
    /// Strip-facing read of `resolveIsImportTail`.
    var resolveIsImportTailForStrip: Bool { resolveIsImportTail }

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

    /// The pool AUDIO key currently being dragged (TODO-013), recorded when a pool
    /// audio row starts a drag and cleared when it ends. Lets the center-workspace
    /// drop overlay name the dragged file in its copy (`Pair selected video with
    /// audio.wav`) DURING the hover — SwiftUI's `.dropDestination` exposes the payload
    /// only in its action closure, not while targeting, so this is the only honest
    /// source for the live name. Purely cosmetic: the actual repair always reads the
    /// dropped payload, never this, so a stale value can never mis-pair.
    var poolDraggingKey: String?
    /// The dragged audio's display name for the live overlay copy, or nil when no
    /// pool audio drag is in flight.
    var poolDraggingName: String? { poolDraggingKey.flatMap { pool.entry(key: $0)?.name } }
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
    /// Activity-strip view of the sync queue (TODO-005): the running couple's 1-based
    /// position, the batch total (running + queued at the time the batch started), and
    /// the running couple's video name — so the strip can read `Syncing couple X of N:
    /// video.mov` without exposing the internal queue. Reset to (0,0) when idle.
    @ObservationIgnored private(set) var syncIndex = 0
    @ObservationIgnored private(set) var syncTotal = 0
    @ObservationIgnored private(set) var syncVideoName: String?
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
        // The whole import → match → sync sequence is ONE unified 0→100% bar (unified-bar
        // boundary). Mark it in-flight up front so the strip composes the stages; the post-import
        // sync queue's drain tail clears it. Default to all three stages; if the drop turns out to
        // be all-duplicates (no new media) we clear immediately below.
        ingestInFlight = true
        ingestStages = [.importStage, .matchStage, .syncStage]
        let added = await pool.importDropped(urls)
        guard !added.isEmpty else {
            // An all-duplicates drop did no real work (the import bar already cleared in the pool);
            // there is no match/sync to run, so end the ingest window cleanly.
            ingestInFlight = false
            activity.refresh()
            return
        }
        // Default the export folder to where the most recent import came from,
        // until the user picks one explicitly.
        if destinationAuto, let dir = added.last?.url.deletingLastPathComponent() {
            exportDestination = dir
        }
        // The resolve is the matching TAIL of this import, so the activity strip shows it
        // as the "Matching couples…" comparison stage (a moving bar) of the same process,
        // rather than a separate standalone pairing line (TODO).
        await resolveQueue(partOfImport: true)
        // If the resolve started no auto-sync (e.g. an audio-only drop with no couples to align, or
        // every couple already synced), the ingest ends at the match stage — close the unified-bar
        // window here so it does not hang. When sync IS running, `pumpSync`'s drain tail closes it.
        if !syncRunning && syncPending.isEmpty {
            ingestInFlight = false
            activity.refresh()
        }
    }

    /// The user explicitly chose an export folder — pin it (stop auto-tracking).
    func chooseExportDestination(_ url: URL) {
        exportDestination = url
        destinationAuto = false
        saveQueue()
    }

    /// Run the §3.3 row resolution over the pool, then build/update couples — pinned/approved
    /// selections survive (§3.6).
    ///
    /// Core-shell boundary (PLAN §3.1, Phase C): the §3.3 DECISION runs in couple-core via a SINGLE
    /// DETERMINATE JOB (`Engine.resolveStart`, the `cpl_resolve_start` twin) — the SAME resolver the
    /// CLI uses, so the resolved `[ResolvedRow]` is byte-identical cross-platform. The job's
    /// `progress` is consumed on a child task that writes `matchingActivity` (main actor), giving the
    /// unified bar real fingerprint→compare→cross-check→verify→resolve progress. Core resolves the
    /// IMPORTED-TRACK videos only; audio-less videos are wrapped here as inert no-match rows. The
    /// resulting rows are then DECORATED off-main (`Resolver.decorate`) with the DISPLAY-ONLY
    /// fine-verify telemetry + (legacy-override) spectral shadow — never re-deciding the rows. No
    /// importable videos ⇒ no job (every row inert), `matchingActivity` stays nil.
    func resolveQueue(partOfImport: Bool = false) async {
        guard let engine, !pool.videos.isEmpty || !pool.audios.isEmpty else { return }
        // A resolve over an already-populated queue is an UPDATE (the strip reads
        // "Updating matches after import..."); the first resolve is the analysis pass.
        resolveIsUpdate = !couples.isEmpty
        // When this resolve is the tail of an import, the strip labels it "Matching
        // couples…" (the comparison stage) instead of a standalone pairing line.
        resolveIsImportTail = partOfImport
        resolving = true
        let videos = pool.videos.map {
            ResolverVideo(key: $0.id, name: $0.name, durationS: $0.durationS,
                          handle: $0.handle, hasAudio: $0.handle != nil)
        }
        let audios = pool.audios.map {
            ResolverAudio(key: $0.id, name: $0.name, durationS: $0.durationS, handle: $0.handle!)
        }
        let params = params

        // Core-shell boundary (PLAN §3.1, Phase C): the §3.3 DECISION runs in core via a SINGLE
        // determinate job (`cpl_resolve_start`) — it replaces the old two-step (matrix job +
        // `Resolver.resolve`). Core resolves over the IMPORTED-TRACK videos only; audio-less videos
        // are wrapped here as inert no-match rows (the header contract). The job's progress drives
        // the determinate match stage; the value is `[ResolvedRow]`, the SAME decision the CLI gets.
        let importableVideoHandles = pool.videos.compactMap(\.handle)
        let audioHandles = pool.audios.compactMap(\.handle)
        let paramsJSON = params.resolveParamsJSON()

        var coreRows: [ResolvedRow] = []
        if !importableVideoHandles.isEmpty,
           let job = try? engine.resolveStart(videos: importableVideoHandles, audios: audioHandles,
                                               paramsJSON: paramsJSON) {
            // Consume the job's progress into the determinate match stage (main actor). The child
            // task ends when the stream finishes (the job's terminal callback closes it).
            let progressTask = Task { @MainActor [weak self] in
                for await update in job.progress {
                    guard let self, self.resolving else { continue }
                    self.matchingActivity = AppModel.matchPhase(from: update)
                    self.activity.refresh()
                }
            }
            coreRows = (try? await job.value()) ?? []
            _ = await progressTask.value
        }

        // Splice core's importable-video rows back into the full pool order, wrapping each audio-less
        // video as an inert no-match row (byte-identical to the old resolver's stripped-audio row).
        // Then DECORATE shell-side: fine-verify telemetry + (legacy-override) spectral shadow — the
        // display-only fields the UI shows alongside core's decision (off-main; never re-decides).
        var byVideo = Dictionary(coreRows.map { ($0.videoKey, $0) }, uniquingKeysWith: { a, _ in a })
        var assembled: [ResolvedRow] = []
        assembled.reserveCapacity(videos.count)
        for v in videos {
            if v.handle != nil, let r = byVideo.removeValue(forKey: v.key) {
                assembled.append(r)
            } else {
                assembled.append(ResolvedRow(
                    videoKey: v.key, videoName: v.name, state: .noMatch, chip: RowState.noMatch.chip,
                    selected: nil, alternates: [],
                    detail: ResolutionDetail(margin: nil, clusters: 0, note: "no audio stream")))
            }
        }
        let toDecorate = assembled
        let rows = await Task.detached(priority: .userInitiated) {
            Resolver.decorate(toDecorate, videos: videos, audios: audios, engine: engine, params: params)
        }.value
        applyResolution(rows)
        // GATE (late-final-frame, mirrors import): clear the matching signal SYNCHRONOUSLY as
        // `resolving` flips false, so a stale 100% match frame from the job's progress task cannot
        // outlive resolution and stick on the strip. The snapshot only reads it while `resolving`.
        matchingActivity = nil
        resolving = false
    }

    /// Map a resolve JOB `ProgressUpdate` (core phase string + 0…1) to the strip's `Matching`
    /// signal (PLAN §3.1). The matrix-build phases map EXACTLY (Open Decision 7); the cheap row-pass
    /// tail phases "Verify"/"Resolve" map to the cross-check stage (the final matrix-side step the
    /// strip shows). An unknown phase holds the fingerprinting stage.
    private static func matchPhase(from u: ProgressUpdate) -> ActivitySignals.Matching {
        let phase: MatchPhase
        switch u.phase {
        case "MatchFingerprint": phase = .fingerprinting
        case "MatchCorrelate":   phase = .comparing
        case "MatchGraph":       phase = .crosschecking
        case "Verify":           phase = .comparing
        case "Resolve":          phase = .crosschecking
        default:                 phase = .fingerprinting
        }
        return ActivitySignals.Matching(phase: phase, fraction: u.fraction)
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
        // Keep the pool selection consistent with the live pool (owner decision 3;
        // TODO-001 foundation). Files only get added today, so this is a no-op now,
        // but the future remove-media feature relies on it pruning vanished keys.
        prunePoolSelection()
    }

    /// §3.6 stability + TODO-007. Fold a fresh resolution into an existing couple.
    /// The WHAT (selection, candidate list, whether to adopt the fresh display,
    /// whether the session must be rebuilt, whether a stale `forced` clears) is the
    /// PURE `RowMergePolicy.plan`; this method only applies the resulting mutations
    /// together so the row is never left half-updated. An unapproved, unpinned
    /// unresolved row therefore adopts a newly confident result WHOLESALE — exactly
    /// as if both files had been imported together — while an approved/pinned row
    /// only gains new alternates (its chosen source is never silently changed).
    private func mergeResolution(into couple: CoupleRow, row: ResolvedRow) {
        let fresh = (row.selected.map { [$0] } ?? []) + row.alternates
        let plan = RowMergePolicy.plan(
            protected: couple.pinned || couple.approved,
            currentSelectedKey: couple.selectedAudioKey,
            currentCandidateKeys: couple.candidates.map(\.audioKey),
            freshSelectedKey: row.selected?.audioKey,
            freshCandidateKeys: fresh.map(\.audioKey))

        if plan.adoptFreshDisplay {
            // Unprotected: take the resolver's full result (state/chip/detail +
            // candidate VALUES, not just keys), so the row equals a fresh build.
            couple.state = row.state
            couple.chip = row.chip
            couple.detail = row.detail
            couple.candidates = fresh
            if plan.clearForced { couple.forced = false }
            // TODO-009: an adopted resolution is the resolver's pick — provenance is
            // AUTOMATIC (its quality is the resolver verdict), or `.none` with no
            // selection. Drop any stale manual evidence so the dot is evidence-fresh.
            couple.selectionProvenance = row.selected != nil ? .automatic : .none
            couple.manualMatchConfident = nil
        } else {
            // Protected: preserve display + selection; surface only genuinely-new
            // candidates as alternates (existing candidate objects kept verbatim;
            // the policy's key order is current-then-new, so this is append-only).
            let known = Set(couple.candidates.map(\.audioKey))
            couple.candidates.append(contentsOf: fresh.filter { !known.contains($0.audioKey) })
        }
        // Session lifecycle fires AT MOST ONCE, and only when the selection
        // actually changed (no duplicate teardown/sync on a re-pick).
        if plan.rebuildSession {
            couple.selectedAudioKey = plan.selectedAudioKey
            ensureSession(for: couple)
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
        if !syncPending.contains(where: { $0 === couple }) {
            syncPending.append(couple)
            // Grow the activity-strip batch total as work is queued (high-water mark
            // across the run, so a couple enqueued mid-batch bumps "of N", TODO-005).
            syncTotal += 1
        }
        pumpSync()
    }

    private func pumpSync() {
        guard !syncRunning, !syncPending.isEmpty else { return }
        syncRunning = true
        // Starting a fresh batch: this couple is position 1 of the batch (any
        // mid-batch additions raise `syncTotal` via enqueueSync).
        syncIndex = 0
        activity.refresh()
        Task {
            while let next = syncPending.first {
                syncPending.removeFirst()
                guard next.pair.session != nil else { continue }
                syncIndex += 1
                syncVideoName = next.video.name
                activity.refresh()           // advance the strip's "couple X of N: name"
                next.pair.runAutoSync(announce: false) // ambient (§V2.4); dots carry the result
                await next.pair.awaitSync()
            }
            syncRunning = false
            syncIndex = 0
            syncTotal = 0
            syncVideoName = nil
            // The post-import sync queue has fully drained ⇒ the unified ingest operation is
            // complete; drop the Ingest window so the unified bar reaches 1.0 and then CLEARS via
            // the idle-clear (it must not stick at 100%, unified-bar boundary R7).
            ingestInFlight = false
            activity.refresh()               // queue drained ⇒ clear the sync/ingest summary
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

    // MARK: - Media-pool selection (owner decision 3; TODO-001 foundation)

    /// Finder-style pool click (owner decision 3). Maps the live `NSEvent`
    /// modifiers to the pure `PoolSelection` transition (plain replace / ⌘ toggle /
    /// ⇧ contiguous range over the displayed pool order), applies the resulting
    /// (selection, primary, anchor), then follows the SINGLE primary: a changed
    /// primary loads it into the sticky bar PAUSED (owner decision 1 — SELECT =
    /// load-paused; video entries feed `entry.url` so the bar plays the movie's
    /// embedded audio, decision 2); a nil primary stops the preview. Space is left
    /// to the global audition transport (decision 4) — no pool key binding here.
    func selectPool(key: String, modifiers: NSEvent.ModifierFlags) {
        let modifier: PoolSelectModifier
        if modifiers.contains(.shift) { modifier = .shift }
        else if modifiers.contains(.command) { modifier = .command }
        else { modifier = .plain }

        let result = PoolSelection.select(
            current: poolSelection, primary: poolPrimary, anchor: poolAnchor,
            clicked: key, ordered: pool.entries.map(\.id), modifier: modifier)

        let primaryChanged = result.primary != poolPrimary
        poolSelection = result.selection
        poolPrimary = result.primary
        poolAnchor = result.anchor
        guard primaryChanged else { return }
        followPrimary()
    }

    /// Drive the sticky preview to the current primary: load-paused (decision 1) for
    /// a real entry, stop when there is none. The single place primary→player flows,
    /// so multi-select never splits the preview.
    private func followPrimary() {
        guard let entry = poolPrimaryEntry else {
            preview.stop()
            return
        }
        preview.select(key: entry.id, url: entry.url)
    }

    /// Drop selection/primary/anchor that no longer name a live pool entry (files
    /// only ever get added today, but the future remove-media feature relies on this
    /// pruning, and a restore could in principle reshuffle keys). Re-homes the
    /// player if the primary vanished.
    func prunePoolSelection() {
        let live = Set(pool.entries.map(\.id))
        let prunedSelection = poolSelection.intersection(live)
        let prunedPrimary = poolPrimary.flatMap { live.contains($0) ? $0 : nil }
            ?? pool.entries.first { prunedSelection.contains($0.id) }?.id
        let prunedAnchor = poolAnchor.flatMap { live.contains($0) ? $0 : nil } ?? prunedPrimary
        let primaryChanged = prunedPrimary != poolPrimary
        poolSelection = prunedSelection
        poolPrimary = prunedPrimary
        poolAnchor = prunedAnchor
        if primaryChanged { followPrimary() }
    }

    // MARK: - Remove media / clear session (TODO-001)

    /// A pending per-file removal confirmation (TODO-001). Set when the selected media
    /// to remove participates in ≥1 couple — removal then regenerates those couples
    /// (a removed video's row disappears; a removed audio's row re-resolves), so the
    /// batch asks ONCE first with the affected counts. `nil` ⇒ no dialog. The pure
    /// decision is `MediaRemovalPolicy.plan`; this carries the confirmed keys + counts.
    struct PendingMediaRemoval: Identifiable {
        let id = UUID()
        let keys: Set<String>
        let affectedCouples: Int
        let affectedApproved: Int
    }
    var pendingMediaRemoval: PendingMediaRemoval?

    /// A pending Clear Session confirmation (TODO-001), carrying the counts the dialog
    /// states. `nil` ⇒ no dialog. Clearing always confirms (it discards the whole queue).
    struct PendingClearSession: Identifiable {
        let id = UUID()
        let media: Int
        let couples: Int
        let approved: Int
    }
    var pendingClearSession: PendingClearSession?

    /// The couples projected for the pure removal/clear policies (CoupleKit can't see
    /// the app `CoupleRow`).
    private var coupleRefs: [CoupleRef] {
        couples.map { CoupleRef(videoKey: $0.id, selectedAudioKey: $0.selectedAudioKey,
                                approved: $0.approved) }
    }

    /// Remove the currently selected pool media from the open session (TODO-001), the
    /// SHARED entry point for both the media-pool trash button and the Delete key. The
    /// pure `MediaRemovalPolicy.plan` decides over the live pool + couples: an empty
    /// plan no-ops; an UNUSED-only removal happens IMMEDIATELY (no dialog); a removal
    /// touching ≥1 referenced file arms ONE confirmation stating the affected couple /
    /// approved counts, and removes only on confirm. Removal affects ONLY this session
    /// and NEVER deletes source media from disk.
    func removeSelectedMedia() {
        let plan = MediaRemovalPolicy.plan(selectedKeys: poolSelection,
                                           poolKeys: Set(pool.entries.map(\.id)),
                                           couples: coupleRefs)
        guard !plan.isEmpty else { return }
        if plan.needsConfirmation {
            pendingMediaRemoval = PendingMediaRemoval(
                keys: plan.removableKeys, affectedCouples: plan.affectedCouples,
                affectedApproved: plan.affectedApproved)
        } else {
            performMediaRemoval(plan.removableKeys)
        }
    }

    /// Confirm the pending per-file removal (TODO-001): remove the keys decided when the
    /// dialog was armed.
    func confirmMediaRemoval() {
        guard let pending = pendingMediaRemoval else { return }
        pendingMediaRemoval = nil
        performMediaRemoval(pending.keys)
    }

    /// Cancel the pending per-file removal (TODO-001): make no change.
    func cancelMediaRemoval() { pendingMediaRemoval = nil }

    /// Apply a removal of `keys` from the open session (TODO-001). Strategy: removal ≡
    /// "imported without these files". We (1) stop every bit of work that touches a
    /// removed key cleanly, (2) drop removed couples + reset rows whose SELECTED audio
    /// vanished to a clean unresolved state, (3) free the pool handles, then (4) re-run
    /// the SAME resolver path the importer uses, so affected couples are regenerated
    /// exactly as a fresh import would produce them (we never hand-edit individual
    /// couples). Finally selection/persistence are pruned so no content key survives.
    private func performMediaRemoval(_ keys: Set<String>) {
        guard !keys.isEmpty else { return }

        // (1) Stop work touching the removed keys BEFORE any handle is freed.
        // The pool-preview primary: stop the player if it follows a removed entry
        // (the §TODO-002 seam — `controller.stop()` before freeing a pool handle).
        if let active = preview.state.activeKey, keys.contains(active) { preview.stop() }
        // Any in-flight import touching these would re-add them; an import that has not
        // yet published an entry can't be named here, but a resolve over the old pool
        // must not race the removal — `resolveQueue` below runs over the NEW pool.

        // Tear down each couple whose video OR selected audio is leaving, so its core
        // session (and the shared audition stream, if it is the selected couple) is
        // closed before the underlying tracks are freed. A removed-video couple is
        // dropped entirely; a removed-audio couple is reset to a clean unresolved row
        // (stale candidate data, selection, pin/forced, approval, provenance, and pair
        // session all discarded) so the re-resolve treats it as never-paired.
        var survivingCouples: [CoupleRow] = []
        for couple in couples {
            if keys.contains(couple.id) {
                // Removing a VIDEO removes its couple row.
                if couple.id == selectedID { audition.teardown() }
                couple.pair.teardown()
                continue
            }
            let audioRemoved = couple.selectedAudioKey.map { keys.contains($0) } ?? false
            if audioRemoved {
                if couple.id == selectedID { audition.teardown() }
                couple.pair.teardown()
                couple.selectedAudioKey = nil
                couple.pinned = false
                couple.forced = false
                couple.selectionProvenance = .none
                couple.manualMatchConfident = nil
                // Drop EVERY candidate that names a removed key; the resolver will
                // repopulate the survivors. (Stale candidates can't linger on the row.)
            }
            // Drop any candidate (selected or alternate) that names a removed key, so a
            // removed content key never survives on a couple even before the re-resolve.
            couple.candidates.removeAll { keys.contains($0.audioKey) }
            survivingCouples.append(couple)
        }
        couples = survivingCouples
        // If the selected couple was removed, drop the selection (re-pick after resolve).
        if let sel = selectedID, keys.contains(sel) { selectedID = nil }

        // (3) Free the pool handles for the removed entries (releases the cache-GC pin;
        // not a cache purge). Source files on disk are untouched.
        pool.remove(keys: keys)

        // Drop removed keys from the pool selection/primary/anchor + re-home the player.
        prunePoolSelection()

        // (4) Re-resolve over the REMAINING media via the normal resolver path, so every
        // affected couple is regenerated exactly as if the survivors had been imported
        // without the removed file. `applyResolution` rebuilds `couples` (a removed
        // video has no row; a reset audio row re-resolves) and re-prunes selection.
        if pool.isEmpty {
            // Nothing left to resolve: empty the queue and wipe the restore record so the
            // emptied session is not offered for restore (same end state as Clear Session
            // reached by removing the last file).
            couples = []
            selectedID = nil
            QueueStore.clear()
        } else {
            Task { await resolveQueue(); saveQueue() }
        }
    }

    /// Request Clear Session (TODO-001): always confirms, stating the media / couple /
    /// approved counts. The button is only meaningful with content; an empty session
    /// no-ops. Blocked while a batch export is running (it owns the couples' files).
    func requestClearSession() {
        guard hasContent, !batch.isRunning else { return }
        let counts = MediaRemovalPolicy.clearCounts(mediaCount: pool.entries.count,
                                                    couples: coupleRefs)
        pendingClearSession = PendingClearSession(media: counts.media, couples: counts.couples,
                                                  approved: counts.approved)
    }

    /// Cancel the pending Clear Session (TODO-001): make no change.
    func cancelClearSession() { pendingClearSession = nil }

    /// Clear the whole session (TODO-001) and return to the initial import screen. Stops
    /// active work, releases every pooled track, clears queue/export state, REMOVES the
    /// persisted restore record, and resets to the empty state (`hasContent == false` ⇒
    /// RootView shows the LoadSurface). Source files and previously exported files are
    /// untouched; the cleared state does NOT reappear on relaunch (the restore record is
    /// gone and the empty state is never re-persisted by the save guard).
    func confirmClearSession() {
        pendingClearSession = nil
        // Stop active work: pool preview, the selected couple's audition stream, and
        // every couple's core session + sync.
        preview.stop()
        audition.teardown()
        for couple in couples { couple.pair.teardown() }
        // Clear queue + selection + export state.
        couples = []
        selectedID = nil
        resetExportSettings()
        // Release every pooled track (frees handles; not a cache purge).
        pool.teardown()
        pool.skipped = []
        // Clear the pool selection entirely (no key survives).
        poolSelection = []
        poolPrimary = nil
        poolAnchor = nil
        poolDraggingKey = nil
        // Remove the persisted restore record so the cleared queue is not offered again.
        QueueStore.clear()
        // Clear the activity strip so a stuck/in-flight bar from the discarded session does
        // not linger into the empty import screen (TODO-001).
        activity.reset()
    }

    /// Reset session-level export state on Clear Session (TODO-001). Per-couple
    /// `exportStatus` lives on the couples (already dropped above); this resets the
    /// session destination back to auto-tracking so a fresh import re-defaults it.
    private func resetExportSettings() {
        exportDestination = nil
        destinationAuto = true
        globalFormat = .auto
        globalKeepCameraAudio = false
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
        let z = couple.candidates.first { $0.audioKey == audioKey }?.z ?? 0
        couple.forced = z > 0 && z < params.zMin
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
            // when the sync can't lock.
            if case .failed = couple.pair.syncActivity { couple.forced = true }
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

        // Delete / forward-delete: remove the selected media from the open session
        // (TODO-001) — only when the media pool has a selection, so Delete never fights
        // another context (and never deletes source files). Routes through the SAME
        // `removeSelectedMedia` the trash button uses (unused ⇒ immediate; referenced ⇒
        // one confirmation). Consumed only when it acts.
        if (event.keyCode == 51 || event.keyCode == 117), !poolSelection.isEmpty {
            removeSelectedMedia()
            return true
        }

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
        case 49: // space: play/pause audition from the visible playhead (TODO-004)
            guard pair.session != nil else { return true }
            audition.toggle()
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
                    enqueueSync(c)
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
            suppressAutoSync = false
            if let first = orderedCouples.first { select(first.id) }
            toasts.show(Toast(style: .success, title: "Session restored",
                              detail: "\(couples.count) couples"))
        }
    }
}
