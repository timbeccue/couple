// Queue rail (left column, §V2.1/§V2.2/§V2.7): the session header, the couple
// list in three live workflow sections (QUEUED FOR EXPORT / TO APPROVE / TO FIX,
// not a sort control — couples migrate between them as they're approved/repaired),
// and the media pool generalizing the v1 unused tray (every imported file with a
// usage indicator + audio preview + drag-to-re-pair). Nothing the user dropped
// ever silently disappears.

import AVFoundation
import CoupleKit
import SwiftUI

struct QueueRail: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            // Two independent scroll regions (redesign 2026-06-15): the couples take
            // the GREEDY top region (their own bounded ScrollView), and the media
            // pool is a bottom-pinned box that owns its own inner scroll + a sticky
            // footer player. The old shared-scroll Spacer/minHeight trick is gone, so
            // the sticky bar stays put while either list scrolls.
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    QueueSection(title: "QUEUED FOR EXPORT", couples: model.queued)
                    QueueSection(title: "TO APPROVE", couples: model.toApprove)
                    QueueSection(title: "TO FIX", couples: model.toFix)
                }
                .padding(.horizontal, 12)
                .padding(.bottom, 8)
            }
            .frame(maxHeight: .infinity)
            MediaPoolSection()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Theme.panelBg)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text("COUPLE")
                    .font(.system(size: 13, weight: .bold, design: .rounded))
                    .kerning(3)
                    .foregroundStyle(Theme.textPrimary)
                Spacer()
                // Clear Session (TODO-001): a clearly-visible session control that
                // returns to the initial import screen. Confirms with media/couple/
                // approved counts; disabled mid-export (the batch owns the files).
                Button {
                    model.requestClearSession()
                } label: {
                    Text("Clear Session…")
                        .font(.system(size: 10))
                        .foregroundStyle(Theme.textSecondary)
                }
                .buttonStyle(.plain)
                .disabled(model.batch.isRunning)
                .help("Discard this session and start over (does not delete source or exported files)")
            }
            Text("\(model.pool.videos.count) video · \(model.pool.audios.count) audio")
                .font(.system(size: 10))
                .foregroundStyle(Theme.textSecondary)
            Text("Approve to queue · \(model.approvedCount)/\(model.couples.count) approved")
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(Theme.textTertiary)
            // Unified long-running activity strip (TODO-005): a compact GLOBAL summary
            // of import / candidate pairing / queued auto-sync / batch export, directly
            // BENEATH the counts/approval text, REPLACING the old resolver-only spinner.
            // It stays visible while either the queue list or the media pool scrolls
            // (both are below this fixed header). Honest by construction — the pure
            // policy gives a determinate bar only for trustworthy fractions (import /
            // export %) and an indeterminate bar with text otherwise (pairing, sync).
            ActivityStripView()
        }
        .padding(.horizontal, 12)
        .padding(.top, 26) // traffic-light clearance
        .padding(.bottom, 12)
    }
}

// MARK: - Unified activity strip (TODO-005)

