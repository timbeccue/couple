// Center column (§V2.1): the video hero (greedy), a resolve banner for couples
// that need a human choice (§V2.6), the waveform stage (narrowed overview sharing
// its row with the sync-tools panel; loupes summoned on demand, §V2.4), and the
// persistent bottom tool bar (⚙ sync tools / ◫ loupes / Approve — same-place
// toggles, nothing repositions). Everything is visible at once; there is no
// "editor screen" you navigate to — ⏎ / ⚙ just expands the sync tools in place.

import CoupleKit
import SwiftUI

struct CenterColumn: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 8) {
            if let couple = model.selectedCouple {
                titleBar(couple)
            }
            VideoHero()
                .frame(maxWidth: .infinity, maxHeight: .infinity) // greedy

            if let couple = model.selectedCouple, couple.needsResolve {
                ResolveBanner(couple: couple)
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
        .padding(.horizontal, 14)
        .padding(.top, 26) // traffic-light clearance
        .padding(.bottom, 8)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
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
                ForEach(couple.candidates) { cand in
                    candidateRow(cand)
                }
            }
        }
        .padding(10)
        .background(Theme.verdictColor(couple.verdict).opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8)
            .strokeBorder(Theme.verdictColor(couple.verdict).opacity(0.3)))
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
