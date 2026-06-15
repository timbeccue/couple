// Center column (§V2.1): the video hero (greedy), a resolve banner for couples
// that need a human choice (§V2.6), the waveform stage (narrowed overview sharing
// its row with the sync-tools panel; loupes summoned on demand, §V2.4), and the
// persistent bottom tool bar (⚙ sync tools / ◫ loupes / Approve — same-place
// toggles, nothing repositions). Everything is visible at once; there is no
// "editor screen" you navigate to — ⏎ / ⚙ just expands the sync tools in place.

import CoupleKit
import SwiftUI

/// Pure center-column layout math, kept out of the SwiftUI `View` so it emits a
/// stable, linkable symbol the layout fixture (`DebugDrive.layoutTest`) can call.
enum CenterLayout {
    /// Responsive cap (pt) for the resolve banner's *candidate* scroll region
    /// (TODO-006). Pure + deterministic so the layout fixture can assert it.
    ///
    /// The banner's title/reason and the candidate list live inside the greedy
    /// center stack, *above* the fixed WaveformRow (124) + BottomBar (~52) that
    /// must stay reachable. We therefore let candidates take only a modest
    /// fraction of the column and clamp it: the floor keeps ~3 rows visible even
    /// on a tiny window; the ceiling stops a huge list from dwarfing the video
    /// on a tall one. `containerHeight == 0` (first layout pass) uses the floor.
    static func candidateScrollCap(containerHeight: CGFloat) -> CGFloat {
        let floor: CGFloat = 96   // ~3 candidate rows — never less, even at min height
        let ceiling: CGFloat = 240
        guard containerHeight > 0 else { return floor }
        return min(ceiling, max(floor, containerHeight * 0.28))
    }
}

struct CenterColumn: View {
    @Environment(AppModel.self) private var model

    /// Live center-column height, captured so the resolve banner's candidate
    /// scroll cap can shrink at small windows (TODO-006) and keep the waveform
    /// row + bottom toolbar reachable at the minimum supported height.
    @State private var containerHeight: CGFloat = 0

    /// True while a valid audio drag hovers the center workspace (TODO-013), driving
    /// the highlight + copy overlay. Only armed when a couple is selected (the drop is
    /// scoped to the selected couple — no couple ⇒ no target).
    @State private var dropTargeted = false

    var body: some View {
        content
            .padding(.horizontal, 14)
            .padding(.top, 26) // traffic-light clearance
            .padding(.bottom, 8)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(GeometryReader { geo in
                Color.clear.onAppear { containerHeight = geo.size.height }
                    .onChange(of: geo.size.height) { _, h in containerHeight = h }
            })
            // ONE coherent audio drop target spanning the whole central content
            // region — video preview, resolve banner, waveform row, and sync-tools
            // area (TODO-013, §V2.6). Scoped to the selected couple: with none
            // selected there is no couple to attach the modifier to, so the region is
            // inert. Routes through the SHARED `requestRepairDrop` (the same path the
            // queue-row drop uses) so the two gestures produce identical results —
            // no-op on the current source, immediate replace when unapproved, confirm
            // before clearing an APPROVED couple's export. A non-audio payload / an
            // unavailable video is rejected there without changing state.
            .modifier(CenterDropTarget(
                couple: model.selectedCouple,
                isTargeted: $dropTargeted,
                draggingName: model.poolDraggingName))
    }

    /// The center stack (extracted so the drop modifier + highlight overlay wrap the
    /// whole region as one unit). Layout is unchanged from before TODO-013.
    private var content: some View {
        VStack(spacing: 8) {
            if let couple = model.selectedCouple {
                titleBar(couple)
            }
            VideoHero()
                .frame(maxWidth: .infinity, maxHeight: .infinity) // greedy

            if let couple = model.selectedCouple, couple.needsResolve {
                ResolveBanner(couple: couple,
                              candidateMaxHeight: CenterLayout.candidateScrollCap(containerHeight: containerHeight))
            }

            if model.selectedCouple?.pair.session != nil {
                if model.loupesShown {
                    LoupeArea()
                        .frame(height: model.pair.loupeLayout == .grid ? 220 : loupeStackHeight)
                        .padding(.horizontal, 14)
                }
                WaveformRow()
                    .frame(height: 124)
            }

            BottomBar()
        }
    }

