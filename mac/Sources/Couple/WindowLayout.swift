// Arrangement + window frame management (UX §V2.11; PLAN-COMPACT-MODE Phase 4.2/4.4).
// One window, two arrangements of the same session. The model owns the mode and
// moves the hosting NSWindow (handed over by `WindowAccessor`) to each
// arrangement's last frame:
//  - compact: default 360 × 700; width clamped 340–420, height free with min 560
//    (§6.2) via the window's min/max size while compact;
//  - full: default 1280 × 820, min 640 × 400 (as before Phase 4).
// Each arrangement's last frame and the mode persist in UserDefaults under keys in
// the app's bundle-id namespace; the mode defaults to COMPACT when the key is
// absent (§6.1). Players and the audition live in controllers (AuditionController,
// PoolPreviewController) owned by the model, so switching never restarts playback.
// Phase 5 adds the animation; this switch is instant.

import AppKit
import CoupleKit

enum LayoutMode: String, Sendable {
    case compact, full
}

extension AppModel {
    static let layoutModeKey = "com.timbeccue.couple.layoutMode"
    static func frameKey(_ mode: LayoutMode) -> String { "com.timbeccue.couple.windowFrame.\(mode.rawValue)" }

    static let compactDefaultSize = NSSize(width: 360, height: 700)
    static let fullDefaultSize = NSSize(width: 1280, height: 820)
    static let compactMinSize = NSSize(width: 340, height: 560)
    static let compactMaxWidth: CGFloat = 420
    static let fullMinSize = NSSize(width: 640, height: 400)

    /// The persisted arrangement; compact when absent (first launch after the update).
    static func storedLayoutMode() -> LayoutMode {
        UserDefaults.standard.string(forKey: layoutModeKey).flatMap(LayoutMode.init(rawValue:)) ?? .compact
    }

    /// Most common parent-folder name among the imported files ("bach session"), or
    /// nil for an empty pool. Presentation only (the session line's identity).
    var sessionName: String? {
        var counts: [String: Int] = [:]
        for e in pool.entries {
            counts[e.url.deletingLastPathComponent().lastPathComponent, default: 0] += 1
        }
        return counts.max { a, b in a.value == b.value ? a.key > b.key : a.value < b.value }?.key
    }

    func toggleLayoutMode() {
        setLayoutMode(layoutMode == .compact ? .full : .compact)
    }

    /// Return in compact: switch to full with the selected couple's editor open.
    func openFullEditor() {
        guard selectedCouple != nil else { return }
        setLayoutMode(.full)
        syncToolsExpanded = true
    }

    /// Switch arrangements: remember the outgoing frame, map the monitor mode
    /// (D10), flip the mode, then move the window to the incoming frame.
    func setLayoutMode(_ mode: LayoutMode) {
        guard mode != layoutMode else { return }
        recordWindowFrame()
        if mode == .compact {
            // D10: Pan L/R has no compact segment — heard (and shown) as Synced
            // while compact; the policy still decides via selectMonitor.
            if audition.monitorMode == .pan {
                audition.selectMonitor(.overlay)
                panHeldForCompact = true
            } else {
                panHeldForCompact = false
            }
            // The overview / loupes leave the window: no stale scroll-routing rects.
            loupeRects = [:]
            overviewRect = .zero
        } else if panHeldForCompact {
            // Back to the user's Pan, unless they picked something else meanwhile.
            if audition.monitorMode == .overlay { audition.selectMonitor(.pan) }
            panHeldForCompact = false
        }
        layoutMode = mode
        if persistLayout { UserDefaults.standard.set(mode.rawValue, forKey: Self.layoutModeKey) }
        applyWindowLayout()
    }

    /// Harness entry (DebugDrive.configure): force an arrangement before the window
    /// exists, and never touch the defaults domain for the rest of the run.
    func forceLayoutForHarness(_ mode: LayoutMode) {
        persistLayout = false
        layoutFrames = [:]
        panHeldForCompact = false
        layoutMode = mode
        applyWindowLayout()
    }

    // MARK: - Window

