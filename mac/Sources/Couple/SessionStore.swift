// Autosave/restore (SPEC §11.5, §14 item 6.8): the §4.5 session-file wrapper,
// written to Application Support keyed by the pair's content keys — on
// verified-toggle, export, and clean quit; restore offered when the same pair
// is reopened. No document type in v1.

import CoupleKit
import Foundation

struct SessionFile: Codable {
    struct TrackRef: Codable {
        var path: String
        var contentKey: String
        var durationS: Double
    }

    var schema: String
    var savedAt: String
    var camera: TrackRef
    var external: TrackRef
    var alignment: AlignmentState
}

enum SessionStore {
    static let schema = "couple-session/1"

    static func directory() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let dir = base.appendingPathComponent("Couple/sessions", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    static func url(cameraKey: String, externalKey: String) -> URL {
        directory().appendingPathComponent("\(cameraKey)_\(externalKey).json")
    }

    static func save(camera: TrackMeta, external: TrackMeta, alignment: AlignmentState) {
        let file = SessionFile(
            schema: schema,
            savedAt: ISO8601DateFormatter().string(from: Date()),
            camera: .init(path: camera.info.path, contentKey: camera.contentKey,
                          durationS: camera.durationS),
            external: .init(path: external.info.path, contentKey: external.contentKey,
                            durationS: external.durationS),
            alignment: alignment
        )
        let enc = JSONEncoder()
        enc.outputFormatting = [.prettyPrinted, .sortedKeys]
        if let data = try? enc.encode(file) {
            try? data.write(to: url(cameraKey: camera.contentKey, externalKey: external.contentKey),
                            options: .atomic)
        }
    }

    static func load(cameraKey: String, externalKey: String) -> SessionFile? {
        guard let data = try? Data(contentsOf: url(cameraKey: cameraKey, externalKey: externalKey)),
              let file = try? JSONDecoder().decode(SessionFile.self, from: data),
              file.schema == schema else {
            return nil
        }
        return file
    }
}
