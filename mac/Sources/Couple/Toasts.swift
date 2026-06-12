// Toasts (UX §2.5/§3): transient, top-right, optionally carrying one action
// (e.g. "Restore", "Reveal"). Styling is implementation license (UX §5).

import Observation
import SwiftUI

struct Toast: Identifiable {
    enum Style {
        case info, success, warning, error
    }

    let id = UUID()
    var style: Style
    var title: String
    var detail: String?
    var actionLabel: String?
    var action: (@MainActor () -> Void)?

    var color: Color {
        switch style {
        case .info: return Theme.accent
        case .success: return Theme.good
        case .warning: return Theme.warn
        case .error: return Theme.bad
        }
    }
}

@MainActor
@Observable
final class ToastCenter {
    private(set) var toasts: [Toast] = []

    func show(_ toast: Toast, seconds: Double = 6) {
        toasts.append(toast)
        let id = toast.id
        Task {
            try? await Task.sleep(nanoseconds: UInt64(seconds * 1e9))
            self.dismiss(id)
        }
    }

    func dismiss(_ id: UUID) {
        toasts.removeAll { $0.id == id }
    }
}

struct ToastOverlay: View {
    let center: ToastCenter

    var body: some View {
        VStack(alignment: .trailing, spacing: 8) {
            ForEach(center.toasts) { toast in
                ToastView(toast: toast, center: center)
            }
            Spacer()
        }
        .padding(.top, 34)
        .padding(.trailing, 14)
        .frame(maxWidth: .infinity, alignment: .trailing)
        .allowsHitTesting(!center.toasts.isEmpty)
    }
}

private struct ToastView: View {
    let toast: Toast
    let center: ToastCenter

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            RoundedRectangle(cornerRadius: 2)
                .fill(toast.color)
                .frame(width: 3, height: 26)
            VStack(alignment: .leading, spacing: 2) {
                Text(toast.title)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(Theme.textPrimary)
                if let detail = toast.detail {
                    Text(detail)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(Theme.textSecondary)
                }
            }
            if let label = toast.actionLabel, let action = toast.action {
                Button(label) {
                    action()
                    center.dismiss(toast.id)
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
            }
            Button {
                center.dismiss(toast.id)
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 8))
                    .foregroundStyle(Theme.textTertiary)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 8))
        .background(Theme.panelBg.opacity(0.9), in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(toast.color.opacity(0.35)))
        .frame(maxWidth: 460, alignment: .trailing)
        .transition(.move(edge: .top).combined(with: .opacity))
    }
}
