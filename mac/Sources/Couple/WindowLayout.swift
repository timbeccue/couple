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
//
// Phase 5 — the animated switch (see `animateLayoutSwitch` for which mitigation
// shipped and why).

import AppKit
import CoupleKit
import SwiftUI

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
        if persistLayout { UserDefaults.standard.set(mode.rawValue, forKey: Self.layoutModeKey) }
        if layoutMotionAllowed, let w = window {
            animateLayoutSwitch(to: mode, window: w)
        } else {
            // Reduce Motion (or no window yet): instant switch, no frame animation.
            layoutMode = mode
            applyWindowLayout()
        }
    }

    /// Phase 5.2: Reduce Motion ⇒ instant switch. The harness can force either way.
    var layoutMotionAllowed: Bool {
        layoutMotionOverride ?? !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
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

    // MARK: - Animated switch (PLAN-COMPACT-MODE Phase 5)

    static let layoutAnimationDuration: Double = 0.45
    static let layoutExtrasFadeDuration: Double = 0.25

    /// The animated arrangement switch: the window frame and the four blocks move
    /// together over 0.45 s on one ease-out curve; the surfaces unique to one
    /// arrangement fade in (0.25 s) once the blocks have settled.
    ///
    /// SHIPPED: mitigation (b) — explicit FLIP. Record of the attempts (Phase 5.2's
    /// mitigation order; harness: COUPLE_LAYOUT_TOGGLE_TEST):
    ///  (a) both arrangements in one ZStack, a shared @Namespace and
    ///      `matchedGeometryEffect` on the four blocks, `withAnimation(.easeOut(0.45))`
    ///      on the mode: FAILED — inside the HSplitView panes the matched blocks got
    ///      garbage frames mid-move (the preview at 1602 × 1001 in a 360-wide window,
    ///      session / list / export stretched to the full 1280 width, the list drawn
    ///      nowhere) and the session / export blocks kept a leftover frame after
    ///      settling (they no longer matched a fresh render).
    ///  (b) FLIP, shipped: the incoming arrangement is swapped in at once, pinned at
    ///      its final size, with each block drawn back on its old frame by a render-
    ///      time `visualEffect` (translate + uniform scale, `FlipBlock`); clearing
    ///      those under an ease-out animation plays the blocks home while
    ///      `NSAnimationContext` moves the window frame on the same curve. Layout
    ///      never changes mid-move, the AVPlayer layer is never re-created by the
    ///      move, and settled frames match a fresh render. NSSplitView panes clip,
    ///      so they are unclipped for the move only.
    ///  (c) crossfade + frame animation: not needed.
    func animateLayoutSwitch(to mode: LayoutMode, window w: NSWindow) {
        applyingLayoutFrame = true
        let target = w.constrainFrameRect(targetFrame(for: mode, current: w.frame), to: w.screen)
        // Lift the outgoing arrangement's resize limits for the move (as
        // applyWindowLayout does); SwiftUI re-applies the incoming ones afterwards.
        w.contentMinSize = NSSize(width: Self.compactMinSize.width, height: Self.fullMinSize.height)
        w.contentMaxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
        // The root lays the incoming arrangement out at its FINAL content size for
        // the whole move (anchored top-left, the window edge revealing / covering
        // it), so no block re-flows at intermediate window widths.
        layoutTransitionSize = w.contentRect(forFrameRect: target).size
        // FIRST: the four blocks' current frames (window space).
        let first = layoutBlockFrames.filter { k, _ in LayoutBlock(rawValue: k) != nil }
        // LAST + INVERT, with no animation: swap in the incoming arrangement with
        // each block drawn back on its old frame, the extras transparent, and make
        // AppKit lay it out and draw it now (the swap and the inverted transform
        // land in the same frame — no flash of the blocks at their new places).
        var t = Transaction(animation: nil)
        t.disablesAnimations = true
        withTransaction(t) {
            layoutFlipFrom = first
            layoutExtrasHidden = true
            layoutAnimating = true
            layoutMode = mode
        }
        w.contentView?.layoutSubtreeIfNeeded()
        // The full arrangement's columns are NSSplitView panes, which clip: a block
        // flying in from (or out to) another column would vanish at the pane edge.
        // Unclip the split view and its panes for the move only.
        unclipSplitPanes(in: w.contentView)
        w.displayIfNeeded()
        // PLAY: the blocks home and the window frame to its target on one curve.
        let duration = Self.layoutAnimationDuration
        DispatchQueue.main.async { [weak self, weak w] in
            guard let self, let w, self.layoutMode == mode else { return }
            // On the first switch to full the split view only exists after that draw.
            self.unclipSplitPanes(in: w.contentView)
            withAnimation(.easeOut(duration: duration)) {
                self.layoutFlipFrom = [:]
            }
            NSAnimationContext.runAnimationGroup { ctx in
                ctx.duration = duration
                ctx.timingFunction = CAMediaTimingFunction(name: .easeOut)
                ctx.allowsImplicitAnimation = true
                w.animator().setFrame(target, display: true)
            } completionHandler: { [weak self, weak w] in
                MainActor.assumeIsolated {
                    self?.finishLayoutSwitch(mode: mode, target: target, window: w)
                }
            }
        }
    }

    private func finishLayoutSwitch(mode: LayoutMode, target: NSRect, window w: NSWindow?) {
        // A newer switch started meanwhile: it owns the flags now.
        guard layoutMode == mode else { return }
        if let w, w.frame != target { w.setFrame(target, display: true, animate: false) }
        // Back to the arrangement's own size limits (no animation: the window is at
        // the pinned size already, so nothing moves).
        restoreSplitPaneClipping()
        var t = Transaction(animation: nil)
        t.disablesAnimations = true
        withTransaction(t) { layoutAnimating = false }
        DispatchQueue.main.async { [weak self, weak w] in
            guard let self else { return }
            if let w, self.layoutMode == mode, w.frame != target { w.setFrame(target, display: true, animate: false) }
            self.applyingLayoutFrame = false
            self.recordWindowFrame()
            withAnimation(.easeIn(duration: Self.layoutExtrasFadeDuration)) {
                self.layoutExtrasHidden = false
            }
        }
    }

    /// Turn off clipping on every NSSplitView under `root` and on its panes (and
    /// their layers), remembering the previous values for `restoreSplitPaneClipping`.
    private func unclipSplitPanes(in root: NSView?) {
        restoreSplitPaneClipping()
        guard let root else { return }
        func visit(_ v: NSView) {
            if let split = v as? NSSplitView {
                // The split view, its panes and their immediate hosts.
                for x in [split] + split.subviews + split.subviews.flatMap(\.subviews) {
                    unclippedViews.append((x, x.clipsToBounds, x.layer?.masksToBounds ?? false))
                    x.clipsToBounds = false
                    x.layer?.masksToBounds = false
                }
            }
            v.subviews.forEach(visit)
        }
        visit(root)
    }

    private func restoreSplitPaneClipping() {
        for (v, clips, masks) in unclippedViews {
            v.clipsToBounds = clips
            v.layer?.masksToBounds = masks
        }
        unclippedViews = []
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

// MARK: - The four moving blocks (Phase 5)

/// The blocks that exist in BOTH arrangements and move between them.
enum LayoutBlock: String, CaseIterable, Sendable {
    case session, preview, list, export
}

/// Tags one of the four blocks: its window-space frame is reported to the model (the
/// FLIP's "first" rect and the harness checks) and the FLIP transform applied.
private struct LayoutBlockModifier: ViewModifier {
    @Environment(AppModel.self) private var model
    let block: LayoutBlock

    func body(content: Content) -> some View {
        content
            .background(GeometryReader { g in
                let f = g.frame(in: .global)
                Color.clear
                    .onAppear { model.layoutBlockFrames[block.rawValue] = f }
                    .onChange(of: f) { _, new in model.layoutBlockFrames[block.rawValue] = new }
            })
            .modifier(FlipBlock(from: model.layoutFlipFrom[block.rawValue]))
    }
}

/// FLIP (PLAN-COMPACT-MODE Phase 5, mitigation b): while `layoutFlipFrom` holds the
/// block's outgoing frame, the block — already laid out in the INCOMING arrangement —
/// is drawn translated and uniformly scaled back onto that frame (a render-time
/// transform; layout never changes, so nothing re-flows and the AVPlayer layer is
/// not touched). Clearing the entry under an ease-out animation plays it home.
/// Uniform scale (geometric mean of the two axis ratios) so text never stretches.
private struct FlipBlock: ViewModifier {
    let from: CGRect?

    func body(content: Content) -> some View {
        content.visualEffect { c, proxy in
            let to = proxy.frame(in: .global)
            var s: CGFloat = 1, dx: CGFloat = 0, dy: CGFloat = 0
            if let from, to.width > 1, to.height > 1, from.width > 1, from.height > 1 {
                s = min(max(((from.width / to.width) * (from.height / to.height)).squareRoot(), 0.4), 2.5)
                dx = from.minX - to.minX
                dy = from.minY - to.minY
            }
            return c.scaleEffect(s, anchor: .topLeading).offset(x: dx, y: dy)
        }
    }
}

/// Fades a surface unique to one arrangement in after the blocks settle.
private struct LayoutExtraModifier: ViewModifier {
    @Environment(AppModel.self) private var model
    func body(content: Content) -> some View {
        content.opacity(model.layoutExtrasHidden ? 0 : 1)
    }
}

extension View {
    func layoutBlock(_ block: LayoutBlock) -> some View { modifier(LayoutBlockModifier(block: block)) }
    func layoutExtra() -> some View { modifier(LayoutExtraModifier()) }
}
