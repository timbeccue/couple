// Couple list (UX §V2.11 / §V2.12; PLAN-COMPACT-MODE Phase 4.1): the two workflow
// sections — TO FIX (hidden when empty, D11) above READY (always shown) — and the
// couple rows. One view for BOTH arrangements: the full window's left rail and the
// compact arrangement's flexible middle block. Only the rows scroll; selecting a
// row previews it and never moves the list (stable identities keep the scroll
// offset across redraws). Each row is a drag-to-repair drop target (§V2.6) and the
// include-for-export checkbox lives on includable rows only (D1).

import CoupleKit
import SwiftUI

struct CoupleList: View {
    @Environment(AppModel.self) private var model
    /// Laid-out height of the sections (content) and of the scroll viewport —
    /// reported for the full rail's pool-growth rule (SPEC-CHANGE 2026-07-01).
    var onContentHeight: (CGFloat) -> Void = { _ in }
    var onViewportHeight: (CGFloat) -> Void = { _ in }
    /// False = the sections at their natural height, no scroll region (the
    /// compact empty state, where the drop target takes the flexible space).
    var scrolls = true

    var body: some View {
        if scrolls {
            ScrollView {
                sections.measuredHeight(onContentHeight)
            }
            .frame(maxHeight: .infinity)
            .measuredHeight(onViewportHeight)
        } else {
            sections
        }
    }

    private var sections: some View {
        VStack(alignment: .leading, spacing: 14) {
            // D2/D11: To fix above Ready; an empty To fix is hidden, Ready
            // always shows (dimmed "—" when empty).
            if !model.toFix.isEmpty {
                QueueSection(title: "TO FIX", couples: model.toFix)
            }
            QueueSection(title: "READY", couples: model.ready)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 12)
        .padding(.bottom, 8)
    }
}

// MARK: - Workflow section

private struct QueueSection: View {
    @Environment(AppModel.self) private var model
    let title: String
    let couples: [CoupleRow]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                sectionLabel(title)
                Text("\(couples.count)")
                    .font(.system(size: 9, weight: .bold, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
            }
            if couples.isEmpty {
                Text("—")
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textTertiary)
                    .padding(.vertical, 2)
            } else {
                ForEach(couples) { couple in
                    CoupleRowView(couple: couple)
                }
            }
        }
        .opacity(couples.isEmpty ? 0.45 : 1) // empty sections stay visible, dimmed
    }
}

// MARK: - One couple row

struct CoupleRowView: View {
    @Environment(AppModel.self) private var model
    let couple: CoupleRow

    private var selected: Bool { model.selectedID == couple.id }

    // UX §V2.12 / D3: no verdict dot, spine, wash or coverage strip on rows. The
    // verdict still decides the section and the action word; coverage is drawn in
    // the preview's waveform strip.
    var body: some View {
        HStack(alignment: .center, spacing: 9) {
            VideoThumbnail(url: couple.video.url)
                .frame(width: 54, height: 32)
                .clipShape(RoundedRectangle(cornerRadius: 4))
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(couple.video.name)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1).truncationMode(.middle)
                    if couple.video.durationS > 0 {
                        Text(Fmt.hmmss(couple.video.durationS))
                            .font(.system(size: 9.5, design: .monospaced))
                            .foregroundStyle(Theme.textTertiary)
                            .fixedSize()
                    }
                }
                audioLine
            }
            Spacer(minLength: 4)
            HStack(spacing: 6) {
                if couple.pinned {
                    Text("📌").font(.system(size: 9))
                        .help("Audio chosen by you — a re-pair never replaces it")
                }
                exportedStamp
                // D1: the include checkbox is offered on includable rows only (verdict
                // good); a "pick one" / "no audio" / forced couple shows none.
                if couple.canInclude {
                    includeBox
                }
            }
        }
        .padding(.vertical, 7)
        .padding(.horizontal, 8)
        .background(selected ? Theme.accent.opacity(0.14) : Theme.chipBg.opacity(0.5),
                    in: RoundedRectangle(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7)
            .strokeBorder(selected ? Theme.accent.opacity(0.6) : .clear))
        .contentShape(Rectangle())
        .onTapGesture { model.select(couple.id) }
        // Drag an audio file from the pool onto a couple to re-pair (§V2.6). Routes
        // through the SHARED `requestRepairDrop` (TODO-013) — the same path the
        // center-workspace drop uses — so the two gestures cannot diverge: it
        // no-ops on the current source, replaces immediately when unpinned, and
        // asks before replacing a PINNED couple's manual pick (D9).
        .dropDestination(for: String.self) { items, _ in
            guard let key = items.first else { return false }
            return model.requestRepairDrop(couple, audioKey: key)
        }
    }

    /// The second line (§V2.12): "syncing…" while this couple's auto-sync runs
    /// (replaces the old pulse), else the action-word capsule when action is
    /// needed ("pick one" red tint / "no audio" grey / "forced"), else the chosen
    /// audio's name.
    @ViewBuilder
    private var audioLine: some View {
        if couple.pair.isSyncRunning {
            Text("syncing…")
                .font(.system(size: 10))
                .foregroundStyle(Theme.textTertiary)
        } else if let word = couple.actionWord {
            let grey = couple.verdict == .none
            let tint = Theme.verdictColor(couple.verdict)
            Text(word)
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(grey ? Theme.textSecondary : tint)
                .padding(.horizontal, 5).padding(.vertical, 1)
                .background((grey ? Color.white : tint).opacity(grey ? 0.08 : 0.16),
                            in: RoundedRectangle(cornerRadius: 3))
        } else {
            Text(couple.selectedCandidate?.audioName ?? "—")
                .font(.system(size: 10))
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(1).truncationMode(.middle)
        }
    }

    /// "✓ HH:mm" once exported (D8), read from the persisted `exportedAt` so it
    /// survives relaunch.
    @ViewBuilder
    private var exportedStamp: some View {
        if let t = couple.exportedAt {
            Text("✓ \(Fmt.clock(t))")
                .font(.system(size: 9, design: .monospaced))
                .foregroundStyle(Theme.good.opacity(0.7))
                .fixedSize()
                .help("Exported at \(Fmt.clock(t))")
        }
    }

    /// The include-for-export checkbox (UX §V2.12 / D1): bound to the DERIVED
    /// `included`, toggled through the model (undoable). No key binding (D4).
    private var includeBox: some View {
        Button {
            model.toggleInclude(couple)
        } label: {
            Image(systemName: couple.included ? "checkmark.square.fill" : "square")
                .font(.system(size: 13))
                .foregroundStyle(couple.included ? Theme.good : Theme.textSecondary)
        }
        .buttonStyle(.plain)
        .help(couple.included ? "Included in the export — click to leave it out"
                              : "Not in the export — click to include it")
    }
}

struct VerdictDot: View {
    let verdict: Verdict
    var body: some View {
        Circle()
            .fill(verdict == .none ? Color.clear : Theme.verdictColor(verdict))
            .frame(width: 9, height: 9)
            .overlay(Circle().strokeBorder(Theme.verdictColor(verdict),
                                           lineWidth: verdict == .none ? 1.2 : 0))
    }
}
