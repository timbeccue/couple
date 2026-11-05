// Pair preview (UX §V2.11; PLAN-COMPACT-MODE Phase 4.1): the selected couple's
// preview block, one view for BOTH arrangements —
//  - title line (video name + chosen audio / action word; "Change…" after a
//    manual pick);
//  - the video hero with the two-lane PairStrip seek surface between the time
//    labels (EditorComponents `VideoHero`), and the monitor picker — 4-way in
//    full, Camera · Synced · External in compact (D10);
//  - the tertiary line (compact: "offset · fit"; both: the candidate-audition
//    readout or "Audio chosen by you · Undo · Compare others");
//  - the inline candidate list (`ResolveBanner`) for a "pick one" couple or a
//    reopened "Compare others"; in compact a "no audio" couple gets the one line
//    "No matching audio in this session · Choose file…" instead.
// Full: the hero is greedy (the overview/loupes/bottom bar below are the center
// column's). Compact: the block is fixed (16:9 player) and never scrolls with
// the list. An empty session shows the import surface in the hero slot.

import CoupleKit
import SwiftUI

struct PairPreview: View {
    @Environment(AppModel.self) private var model
    var compact = false
    /// Cap for the inline candidate list's scroll region (TODO-006).
    var candidateMaxHeight: CGFloat = 132

    /// A manual pick (📌 with an audio): the banner folds into the
    /// "Audio chosen by you" line until Compare others / Change… reopens it.
    private func isManualPick(_ couple: CoupleRow) -> Bool {
        couple.pinned && couple.selectedAudioKey != nil
    }

    var body: some View {
        VStack(spacing: compact ? 6 : 8) {
            if let couple = model.selectedCouple {
                titleLine(couple)
            }
            if model.hasContent {
                if compact {
                    VideoHero(compact: true)
                } else {
                    VideoHero()
                        .frame(maxWidth: .infinity, maxHeight: .infinity) // greedy
                }
            } else {
                // Empty session (revised 2026-07-11): the import surface takes the
                // hero slot — no separate splash screen.
                LoadSurface()
                    .frame(maxWidth: .infinity, maxHeight: .infinity) // greedy
            }

            if let couple = model.selectedCouple {
                let comparing = model.compareCoupleID == couple.id && isManualPick(couple)
                let auditioning = model.candidateAudition?.coupleID == couple.id
                if compact && !auditioning && couple.pair.session != nil {
                    OffsetFitLine(pair: couple.pair)
                }
                PreviewStatusLine(couple: couple,
                                  manualPick: isManualPick(couple) && !comparing,
                                  onCompare: { model.compareCoupleID = couple.id })
                if compact && couple.state == .noMatch && couple.candidates.isEmpty {
                    NoAudioLine(couple: couple)
                } else if comparing || (couple.needsResolve && !isManualPick(couple)) {
                    ResolveBanner(couple: couple,
                                  candidateMaxHeight: candidateMaxHeight,
                                  onDone: comparing ? { model.compareCoupleID = nil } : nil)
                }
            }
        }
    }

    @ViewBuilder
    private func titleLine(_ couple: CoupleRow) -> some View {
        HStack(spacing: compact ? 6 : 8) {
            if !compact {
                VerdictDot(verdict: couple.verdict)
            }
            Text(couple.video.name)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(Theme.textPrimary)
            if let audio = couple.selectedCandidate?.audioName {
                Text(compact ? "+" : "×").foregroundStyle(Theme.textTertiary)
                Text(audio)
                    .font(.system(size: compact ? 11 : 12))
                    .foregroundStyle(compact ? Theme.textSecondary : Theme.external)
                    .truncationMode(.middle)
            }
            if let word = couple.actionWord {
                if compact {
                    // Same capsule as the row's action word (no dots, D3).
                    let grey = couple.verdict == .none
                    let tint = Theme.verdictColor(couple.verdict)
                    Text(word)
                        .font(.system(size: 9.5, weight: .semibold))
                        .foregroundStyle(grey ? Theme.textSecondary : tint)
                        .padding(.horizontal, 6).padding(.vertical, 1)
                        .background((grey ? Color.white : tint).opacity(grey ? 0.08 : 0.16),
                                    in: RoundedRectangle(cornerRadius: 3))
                } else {
                    Text(word.uppercased())
                        .font(.system(size: 8, weight: .bold)).kerning(0.5)
                        .foregroundStyle(Theme.verdictColor(couple.verdict))
                        .padding(.horizontal, 5).padding(.vertical, 2)
                        .background(Theme.verdictColor(couple.verdict).opacity(0.15), in: Capsule())
                }
            }
            Spacer(minLength: 4)
            // After a manual pick: reopen the candidate list inline (§V2.11).
            if isManualPick(couple) && model.compareCoupleID != couple.id {
                Button("Change…") { model.compareCoupleID = couple.id }
                    .buttonStyle(.plain)
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.accent)
                    .help("Compare this take's other audio candidates")
            }
        }
        .lineLimit(1)
    }
}