/// The compact global activity strip beneath the session header (TODO-005). Renders
/// `model.activity.summary`: nothing when nil (idle, or work younger than the ~250 ms
/// debounce), an ERROR line (no bar) for a failed operation, a DETERMINATE bar when
/// the summary carries a trustworthy fraction, and an INDETERMINATE bar otherwise.
/// Never shows a fabricated percentage — that honesty is enforced upstream in the
/// pure `ActivitySummaryPolicy`; this view only draws what it is handed.
private struct ActivityStripView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if let summary = model.activity.summary {
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 5) {
                    if summary.isError {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .font(.system(size: 8))
                            .foregroundStyle(Theme.bad)
                    }
                    Text(summary.text)
                        .font(.system(size: 9.5))
                        .foregroundStyle(summary.isError ? Theme.bad : Theme.textSecondary)
                        .lineLimit(1).truncationMode(.middle)
                    Spacer(minLength: 0)
                }
                // A bar only when there is progress to show; an error line carries no
                // bar (TODO-005: failure is an actionable line, not a stuck bar).
                if !summary.isError {
                    if let fraction = summary.fraction {
                        ProgressView(value: fraction)   // determinate (trustworthy %)
                            .progressViewStyle(.linear)
                            .controlSize(.mini)
                            .tint(Theme.accent)
                    } else {
                        ProgressView()                  // indeterminate (text-only honesty)
                            .progressViewStyle(.linear)
                            .controlSize(.mini)
                            .tint(Theme.accent)
                    }
                }
            }
            .padding(.top, 2)
            .transition(.opacity)
        }
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

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            VStack(spacing: 3) {
                VideoThumbnail(url: couple.video.url)
                    .frame(width: 54, height: 32)
                    .clipShape(RoundedRectangle(cornerRadius: 4))
                coverageStrip
                    .frame(width: 54)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(couple.video.name)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1).truncationMode(.middle)
                Text(audioLine)
                    .font(.system(size: 10))
                    .foregroundStyle(audioLineColor)
                    .lineLimit(1).truncationMode(.middle)
            }
            Spacer(minLength: 4)
            VStack(alignment: .trailing, spacing: 6) {
                VerdictDot(verdict: couple.verdict)
                if couple.pair.session != nil {
                    approveBox
                } else if couple.pinned {
                    Image(systemName: "pin.fill")
                        .font(.system(size: 9)).foregroundStyle(Theme.textTertiary)
                }
            }
        }
        .padding(8)
        .background(selected ? Theme.accent.opacity(0.14) : Theme.chipBg.opacity(0.5),
                    in: RoundedRectangle(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7)
            .strokeBorder(selected ? Theme.accent.opacity(0.6) : .clear))
        .contentShape(Rectangle())
        .onTapGesture { model.select(couple.id) }
        // Drag an audio file from the pool onto a couple to re-pair (§V2.6). Routes
        // through the SHARED `requestRepairDrop` (TODO-013) — the same path the
        // center-workspace drop uses — so the two gestures cannot diverge: it
        // no-ops on the current source, replaces immediately when unapproved, and
        // asks before clearing an APPROVED couple's export.
        .dropDestination(for: String.self) { items, _ in
            guard let key = items.first else { return false }
            return model.requestRepairDrop(couple, audioKey: key)
        }
    }

    private var audioLine: String {
        if let word = couple.actionWord { return word }
        return couple.selectedCandidate?.audioName ?? "—"
    }

    private var audioLineColor: Color {
        couple.actionWord != nil ? Theme.verdictColor(couple.verdict) : Theme.textSecondary
    }

    /// Thin amber bar under the thumbnail (§V2.2): the span the audio covers.
    /// Amber = the external-audio identity; full coverage = full bar; coverage is
    /// never a quality penalty.
    @ViewBuilder
    private var coverageStrip: some View {
        if let cov = couple.selectedCandidate?.coverage, couple.video.durationS > 0 {
            GeometryReader { geo in
                let dur = couple.video.durationS
                let x0 = CGFloat(cov.t0 / dur) * geo.size.width
                let w = CGFloat(cov.seconds / dur) * geo.size.width
                ZStack(alignment: .leading) {
                    Capsule().fill(Theme.external.opacity(0.12))
                    Capsule().fill(Theme.external.opacity(0.75))
                        .frame(width: max(w, 2)).offset(x: x0)
                }
            }
            .frame(height: 2.5)
        } else {
            Color.clear.frame(height: 2.5)
        }
    }

    private var approveBox: some View {
        Button {
            model.toggleApprove(couple)
        } label: {
            Image(systemName: couple.approved ? "checkmark.square.fill" : "square")
                .font(.system(size: 13))
                .foregroundStyle(couple.approved ? Theme.good : Theme.textSecondary)
        }
        .buttonStyle(.plain)
        .help("Approve — queue this couple for export (v)")
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

// MARK: - Media pool (§V2.7)

/// The media pool, anchored to the bottom of the rail (redesign 2026-06-15): a
/// FIXED header, a BOUNDED inner ScrollView of selectable rows, and a STICKY footer
/// player that does not scroll. The box height stays constant — the sticky bar
/// keeps its placeholder state when nothing is selected — so the items scroll
/// inside the box while the couples scroll independently above.
private struct MediaPoolSection: View {
    @Environment(AppModel.self) private var model
    @State private var collapsed = false
    @State private var importerShown = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            header
            if !collapsed {
                // Bounded so a long pool scrolls WITHIN the box instead of pushing
                // the sticky footer off-screen; capped at a few rows so the couples
                // region above keeps the lion's share of the rail.
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(model.pool.entries) { entry in
                            PoolEntryRow(entry: entry)
                        }
                    }
                }
                .frame(maxHeight: 168)
                // The sticky transport (owner decisions 1–3): pinned, always visible,
                // follows the single primary; placeholder when nothing is selected.
                PoolStickyBar()
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 12)
        .background(Theme.panelBg)
        .overlay(alignment: .top) { Divider().overlay(Theme.panelStroke) }
    }

    private var header: some View {
        HStack(spacing: 6) {
            Button {
                collapsed.toggle()
            } label: {
                HStack(spacing: 4) {
                    Image(systemName: collapsed ? "chevron.right" : "chevron.down")
                        .font(.system(size: 8))
                    sectionLabel("MEDIA POOL · \(model.pool.entries.count)")
                }
            }
            .buttonStyle(.plain)
            Spacer()
            // Trash affordance (TODO-001): visible only when ≥1 pool entry is selected.
            // Removes the WHOLE selection in one action (unused ⇒ immediate; referenced
            // ⇒ one confirmation). Mirrors the Delete key — both call `removeSelectedMedia`.
            if !model.poolSelection.isEmpty {
                Button {
                    model.removeSelectedMedia()
                } label: {
                    Label("remove \(model.poolSelection.count)", systemImage: "trash")
                        .font(.system(size: 9))
                        .foregroundStyle(Theme.bad)
                }
                .buttonStyle(.plain)
                .help("Remove the selected file\(model.poolSelection.count == 1 ? "" : "s") from this session (does not delete from disk) — Delete")
            }
            Button {
                importerShown = true
            } label: {
                Label("import", systemImage: "plus")
                    .font(.system(size: 9))
                    .foregroundStyle(Theme.textSecondary)
            }
            .buttonStyle(.plain)
            .fileImporter(isPresented: $importerShown,
                          allowedContentTypes: [.movie, .audio, .audiovisualContent, .folder],
                          allowsMultipleSelection: true) { result in
                if case .success(let urls) = result { model.importDropped(urls) }
            }
        }
    }
}

