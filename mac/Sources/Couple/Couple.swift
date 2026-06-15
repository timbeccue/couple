// Couple (SPEC-V2 §3.1 / §V2.0) — the user-facing queue row: one video + its
// selected audio source + an alternates list. Owns the per-couple PairSession
// (alignment shadow, view state, export options, sync), the resolution the user
// can override, and the per-couple export overrides + status.
//
// "Approve ≡ queued" (§V2.5): there is no separate queue flag — `pair.verified`
// IS queue membership. The verdict dot (§V2.3) is good/questionable/none, derived
// from the resolution state (coverage is never a quality penalty) with "forced"
// for a kept below-floor manual pairing.

import CoupleKit
import Foundation
import Observation

// (Type named `CoupleRow`, not `Couple`, to avoid colliding with the executable
// target's module name `Couple` — `Couple.ID` would otherwise resolve to the
// module. "Couple" remains the user-facing noun in all copy.)
@MainActor
@Observable
final class CoupleRow: Identifiable {
    /// Stable identity = the video's content key (survives re-resolve / restore).
    let id: String
    let video: MediaPool.Entry
    let engine: Engine
    let toasts: ToastCenter

    // ----- resolution (resolver output; user-overridable) -----
    var state: RowState
    var chip: String
    var detail: ResolutionDetail
    /// Every candidate the resolver found, in z-desc/name order — never destroyed
    /// (§V2.6). Selection is just `selectedAudioKey`; switching is one reversible
    /// click. A re-resolve over new files may append alternates but never changes
    /// a pinned selection.
    var candidates: [RowCandidate]
    var selectedAudioKey: String?
    var pinned = false   // 📌 manual choice; re-runs never override (§3.6)
    var forced = false   // kept below-floor manual pairing (honest, §3.6 / §V2.6)

    /// TODO-009: how the current `selectedAudioKey` came to be selected. Match
    /// quality (the verdict dot) is derived from EVIDENCE for the selected pair, not
    /// from the act of selecting, so a manual pick is treated differently from an
    /// auto-resolved one. `.automatic` when the resolver selected this source;
    /// `.manual` after a candidate pick or pool drag (`repair`); `.none` when nothing
    /// is selected. Persisted so a restored manual pick keeps its provenance.
    var selectionProvenance: SelectionProvenance
    /// TODO-009: for a `.manual` selection, whether THIS specific video/audio pair
    /// independently cleared the SAME confident classifier an automatic confident
    /// match uses (the resolver's `matchPair`/`verify` path, re-run by `repair`).
    /// `nil` ⇒ not yet evaluated (treated as not-confident, so a manual pick is never
    /// green until evidence says so). Meaningless for non-manual selections.
    var manualMatchConfident: Bool?

    /// The drill-down editor for the current selection. Created empty; gets a
    /// session via `pair.load(...)` whenever a selection exists.
    let pair: PairSession

    // ----- per-couple export naming (the filename is edited in place, §V2.8).
    // Format is global (the export rail), not per-couple (owner 2026-06-13).
    var stemOverride: String?

    enum ExportStatus: Equatable {
        case idle
        case queued
        case running(Double)
        case done(URL)
        case failed(String)
    }
    var exportStatus: ExportStatus = .idle

    init(video: MediaPool.Entry, row: ResolvedRow, engine: Engine, toasts: ToastCenter,
         audition: AuditionController) {
        self.id = video.id
        self.video = video
        self.engine = engine
        self.toasts = toasts
        self.state = row.state
        self.chip = row.chip
        self.detail = row.detail
        self.candidates = (row.selected.map { [$0] } ?? []) + row.alternates
        self.selectedAudioKey = row.selected?.audioKey
        // A resolver-supplied selection is AUTOMATIC (its quality is the resolver's
        // verdict); no selection is `.none` (TODO-009).
        self.selectionProvenance = row.selected != nil ? .automatic : .none
        self.pair = PairSession(engine: engine, toasts: toasts, audition: audition)
    }

    // ----- derived -----

    /// Approve ≡ queued for export (§V2.5). Only meaningful once a session exists.
    var approved: Bool { pair.verified }

    var selectedCandidate: RowCandidate? {
        guard let k = selectedAudioKey else { return nil }
        return candidates.first { $0.audioKey == k }
    }

    var alternates: [RowCandidate] {
        candidates.filter { $0.audioKey != selectedAudioKey }
    }

    var hasSession: Bool { pair.session != nil }

    /// Three-category verdict (§V2.3), evidence-based per TODO-009. Match quality
    /// derives ONLY from classifier/resolver evidence for the *selected* pair —
    /// never from the act of selecting. The decision is the pure
    /// `MatchQualityPolicy`: `forced` ⇒ questionable; an automatic selection (or no
    /// selection) takes the resolver's verdict; a MANUAL pick is good ONLY when that
    /// exact pair independently cleared the same confident classifier
    /// (`manualMatchConfident`), else it stays questionable. Approval and alignment
    /// RMS are deliberately not inputs.
    var verdict: Verdict {
        Verdict(MatchQualityPolicy.decide(
            forced: forced,
            selection: selectedAudioKey == nil ? .none : selectionProvenance,
            autoVerdict: MatchQuality(state.verdict),
            manualMatchConfident: manualMatchConfident))
    }

    /// The short word that rides along only when action is needed (§V2.3).
    var actionWord: String? {
        if forced { return "forced" }
        if selectedAudioKey == nil {
            switch state {
            case .ambiguous: return "pick one"
            case .noMatch: return "no audio"
            default: return nil
            }
        }
        return nil
    }

    /// True when the couple needs the resolve banner (§V2.6): no confident
    /// single auto-pairing — the user must choose or there is nothing to choose.
    var needsResolve: Bool {
        switch state {
        case .ambiguous, .noMatch, .split, .twoSources: return true
        default: return selectedAudioKey == nil
        }
    }

    /// Resolve-banner title (§V2.6, plain language). Once a source is chosen the
    /// banner becomes the alternates picker, not a demand to choose.
    var resolveTitle: String {
        if selectedAudioKey != nil { return "Audio source — alternates available" }
        switch state {
        case .noMatch: return "No matching audio found"
        case .ambiguous: return "Choose the audio for this take"
        default: return "Audio source — alternates available"
        }
    }

    // ----- export naming (§V2.8) -----

    /// Default output filename stem (no global template — per-file naming, owner
    /// 2026-06-13): `<video-stem>_synced`. The extension is owned by the rail's
    /// format selector (§V2.8).
    var defaultStem: String {
        video.url.deletingPathExtension().lastPathComponent + "_synced"
    }

    /// Effective output stem: the in-place edit if any, else the default.
    var outputStem: String {
        if let s = stemOverride, !s.isEmpty { return s }
        return defaultStem
    }

    /// Output extension for a format (Auto resolves from the camera container).
    func ext(format: PairSession.ExportFormat) -> String {
        switch format {
        case .wav: return "wav"
        case .mov: return "mov"
        case .mp4: return "mp4"
        case .auto: return video.url.pathExtension.lowercased() == "mov" ? "mov" : "mp4"
        }
    }
}
