// Queue rail (left column, §V2.1/§V2.2/§V2.7/§V2.12): the session header, the
// couple list in two live workflow sections (TO FIX / READY — membership is the
// verdict alone; an empty TO FIX is hidden, D11; the row checkbox is include-for-
// export and never moves a row), and the media pool generalizing the v1 unused tray (every imported file with a
// usage indicator + audio preview + drag-to-re-pair). Nothing the user dropped
// ever silently disappears.

import AVFoundation
import CoupleKit
import SwiftUI

struct QueueRail: View {
    @Environment(AppModel.self) private var model

    // Live layout measurements for the pool-growth rule (SPEC-CHANGE 2026-07-01):
    // the couples viewport + pool list viewport sum to the rail space the two
    // lists share (invariant however it is split), and the two content heights
    // are the lists' natural sizes. `PoolRailLayout.listHeight` turns these into
    // the pool's height budget.
    @State private var couplesViewportH: CGFloat = 0
    @State private var couplesContentH: CGFloat = 0
    @State private var poolListViewportH: CGFloat = 0
    @State private var poolListContentH: CGFloat = 0

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SessionLine()
            // Two independent scroll regions (redesign 2026-06-15): the couples take
            // the GREEDY top region (CoupleList's own bounded ScrollView, shared with
            // the compact arrangement — PLAN-COMPACT-MODE 4.1), and the media pool is
            // a bottom-pinned box that owns its own inner scroll + a sticky footer
            // player, so the sticky bar stays put while either list scrolls.
            CoupleList(onContentHeight: { couplesContentH = $0 },
                       onViewportHeight: { couplesViewportH = $0 })
            MediaPoolSection(listMaxHeight: poolListMax,
                             listViewportH: $poolListViewportH,
                             listContentH: $poolListContentH)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Theme.panelBg)
    }

    /// The pool list's height budget (SPEC-CHANGE 2026-07-01): at least the fixed
    /// v2.7 cap, growing into rail space the couples list leaves unused — bounded
    /// by the pool's own content so the box never inflates past its rows. The
    /// couples keep first claim, so a long couples list sees the old fixed cap.
    private var poolListMax: CGFloat {
        CGFloat(PoolRailLayout.listHeight(
            available: Double(couplesViewportH + poolListViewportH),
            couplesContent: Double(couplesContentH),
            poolContent: Double(poolListContentH)))
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

    /// Height budget for the inner list, computed by QueueRail from the live
    /// measurements below (SPEC-CHANGE 2026-07-01; `PoolRailLayout.listHeight`).
    let listMaxHeight: CGFloat
    @Binding var listViewportH: CGFloat
    @Binding var listContentH: CGFloat

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            header
            if !collapsed {
                // Bounded so a long pool scrolls WITHIN the box instead of pushing
                // the sticky footer off-screen. The bound is no longer a fixed cap
                // (SPEC-CHANGE 2026-07-01): the list may grow into rail height the
                // couples list is not using (never past its own content), and falls
                // back to the original few-rows cap when the couples need the room.
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(model.pool.entries) { entry in
                            PoolEntryRow(entry: entry)
                        }
                    }
                    .measuredHeight { listContentH = $0 }
                }
                .frame(maxHeight: listMaxHeight)
                .measuredHeight { listViewportH = $0 }
                // Collapsed ⇒ the pool cedes all list space to the couples; zeroing
                // the measurement keeps QueueRail's `available` sum honest so the
                // reopen budget is computed from the real free space, not a stale one.
                .onDisappear { listViewportH = 0 }
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
            // The chevron + label are plain (not a button): the WHOLE header toggles the
            // pool (see the gesture below), so the disclosure affordance is the bar, not
            // just this text.
            Image(systemName: collapsed ? "chevron.right" : "chevron.down")
                .font(.system(size: 8))
            sectionLabel("MEDIA POOL · \(model.pool.entries.count)")
            Spacer()
            // Trash affordance (TODO-001): icon only, visible only when ≥1 pool entry is
            // selected. Removes the WHOLE selection in one action (unused ⇒ immediate;
            // referenced ⇒ one confirmation). Mirrors the Delete key — both call
            // `removeSelectedMedia`.
            if !model.poolSelection.isEmpty {
                Button {
                    model.removeSelectedMedia()
                } label: {
                    Image(systemName: "trash")
                        .font(.system(size: 11))
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
        // Collapse / expand on a click ANYWHERE in the header except the two buttons:
        // the import and trash buttons sit above this gesture and consume their own taps,
        // so the chevron, the label, and the empty bar between them all toggle the pool
        // (previously only the label text did).
        .contentShape(Rectangle())
        .onTapGesture { collapsed.toggle() }
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
                scrubber(for: primary)
                infoButton(for: primary)
            } else {
                placeholder
            }
        }
        // 26 → 34 (SPEC-CHANGE 2026-07-01): the seek strip now draws the file's
        // waveform, which needs a little more vertical room to read than the old
        // slider track did.
        .frame(height: 34)
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
        // Space belongs to the MAIN audition whenever one exists (SPEC-CHANGE
        // 2026-07-02); it only reaches this player when nothing is synced.
        .help(isPlaying ? "Pause preview" : "Play preview")
    }

    /// Waveform seek strip + elapsed/total (SPEC-CHANGE 2026-07-01, replacing the
    /// mini Slider): the strip draws the primary's whole-file envelope and is a
    /// timeline surface — click or drag anywhere seeks, with the retired slider's
    /// exact scrub semantics (scrubBegin freezes the periodic tick, scrubTo seeks
    /// live, scrubEnd re-enables ticks; works while playing or paused).
    private func scrubber(for entry: MediaPool.Entry) -> some View {
        let state = model.preview.state
        return HStack(spacing: 6) {
            Text(Fmt.mmss(state.currentTime))
                .font(.system(size: 8, design: .monospaced))
                .foregroundStyle(Theme.textTertiary)
                .frame(width: 30, alignment: .leading)
            PoolSeekStrip(entry: entry, duration: duration)
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

/// The sticky bar's seek surface (SPEC-CHANGE 2026-07-01, replacing the mini
/// Slider): the primary's whole-file min/max envelope drawn from the core band
/// pyramid — the same query and normalization the overview lanes use, so a quiet
/// recorder reads at the same visual weight — with the played span bright, the
/// remainder dim, and a playhead. The WHOLE strip is a timeline surface: click or
/// drag anywhere seeks (the direct-manipulation default for timeline-like
/// surfaces), through the exact scrub transitions the slider used.
private struct PoolSeekStrip: View {
    @Environment(AppModel.self) private var model
    let entry: MediaPool.Entry
    let duration: Double

    /// Reusable band buffer (SPEC §4.3 idiom — no per-draw allocation).
    @State private var buf = BandBuffer(capacity: 1024)
    @State private var scrubbing = false

    var body: some View {
        // Read the observed playback fields in body (not only inside the Canvas
        // closure) so the 10 Hz ticks reliably invalidate the strip.
        let t = model.preview.state.currentTime
        let active = model.preview.state.isPlaying || scrubbing
        GeometryReader { geo in
            Canvas { ctx, sz in
                draw(ctx, sz: sz, t: t, active: active)
            }
            .contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 0)
                .onChanged { v in
                    if !scrubbing {
                        scrubbing = true
                        model.preview.scrubBegin()
                    }
                    model.preview.scrubTo(time(atX: v.location.x, width: geo.size.width))
                }
                .onEnded { v in
                    model.preview.scrubTo(time(atX: v.location.x, width: geo.size.width))
                    model.preview.scrubEnd()
                    scrubbing = false
                })
        }
    }

    private func time(atX x: CGFloat, width: CGFloat) -> Double {
        guard width > 0, duration > 0 else { return 0 }
        return Double(min(max(0, x), width) / width) * duration
    }

    private func draw(_ ctx: GraphicsContext, sz: CGSize, t: Double, active: Bool) {
        let midY = sz.height / 2
        // Role color = the identity the rest of the app already teaches (amber
        // external audio / camera video).
        let color = entry.role == .video ? Theme.camera : Theme.external
        let cols = min(Int(sz.width), 1024)
        guard duration > 0, cols > 4, let handle = entry.handle,
              handle.band(t0: 0, t1: duration, nCols: cols, into: buf) else {
            // No envelope available (handle mid-import / band error): an honest
            // flat baseline, never a fabricated wave.
            var line = Path()
            line.move(to: CGPoint(x: 0, y: midY))
            line.addLine(to: CGPoint(x: sz.width, y: midY))
            ctx.stroke(line, with: .color(color.opacity(0.3)), lineWidth: 1)
            return
        }
        // Same normalization as the overview lanes (UX §4.1): min/max amplitudes
        // are half an envelope span, hence gain/2 (typical loud content ≈ 72%).
        let gain = 1.44 / max(handle.meta.stats.activeP959600, 1e-6)
        let scale = 0.5 * gain * handle.meta.normalization / 127.0
        let colW = sz.width / CGFloat(cols)
        let halfH = midY - 1
        let playX = duration > 0 ? CGFloat(t / duration) * sz.width : 0
        var played = Path()
        var rest = Path()
        for i in 0..<min(cols, buf.count) {
            let vMin = max(-1, min(1, Double(buf.mins[i]) * scale))
            let vMax = max(-1, min(1, Double(buf.maxs[i]) * scale))
            guard vMax - vMin > 1e-4 else { continue }
            let x = CGFloat(i) * colW
            let yTop = midY - CGFloat(vMax) * halfH
            let yBot = midY - CGFloat(vMin) * halfH
            let rect = CGRect(x: x, y: yTop, width: colW, height: max(yBot - yTop, 0.7))
            if x + colW <= playX { played.addRect(rect) } else { rest.addRect(rect) }
        }
        ctx.fill(rest, with: .color(color.opacity(0.35)))
        ctx.fill(played, with: .color(color.opacity(0.9)))
        // Playhead: bright while playing/scrubbing, grey when parked (the
        // overview's idiom).
        var ph = Path()
        ph.move(to: CGPoint(x: playX, y: 0))
        ph.addLine(to: CGPoint(x: playX, y: sz.height))
        ctx.stroke(ph, with: .color(.white.opacity(active ? 0.85 : 0.5)), lineWidth: 1)
    }
}

// MARK: - Height measurement (pool-growth rule, SPEC-CHANGE 2026-07-01)

extension View {
    /// Report this view's laid-out height to `update` (repo idiom: a clear
    /// GeometryReader in the background with onAppear + onChange — see the
    /// overview frame registration in WaveformStage).
    func measuredHeight(_ update: @escaping (CGFloat) -> Void) -> some View {
        background(GeometryReader { g in
            Color.clear
                .onAppear { update(g.size.height) }
                .onChange(of: g.size.height) { _, h in update(h) }
        })
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
