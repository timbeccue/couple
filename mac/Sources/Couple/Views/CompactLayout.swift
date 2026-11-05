// Compact arrangement (UX §V2.11; PLAN-COMPACT-MODE Phase 4.3): four blocks top to
// bottom — SessionLine (identity, counts, "+ Add"), PairPreview (fixed:
// 16:9 player, PairStrip, Camera · Synced · External, offset · fit, or the inline
// candidate picker / "No audio" line), CoupleList (flex; only the rows scroll) and
// ExportPanel (pinned at the bottom), with the activity strip under it; no footer.
// No loupes, overview, nudges, anchors or media-pool list. Empty session: the drop
// target (LoadSurface) fills the preview area and the export panel stays pinned.

import CoupleKit
import SwiftUI

struct CompactLayout: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 0) {
            SessionLine(compact: true)
                .layoutBlock(.session)
            if model.hasContent {
                PairPreview(compact: true, candidateMaxHeight: 132)
                    .layoutBlock(.preview)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 10)
                    // The preview block is fixed: the list yields height first, so
                    // the 16:9 player never shrinks under the candidate picker.
                    .layoutPriority(1)
                Rectangle().fill(Theme.panelStroke).frame(height: 1)
                CoupleList()
                    .layoutBlock(.list)
                    .padding(.top, 10)
            } else {
                LoadSurface(compact: true)
                    .layoutBlock(.preview)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 10)
                    .frame(maxHeight: .infinity)
                // The list keeps its (empty) Ready section under the drop target.
                CoupleList(scrolls: false)
                    .layoutBlock(.list)
                    .padding(.bottom, 8)
            }
            ExportPanel()
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Theme.panelBg)
                .overlay(alignment: .top) {
                    Rectangle().fill(Theme.panelStroke).frame(height: 1)
                }
                .layoutBlock(.export)
            ActivityStripView()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        // The session line's first row IS the title-bar row (window is
        // .hiddenTitleBar): the arrangement toggle (RootView's corner overlay) sits
        // level with the traffic lights.
        .ignoresSafeArea(.container, edges: .top)
        .background(Theme.windowBg)
    }
}