    private var loupeStackHeight: CGFloat {
        let n = model.pair.anchors.count
        return CGFloat(n) * 96 + CGFloat(max(n - 1, 0)) * 8
    }

    private func titleBar(_ couple: CoupleRow) -> some View {
        HStack(spacing: 8) {
            VerdictDot(verdict: couple.verdict)
            Text(couple.video.name)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(Theme.textPrimary)
            if let audio = couple.selectedCandidate?.audioName {
                Text("×").foregroundStyle(Theme.textTertiary)
                Text(audio)
                    .font(.system(size: 12))
                    .foregroundStyle(Theme.external)
            }
            if let word = couple.actionWord {
                Text(word.uppercased())
                    .font(.system(size: 8, weight: .bold)).kerning(0.5)
                    .foregroundStyle(Theme.verdictColor(couple.verdict))
                    .padding(.horizontal, 5).padding(.vertical, 2)
                    .background(Theme.verdictColor(couple.verdict).opacity(0.15), in: Capsule())
            }
            Spacer()
        }
        .lineLimit(1)
    }
}

// MARK: - Center-workspace drop target (TODO-013, §V2.6)

/// Makes the selected couple's central content region one coherent AUDIO drop
/// target (TODO-013): drag an audio file from the media pool anywhere over the video
/// preview / resolve banner / waveform row / sync-tools area to re-pair the take.
/// During a valid drag it highlights the region in the app's accent language (a soft
/// accent wash + an accent stroke, NOT a yellow box) and shows a chip reading
/// `Pair selected video with <audio>` using the dragged file's display name. The
/// drop itself goes through `AppModel.requestRepairDrop` — the SAME shared path the
/// queue-row target uses — so the two gestures cannot diverge.
///
/// Scoped to the selected couple: with `couple == nil` the modifier attaches no drop
/// destination at all, so the region is inert (no highlight, no target) — satisfying
/// "no selected couple rejects the drop without changing state". The repair op's own
/// guards reject a non-audio payload or an unavailable video.
private struct CenterDropTarget: ViewModifier {
    @Environment(AppModel.self) private var model
    let couple: CoupleRow?
    @Binding var isTargeted: Bool
    /// The dragged audio's display name (for the live copy), or nil when unknown.
    let draggingName: String?

    func body(content: Content) -> some View {
        if let couple {
            content
                .overlay { if isTargeted { highlight } }
                .dropDestination(for: String.self) { items, _ in
                    isTargeted = false
                    model.poolDraggingKey = nil
                    guard let key = items.first else { return false }
                    return model.requestRepairDrop(couple, audioKey: key)
                } isTargeted: { hovering in
                    isTargeted = hovering
                }
        } else {
            content // no selected couple ⇒ no drop target (inert region)
        }
    }

    /// The valid-drop highlight: an accent wash + stroke (the app's selection/accent
    /// language, consistent with the queue-row drop affordance) and a centered chip
    /// naming the action. Reuses the panel/chip vocabulary; never a yellow box.
    private var highlight: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 12)
                .fill(Theme.accent.opacity(0.10))
            RoundedRectangle(cornerRadius: 12)
                .strokeBorder(Theme.accent.opacity(0.7), lineWidth: 2)
            HStack(spacing: 8) {
                Image(systemName: "waveform.badge.plus")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(Theme.accent)
                Text(copy)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1).truncationMode(.middle)
            }
            .padding(.horizontal, 14).padding(.vertical, 9)
            .background(Theme.panelBg.opacity(0.95), in: Capsule())
            .overlay(Capsule().strokeBorder(Theme.accent.opacity(0.45)))
        }
        .allowsHitTesting(false) // the drop destination underneath owns the hit-test
        .padding(2)
    }

    /// `Pair selected video with <audio>.wav` (TODO-013), using the dragged file's
    /// display name when known; a graceful fallback when the live name is unavailable.
    private var copy: String {
        if let name = draggingName { return "Pair selected video with \(name)" }
        return "Pair selected video with this audio"
    }
}

// MARK: - Overview row (narrowed overview + fit status / sparkline / re-sync)

