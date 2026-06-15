// Media pool (SPEC-V2 §5.1) — the one real refactor of the v1 load path. v1's
// PairSession.loadFile owned probe → classify → import: files entered the world
// through a pair. v2 inverts this: a session-level pool owns probe/import and
// holds TrackHandles keyed by content key; tracks exist before couples do, and a
// couple is built from two existing handles.
//
// The pool holding open TrackHandles IS the cache-GC pin (V2-F3 disposition,
// Session F: Ctx::cache_gc exempts the content keys of all loaded tracks). The
// pool never silently drops anything the user dropped (§3.6 / §V2.7): an
// unrecognized file is surfaced once in `skipped`, an analyzable file becomes an
// Entry that stays visible with a usage indicator.
//
// ABI-constrained limit (recorded): the shell only has the strict `cpl_probe_json`
// (probe_lenient is intentionally not in the C ABI, and Phase 9 forbids new
// symbols), so a video whose audio stream was stripped is surfaced as `skipped`
// rather than a `no match` row. Cameras record audio; the common zero-candidate
// `no match` remains reachable. (CLI gate §7.5's stripped-video case is headless.)

import CoupleKit
import Foundation

@MainActor
@Observable
final class MediaPool {
    let engine: Engine
    let toasts: ToastCenter

    enum Role: Sendable { case video, audio }

    /// One imported file. Identity is the content key, so the same file dropped
    /// twice dedups to one entry (and stays pinned in the cache while live).
    @MainActor
    @Observable
    final class Entry: Identifiable {
        let id: String          // content key
        let url: URL
        let name: String        // display filename
        let role: Role
        let durationS: Double
        let creationTimeUtc: String?
        /// The full probe result, retained so the metadata popover (TODO-003) can
        /// render container/codec/dimensions/streams without re-probing. Cheap
        /// (a small Codable value) and the single technical-metadata source — the
        /// popover never infers anything from the filename.
        let info: MediaInfo?
        /// On-disk byte size captured at import via filesystem resource values
        /// (TODO-003); `nil` if the filesystem did not report it.
        let fileSizeBytes: UInt64?
        /// nil only transiently (mid-import) — the pool publishes entries once
        /// imported, so a live entry always carries its handle.
        var handle: TrackHandle?

        init(id: String, url: URL, name: String, role: Role, durationS: Double,
             creationTimeUtc: String?, info: MediaInfo?, fileSizeBytes: UInt64?,
             handle: TrackHandle?) {
            self.id = id
            self.url = url
            self.name = name
            self.role = role
            self.durationS = durationS
            self.creationTimeUtc = creationTimeUtc
            self.info = info
            self.fileSizeBytes = fileSizeBytes
            self.handle = handle
        }

        /// The displayable metadata projection for the info popover (TODO-003).
        /// Pure transform of the already-probed `info` + filesystem size; built on
        /// demand (small) so nothing is re-probed when the popover opens.
        var metaCard: MediaMetaCard {
            MediaMetaCard.make(
                role: role == .video ? .video : .audio,
                durationS: durationS,
                info: info,
                fileSizeBytes: fileSizeBytes,
                creationTimeUtc: creationTimeUtc)
        }
    }

    /// Pool order = import order (deterministic display; the resolver sorts its
    /// own candidates by z/name, so display order never affects resolution).
    var entries: [Entry] = []
    /// Unrecognized files, surfaced once (§2.1 "skipped N unrecognized files").
    var skipped: [String] = []
    /// True while a drop is being probed/imported (drives the load spinner).
    var importing = false
    private(set) var importProgress = ""

    /// Structured import progress for the unified activity strip (TODO-005), kept
    /// alongside the human `importProgress` string (which the initial LoadSurface
    /// still renders verbatim). `nil` when no import is running. Carries the honest
    /// phase + trustworthy fraction + the 1-based file position so the strip can show a
    /// determinate bar (`Building waveform 2 of 5: clip.mov - 68%`). The pure
    /// `ActivitySummaryPolicy` formats it; this only reports the raw signal.
    private(set) var importActivity: ActivitySignals.Importing?
    /// Called whenever `importing` / `importActivity` changes, so the owner (AppModel)
    /// can recompute the activity strip. Set by AppModel; the pool stays decoupled.
    @ObservationIgnored var onImportChange: (() -> Void)?

    init(engine: Engine, toasts: ToastCenter) {
        self.engine = engine
        self.toasts = toasts
    }

    var videos: [Entry] { entries.filter { $0.role == .video } }
    /// Audio entries that imported successfully (a handle is the precondition
    /// for being a pairing resource).
    var audios: [Entry] { entries.filter { $0.role == .audio && $0.handle != nil } }
    var isEmpty: Bool { entries.isEmpty }

    func entry(key: String) -> Entry? { entries.first { $0.id == key } }

    // MARK: - Import (probe → classify → import; dedup by content key)

