// Import → resolve → couples orchestration (SPEC-V2 §5.2), extracted from AppModel.
// Owns the ad-hoc activity-strip flag cluster (`resolving`, `matchingActivity`,
// `ingestInFlight`, …) and the serialized per-couple sync queue, so the model shell
// stays a façade. AppModel keeps the couples list + selection + audition; this drives
// them through the back-reference. The ActivityStrip reads these signals via AppModel's
// forwarding accessors, so its exact refresh triggers and synchronous-clear invariants
// are preserved verbatim (see the property comments below).

import AppKit
import CoupleKit
import Observation

@MainActor
@Observable
final class IngestController {
    @ObservationIgnored private unowned let model: AppModel

    /// True while the candidate matrix + resolution runs (off-main).
    var resolving = false {
        didSet { if resolving != oldValue { model.activity.refresh() } }
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
    @ObservationIgnored var suppressAutoSync = false

    init(model: AppModel) {
        self.model = model
    }

    // MARK: - Import → resolve → couples

    func importDropped(_ urls: [URL]) {
        Task { await ingest(urls) }
    }

    func ingest(_ urls: [URL]) async {
        // The whole import → match → sync sequence is ONE unified 0→100% bar (unified-bar
        // boundary). Mark it in-flight up front so the strip composes the stages; the post-import
        // sync queue's drain tail clears it. Default to all three stages; if the drop turns out to
        // be all-duplicates (no new media) we clear immediately below.
        ingestInFlight = true
        ingestStages = [.importStage, .matchStage, .syncStage]
        let added = await model.pool.importDropped(urls)
        guard !added.isEmpty else {
            // An all-duplicates drop did no real work (the import bar already cleared in the pool);
            // there is no match/sync to run, so end the ingest window cleanly.
            ingestInFlight = false
            model.activity.refresh()
            return
        }
        // Default the export folder to where the most recent import came from,
        // until the user picks one explicitly.
        if model.destinationAuto, let dir = added.last?.url.deletingLastPathComponent() {
            model.exportDestination = dir
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
            model.activity.refresh()
        }
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
        let pool = model.pool
        guard !pool.videos.isEmpty || !pool.audios.isEmpty else { return }
        let engine = model.engine
        // A resolve over an already-populated queue is an UPDATE (the strip reads
        // "Updating matches after import..."); the first resolve is the analysis pass.
        resolveIsUpdate = !model.couples.isEmpty
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
        let params = model.params

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
                    self.matchingActivity = IngestController.matchPhase(from: update)
                    self.model.activity.refresh()
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
            guard let videoEntry = model.pool.entry(key: row.videoKey) else { continue }
            if let existing = model.couples.first(where: { $0.id == row.videoKey }) {
                mergeResolution(into: existing, row: row)
                order.append(existing)
            } else {
                let couple = CoupleRow(video: videoEntry, row: row, engine: model.engine,
                                    toasts: model.toasts, audition: model.audition)
                order.append(couple)
                ensureSession(for: couple)
            }
        }
        // Keep any couples whose video left the pool? Files only ever get added,
        // so `rows` always covers every video; `order` is the authoritative list.
        model.couples = order
        if model.selectedID == nil { model.select(model.orderedCouples.first?.id) }
        // Keep the pool selection consistent with the live pool (owner decision 3;
        // TODO-001 foundation). Files only get added today, so this is a no-op now,
        // but the future remove-media feature relies on it pruning vanished keys.
        model.poolController.prunePoolSelection()
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
    func ensureSession(for couple: CoupleRow) {
        // Close the shared audition's stream before freeing this couple's old
        // core session (the stream was opened from it — Phase-6 audio-graph
        // ordering, which v1's teardown enforced before audition moved out).
        if couple.id == model.selectedID { model.audition.teardown() }
        guard let key = couple.selectedAudioKey,
              let cam = couple.video.handle,
              let audio = model.pool.entry(key: key)?.handle else {
            couple.pair.teardown()
            return
        }
        couple.pair.load(cameraTrack: cam, externalTrack: audio,
                         evidence: couple.candidates.first { $0.audioKey == key })
        if !suppressAutoSync { enqueueSync(couple) }
        if couple.id == model.selectedID { model.attachAudition(couple) }
    }

    // MARK: - Serialized per-couple sync (§5.2)

    func enqueueSync(_ couple: CoupleRow) {
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
        model.activity.refresh()
        Task {
            while let next = syncPending.first {
                syncPending.removeFirst()
                guard next.pair.session != nil else { continue }
                syncIndex += 1
                syncVideoName = next.video.name
                model.activity.refresh()     // advance the strip's "couple X of N: name"
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
            model.activity.refresh()         // queue drained ⇒ clear the sync/ingest summary
        }
    }
}