private struct WaveformRow: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        HStack(spacing: 10) {
            OverviewStrip()
                .frame(maxWidth: .infinity)
            VStack(alignment: .leading, spacing: 4) {
                fitStatusLine(pair)
                SparklineView()
                    .frame(width: 150, height: 56)
                Button {
                    pair.runAutoSync()
                } label: {
                    Label("re-sync", systemImage: "arrow.clockwise")
                        .font(.system(size: 9))
                        .foregroundStyle(Theme.textSecondary)
                }
                .buttonStyle(.bordered).controlSize(.small)
                .disabled(!pair.inputsEnabled)
                .help("Re-run auto-sync — never re-proposes anchors or re-frames a view you've seen")
            }
            .frame(width: 158)
        }
    }

    /// Fit status; clicking it summons the loupes (§V2.4). Surfaces the raw fit
    /// RMS so the owner can calibrate (v1's thresholds don't transfer — V2-F2).
    @ViewBuilder
    private func fitStatusLine(_ pair: PairSession) -> some View {
        Button {
            model.loupesShown = true
        } label: {
            Group {
                if case .running(let label, _) = pair.syncActivity {
                    Text(label)
                } else if case .failed(let message) = pair.syncActivity {
                    Text(message).foregroundStyle(Theme.bad).lineLimit(2)
                } else if let fit = pair.fitStatus {
                    Text(String(format: "fit RMS %.2f ms · %d win", fit.rmsMs, fit.windows))
                        .foregroundStyle(Theme.textSecondary)
                } else {
                    Text("not measured").foregroundStyle(Theme.textTertiary)
                }
            }
            .font(.system(size: 9, design: .monospaced))
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Persistent bottom tool bar (§V2.4)

private struct BottomBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        let hasSession = model.selectedCouple?.pair.session != nil
        VStack(spacing: 6) {
            if model.syncToolsExpanded && hasSession {
                HStack(alignment: .top, spacing: 16) {
                    SyncReadouts()
                    driftModePicker(pair)
                    Spacer()
                }
            }
            HStack(spacing: 10) {
                toggle("sync tools", icon: "gearshape", on: model.syncToolsExpanded) {
                    model.syncToolsExpanded.toggle()
                }
                .disabled(!hasSession)
                toggle("loupes", icon: "rectangle.split.3x1", on: model.loupesShown) {
                    model.loupesShown.toggle()
                }
                .disabled(!hasSession)
                Spacer()
                approveButton(hasSession: hasSession)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Theme.panelBg, in: RoundedRectangle(cornerRadius: 9))
        .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Theme.panelStroke))
    }

    private func toggle(_ label: String, icon: String, on: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(label, systemImage: icon)
                .font(.system(size: 11, weight: on ? .semibold : .regular))
                .foregroundStyle(on ? Theme.accent : Theme.textSecondary)
                .padding(.horizontal, 9).padding(.vertical, 5)
                .background(on ? Theme.accent.opacity(0.15) : Theme.chipBg,
                            in: RoundedRectangle(cornerRadius: 6))
        }
        .buttonStyle(.plain)
    }

    private func driftModePicker(_ pair: PairSession) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            sectionLabel("DRIFT MODE")
            Picker("", selection: Binding(
                get: { pair.driftMode },
                set: { pair.setDriftMode($0) }
            )) {
                Text("Resample").tag(DriftMode.resample)
                Text("None").tag(DriftMode.none)
            }
            .pickerStyle(.segmented).controlSize(.mini).frame(width: 150)
            .disabled(!pair.inputsEnabled)
        }
    }

    private func approveButton(hasSession: Bool) -> some View {
        let approved = model.selectedCouple?.approved ?? false
        return Button {
            if let c = model.selectedCouple { model.toggleApprove(c) }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: approved ? "checkmark.circle.fill" : "circle")
                Text(approved ? "Approved" : "Approve")
                    .font(.system(size: 12, weight: .semibold))
                Text("v").font(.system(size: 9, design: .monospaced)).opacity(0.6)
            }
            .foregroundStyle(approved ? .black.opacity(0.8) : Theme.textPrimary)
            .padding(.horizontal, 14).padding(.vertical, 7)
            .background(approved ? Theme.good : Theme.chipBg, in: RoundedRectangle(cornerRadius: 7))
        }
        .buttonStyle(.plain)
        .disabled(!hasSession)
        .help("Approve ≡ queue for export (no audition required)")
    }
}

// MARK: - Resolve banner (§V2.6): the manual-choice surface

private struct ResolveBanner: View {
    @Environment(AppModel.self) private var model
    let couple: CoupleRow
    /// Responsive cap for the candidate scroll region (TODO-006); the title and
    /// reason above it stay fixed and always visible.
    let candidateMaxHeight: CGFloat

