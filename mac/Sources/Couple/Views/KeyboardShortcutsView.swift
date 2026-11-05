// Keyboard-shortcut reference (UX §V2.13, 2026-11-05; replaces the footer cheat
// strip, which was too small to read): a readable key / action list grouped by
// surface. Opens from the export rail's "Keyboard shortcuts" button
// (ShortcutsButton below, a popover), Help ▸ Keyboard Shortcuts (⌘/) or the `?`
// key; Esc closes it. Presentation
// only — every binding listed here is routed in InputRouter.swift (keys),
// LoupeView.swift (drags) or CoupleApp.swift (menu); keep them in step. In the
// compact arrangement only the rows that apply there are listed (UX §V2.11:
// nudges, drift, loupes and the media pool have no surface in compact).

import SwiftUI

/// The "Keyboard shortcuts" button at the bottom-right of the export rail (full
/// arrangement) with the reference as its popover. It took a footer row of its own
/// at first; the owner moved it into the rail so no window height goes to it.
struct ShortcutsButton: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        Button {
            model.shortcutsShown.toggle()
        } label: {
            Label("Keyboard shortcuts", systemImage: "keyboard")
                .font(.system(size: 11))
                .foregroundStyle(Theme.textSecondary)
        }
        .buttonStyle(.plain)
        .help("Show the keyboard shortcuts (? or ⌘/)")
        .popover(isPresented: $model.shortcutsShown, arrowEdge: .bottom) {
            KeyboardShortcutsView()
        }
    }
}

struct KeyboardShortcutsView: View {
    @Environment(AppModel.self) private var model
    /// A sheet (compact) needs its own Close; the popover closes on click-away / Esc.
    var showsClose = false

    private struct Row {
        let keys: String
        let action: String
        /// What the key does in compact: nil = the same as `action`.
        var compactAction: String? = nil
        /// False = not listed in compact (no surface for it there).
        var inCompact = true

        func text(compact: Bool) -> String? {
            guard !compact || inCompact else { return nil }
            return compact ? (compactAction ?? action) : action
        }
    }

    private struct Group {
        let title: String
        let rows: [Row]
    }

    private static let groups: [Group] = [
        Group(title: "SESSION", rows: [
            Row(keys: "Space", action: "Play / pause"),
            Row(keys: "J  K", action: "Next / previous couple"),
            Row(keys: "⏎", action: "Open / close the sync tools",
                compactAction: "Open the full window on this couple"),
            Row(keys: "Esc", action: "Cancel a sync; close loupes or tools",
                compactAction: "Cancel a running sync"),
            Row(keys: "⌘Z  ⇧⌘Z", action: "Undo / redo"),
            Row(keys: "⌘⇧C", action: "Compact window / full window"),
            Row(keys: "?  ⌘/", action: "This list"),
        ]),
        Group(title: "MONITOR", rows: [
            Row(keys: "A", action: "Camera ⇄ external (solo)"),
            Row(keys: "B", action: "Hear both again"),
        ]),
        Group(title: "SYNC TOOLS", rows: [
            Row(keys: "←  →", action: "Offset by 1 ms", inCompact: false),
            Row(keys: "⇧ ←  →", action: "Offset by 10 ms", inCompact: false),
            Row(keys: "⌥ ←  →", action: "Offset by 0.1 ms", inCompact: false),
            Row(keys: "[  ]", action: "Drift one step (⌥ a tenth)", inCompact: false),
            Row(keys: "L", action: "Show / hide the loupes", inCompact: false),
            Row(keys: "⌘0", action: "Fit the overview", inCompact: false),
            Row(keys: "Drag a loupe", action: "Slide the offset", inCompact: false),
            Row(keys: "⇧-drag a loupe", action: "Pivot the drift", inCompact: false),
        ]),
        Group(title: "MEDIA POOL", rows: [
            Row(keys: "Click  ⌘  ⇧", action: "Select / add / range", inCompact: false),
            Row(keys: "⌫", action: "Remove the selected files from the session", inCompact: false),
        ]),
    ]

    var body: some View {
        let compact = model.layoutMode == .compact
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                Text("Keyboard shortcuts")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                Spacer()
                if showsClose {
                    Button("Close") { model.shortcutsShown = false }
                        .keyboardShortcut(.cancelAction)
                }
            }
            ForEach(Self.groups, id: \.title) { group in
                let rows = group.rows.compactMap { row in row.text(compact: compact).map { (row.keys, $0) } }
                if !rows.isEmpty {
                    VStack(alignment: .leading, spacing: 5) {
                        sectionLabel(group.title)
                        Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 4) {
                            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                                GridRow {
                                    Text(row.0)
                                        .font(.system(size: 11.5, weight: .medium, design: .monospaced))
                                        .foregroundStyle(Theme.textSecondary)
                                        .frame(minWidth: 96, alignment: .leading)
                                    Text(row.1)
                                        .font(.system(size: 12))
                                        .foregroundStyle(Theme.textPrimary)
                                }
                            }
                        }
                    }
                }
            }
        }
        .padding(18)
        .frame(width: 400, alignment: .leading)
    }
}