private struct PoolEntryRow: View {
    @Environment(AppModel.self) private var model
    let entry: MediaPool.Entry

    /// Finder-style multi-select highlight (owner decision 3). Selected rows get the
    /// accent background + stroke, consistent with how `CoupleRowView` shows its
    /// selected state; the PRIMARY (the entry the sticky player follows) reads a
    /// touch stronger so a multi-select still tells you which one is loaded.
    private var selected: Bool { model.poolSelection.contains(entry.id) }
    private var isPrimary: Bool { model.poolPrimary == entry.id }

    var body: some View {
        HStack(spacing: 7) {
            Circle().fill(entry.role == .video ? Theme.camera : Theme.external)
                .frame(width: 6, height: 6)
            VStack(alignment: .leading, spacing: 1) {
                Text(entry.name)
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1).truncationMode(.middle)
                // Second line: duration, then a CONCISE usage state/count
                // (TODO-003). Long couple filenames never appear here — the info
                // popover (now on the sticky bar) carries the rich detail.
                Text(secondLine)
                    .font(.system(size: 8.5, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
                    .lineLimit(1).truncationMode(.middle)
            }
            Spacer(minLength: 2)
        }
        .padding(.vertical, 4)
        .padding(.horizontal, 6)
        .background(selected ? Theme.accent.opacity(isPrimary ? 0.18 : 0.10) : .clear,
                    in: RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6)
            .strokeBorder(isPrimary ? Theme.accent.opacity(0.55) : .clear))
        .contentShape(Rectangle())
        // Finder-style select (owner decision 3): plain replaces, ⌘ toggles, ⇧ ranges.
        // Modifier flags are read off the live NSEvent (the established repo idiom —
        // see LoupeView / WaveformStage); SELECT loads the primary PAUSED (decision 1).
        .onTapGesture {
            model.selectPool(key: entry.id, modifiers: NSEvent.modifierFlags)
        }
        // Drag an audio entry onto a couple to re-pair (§V2.6); payload = key. The
        // draggable closure runs at drag START, so it also records the dragging key
        // (TODO-013) — the center-workspace drop overlay reads it to name the file
        // while hovering (the payload itself is the only thing the repair ever uses).
        .draggable(audioDragPayload())
    }