    /// Measured natural height of the candidate list, so a short list sizes to
    /// its content (no empty filler, no needless scrollbar) while a long list is
    /// clamped to `candidateMaxHeight` and scrolls internally.
    @State private var candidatesContentHeight: CGFloat = 0

    private var maxZ: Double { max(couple.candidates.map(\.z).max() ?? 1, 1) }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: couple.state == .noMatch ? "exclamationmark.triangle" : "questionmark.circle")
                    .foregroundStyle(Theme.verdictColor(couple.verdict))
                Text(couple.resolveTitle)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
            }
            Text(reason)
                .font(.system(size: 10))
                .foregroundStyle(Theme.textSecondary)
            if couple.candidates.isEmpty {
                Text("No audio cleared the floor — drag a file from the media pool to force a pairing.")
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textTertiary)
            } else {
                candidateList
            }
            // Boundary-9 (§14.4): the OBSERVATIONAL spectral shadow evidence appears ONLY in this
            // existing diagnostic disclosure (never in a normal queue row), and ONLY when the owner
            // opted in (COUPLE_SHADOW_MATCHER=1 ⇒ `detail.shadowEvidence` populated). Algorithm terms
            // are confined here — the user-facing row vocabulary (confident/partial/…/no match) is
            // untouched. Read-only: it reflects core's classification, it does not change the row.
            if let shadow = couple.detail.shadowEvidence, !shadow.isEmpty {
                shadowDisclosure(shadow)
            }
        }
        .padding(10)
        .background(Theme.verdictColor(couple.verdict).opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8)
            .strokeBorder(Theme.verdictColor(couple.verdict).opacity(0.3)))
    }

    /// The candidate picker (TODO-006): a height-bounded internal scroll region
    /// so a large source list can never push the waveform row + bottom toolbar
    /// off screen. The title/reason above stay fixed. A short list sizes to its
    /// content (no filler, no scrollbar); a long list clamps to
    /// `candidateMaxHeight` and scrolls. The current selection is scrolled into
    /// view when the banner opens or the selection changes. Ordering/colors/
    /// selection semantics are unchanged — only the container is bounded.
    private var candidateList: some View {
        // Natural height when it fits; otherwise the responsive cap.
        let height = candidatesContentHeight > 0
            ? min(candidatesContentHeight, candidateMaxHeight)
            : candidateMaxHeight
        return ScrollViewReader { proxy in
            ScrollView(.vertical) {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(couple.candidates) { cand in
                        candidateRow(cand)
                            .id(cand.audioKey)   // stable id for scroll-to-selection
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(GeometryReader { geo in
                    Color.clear
                        .onAppear { candidatesContentHeight = geo.size.height }
                        .onChange(of: geo.size.height) { _, h in candidatesContentHeight = h }
                })
            }
            .frame(height: height)
            // Hide the scrollbar when the list fits; show it only when clamped.
            .scrollIndicators(candidatesContentHeight > candidateMaxHeight ? .automatic : .hidden)
            .onAppear { scrollToSelection(proxy) }
            .onChange(of: couple.selectedAudioKey) { _, _ in scrollToSelection(proxy) }
            .onChange(of: couple.id) { _, _ in scrollToSelection(proxy) }
        }
    }

    /// Bring the current selection into view (centered). Falls back to the first
    /// candidate so a no-selection ambiguous row opens at the top of the list.
    private func scrollToSelection(_ proxy: ScrollViewProxy) {
        let target = couple.selectedAudioKey ?? couple.candidates.first?.audioKey
        guard let target else { return }
        DispatchQueue.main.async {
            withAnimation(.easeOut(duration: 0.15)) {
                proxy.scrollTo(target, anchor: .center)
            }
        }
    }

    /// Diagnostic-disclosure-only spectral shadow read-out (§14.2/§14.4). One line per audio,
    /// ordered by the projector's deterministic `score` rank. Shows core's `classification` verbatim
    /// (the acceptance authority) plus the consensus mapping + quality metrics for owner calibration.
    /// This is the ONLY place algorithm jargon is allowed; it carries no decision (resolved rows are
    /// legacy). Refined lag/drift (boundary 7) are shown when core supplied them.
    private func shadowDisclosure(_ rows: [ShadowEvidenceRow]) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Spectral shadow (observational — does not change the match)")
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(Theme.textTertiary)
            ForEach(rows, id: \.audioKey) { ev in
                Text(shadowLine(ev))
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
                    .lineLimit(1).truncationMode(.middle)
            }
        }
        .padding(.top, 4)
    }

    /// One shadow line: `<class> <audio> · lag <ms> · <ppm> ppm · supp <n> · in <%> · cov <%> · p95 <ms> · score <s>`.
    /// `class` is core's verdict (match/no-match/ambiguous/…); the refined lag/drift replace the
    /// consensus pair when present. `p95` reads `—` when core had no fittable hypothesis.
    private func shadowLine(_ ev: ShadowEvidenceRow) -> String {
        let cls: String
        switch ev.classification {
        case .match: cls = "match"
        case .noMatch: cls = "no-match"
        case .ambiguous: cls = "ambiguous"
        case .insufficientOverlap: cls = "insuff-overlap"
        case .unavailable: cls = "unavailable"
        }
        let lag = ev.refinedLagMs ?? ev.lagMs
        let ppm = ev.refinedDriftPpm ?? ev.driftPpm
        let p95 = ev.residualP95Ms.map { String(format: "%.1f", $0) } ?? "—"
        return String(format: "%@ %@ · lag %.0f · %.0f ppm · supp %d · in %.0f%% · cov %.0f%% · p95 %@ · score %.2f",
                      cls, ev.audioName, lag, ppm, ev.support,
                      ev.inlierFraction * 100, ev.temporalCoverage * 100, p95, ev.score)
    }

    private var reason: String {
        switch couple.state {
        case .noMatch: return "Nothing in the pool correlates with this take's camera audio."
        case .ambiguous:
            // Phase-1 telemetry (§4.5): when drift-fit RMS was measured for the two
            // best contested candidates, show the separation — the validation signal.
            let fitLine = (couple.detail.fitBestRmsMs).flatMap { best in
                couple.detail.fitSecondRmsMs.map { second in
                    String(format: " Fit separates them: %.1f ms vs %.1f ms.", best, second)
                }
            } ?? ""
            if let m = couple.detail.margin {
                return String(format: "Two takes are too close to call (margin %.1f < %.1f). A human must pick.%@",
                              m, model.params.zMargin, fitLine)
            }
            return "Candidates conflict — a human must pick.\(fitLine)"
        default: return "Pick the audio source for this take."
        }
    }

    /// Candidate row (§V2.6): a relative strength bar + coverage on the right —
    /// never the raw z number (bimodal; z lives in the tooltip).
    private func candidateRow(_ cand: RowCandidate) -> some View {
        let selected = couple.selectedAudioKey == cand.audioKey
        return Button {
            model.repair(couple, audioKey: cand.audioKey)
        } label: {
            HStack(spacing: 8) {
                Image(systemName: selected ? "largecircle.fill.circle" : "circle")
                    .font(.system(size: 11))
                    .foregroundStyle(selected ? Theme.accent : Theme.textTertiary)
                Text(cand.audioName)
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1).truncationMode(.middle)
                Spacer(minLength: 8)
                // Relative strength bar (verdict colors), the one place strength helps.
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule().fill(Theme.chipBg)
                        Capsule().fill(Theme.verdictGood.opacity(0.8))
                            .frame(width: max(6, geo.size.width * CGFloat(cand.z / maxZ)))
                    }
                }
                .frame(width: 54, height: 5)
                Text("\(Int((cand.coverage.fraction * 100).rounded()))%")
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
                    .frame(width: 34, alignment: .trailing)
            }
            .padding(.vertical, 4).padding(.horizontal, 6)
            .background(selected ? Theme.accent.opacity(0.1) : .clear, in: RoundedRectangle(cornerRadius: 5))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(String(format: "z %.1f · lag %.0f ms%@%@", cand.z, cand.lagMs,
                     cand.fitRmsMs.map { String(format: " · fit %.1f ms (%d win)", $0, cand.fitWindows ?? 0) } ?? "",
                     // §4.5 fine-verify telemetry (read-only): lock fraction + drift ppm.
                     cand.lockFraction.map { lf in
                         String(format: " · %.0f%% lock · %.0f ppm", lf * 100, cand.ppm ?? 0)
                     } ?? ""))
    }
}