    /// Called once per hosting window by `WindowAccessor`.
    func attachWindow(_ w: NSWindow) {
        guard window !== w else { return }
        window = w
        if persistLayout {
            for mode in [LayoutMode.compact, .full] {
                if let s = UserDefaults.standard.string(forKey: Self.frameKey(mode)) {
                    let r = NSRectFromString(s)
                    if r.width > 0, r.height > 0 { layoutFrames[mode] = r }
                }
            }
        }
        for o in windowObservers { NotificationCenter.default.removeObserver(o) }
        windowObservers = [NSWindow.didResizeNotification, NSWindow.didMoveNotification].map { name in
            NotificationCenter.default.addObserver(forName: name, object: w, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.recordWindowFrame() }
            }
        }
        applyWindowLayout()
    }

    /// Remember the current window frame as the current arrangement's last frame.
    func recordWindowFrame() {
        guard let w = window, !applyingLayoutFrame, w.frame.width > 0 else { return }
        // A frame outside the arrangement's clamps is a transient (mid-switch) frame.
        if layoutMode == .compact && w.frame.width > Self.compactMaxWidth + 0.5 { return }
        if layoutMode == .full && w.frame.width < Self.fullMinSize.width - 0.5 { return }
        layoutFrames[layoutMode] = w.frame
        if persistLayout {
            UserDefaults.standard.set(NSStringFromRect(w.frame), forKey: Self.frameKey(layoutMode))
        }
    }

    /// The frame the window takes for `mode`: its last frame (clamped), else the
    /// default size anchored at the current top-left (with `.contentSize`
    /// resizability the scene's defaultSize no longer applies, so the defaults
    /// live here).
    func targetFrame(for mode: LayoutMode, current: NSRect) -> NSRect {
        var f: NSRect
        if let last = layoutFrames[mode] {
            f = last
        } else {
            let size = mode == .compact ? Self.compactDefaultSize : Self.fullDefaultSize
            f = NSRect(x: current.minX, y: current.maxY - size.height, width: size.width, height: size.height)
        }
        if mode == .compact {
            let w = min(max(f.width, Self.compactMinSize.width), Self.compactMaxWidth)
            let h = max(f.height, Self.compactMinSize.height)
            f = NSRect(x: f.minX, y: f.maxY - h, width: w, height: h)
        } else {
            let w = max(f.width, Self.fullMinSize.width)
            let h = max(f.height, Self.fullMinSize.height)
            f = NSRect(x: f.minX, y: f.maxY - h, width: w, height: h)
        }
        return f
    }

    /// Move the window to the current arrangement's frame. The resize LIMITS are
    /// SwiftUI's: the scene uses `.windowResizability(.contentSize)`, so the root
    /// view's per-arrangement frame (RootView) becomes the window's min/max size —
    /// compact 340–420 wide, min 560 high; full min 640 × 400. They are lifted here
    /// for the move only (the outgoing arrangement's limits would refuse the
    /// incoming frame) and SwiftUI re-applies the new ones on its next pass.
    func applyWindowLayout() {
        guard let w = window else { return }
        applyingLayoutFrame = true
        let target = w.constrainFrameRect(targetFrame(for: layoutMode, current: w.frame), to: w.screen)
        let mode = layoutMode
        w.contentMinSize = NSSize(width: Self.compactMinSize.width, height: Self.fullMinSize.height)
        w.contentMaxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
        w.setFrame(target, display: true, animate: false)
        // SwiftUI swaps the arrangement (and its size limits) on the next pass;
        // re-assert the frame once it has, then start recording frames again.
        DispatchQueue.main.async { [weak self, weak w] in
            guard let self, let w, self.layoutMode == mode else { self?.applyingLayoutFrame = false; return }
            if w.frame != target { w.setFrame(target, display: true, animate: false) }
            self.applyingLayoutFrame = false
            self.recordWindowFrame()
        }
    }

    // MARK: - "No audio" → Choose file… (compact)

    /// Pair `url` with `couple` as a manual pick. A file already in the pool repairs
    /// at once (the same `requestRepairDrop` path a drop uses); a new file is
    /// imported first and paired once the pool has it.
    func pairWithChosenFile(_ couple: CoupleRow, url: URL) {
        let target = url.standardizedFileURL
        func entryKey() -> String? {
            pool.entries.first { $0.url.standardizedFileURL == target && $0.handle != nil }?.id
        }
        if let key = entryKey() {
            _ = requestRepairDrop(couple, audioKey: key)
            return
        }
        importDropped([url])
        let id = couple.id
        Task { @MainActor [weak self] in
            for _ in 0..<1200 {
                try? await Task.sleep(nanoseconds: 100_000_000)
                guard let self else { return }
                if !self.pool.importing, !self.resolving, let key = entryKey() {
                    if let c = self.couples.first(where: { $0.id == id }) {
                        _ = self.requestRepairDrop(c, audioKey: key)
                    }
                    return
                }
            }
        }
    }
}
