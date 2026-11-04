// Bundled third-party notices (docs/LICENSING.md §3): where the Licenses
// folder is, which components it covers, and the notice text per component.
// Pure loading + parsing of what scripts/third-party-notices.sh generated and
// make-app.sh copied into Contents/Resources/Licenses/; nothing here decides
// anything (PLAN-CORE-SHELL-BOUNDARY §1). The Electron shell's
// app/Licenses.ts reads the same files the same way.
//
// Layout of the folder (make-app.sh):
//   Licenses/components.json      component list (name, version, license, tree)
//   Licenses/rust-crates.txt      one `==== name version` section per component
//   Licenses/npm-packages.txt
//   Licenses/swift-packages.txt
//   Licenses/ffmpeg/              COPYING.LGPLv2.1, LICENSE.md, SOURCE.txt —
//                                 only in a --dist bundle (ffmpeg is otherwise
//                                 the machine's own copy, not shipped)
//
// COUPLE_LICENSES_DIR overrides the folder (dev runs from `swift build` have
// no bundle; point it at the repo's third-party/).

import Foundation

struct LicenseComponent: Identifiable, Hashable {
    let id: String
    let name: String
    let version: String
    let license: String
    /// Set when the declared license is an OR and a single alternative was chosen.
    let usedUnder: String?
    let source: String?
}

/// One `==== name version` block of a notices file.
struct NoticeSection: Identifiable {
    let id: String
    let title: String
    /// `License:` / `Used under:` / `Source:` lines, as written.
    let header: [String]
    let body: String
}

/// One file the Acknowledgements view can show in full.
struct NoticeDocument: Identifiable {
    let id: String
    let fileName: String
    let sections: [NoticeSection]
}

/// A group in the component list (ffmpeg, Rust crates, npm packages, ...)
/// and the documents that hold its notices.
struct NoticeTree: Identifiable {
    let id: String
    let title: String
    let components: [LicenseComponent]
    let documents: [NoticeDocument]
    /// Which document a component's section lives in, by component id.
    let documentByComponent: [String: String]
}

struct Acknowledgements {
    let folder: URL?
    let trees: [NoticeTree]

    var isEmpty: Bool { trees.allSatisfy { $0.components.isEmpty } }
    var componentCount: Int { trees.reduce(0) { $0 + $1.components.count } }

    static func locateFolder() -> URL? {
        if let override = ProcessInfo.processInfo.environment["COUPLE_LICENSES_DIR"], !override.isEmpty {
            return URL(fileURLWithPath: override, isDirectory: true)
        }
        return Bundle.main.resourceURL?.appendingPathComponent("Licenses", isDirectory: true)
    }

    static func load() -> Acknowledgements {
        let folder = locateFolder()
        guard let folder, FileManager.default.fileExists(atPath: folder.path) else {
            return Acknowledgements(folder: folder, trees: [])
        }
        var trees: [NoticeTree] = []
        if let ffmpeg = loadFFmpeg(folder.appendingPathComponent("ffmpeg", isDirectory: true)) {
            trees.append(ffmpeg)
        }
        trees.append(contentsOf: loadGenerated(folder))
        return Acknowledgements(folder: folder, trees: trees)
    }

    // MARK: - ffmpeg (make-app.sh --dist; fetch-ffmpeg.sh writes the files)

