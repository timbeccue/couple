import AppKit
import CoupleKit
import SwiftUI

@main
struct CoupleApp: App {
    @State private var model = AppModel()

    init() {
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
    }
}
