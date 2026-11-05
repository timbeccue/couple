// Hands the hosting NSWindow to the model once (PLAN-COMPACT-MODE Phase 4.4): the
// SwiftUI scene gives no window access, and the arrangement switch must set the
// window's frame and min/max size.

import AppKit
import SwiftUI

struct WindowAccessor: NSViewRepresentable {
    let onWindow: (NSWindow) -> Void

    func makeNSView(context: Context) -> NSView {
        let v = WindowReportingView()
        v.onWindow = onWindow
        return v
    }

    func updateNSView(_ nsView: NSView, context: Context) {}

    final class WindowReportingView: NSView {
        var onWindow: ((NSWindow) -> Void)?
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            guard let window else { return }
            DispatchQueue.main.async { [weak self] in
                guard self?.window === window else { return }
                self?.onWindow?(window)
            }
        }
    }
}
