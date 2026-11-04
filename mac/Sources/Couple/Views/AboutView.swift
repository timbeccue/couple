// About panel (replaces the standard one so Acknowledgements is reachable from
// it; docs/LICENSING.md §1 obligation 2 "tell the user"). Version and the
// copyright/ffmpeg line come from Info.plist as make-app.sh wrote them; a bare
// `swift build` run has no bundle and says so. Electron twin: views/AboutView.tsx.

import AppKit
import SwiftUI

struct AboutView: View {
    @Environment(\.openWindow) private var openWindow

    private var version: String {
        let info = Bundle.main.infoDictionary
        if let v = info?["CFBundleShortVersionString"] as? String { return "Version \(v)" }
        return "Development run (no app bundle)"
    }

    private var copyright: String {
        (Bundle.main.infoDictionary?["NSHumanReadableCopyright"] as? String)
            ?? "© 2026 Tim Beccue."
    }

    var body: some View {
        VStack(spacing: 14) {
            Image(nsImage: NSApp.applicationIconImage)
                .resizable()
                .frame(width: 96, height: 96)
            VStack(spacing: 4) {
                Text("Couple")
                    .font(.system(size: 22, weight: .semibold))
                Text(version)
                    .font(.system(size: 12))
                    .foregroundStyle(Theme.textSecondary)
            }
            Text(copyright)
                .font(.system(size: 11))
                .foregroundStyle(Theme.textSecondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 10) {
                Button("Acknowledgements") { openWindow(id: "acknowledgements") }
                    .keyboardShortcut(.defaultAction)
                if let folder = Acknowledgements.locateFolder(),
                   FileManager.default.fileExists(atPath: folder.path) {
                    Button("Show Licenses Folder") {
                        NSWorkspace.shared.activateFileViewerSelecting([folder])
                    }
                }
            }
            .padding(.top, 4)
        }
        .padding(28)
        .frame(width: 380)
        .background(Theme.windowBg)
        .background(NonRestorableWindow())
    }
}

/// Opts the hosting window out of state restoration: an About or
/// Acknowledgements window left open at quit should not reopen at launch.
struct NonRestorableWindow: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async { view.window?.isRestorable = false }
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {
        view.window?.isRestorable = false
    }
}
