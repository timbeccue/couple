// Session line (UX §V2.11; PLAN-COMPACT-MODE Phase 4.1): the session's identity and
// media counts, the arrangement toggle and — per arrangement — the session controls.
// One view for BOTH arrangements:
//  - full (the left rail's header): the wordmark, Clear Session…, "⤡ Compact",
//    the media counts and "N to export". In an EMPTY session only the wordmark
//    and the toggle show (sidebar cleanup, UX §V2.13): no zero counts, no
//    Clear Session… — the import surface is already the start screen;
//  - compact: a title-bar row carrying "⤢ Full" at the right (the window is
//    `.hiddenTitleBar`; the traffic lights sit at the left of that row), then
//    "<session> · N video · N audio" with "+ Add" — the ONLY media-pool affordance
//    in compact (§6.4). Clear Session… is in the session line's context menu there.

import CoupleKit
import SwiftUI

struct SessionLine: View {
    @Environment(AppModel.self) private var model
    var compact = false
    @State private var importerShown = false

    var body: some View {
        if compact { compactBody } else { fullBody }
    }

    // MARK: Full arrangement (left-rail header)

    private var fullBody: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                // Brand wordmark: the C and O are the interlocked mark, UPLE the
                // outlined glyphs — one lockup identical to design/logo/couple-wordmark.svg.
                // 22 pt (was 15; owner 2026-11-05: larger). At the rail's minimum
                // width this leaves room for ONE control beside it, so Clear
                // Session… moved down to the counts row (it wrapped here).
                CoupleWordmark(height: 22)
                Spacer(minLength: 8)
                ArrangementToggle()
            }
            // Counts only once there is media (UX §V2.13): an empty rail reads as
            // quiet, not as a list of zeros. Clear Session… (TODO-001) sits at the
            // right of this row: a clearly-visible session control that returns to
            // the import screen; confirms with media/couple/included counts;
            // disabled mid-export (the batch owns the files); hidden with the row
            // while the session is empty (nothing to clear).
            if model.hasContent {
                HStack(alignment: .firstTextBaseline) {
                    Text("\(model.pool.videos.count) video · \(model.pool.audios.count) audio")
                        .font(.system(size: 10))
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)
                    Spacer(minLength: 8)
                    clearSessionButton
                }
                Text("\(model.includedCount) to export")
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 26) // traffic-light clearance
        .padding(.bottom, 12)
    }

    private var clearSessionButton: some View {
        Button {
            model.requestClearSession()
        } label: {
            Text("Clear Session…")
                .font(.system(size: 10))
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(1)
                .fixedSize()
        }
        .buttonStyle(.plain)
        .disabled(model.batch.isRunning || !model.hasContent)
        .help("Discard this session and start over (does not delete source or exported files)")
    }

    // MARK: Compact arrangement

    private var compactBody: some View {
        VStack(alignment: .leading, spacing: 4) {
            // Title-bar row: traffic lights at the left (system), the toggle at the right.
            HStack {
                Spacer()
                ArrangementToggle()
            }
            .frame(height: 28)
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                if let name = model.sessionName {
                    Text(name)
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1).truncationMode(.middle)
                    Text("·").foregroundStyle(Theme.textTertiary)
                }
                // Counts only once there is media (UX §V2.13); "+ Add" stays — it is
                // the only pool affordance in compact (§6.4).
                if model.hasContent {
                    Text("\(model.pool.videos.count) video · \(model.pool.audios.count) audio")
                        .font(.system(size: 10.5))
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)
                        .layoutPriority(1)
                }
                Spacer(minLength: 6)
                Button { importerShown = true } label: {
                    Text("+ Add")
                        .font(.system(size: 11))
                        .foregroundStyle(Theme.textSecondary)
                }
                .buttonStyle(.plain)
                .help("Add videos, audio files or a folder to this session")
                .fileImporter(isPresented: $importerShown,
                              allowedContentTypes: [.movie, .audio, .audiovisualContent, .folder],
                              allowsMultipleSelection: true) { result in
                    if case .success(let urls) = result { model.importDropped(urls) }
                }
            }
            .contentShape(Rectangle())
            .contextMenu {
                Button("Clear Session…") { model.requestClearSession() }
                    .disabled(model.batch.isRunning || !model.hasContent)
            }
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 8)
    }
}

/// "⤢ Full" in compact / "⤡ Compact" in full (UX §V2.11). Same action as the View
/// menu's Compact Window / Full Window item (⌘⇧C).
struct ArrangementToggle: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let compact = model.layoutMode == .compact
        Button {
            model.toggleLayoutMode()
        } label: {
            Text(compact ? "⤢ Full" : "⤡ Compact")
                .font(.system(size: 10.5))
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(1)
                .fixedSize()
        }
        .buttonStyle(.plain)
        .help(compact ? "Open the full window: overview, loupes, sync tools, media pool (⌘⇧C)"
                      : "Switch to the compact window (⌘⇧C)")
    }
}
