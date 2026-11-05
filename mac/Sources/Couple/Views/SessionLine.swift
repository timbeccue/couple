// Session line (UX §V2.11; PLAN-COMPACT-MODE Phase 4.1): the session's identity and
// media counts and — per arrangement — the session controls. One view for BOTH
// arrangements:
//  - full (the left rail's header): the wordmark, Clear Session…, the media counts
//    and "N to export";
//  - compact: an empty title-bar row (the window is `.hiddenTitleBar`; the traffic
//    lights sit at its left and the arrangement toggle — RootView's corner overlay,
//    ArrangementToggle.swift — at its right), then "<session> · N video · N audio"
//    with "+ Add" — the ONLY media-pool affordance in compact (§6.4). Clear
//    Session… is in the session line's context menu there.

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
                CoupleWordmark(height: 15)
                Spacer()
                // Clear Session (TODO-001): a clearly-visible session control that
                // returns to the initial import screen. Confirms with media/couple/
                // included counts; disabled mid-export (the batch owns the files).
                clearSessionButton
            }
            Text("\(model.pool.videos.count) video · \(model.pool.audios.count) audio")
                .font(.system(size: 10))
                .foregroundStyle(Theme.textSecondary)
            Text("\(model.includedCount) to export")
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(Theme.textTertiary)
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
        }
        .buttonStyle(.plain)
        .disabled(model.batch.isRunning || !model.hasContent)
        .help("Discard this session and start over (does not delete source or exported files)")
    }

    // MARK: Compact arrangement

    private var compactBody: some View {
        VStack(alignment: .leading, spacing: 4) {
            // Title-bar row: traffic lights at the left (system), the arrangement
            // toggle at the right (RootView's corner overlay). Nothing of its own.
            Color.clear.frame(height: 28)
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                if let name = model.sessionName {
                    Text(name)
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1).truncationMode(.middle)
                    Text("·").foregroundStyle(Theme.textTertiary)
                }
                Text("\(model.pool.videos.count) video · \(model.pool.audios.count) audio")
                    .font(.system(size: 10.5))
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(1)
                    .layoutPriority(1)
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
