import AppKit
import CoupleKit
import SwiftUI

/// Held for the app's lifetime when the debug drive is active: App Nap
/// otherwise stretches the drive's timers by an order of magnitude.
private var debugActivityToken: NSObjectProtocol?

@main
struct CoupleApp: App {
    @State private var model = AppModel()

    init() {
        // Diagnostics must reach pipes promptly (debug drives grep stdout).
        setvbuf(stdout, nil, _IOLBF, 0)
        if ProcessInfo.processInfo.environment.keys.contains(where: { $0.hasPrefix("COUPLE_") }) {
            debugActivityToken = ProcessInfo.processInfo.beginActivity(
                options: [.userInitiated, .latencyCritical], reason: "couple debug drive")
        }
        // SPM executables launch as background processes; make this a regular app.
        NSApplication.shared.setActivationPolicy(.regular)
        DispatchQueue.main.async {
            NSApplication.shared.activate(ignoringOtherApps: true)
        }
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .preferredColorScheme(.dark)
                // Minimum sizes are per arrangement now (RootView: compact 340 × 560,
                // full 640 × 400; the window's min/max follow the arrangement —
                // AppModel.applyWindowLayout), so compact can be 360 wide.
        }
        .windowStyle(.hiddenTitleBar)
        // The window's min AND max size follow the root view's per-arrangement
        // frame limits (compact: width 340–420, min height 560; full: min 640 × 400).
        .windowResizability(.contentSize)
        .defaultSize(width: 1280, height: 820)
        .commands {
            // About opens our own panel (the standard one cannot link to
            // Acknowledgements); Help lists Acknowledgements directly too.
            CommandGroup(replacing: .appInfo) { AboutCommands() }
            CommandGroup(after: .help) { AcknowledgementsCommands() }
            // View ▸ Compact Window / Full Window (⌘⇧C) — same action as the
            // session line's ⤢ Full / ⤡ Compact (UX §V2.11).
            CommandGroup(before: .toolbar) { LayoutCommands(model: model) }
        }

        Window("About Couple", id: "about") {
            AboutView()
                .preferredColorScheme(.dark)
        }
        .windowResizability(.contentSize)

        Window("Acknowledgements", id: "acknowledgements") {
            AcknowledgementsView()
                .preferredColorScheme(.dark)
                .frame(minWidth: 720, minHeight: 420)
        }
        .defaultSize(width: 920, height: 600)
    }
}

/// `Commands` cannot hold an environment value directly; a view can.
private struct AboutCommands: View {
    @Environment(\.openWindow) private var openWindow
    var body: some View {
        Button("About Couple") { openWindow(id: "about") }
    }
}

private struct AcknowledgementsCommands: View {
    @Environment(\.openWindow) private var openWindow
    var body: some View {
        Button("Acknowledgements") { openWindow(id: "acknowledgements") }
    }
}

private struct LayoutCommands: View {
    let model: AppModel
    var body: some View {
        Button(model.layoutMode == .compact ? "Full Window" : "Compact Window") {
            model.toggleLayoutMode()
        }
        .keyboardShortcut("c", modifiers: [.command, .shift])
    }
}
