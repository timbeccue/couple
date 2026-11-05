// Visual identity. The semantic color mapping is DECIDED (UX §1.3/§1.6:
// camera = teal, external = amber, fixed 4-hue anchor palette); exact values
// are implementation license (UX §5).

import SwiftUI

enum Theme {
    // Surfaces
    static let windowBg = Color(red: 0.075, green: 0.08, blue: 0.09)
    static let panelBg = Color(red: 0.105, green: 0.11, blue: 0.125)
    static let panelStroke = Color.white.opacity(0.07)
    static let canvasBg = Color(red: 0.058, green: 0.062, blue: 0.072)
    static let chipBg = Color.white.opacity(0.06)

    // Track identities (DECIDED semantics)
    static let camera = Color(red: 0.31, green: 0.85, blue: 0.77)   // teal
    static let external = Color(red: 0.94, green: 0.70, blue: 0.37) // amber

    // Anchor palette: 4 hues distinct from teal/amber (DECIDED count/semantics)
    static let anchorColors: [Color] = [
        Color(red: 0.65, green: 0.55, blue: 0.98), // violet
        Color(red: 0.98, green: 0.44, blue: 0.52), // rose
        Color(red: 0.38, green: 0.65, blue: 0.98), // sky
        Color(red: 0.64, green: 0.90, blue: 0.21), // lime
    ]

    // Status colors (Δ thresholds DECIDED, UX §1.6)
    static let good = Color(red: 0.35, green: 0.84, blue: 0.45)
    static let warn = Color(red: 0.95, green: 0.72, blue: 0.25)
    static let bad = Color(red: 0.96, green: 0.36, blue: 0.32)

    static let textPrimary = Color.white.opacity(0.92)
    static let textSecondary = Color.white.opacity(0.55)
    static let textTertiary = Color.white.opacity(0.32)

    static let accent = Color(red: 0.36, green: 0.62, blue: 0.98)

    // Verdict-model colors (UX-DECISIONS-V2 §V2.3): good / questionable / none.
    // Since §V2.12 queue rows carry no dot; these tint the action-word capsules
    // ("pick one" / "no audio" / "forced"), the candidate strength bar and the
    // editor's verdict mark. [TUNE] thresholds.
    static let verdictGood = Color(red: 0.28, green: 0.80, blue: 0.58)        // emerald
    static let verdictQuestionable = Color(red: 0.97, green: 0.40, blue: 0.42) // red
    static let verdictNone = Color.white.opacity(0.28)                        // hollow grey

    static func verdictColor(_ v: Verdict) -> Color {
        switch v {
        case .good: return verdictGood
        case .questionable: return verdictQuestionable
        case .none: return verdictNone
        }
    }

    static func deltaColor(absMs: Double) -> Color {
        if absMs < 0.7 { return good }
        if absMs < 3.0 { return warn }
        return bad
    }

    static func rmsColor(_ rms: Double) -> Color {
        if rms < 0.7 { return good }
        if rms < 3.0 { return warn }
        return bad
    }
}

extension View {
    func panel() -> some View {
        padding(12)
            .background(Theme.panelBg, in: RoundedRectangle(cornerRadius: 9))
            .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Theme.panelStroke))
    }
}

/// Time formatting helpers.
enum Fmt {
    static func mmss(_ seconds: Double) -> String {
        let s = max(0, Int(seconds.rounded()))
        return String(format: "%d:%02d", s / 60, s % 60)
    }

    static func hmmss(_ seconds: Double) -> String {
        let s = max(0, Int(seconds.rounded()))
        if s >= 3600 {
            return String(format: "%d:%02d:%02d", s / 3600, (s % 3600) / 60, s % 60)
        }
        return mmss(seconds)
    }

    /// "= −5m 21.355s" human-readable offset (UX §1.4, shown when |offset| ≥ 1 s).
    static func humanOffset(ms: Double) -> String {
        let sign = ms < 0 ? "−" : "+"
        let absS = abs(ms) / 1000
        let m = Int(absS / 60)
        let s = absS - Double(m) * 60
        if m > 0 {
            return String(format: "= %@%dm %.3fs", sign, m, s)
        }
        return String(format: "= %@%.3fs", sign, s)
    }

    /// "HH:mm" wall-clock time — the "✓ <time>" export stamp on queue rows and
    /// in the Files list (UX §V2.12 / D8).
    static func clock(_ date: Date) -> String { clockFormatter.string(from: date) }

    private static let clockFormatter: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "HH:mm"
        return f
    }()

    static func ms(_ v: Double, decimals: Int = 2) -> String {
        String(format: "%+.\(decimals)f", v)
    }
}