    private static func loadFFmpeg(_ dir: URL) -> NoticeTree? {
        let sourceURL = dir.appendingPathComponent("SOURCE.txt")
        guard let source = try? String(contentsOf: sourceURL, encoding: .utf8) else { return nil }
        // First line: "ffmpeg/ffprobe 9.0.2 — LGPL v2.1 or later build for macOS arm64"
        // (fetch-ffmpeg.sh) or "… — LGPL v3 or later build for Windows x64" (the
        // Electron shell's scripts/fetch-ffmpeg.mjs; mirrored here to keep the
        // loaders identical — the mac bundle itself is always the v2.1 build).
        let firstLine = source.split(separator: "\n", maxSplits: 1).first.map(String.init) ?? ""
        let version = firstLine.split(separator: " ").first(where: { $0.first?.isNumber == true }).map(String.init) ?? ""
        let license = firstLine.contains("LGPL v3") ? "LGPL-3.0-or-later" : "LGPL-2.1-or-later"
        let component = LicenseComponent(
            id: "ffmpeg", name: "ffmpeg / ffprobe", version: version,
            license: license, usedUnder: nil, source: "https://ffmpeg.org")
        var documents: [NoticeDocument] = []
        for name in ["COPYING.LGPLv2.1", "COPYING.LGPLv3", "LICENSE.md", "SOURCE.txt"] {
            let url = dir.appendingPathComponent(name)
            guard let text = try? String(contentsOf: url, encoding: .utf8) else { continue }
            documents.append(NoticeDocument(
                id: "ffmpeg/\(name)", fileName: name,
                sections: [NoticeSection(id: "ffmpeg/\(name)", title: name, header: [], body: text)]))
        }
        guard !documents.isEmpty else { return nil }
        return NoticeTree(
            id: "ffmpeg", title: "ffmpeg", components: [component], documents: documents,
            documentByComponent: ["ffmpeg": documents[0].id])
    }

    // MARK: - generated trees (components.json + one notices file each)

    private struct Manifest: Decodable {
        struct Tree: Decodable {
            let id: String
            let title: String
            let file: String
        }
        struct Component: Decodable {
            let name: String
            let version: String
            let license: String
            let usedUnder: String?
            let tree: String
            let source: String?
        }
        let trees: [Tree]
        let components: [Component]
    }

    private static func loadGenerated(_ folder: URL) -> [NoticeTree] {
        guard let data = try? Data(contentsOf: folder.appendingPathComponent("components.json")),
              let manifest = try? JSONDecoder().decode(Manifest.self, from: data)
        else { return [] }
        return manifest.trees.map { tree in
            let components = manifest.components.filter { $0.tree == tree.id }.map { c in
                LicenseComponent(
                    id: "\(tree.id)/\(c.name)@\(c.version)", name: c.name, version: c.version,
                    license: c.license, usedUnder: c.usedUnder == c.license ? nil : c.usedUnder,
                    source: c.source)
            }
            let text = (try? String(contentsOf: folder.appendingPathComponent(tree.file), encoding: .utf8)) ?? ""
            let document = NoticeDocument(id: tree.file, fileName: tree.file, sections: parseSections(text, prefix: tree.id))
            return NoticeTree(
                id: tree.id, title: tree.title, components: components, documents: [document],
                documentByComponent: Dictionary(uniqueKeysWithValues: components.map { ($0.id, document.id) }))
        }
    }

    /// Splits a generated notices file into its `==== name version` sections.
    /// The preamble before the first marker becomes a section of its own, so
    /// the file's own explanation of what it covers is shown too. Section ids
    /// match `LicenseComponent.id` so a list selection can scroll to its text.
    static func parseSections(_ text: String, prefix: String) -> [NoticeSection] {
        var sections: [NoticeSection] = []
        var title = "About this file"
        var id = "\(prefix)/preamble"
        var header: [String] = []
        var body: [String] = []
        var inHeader = false

        func flush() {
            let joined = body.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
            if !joined.isEmpty || !header.isEmpty {
                sections.append(NoticeSection(id: id, title: title, header: header, body: joined))
            }
        }

        for rawLine in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(rawLine)
            if line.hasPrefix("==== ") {
                flush()
                title = String(line.dropFirst(5))
                let parts = title.split(separator: " ", maxSplits: 1).map(String.init)
                id = "\(prefix)/\(parts.first ?? title)@\(parts.count > 1 ? parts[1] : "")"
                header = []
                body = []
                inHeader = true
            } else if inHeader {
                if line == "----" {
                    inHeader = false
                } else {
                    header.append(line)
                }
            } else {
                body.append(line)
            }
        }
        flush()
        return sections
    }
}
