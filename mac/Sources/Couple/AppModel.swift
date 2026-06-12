// AppModel: engine lifecycle, the (single, v2-list-ready) pair, global keyboard
// map (UX §2.9), and scroll-wheel routing to loupes.

import AppKit
import CoupleKit
import Observation
import SwiftUI

@MainActor
@Observable
final class AppModel {
    let engine: Engine?
    let engineError: String?
    let toasts = ToastCenter()
    var pair: PairSession
    /// "1 of N": pairs opened this app session (resets each launch, §14 item 6.11).
    var pairsOpened = 1
    var confirmingNewPair = false

    /// Loupe canvas frames in window coordinates, for scroll-wheel routing.
    @ObservationIgnored var loupeRects: [Int: CGRect] = [:]
    @ObservationIgnored private var keyMonitor: Any?
    @ObservationIgnored private var scrollMonitor: Any?

    init() {
        var engine: Engine?
        var engineError: String?
        do {
            engine = try Engine()
        } catch {
            engineError = (error as? CoreError)?.message ?? error.localizedDescription
        }
        self.engine = engine
        self.engineError = engineError
        guard let engine else {
            fatalError("couple-core context failed: \(engineError ?? "?")")
        }
        self.pair = PairSession(engine: engine, toasts: toasts)
        installMonitors()
        // Autosave on clean quit (§14 item 6.8). willTerminate fires on main.
        NotificationCenter.default.addObserver(
            forName: NSApplication.willTerminateNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.pair.autosave()
            }
        }
        DebugDrive.shared.configure(model: self)
    }

    // MARK: - Pair lifecycle ("+ add pair", UX §1.3/§2.6)

    func requestNewPair() {
        if pair.session != nil || pair.bothLoaded {
            confirmingNewPair = true // unsaved-changes confirmation
        } else {
            startNewPair()
        }
    }

    func startNewPair() {
        confirmingNewPair = false
        guard let engine else { return }
        pair.autosave()
        pair.teardown()
        loupeRects = [:]
        pair = PairSession(engine: engine, toasts: toasts)
        pairsOpened += 1
    }

    // MARK: - Keyboard (UX §2.9 verbatim; j/k/Enter reserved no-ops)

    private func installMonitors() {
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self else { return event }
            return self.handleKey(event) ? nil : event
        }
        scrollMonitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self] event in
            guard let self else { return event }
            return self.handleScroll(event) ? nil : event
        }
    }

    private var isEditingText: Bool {
        NSApp.keyWindow?.firstResponder is NSTextView
    }

    private func handleKey(_ event: NSEvent) -> Bool {
        // Esc cancels a running auto-sync even from a text field.
        if event.keyCode == 53, pair.isSyncRunning {
            pair.cancelSync()
            return true
        }
        guard !isEditingText else { return false }
        let mods = event.modifierFlags
        let chars = event.charactersIgnoringModifiers ?? ""

        switch event.keyCode {
        case 123, 124: // ← → — arrows move the EXTERNAL content in the arrow's
            // direction (owner 2026-06-12; +offset shifts external left, so → = −offset)
            guard pair.inputsEnabled else { return true }
            var step = 1.0
            if mods.contains(.shift) { step = 10 }
            if mods.contains(.option) { step = 0.1 }
            pair.nudgeOffset(event.keyCode == 123 ? step : -step)
            return true
        case 36, 76: // Return / keypad Enter — reserved for v2 queue triage
            return true
        case 49: // space: play/pause audition (UX §2.9)
            guard pair.session != nil else { return true }
            let start = max(0, (pair.anchors.first ?? 0.8) - 0.8)
            pair.audition.toggle(defaultStart: start)
            return true
        default:
            break
        }

        switch chars {
        case "[", "]":
            guard pair.inputsEnabled else { return true }
            let step = mods.contains(.option) ? 0.1 : 1.0
            pair.nudgeDrift(chars == "[" ? -step : step)
            return true
        case "v":
            pair.toggleVerified()
            return true
        case "j", "k": // reserved for v2 queue triage
            return true
        default:
            return false
        }
    }

    // MARK: - Scroll-wheel zoom routing (UX §2.9: scroll on loupe zooms)

    private func handleScroll(_ event: NSEvent) -> Bool {
        guard let window = event.window, let contentView = window.contentView else { return false }
        let p = contentView.convert(event.locationInWindow, from: nil)
        let point = contentView.isFlipped ? p : CGPoint(x: p.x, y: contentView.bounds.height - p.y)
        return zoom(at: point, deltaY: event.scrollingDeltaY,
                    precise: event.hasPreciseScrollingDeltas)
    }

    /// Zoom routing, separated from NSEvent for testability: scroll up = zoom in.
    @discardableResult
    func zoom(at point: CGPoint, deltaY: CGFloat, precise: Bool) -> Bool {
        guard pair.inputsEnabled,
              let (index, _) = loupeRects.first(where: { $0.value.contains(point) }) else {
            return false
        }
        guard deltaY != 0 else { return true }
        let factor = exp(deltaY * (precise ? 0.004 : 0.06))
        pair.manualZoom(loupe: index, factor: 1 / factor)
        return true
    }
}
