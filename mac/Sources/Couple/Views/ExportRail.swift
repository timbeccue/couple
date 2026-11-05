// Export rail (right column, UX §V2.12 / §V2.8 / §6) = the shared ExportPanel
// (INTO / AS / keep camera audio / Export N files — also the compact
// arrangement's pinned bottom block) followed by the Files list. FILES:
// included ∪ already exported (rule in core `export_include`), one
// `<stem>_synced.<ext>` per couple, stem editable in place with ↺ reset, per-file
// status; an exported row shows its ✓ time from `exportedAt` (D8). Each
// per-couple job is the unchanged Phase-4 pipeline, so a batch is byte-identical
// to the CLI export, couple by couple. The rail's bottom-right corner carries the
// "Keyboard shortcuts" button (ShortcutsButton, UX §V2.13) — inside the rail
// rather than on a footer row of its own, so no window height goes to it.

import AppKit
import CoupleKit
import SwiftUI

struct ExportRail: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ExportPanel()
                .layoutBlock(.export)

            // FILES — included ∪ exported (D1/D8)
            if !model.filesList.isEmpty {
                sectionLabel("FILES · \(model.filesList.count)")
                    .padding(.top, 6)
                    .layoutExtra()
                ScrollView {
                    VStack(alignment: .leading, spacing: 5) {
                        ForEach(model.filesList) { couple in
                            ExportRowView(couple: couple)
                        }
                    }
                }
                .layoutExtra()
            }
            Spacer(minLength: 0)
            HStack {
                Spacer(minLength: 0)
                ShortcutsButton()
            }
            .layoutExtra()
        }
        .padding(12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Theme.panelBg)
    }
}

private struct ExportRowView: View {
    @Environment(AppModel.self) private var model
    let couple: CoupleRow

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                statusIcon
                // The filename IS the row, edited in place by clicking it (§V2.8).
                EditableFilename(
                    stem: couple.outputStem,
                    ext: couple.ext(format: model.globalFormat),
                    isCustom: couple.stemOverride != nil,
                    onBeginEdit: { model.select(couple.id) },
                    onCommit: { newStem in
                        let trimmed = newStem.trimmingCharacters(in: .whitespacesAndNewlines)
                        // Empty edit = no-op: keep the last saved name (custom or
                        // default) rather than silently resetting to the original.
                        guard !trimmed.isEmpty else { return }
                        // Through the model so the edit is undoable (⌘Z).
                        model.setStemOverride((trimmed == couple.defaultStem) ? nil : trimmed,
                                              for: couple)
                    },
                    onReset: { model.setStemOverride(nil, for: couple) })
                Spacer(minLength: 0)
                exportedStamp
            }
            if case .failed(let msg) = couple.exportStatus {
                Text(msg).font(.system(size: 8.5)).foregroundStyle(Theme.bad).lineLimit(2)
            }
        }
        .padding(.vertical, 4).padding(.horizontal, 6)
        .background(model.selectedID == couple.id ? Theme.accent.opacity(0.1) : Theme.chipBg.opacity(0.4),
                    in: RoundedRectangle(cornerRadius: 5))
        .contentShape(Rectangle())
        .onTapGesture { model.select(couple.id) }
        .help(couple.video.name)
    }

    /// "✓ HH:mm" once the couple has been exported (D8) — read from the persisted
    /// `exportedAt`, so it survives relaunch unlike the live batch's `exportStatus`.
    @ViewBuilder
    private var exportedStamp: some View {
        if let t = couple.exportedAt {
            Text("✓ \(Fmt.clock(t))")
                .font(.system(size: 9, design: .monospaced))
                .foregroundStyle(Theme.good.opacity(0.7))
                .help("Exported at \(Fmt.clock(t))")
        }
    }

    // Width reserved for the status column so the filename baseline never shifts
    // as a row moves between idle/queued (empty), running, done, and failed.
    private static let statusColumnWidth: CGFloat = 12

    // TODO-008: idle and pre-run queued rows carry no status info, so they render
    // nothing — but the column keeps its width so filenames stay aligned when a
    // progress spinner, success check, or failure icon later appears.
    @ViewBuilder
    private var statusIcon: some View {
        Group {
            switch couple.exportStatus {
            case .idle, .queued:
                Color.clear
            case .running(let f):
                ProgressView(value: f).controlSize(.mini)
            case .done:
                Image(systemName: "checkmark.circle.fill").font(.system(size: 9)).foregroundStyle(Theme.good)
            case .failed:
                Image(systemName: "exclamationmark.circle.fill").font(.system(size: 9)).foregroundStyle(Theme.bad)
            }
        }
        .frame(width: Self.statusColumnWidth)
    }
}

