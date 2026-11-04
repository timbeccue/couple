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
                // §1.2 resize policy: below ~1000×560 the window scrolls rather
                // than crushing the waveforms (RootView wraps in a ScrollView
                // when smaller; the frame minimums keep the default sane).
                .frame(minWidth: 640, minHeight: 400)
        }
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: 1280, height: 820)
        .commands {
            // About opens our own panel (the standard one cannot link to
            // Acknowledgements); Help lists Acknowledgements directly too.
            CommandGroup(replacing: .appInfo) { AboutCommands() }
            CommandGroup(after: .help) { AcknowledgementsCommands() }
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
