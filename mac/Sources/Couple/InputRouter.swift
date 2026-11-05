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

        // ⌘0 (TODO-010): fit the overview back to the full union. Guarded by the
        // text-edit check above so it never fires mid-rename; only meaningful with a
        // session (else pass through).
        if mods.contains(.command), chars == "0" {
            guard pair.session != nil else { return false }
            pair.overviewFit()
            return true
        }

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
        case 49: // space (SPEC-CHANGE 2026-07-02, revising §V2.7 "pool is button-
            // only"): the MAIN audition transport always wins Space when it is
            // available (owner: the two transports must not compete on recency) —
            // play/pause from the visible playhead (TODO-004). Only when no
            // audition exists (nothing synced / no couple open) does Space fall
            // back to the sticky pool player's loaded primary.
            if pair.session != nil {
                // Starting the audition pauses a still-playing pool preview, so
                // the keyboard path never sounds two transports at once.
                if model.preview.state.isPlaying, let active = model.preview.state.activeKey,
                   let pooled = model.pool.entry(key: active) {
                    model.preview.toggle(key: pooled.id, url: pooled.url)
                }
                model.audition.toggle()
                return true
            }
            if let entry = model.poolPrimaryEntry {
                model.preview.toggle(key: entry.id, url: entry.url)
            }
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
        // `v` is UNBOUND (UX §V2.12 / D4): inclusion is the row checkbox only.
        case "l": // loupes (§V2.4)
            model.loupesShown.toggle()
            return true
        case "a": // A/B: camera ⇄ external solo (UX-DECISIONS 2026-11-03)
            guard pair.session != nil else { return false }
            pair.audition.flipMonitor()
            return true
        case "b": // back to hearing both (the pan/overlay mode used last)
            guard pair.session != nil else { return false }
            pair.audition.restoreBothMonitor()
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

    // MARK: - Scroll-wheel zoom routing (editor loupes; UX §2.9)

    private func handleScroll(_ event: NSEvent) -> Bool {
        guard let window = event.window, let contentView = window.contentView else { return false }
        let p = contentView.convert(event.locationInWindow, from: nil)
        let point = contentView.isFlipped ? p : CGPoint(x: p.x, y: contentView.bounds.height - p.y)
        let precise = event.hasPreciseScrollingDeltas
        // Route by pointer location (UX §2.9): over the overview strip, scroll zooms/pans
        // the OVERVIEW; over a loupe it zooms that loupe — the two never cross (TODO-010).
        if model.overviewRect.contains(point) {
            return scrollOverview(at: point, deltaX: event.scrollingDeltaX,
                                  deltaY: event.scrollingDeltaY, precise: precise)
        }
        return zoom(at: point, deltaY: event.scrollingDeltaY, precise: precise)
    }

    /// Overview scroll (TODO-010): vertical scroll zooms about the pointer with the
    /// SAME exp() sensitivity and direction as loupe zoom; horizontal scroll pans. Pure
    /// presentation — allowed whenever a session exists (independent of `lockZoom` and
    /// of the sync-disabled `inputsEnabled` gate, which only guards alignment edits).
    @discardableResult
    func scrollOverview(at point: CGPoint, deltaX: CGFloat, deltaY: CGFloat, precise: Bool) -> Bool {
        let pair = model.pair
        let rect = model.overviewRect
        guard pair.session != nil, rect.width > 0 else { return false }
        let visible = pair.overviewVisible
        let span = visible.upperBound - visible.lowerBound
        if abs(deltaX) > abs(deltaY) {
            // Content follows the fingers (same sign convention as the coarse-offset
            // and loupe drags): a rightward scroll reveals earlier time.
            pair.overviewPan(by: -Double(deltaX / rect.width) * span)
        } else if deltaY != 0 {
            let factor = exp(deltaY * (precise ? 0.004 : 0.06))
            let t = OverviewTimeline.time(atX: point.x - rect.minX, width: rect.width, visible: visible)
            pair.overviewZoom(factor: 1 / factor, around: t) // 1/factor mirrors loupe manualZoom
        }
        return true
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