// MARK: - Compact tertiary line: offset · fit

/// "offset +1.28 s" … "fit 4.6 ms" (compact only; the full arrangement's numeric
/// readouts live in the sync tools / fit status line).
private struct OffsetFitLine: View {
    let pair: PairSession

    var body: some View {
        HStack {
            Text(String(format: "offset %+.2f s", pair.offsetMs / 1000))
            Spacer()
            if case .running(let label, _) = pair.syncActivity {
                Text(label)
            } else if let fit = pair.fitStatus {
                Text(String(format: "fit %.1f ms", fit.rmsMs))
            }
        }
        .font(.system(size: 9.5, design: .monospaced))
        .foregroundStyle(Theme.textTertiary)
        .lineLimit(1)
        .padding(.horizontal, 4)
    }
}

// MARK: - "No audio" (compact)

/// "No matching audio in this session · Choose file…" — the file picker pairs the
/// chosen audio with this take as a manual pick (the same repair path a drop uses).
private struct NoAudioLine: View {
    @Environment(AppModel.self) private var model
    let couple: CoupleRow
    @State private var importerShown = false

    var body: some View {
        HStack(spacing: 6) {
            Text("No matching audio in this session")
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(1)
            Text("·").foregroundStyle(Theme.textTertiary)
            Button("Choose file…") { importerShown = true }
                .buttonStyle(.plain)
                .foregroundStyle(Theme.accent)
                .help("Pick the audio recording for this take")
            Spacer(minLength: 0)
        }
        .font(.system(size: 10.5))
        .padding(.horizontal, 4)
        .fileImporter(isPresented: $importerShown,
                      allowedContentTypes: [.audio, .audiovisualContent],
                      allowsMultipleSelection: false) { result in
            if case .success(let urls) = result, let url = urls.first {
                model.pairWithChosenFile(couple, url: url)
            }
        }
    }
}

// MARK: - Preview status line (PLAN-COMPACT-MODE Phase 3 steps 2–3)

/// One tertiary line under the player: while a candidate is auditioned, the blue
/// readout "▶ <file> at best fit · offset <lag> s · fit <rms>" (fit only when the
/// candidate carries fine-verify telemetry) with ■; after a manual pick, "Audio
/// chosen by you · Undo · Compare others". Otherwise nothing.
struct PreviewStatusLine: View {
    @Environment(AppModel.self) private var model
    let couple: CoupleRow
    let manualPick: Bool
    let onCompare: () -> Void

    var body: some View {
        if let a = model.candidateAudition, a.coupleID == couple.id {
            HStack(spacing: 6) {
                Text(Self.auditionReadout(a.candidate))
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(Theme.accent)
                    .lineLimit(1).truncationMode(.middle)
                Spacer()
                Button {
                    model.stopCandidateAudition()
                } label: {
                    Image(systemName: "stop.fill").font(.system(size: 9))
                        .foregroundStyle(Theme.accent)
                }
                .buttonStyle(.plain)
                .help("Stop auditioning — back to this couple's own audio")
            }
            .padding(.horizontal, 4)
        } else if manualPick {
            HStack(spacing: 10) {
                Text("Audio chosen by you")
                    .foregroundStyle(Theme.textSecondary)
                Spacer()
                if model.canUndoManualPick(couple) {
                    Button("Undo") { model.undoManualPick(couple) }
                        .buttonStyle(.plain)
                        .foregroundStyle(Theme.accent)
                        .help("Undo the pick — the couple goes back to To fix")
                }
                Button("Compare others", action: onCompare)
                    .buttonStyle(.plain)
                    .foregroundStyle(Theme.accent)
                    .help("Reopen the candidate list to listen to the alternatives")
            }
            .font(.system(size: 10))
            .padding(.horizontal, 4)
        }
    }

