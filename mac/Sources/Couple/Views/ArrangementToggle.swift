// Arrangement toggle (UX §V2.13; revises §V2.11): the ONE control that switches the
// window between its compact and full arrangements. It is window chrome, not a block:
// RootView overlays it at the window's top-right corner in BOTH arrangements, level
// with the traffic lights, so it sits where macOS keeps its window-shape controls
// and never moves when the arrangement flips. A small text button — "Full" while
// compact, "Compact" while full (it names the arrangement it opens) — 24 pt tall
// with a hover background (the toolbar-button idiom); the owner found an icon
// alone unclear. Same action as View ▸ Compact Window / Full Window (⌘⇧C).
// Replaces the 2026-11-05 plain "⤢ Full / ⤡ Compact" label the session line carried.

import SwiftUI

struct ArrangementToggle: View {
    @Environment(AppModel.self) private var model
    @State private var hovered = false

    /// Height of the hit area (the title-bar strip it is centered in).
    static let height: CGFloat = 24

    var body: some View {
        let compact = model.layoutMode == .compact
        Button {
            model.toggleLayoutMode()
        } label: {
            Text(compact ? "Full" : "Compact")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(hovered ? Theme.textPrimary : Theme.textSecondary)
                .padding(.horizontal, 8)
                .frame(height: Self.height)
                .contentShape(Rectangle())
        }
        .buttonStyle(CornerIconButtonStyle(hovered: hovered))
        .onHover { hovered = $0 }
        .help(compact ? "Full Window — overview, loupes, sync tools, media pool (⌘⇧C)"
                      : "Compact Window (⌘⇧C)")
        .accessibilityLabel(compact ? "Full Window" : "Compact Window")
    }
}

/// Toolbar-button look for a bare label: no chrome at rest, a chip background while
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
