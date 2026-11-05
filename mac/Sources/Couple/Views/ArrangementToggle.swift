// Arrangement toggle (UX §V2.13; revises §V2.11): the ONE control that switches the
// window between its compact and full arrangements. It is window chrome, not a block:
// RootView overlays it at the window's top-right corner in BOTH arrangements, level
// with the traffic lights, so it sits where macOS keeps its window-shape controls
// and never moves when the arrangement flips. A 26 × 26 icon button with a hover
// background (the toolbar-button idiom); the icon names the arrangement it opens
// (three columns ⇒ full, a tall rectangle ⇒ compact). Same action as View ▸
// Compact Window / Full Window (⌘⇧C). Replaces the 2026-11-05 "⤢ Full / ⤡ Compact"
// text label the session line carried.

import SwiftUI

struct ArrangementToggle: View {
    @Environment(AppModel.self) private var model
    @State private var hovered = false

    /// Hit area (and the title-bar strip it is centered in).
    static let size: CGFloat = 26

    var body: some View {
        let compact = model.layoutMode == .compact
        Button {
            model.toggleLayoutMode()
        } label: {
            Image(systemName: compact ? "rectangle.split.3x1" : "rectangle.portrait")
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(hovered ? Theme.textPrimary : Theme.textSecondary)
                .frame(width: Self.size, height: Self.size)
                .contentShape(Rectangle())
        }
        .buttonStyle(CornerIconButtonStyle(hovered: hovered))
        .onHover { hovered = $0 }
        .help(compact ? "Full Window — overview, loupes, sync tools, media pool (⌘⇧C)"
                      : "Compact Window (⌘⇧C)")
        .accessibilityLabel(compact ? "Full Window" : "Compact Window")
    }
}

/// Toolbar-button look for a bare icon: no chrome at rest, a chip background while
/// hovered, a brighter one while pressed. Hover is tracked by the caller (a
/// ButtonStyle has no hover state of its own).
private struct CornerIconButtonStyle: ButtonStyle {
    var hovered: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .fill(configuration.isPressed ? Color.white.opacity(0.14)
                          : hovered ? Theme.chipBg : Color.clear)
            )
    }
}