    /// "▶ ZOOM0027.WAV at best fit · offset +0.38 s · fit 5.1 ms".
    static func auditionReadout(_ c: RowCandidate) -> String {
        var s = "▶ \(c.audioName) at best fit · offset " + String(format: "%+.2f s", c.lagMs / 1000)
        if let fit = c.fitRmsMs { s += String(format: " · fit %.1f ms", fit) }
        return s
    }
}

// MARK: - Resolve banner (§V2.6): the manual-choice surface

struct ResolveBanner: View {
    @Environment(AppModel.self) private var model
    let couple: CoupleRow
    /// Responsive cap for the candidate scroll region (TODO-006); the title and
    /// reason above it stay fixed and always visible.
    let candidateMaxHeight: CGFloat
    /// Set when the banner is the reopened "Compare others" list after a manual
    /// pick (PLAN-COMPACT-MODE §6.3): the header gains "Done", which closes it.
    var onDone: (() -> Void)?

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
                Spacer()
                if let onDone {
                    Button("Done", action: onDone)
                        .buttonStyle(.plain)
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Theme.accent)
                }
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
        // Closing the banner (Done, a pick, another row) ends any audition of its
        // candidates and restores the couple's own audio.
        .onDisappear {
            if model.candidateAudition?.coupleID == couple.id { model.stopCandidateAudition() }
        }
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

    /// Candidate row (§V2.6 + PLAN-COMPACT-MODE Phase 3): ▶ audition at best fit
    /// (■ while this candidate plays; nothing is pinned), the name, a relative
    /// strength bar + coverage — never the raw z number (bimodal; z lives in the
    /// tooltip) — and "use" (manual pin + re-sync, undoable) or "✓ current".
    private func candidateRow(_ cand: RowCandidate) -> some View {
        let selected = couple.selectedAudioKey == cand.audioKey
        let playing = model.candidateAudition?.coupleID == couple.id
            && model.candidateAudition?.candidate.audioKey == cand.audioKey
        let playable = model.selectedID == couple.id && couple.video.handle != nil
            && model.pool.entry(key: cand.audioKey)?.handle != nil
        return HStack(spacing: 8) {
            Button {
                if playing { model.stopCandidateAudition() } else { model.auditionCandidate(couple, cand) }
            } label: {
                Image(systemName: playing ? "stop.fill" : "play.fill")
                    .font(.system(size: 8))
                    .foregroundStyle(playing ? Theme.accent : Theme.textSecondary)
                    .frame(width: 20, height: 16)
                    .background(playing ? Theme.accent.opacity(0.18) : Theme.chipBg,
                                in: RoundedRectangle(cornerRadius: 4))
            }
            .buttonStyle(.plain)
            .disabled(!playable)
            .help(playing ? "Stop" : "Play the video with this audio at its best-fit offset (nothing is changed)")
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
            if selected {
                Text("✓ current")
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textSecondary)
                    .frame(width: 58, alignment: .trailing)
            } else {
                Button("use") { model.repair(couple, audioKey: cand.audioKey) }
                    .buttonStyle(.plain)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(Theme.accent)
                    .frame(width: 58, alignment: .trailing)
                    .help("Use this audio for the take (pins it and re-syncs; ⌘Z undoes)")
            }
        }
        .padding(.vertical, 4).padding(.horizontal, 6)
        .background(playing ? Theme.accent.opacity(0.12) : (selected ? Theme.accent.opacity(0.06) : .clear),
                    in: RoundedRectangle(cornerRadius: 5))
        .help(String(format: "z %.1f · lag %.0f ms%@%@", cand.z, cand.lagMs,
                     cand.fitRmsMs.map { String(format: " · fit %.1f ms (%d win)", $0, cand.fitWindows ?? 0) } ?? "",
                     // §4.5 fine-verify telemetry (read-only): lock fraction + drift ppm.
                     cand.lockFraction.map { lf in
                         String(format: " · %.0f%% lock · %.0f ppm", lf * 100, cand.ppm ?? 0)
                     } ?? ""))
    }
}