    /// The drag payload (TODO-013): the audio key for an audio row (the empty
    /// sentinel for a non-audio row, which the drop policy rejects). As a side effect
    /// at drag START it records the dragging key on the model so the center-workspace
    /// drop overlay can name the file while hovering; a non-audio drag clears it.
    private func audioDragPayload() -> String {
        if entry.role == .audio {
            model.poolDraggingKey = entry.id
            return entry.id
        }
        model.poolDraggingKey = nil
        return ""
    }

    /// Compact second line (TODO-003): duration first, then the concise usage
    /// state. Joined with a single separator only when both parts exist, so a
    /// zero-duration file never shows a dangling `· `.
    private var secondLine: String {
        let dur = entry.durationS > 0 ? Fmt.hmmss(entry.durationS) : nil
        let parts = [dur, usage].compactMap { $0 }
        return parts.joined(separator: " · ")
    }

    /// Live usage state (§V2.7), kept CONCISE (TODO-003): a state word or a couple
    /// COUNT — never a list of couple filenames (that detail lives in the popover).
    private var usage: String {
        if entry.role == .video {
            if let c = model.couples.first(where: { $0.id == entry.id }) {
                return c.state.chip
            }
            return "video"
        }
        let usedBy = model.couples.filter { $0.selectedAudioKey == entry.id }.count
        if usedBy > 0 { return "used by \(usedBy) couple\(usedBy == 1 ? "" : "s")" }
        let altOf = model.couples.filter { c in c.alternates.contains { $0.audioKey == entry.id } }.count
        if altOf > 0 { return "alternate for \(altOf) couple\(altOf == 1 ? "" : "s")" }
        return "unused"
    }
}

/// The compact metadata popover body (TODO-003). Pure rendering of the entry's
/// `metaCard`: the filename header, a common block (role/duration/created/size/
/// path), the audio- or video-stream technical block, and — for video — an
/// embedded-audio summary. Every group omits genuinely-missing fields, so there
/// are never dangling separators or fabricated values.
private struct MediaMetaPopover: View {
    let entry: MediaPool.Entry

    var body: some View {
        let card = entry.metaCard
        VStack(alignment: .leading, spacing: 8) {
            Text(entry.name)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(Theme.textPrimary)
                .lineLimit(2).truncationMode(.middle)
                .textSelection(.enabled)
            metaGroup(card.common)
            if !card.stream.isEmpty {
                Divider().overlay(Theme.panelStroke)
                metaGroup(card.stream)
            }
            if !card.embeddedAudio.isEmpty {
                Divider().overlay(Theme.panelStroke)
                metaGroup(card.embeddedAudio)
            }
        }
        .padding(12)
        .frame(width: 280, alignment: .leading)
    }

