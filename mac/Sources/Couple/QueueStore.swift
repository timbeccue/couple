// Queue persistence (SPEC-V2 §5.4) — the new versioned session file
// `couple-queue/1`, written beside (and not replacing) the per-pair alignment
// autosave (SessionStore, unchanged: each couple keeps its own file keyed by
// content keys). The queue file stores the media-pool file list, each couple's
// resolution (selected source + pinned/forced + per-couple export overrides),
// the include-for-export state (the user's explicit toggle + `exportedAt`, D8),
// and the export destination/template — so reopening the app restores the whole
// queue. Saved on the v1 triggers (include toggle / export / quit) plus pairing
// changes.

import CoupleKit
import Foundation

struct SavedQueue: Codable {
    struct File: Codable {
        var path: String
        var contentKey: String
        var role: String // "video" | "audio"
    }

    struct Couple: Codable {
        var videoKey: String
        var selectedAudioKey: String?
        var pinned: Bool
        var forced: Bool
        var stemOverride: String?
        // TODO-009: selection provenance + the manual pair's evidence verdict, so a
        // restored manual pick keeps both WHICH source it used and WHETHER that pair
        // independently matched (its dot color). OPTIONAL for back-compat: a pre-009
        // queue file lacks them and decodes to nil; `applyRestore` then infers
        // provenance from `pinned` (a pinned pick is manual) and leaves the evidence
        // to be recomputed, so the schema stays `couple-queue/1` (no version bump,
        // mirroring how RowCandidate-era optionals were added).
        var selectionProvenance: SelectionProvenance?
        var manualMatchConfident: Bool?
        // PLAN-COMPACT-MODE D8: the include-for-export inputs, both OPTIONAL with the
        // same back-compat idiom (schema stays `couple-queue/1`). A file saved under
        // the approve model has neither (its `approved` key is simply not read), so it
        // decodes to nil/nil ⇒ the policy default: every good couple included, no
        // export history (SPEC-CHANGE 2026-11-05 [D1] loading note).
        var explicit: Bool?
        var exportedAt: Date?
    }

    struct Export: Codable {
        var destination: String?
        var globalFormat: String
        var keepCameraAudio: Bool
    }

    var schema: String
    var savedAt: String
    var files: [File]
    var couples: [Couple]
    var export: Export
}

enum QueueStore {
    static let schema = "couple-queue/1"

    private static func url() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let dir = base.appendingPathComponent("Couple", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("queue.json")
    }

    @MainActor
    static func save(model: AppModel) {
        // Don't clobber the saved session with an empty one mid-teardown.
        guard !model.couples.isEmpty || !model.pool.isEmpty else { return }
        let files = model.pool.entries.map {
            SavedQueue.File(path: $0.url.path, contentKey: $0.id,
                            role: $0.role == .video ? "video" : "audio")
        }
        let couples = model.couples.map {
            SavedQueue.Couple(videoKey: $0.id, selectedAudioKey: $0.selectedAudioKey,
                              pinned: $0.pinned, forced: $0.forced,
                              stemOverride: $0.stemOverride,
                              selectionProvenance: $0.selectionProvenance,
                              manualMatchConfident: $0.manualMatchConfident,
                              explicit: $0.explicit, exportedAt: $0.exportedAt)
        }
        let saved = SavedQueue(
            schema: schema,
            savedAt: ISO8601DateFormatter().string(from: Date()),
            files: files, couples: couples,
            export: .init(destination: model.exportDestination?.path,
                          globalFormat: model.globalFormat.rawValue,
                          keepCameraAudio: model.globalKeepCameraAudio))
        let enc = JSONEncoder()
        enc.outputFormatting = [.prettyPrinted, .sortedKeys]
        enc.dateEncodingStrategy = .iso8601 // `exportedAt` reads like `savedAt`
        if let data = try? enc.encode(saved) {
            try? data.write(to: url(), options: .atomic)
        }
    }

    /// Remove the persisted restore record (TODO-001 Clear Session). Deleting the file
    /// IS persisting the cleared state: `load()` then returns nil, so the cleared queue
    /// is never offered for restore on relaunch. The `save` guard already refuses to
    /// write an empty session, so a teardown `saveAll()` after a clear cannot recreate
    /// it. Source files and previously exported files are untouched. Idempotent.
    static func clear() {
        try? FileManager.default.removeItem(at: url())
    }

    /// The saved queue, if one exists whose files are all still on disk.
    static func load() -> SavedQueue? {
        let dec = JSONDecoder()
        dec.dateDecodingStrategy = .iso8601
        guard let data = try? Data(contentsOf: url()),
              let saved = try? dec.decode(SavedQueue.self, from: data),
              saved.schema == schema, !saved.files.isEmpty else { return nil }
        // Only offer restore if every file still exists (paths are stable).
        guard saved.files.allSatisfy({ FileManager.default.fileExists(atPath: $0.path) }) else { return nil }
        return saved
    }
}