/// A Finder-style inline rename: the filename shows as plain text (extension in
/// the same font, directly adjacent — it reads as one name); clicking it turns
/// the stem into an editor with the fixed extension riding alongside. The editor
/// exists ONLY while editing, so the moment it ends there is no field to capture
/// keystrokes and the global shortcuts resume. It ends on:
///  - **Enter** (commit) / **esc** (cancel, revert) — via onSubmit / onExitCommand;
///  - **focus loss** (commit) — onChange(focused);
///  - **a click anywhere outside the field** (commit) — a SwiftUI TextField does
///    not resign on clicks to non-focusable areas, so a local mouse-down monitor
///    commits when the hit-tested view isn't the field's editor (Finder behavior).
private struct EditableFilename: View {
    let stem: String
    let ext: String
    let isCustom: Bool
    let onBeginEdit: () -> Void
    let onCommit: (String) -> Void
    let onReset: () -> Void

    @State private var editing = false
    @State private var draft = ""
    @State private var clickMonitor: Any?
    @FocusState private var focused: Bool

    // Same font as the filename, so the extension reads as part of the name.
    private let nameFont = Font.system(size: 10, weight: .medium, design: .monospaced)

    var body: some View {
        HStack(spacing: 0) {
            if editing {
                TextField("", text: $draft)
                    .textFieldStyle(.plain)
                    .font(nameFont)
                    .foregroundStyle(Theme.textPrimary)
                    .fixedSize() // hug the typed text so the extension sits right after
                    .focused($focused)
                    .onSubmit { commit() }
                    .onExitCommand { cancel() }
                    .onChange(of: focused) { _, isFocused in if !isFocused { commit() } }
                    .onDisappear { removeMonitor() }
                Text(".\(ext)")
                    .font(nameFont)
                    .foregroundStyle(Theme.textSecondary)
            } else {
                (Text(stem).foregroundStyle(Theme.textPrimary)
                    + Text(".\(ext)").foregroundStyle(Theme.textSecondary))
                    .font(nameFont)
                    .lineLimit(1).truncationMode(.middle)
                    .contentShape(Rectangle())
                    .onTapGesture { begin() }
                    .help("Click to rename")
                if isCustom {
                    Button { onReset() } label: {
                        Image(systemName: "arrow.uturn.backward").font(.system(size: 8))
                    }
                    .buttonStyle(.plain).foregroundStyle(Theme.textTertiary)
                    .padding(.leading, 5)
                    .help("Reset to default name")
                }
            }
        }
    }

    private func begin() {
        draft = stem
        onBeginEdit()
        editing = true
        focused = true
        installMonitor()
    }

    /// Apply the edit. Ending editing removes the field (releasing first
    /// responder), so the global shortcuts resume — no explicit resign needed,
    /// which also avoids stealing focus when the commit was a click into another
    /// field.
    private func commit() {
        guard editing else { return } // onSubmit / blur / click-outside can overlap
        endEditing()
        onCommit(draft)
    }

    /// Discard the edit (esc) — revert to the prior name.
    private func cancel() {
        guard editing else { return }
        endEditing()
    }

    private func endEditing() {
        editing = false
        focused = false
        removeMonitor()
    }

    private func installMonitor() {
        removeMonitor()
        clickMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { event in
            guard let win = event.window else { return event }
            // Commit when the click lands outside the field's editor (the field
            // editor is an NSTextView; anything else = "outside").
            if !(win.contentView?.hitTest(event.locationInWindow) is NSTextView) {
                DispatchQueue.main.async { commit() }
            }
            return event // never consume — the click still does its job
        }
    }

    private func removeMonitor() {
        if let m = clickMonitor { NSEvent.removeMonitor(m); clickMonitor = nil }
    }
}
