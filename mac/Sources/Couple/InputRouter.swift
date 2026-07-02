// NSEvent monitors + key/scroll routing (UX §2.9 + j/k/v/Enter queue triage),
// extracted from AppModel. Installs the local key + scroll monitors and maps events to
// the model's verbs through the back-reference. Purely a router: every decision it makes
// (nudge, selection move, approve+advance, zoom target) delegates to AppModel state that
// stays the source of truth.

import AppKit
import CoupleKit
import Observation

@MainActor
@Observable
final class InputRouter {
    @ObservationIgnored private unowned let model: AppModel
    @ObservationIgnored private var keyMonitor: Any?
    @ObservationIgnored private var scrollMonitor: Any?

    init(model: AppModel) {
        self.model = model
    }

    // MARK: - Keyboard (UX §2.9 + j/k/v/Enter queue triage)

    func installMonitors() {
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
        let pair = model.pair
        // Esc: while editing a text field (e.g. an inline filename rename), let
        // it through so the field can cancel/revert. Otherwise cancel a running
        // sync, else collapse the bottom-bar tools / dismiss loupes.
        if event.keyCode == 53 {
            if isEditingText { return false }
            if pair.isSyncRunning { pair.cancelSync(); return true }
            if model.loupesShown { model.loupesShown = false; return true }
            if model.syncToolsExpanded { model.syncToolsExpanded = false; return true }
            return false
        }
        guard !isEditingText else { return false }
        let mods = event.modifierFlags
        let chars = event.charactersIgnoringModifiers ?? ""

        // Delete / forward-delete: remove the selected media from the open session
        // (TODO-001) — only when the media pool has a selection, so Delete never fights
        // another context (and never deletes source files). Routes through the SAME
        // `removeSelectedMedia` the trash button uses (unused ⇒ immediate; referenced ⇒
        // one confirmation). Consumed only when it acts.
        if (event.keyCode == 51 || event.keyCode == 117), !model.poolController.poolSelection.isEmpty {
            model.poolController.removeSelectedMedia()
            return true
        }

        switch event.keyCode {
        case 123, 124: // ← → — move the EXTERNAL content in the arrow's direction
            guard pair.inputsEnabled else { return true }
            var step = 1.0
            if mods.contains(.shift) { step = 10 }
            if mods.contains(.option) { step = 0.1 }
            pair.nudgeOffset(event.keyCode == 123 ? step : -step)
            return true
        case 38: // j — next couple
            moveSelection(1)
            return true
        case 40: // k — previous couple
            moveSelection(-1)
            return true
        case 36, 76: // Return / keypad Enter — open/close the editor (sync tools)
            guard model.selectedCouple != nil else { return true }
            model.syncToolsExpanded.toggle()
            return true
        case 49: // space: play/pause audition from the visible playhead (TODO-004)
            guard pair.session != nil else { return true }
            model.audition.toggle()
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
        case "v": // approve + advance to next unapproved (§V2.5)
            approveAndAdvance()
            return true
        case "l": // loupes (§V2.4)
            model.loupesShown.toggle()
            return true
        default:
            return false
        }
    }

    private func moveSelection(_ delta: Int) {
        let order = model.orderedCouples
        guard !order.isEmpty else { return }
        guard let cur = order.firstIndex(where: { $0.id == model.selectedID }) else {
            model.select(order.first?.id); return
        }
        let next = max(0, min(order.count - 1, cur + delta))
        model.select(order[next].id)
    }

    private func approveAndAdvance() {
        guard let couple = model.selectedCouple else { return }
        model.toggleApprove(couple)
        // Advance to the next couple still needing approval (TO APPROVE / TO FIX).
        let pendingAfter = (model.toApprove + model.toFix)
        if let next = pendingAfter.first(where: { $0.id != couple.id }) {
            model.select(next.id)
        }
    }

    // MARK: - Scroll-wheel zoom routing (editor loupes; UX §2.9)

    private func handleScroll(_ event: NSEvent) -> Bool {
        guard let window = event.window, let contentView = window.contentView else { return false }
        let p = contentView.convert(event.locationInWindow, from: nil)
        let point = contentView.isFlipped ? p : CGPoint(x: p.x, y: contentView.bounds.height - p.y)
        return zoom(at: point, deltaY: event.scrollingDeltaY, precise: event.hasPreciseScrollingDeltas)
    }

    @discardableResult
    func zoom(at point: CGPoint, deltaY: CGFloat, precise: Bool) -> Bool {
        let pair = model.pair
        guard pair.inputsEnabled,
              let (index, _) = model.loupeRects.first(where: { $0.value.contains(point) }) else {
            return false
        }
        guard deltaY != 0 else { return true }
        let factor = exp(deltaY * (precise ? 0.004 : 0.06))
        pair.manualZoom(loupe: index, factor: 1 / factor)
        return true
    }
}