    /// Import a batch of dropped files and/or folders. Returns the newly added
    /// entries (existing content keys are skipped as already-present). Probing
    /// and the content-key hash run off the main actor; the import job streams
    /// progress. Surfaces a single "skipped N" toast for unrecognized files.
    @discardableResult
    func importDropped(_ urls: [URL]) async -> [Entry] {
        let files = Self.expand(urls)
        importing = true
        defer {
            importing = false; importProgress = ""; importActivity = nil
            onImportChange?()   // clear the strip's import summary on completion (TODO-005)
        }
        let total = files.count
        var added: [Entry] = []
        var newlySkipped: [String] = []
        for (idx, url) in files.enumerated() {
            let name = url.lastPathComponent
            // Re-dropping the exact same file is a no-op; true content dedup (same
            // bytes via a different path) happens post-import on the content key.
            if entries.contains(where: { $0.url == url }) { continue }

            importProgress = "Reading \(name)…"
            // Probe sub-phase: no trustworthy fraction yet (indeterminate, TODO-005).
            importActivity = ActivitySignals.Importing(
                phase: .probing, fraction: 0, index: idx + 1, total: total, filename: name)
            onImportChange?()
            let info: MediaInfo
            do {
                info = try await Task.detached(priority: .userInitiated) { [engine] in
                    try engine.probe(url.path)
                }.value
            } catch {
                newlySkipped.append(name)
                continue
            }
            let role: Role = info.video != nil ? .video : .audio
            guard role == .video || !info.audio.isEmpty else {
                newlySkipped.append(name)
                continue
            }
            // Import the track (decode + pyramid); this is what pins it.
            do {
                let meta = try await importTrack(url: url, role: role, name: name,
                                                 index: idx + 1, total: total)
                // True content dedup: same bytes already pooled via another path.
                // Release the redundant core track and keep the existing entry.
                if entries.contains(where: { $0.id == meta.contentKey }) {
                    engine.track(meta).free()
                    continue
                }
                let entry = Entry(id: meta.contentKey, url: url, name: name, role: role,
                                  durationS: meta.durationS, creationTimeUtc: info.creationTimeUtc,
                                  info: info, fileSizeBytes: Self.fileSize(url),
                                  handle: engine.track(meta))
                entries.append(entry)
                added.append(entry)
            } catch {
                newlySkipped.append(name)
            }
        }
        if !newlySkipped.isEmpty {
            skipped.append(contentsOf: newlySkipped)
            toasts.show(Toast(style: .info,
                              title: "Skipped \(newlySkipped.count) unrecognized file\(newlySkipped.count == 1 ? "" : "s")",
                              detail: newlySkipped.joined(separator: ", ")))
        }
        return added
    }

    private func importTrack(url: URL, role: Role, name: String,
                             index: Int, total: Int) async throws -> TrackMeta {
        let job = try engine.importStart(url.path, role: role == .video ? .camera : .external)
        let progressTask = Task {
            for await p in job.progress {
                self.importProgress = "\(name) — \(Self.phaseLabel(p.phase)) \(Int(p.fraction * 100))%"
                // Structured signal for the activity strip (TODO-005): honest phase +
                // the job's trustworthy fraction + the batch position.
                self.importActivity = ActivitySignals.Importing(
                    phase: Self.importPhase(p.phase), fraction: p.fraction,
                    index: index, total: total, filename: name)
                self.onImportChange?()
            }
        }
        defer { progressTask.cancel() }
        return try await job.value()
    }

    static func phaseLabel(_ phase: String) -> String {
        switch phase {
        case "Decode": return "decoding"
        case "Pyramid": return "peaks"
        default: return "finishing"
        }
    }

    /// Map the core job's phase string to the strip's honest `ImportPhase` (TODO-005).
    /// Only phases the job reports DISTINCTLY get a distinct label; everything else is
    /// "Finishing import" rather than a fabricated phase.
    static func importPhase(_ phase: String) -> ImportPhase {
        switch phase {
        case "Probe": return .probing
        case "Decode": return .decoding
        case "Pyramid": return .waveform
        default: return .finishing   // "Finalize" and any future phase
        }
    }

    /// Free every handle (app teardown). The pool is the sole owner of pool
    /// tracks; couples reference them and never free them.
    func teardown() {
        for e in entries { e.handle?.free() }
        entries = []
    }

    /// Remove the given content keys from the OPEN SESSION (TODO-001). The pool is the
    /// sole owner of pool tracks, so freeing a removed entry's `TrackHandle` here is what
    /// releases it — which only makes its cached content GC-ELIGIBLE (the loaded-track
    /// pin is gone), never a synchronous cache purge (V2-F3). Source files on disk are
    /// untouched. The CALLER must first stop any work touching these keys (preview /
    /// audition / sync / import / resolve) and tear down referencing pair sessions, so a
    /// freed handle is never read after this. Returns the keys actually removed (those
    /// that were live), so the caller can prune selection/couples/persistence to match.
    @discardableResult
    func remove(keys: Set<String>) -> Set<String> {
        guard !keys.isEmpty else { return [] }
        var removed: Set<String> = []
        entries.removeAll { e in
            guard keys.contains(e.id) else { return false }
            e.handle?.free()  // release the cache-GC pin; not a cache purge
            removed.insert(e.id)
            return true
        }
        return removed
    }

    // MARK: - helpers

    /// On-disk size in bytes via filesystem resource values (TODO-003) — a
    /// structured API, never inferred. `nil` if the filesystem does not report it.
    static func fileSize(_ url: URL) -> UInt64? {
        guard let size = try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize else { return nil }
        return UInt64(size)
    }

    /// Expand any dropped directories into their immediate files (sorted, dotfiles
    /// skipped) — matches `couple pair`'s deterministic directory listing.
    private static func expand(_ urls: [URL]) -> [URL] {
        var out: [URL] = []
        let fm = FileManager.default
        for url in urls {
            var isDir: ObjCBool = false
            guard fm.fileExists(atPath: url.path, isDirectory: &isDir) else { continue }
            if isDir.boolValue {
                let kids = (try? fm.contentsOfDirectory(at: url, includingPropertiesForKeys: nil))?
                    .filter { !$0.lastPathComponent.hasPrefix(".") }
                    .sorted { $0.lastPathComponent < $1.lastPathComponent } ?? []
                out.append(contentsOf: kids.filter { (try? $0.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) != true })
            } else if !url.lastPathComponent.hasPrefix(".") {
                out.append(url)
            }
        }
        return out
    }
}