    @ViewBuilder
    private func metaGroup(_ rows: [MediaMetaRow]) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            ForEach(rows) { row in
                HStack(alignment: .top, spacing: 8) {
                    Text(row.label)
                        .font(.system(size: 9, weight: .medium))
                        .foregroundStyle(Theme.textTertiary)
                        .frame(width: 78, alignment: .leading)
                    Text(row.value)
                        .font(.system(size: 9, design: .monospaced))
                        .foregroundStyle(Theme.textSecondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }
}

/// The pool's STICKY bottom player (redesign 2026-06-15, owner decisions 1–3),
/// replacing the per-row inline preview/scrubber and the per-row info glyph. Pinned
/// to the bottom of the pool box, ~one entry-row tall, ALWAYS visible: it follows
/// the single PRIMARY (`model.poolPrimary`) — a seek slider + elapsed/total time +
/// a play/pause button + an info button. When nothing is selected it shows a
/// disabled placeholder ("Select a file to preview") so the box height never jumps.
///
/// SELECT = load-paused (decision 1): selecting an entry loads it here paused (its
/// duration shows); the user presses play to hear it. VIDEO entries preview their
/// EMBEDDED audio through the same controller (decision 2 — `entry.url` is fed to
/// the AVPlayer, which decodes the movie's audio track). The slider reuses the EXACT
/// scrub logic of the old per-row scrubber (scrubBegin/scrubTo/scrubEnd), and the
/// info button reuses `MediaMetaPopover`, re-targeted to the primary entry.
private struct PoolStickyBar: View {
    @Environment(AppModel.self) private var model
    @State private var infoShown = false

    private var primary: MediaPool.Entry? { model.poolPrimaryEntry }

    /// The primary's probed duration, used until AVFoundation reports the item's own.
    private var duration: Double {
        let d = model.preview.state.duration
        let fallback = primary?.durationS ?? 0
        return d > 0 ? d : max(fallback, 0.01)
    }

    private var isPlaying: Bool { model.preview.state.isPlaying }

    var body: some View {
        HStack(spacing: 7) {
            if let primary {
                playPauseButton(for: primary)
                scrubber
                infoButton(for: primary)
            } else {
                placeholder
            }
        }
        .frame(height: 26)
        .padding(.horizontal, 6)
        .background(Theme.chipBg.opacity(0.5), in: RoundedRectangle(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Theme.panelStroke))
    }

    private var placeholder: some View {
        HStack(spacing: 6) {
            Image(systemName: "play.circle")
                .font(.system(size: 11))
                .foregroundStyle(Theme.textTertiary)
            Text("Select a file to preview")
                .font(.system(size: 9))
                .foregroundStyle(Theme.textTertiary)
            Spacer(minLength: 0)
        }
    }

    private func playPauseButton(for entry: MediaPool.Entry) -> some View {
        Button {
            // Play/pause the PRIMARY via the shared controller. The primary is
            // already loaded-paused (SELECT), so the first press plays from 0.
            model.preview.toggle(key: entry.id, url: entry.url)
        } label: {
            Image(systemName: isPlaying ? "pause.fill" : "play.fill")
                .font(.system(size: 10))
                .foregroundStyle(isPlaying ? Theme.external : Theme.textSecondary)
                .frame(width: 22, height: 18)
                .background(Theme.chipBg, in: RoundedRectangle(cornerRadius: 4))
        }
        .buttonStyle(.plain)
        .help(isPlaying ? "Pause preview" : "Play preview")
    }

    /// Seek slider + elapsed/total. Identical scrub semantics to the retired
    /// per-row scrubber: scrubBegin freezes the periodic tick, scrubTo seeks live,
    /// scrubEnd re-enables ticks (works while playing or paused).
    private var scrubber: some View {
        let state = model.preview.state
        return HStack(spacing: 6) {
            Text(Fmt.mmss(state.currentTime))
                .font(.system(size: 8, design: .monospaced))
                .foregroundStyle(Theme.textTertiary)
                .frame(width: 30, alignment: .leading)
            Slider(
                value: Binding(
                    get: { min(state.currentTime, duration) },
                    set: { model.preview.scrubTo($0) }
                ),
                in: 0...duration,
                onEditingChanged: { editing in
                    if editing { model.preview.scrubBegin() } else { model.preview.scrubEnd() }
                }
            )
            .controlSize(.mini)
            .tint(Theme.external)
            Text(Fmt.mmss(duration))
                .font(.system(size: 8, design: .monospaced))
                .foregroundStyle(Theme.textTertiary)
                .frame(width: 30, alignment: .trailing)
        }
    }

    private func infoButton(for entry: MediaPool.Entry) -> some View {
        Button {
            infoShown = true
        } label: {
            Image(systemName: "info.circle")
                .font(.system(size: 10))
                .foregroundStyle(Theme.textTertiary)
                .frame(width: 20, height: 18)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("Show file details")
        .popover(isPresented: $infoShown, arrowEdge: .top) {
            MediaMetaPopover(entry: entry)
        }
    }
}

// MARK: - Video thumbnail (async frame extraction)

struct VideoThumbnail: View {
    let url: URL
    @State private var image: NSImage?

    var body: some View {
        ZStack {
            Rectangle().fill(Theme.canvasBg)
            if let image {
                Image(nsImage: image).resizable().scaledToFill()
            } else {
                Image(systemName: "film")
                    .font(.system(size: 12))
                    .foregroundStyle(Theme.textTertiary)
            }
        }
        .task(id: url) { await loadThumbnail() }
    }

    private func loadThumbnail() async {
        let img = await Task.detached(priority: .utility) { () -> NSImage? in
            let asset = AVURLAsset(url: url)
            let gen = AVAssetImageGenerator(asset: asset)
            gen.appliesPreferredTrackTransform = true
            gen.maximumSize = CGSize(width: 160, height: 160)
            let time = CMTime(seconds: 1.0, preferredTimescale: 600)
            guard let cg = try? gen.copyCGImage(at: time, actualTime: nil) else { return nil }
            return NSImage(cgImage: cg, size: .zero)
        }.value
        image = img
    }
}
