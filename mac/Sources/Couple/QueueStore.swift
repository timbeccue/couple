// Queue persistence (SPEC-V2 §5.4) — the new versioned session file
// `couple-queue/1`, written beside (and not replacing) the per-pair alignment
// autosave (SessionStore, unchanged: each couple keeps its own file keyed by
// content keys). The queue file stores the media-pool file list, each couple's
// resolution (selected source + pinned/forced + per-couple export overrides),
// the approved flags, and the export destination/template — so reopening the app
// restores the whole queue. Saved on the v1 triggers (approve / export / quit)
// plus pairing changes.

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
        var approved: Bool
        var stemOverride: String?
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
                              pinned: $0.pinned, forced: $0.forced, approved: $0.approved,
                              stemOverride: $0.stemOverride)
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
        if let data = try? enc.encode(saved) {
            try? data.write(to: url(), options: .atomic)
        }
    }

    /// The saved queue, if one exists whose files are all still on disk.
    static func load() -> SavedQueue? {
        guard let data = try? Data(contentsOf: url()),
              let saved = try? JSONDecoder().decode(SavedQueue.self, from: data),
              saved.schema == schema, !saved.files.isEmpty else { return nil }
        // Only offer restore if every file still exists (paths are stable).
        guard saved.files.allSatisfy({ FileManager.default.fileExists(atPath: $0.path) }) else { return nil }
        return saved
    }
}
