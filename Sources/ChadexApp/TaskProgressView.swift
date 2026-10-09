import SwiftUI

struct TaskProgressView: View {
    let task: TaskProgressSnapshot
    let onCancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                SectionTitle(title: L10n.string("task.title"))
                Spacer(minLength: 12)
                ChadexStatusLabel(
                    title: statusText,
                    systemImage: statusSymbol,
                    tint: statusColor,
                    emphasized: statusColor != .secondary
                )
            }

            Text(task.goal)
                .chadexFont(.callout, weight: .medium)
                .fixedSize(horizontal: false, vertical: true)

            TimelineView(.periodic(from: .now, by: 1)) { context in
                HStack(spacing: 18) {
                    TaskMetadataLabel(
                        title: L10n.string("task.elapsed"),
                        value: elapsedText(at: context.date)
                    )
                    TaskMetadataLabel(
                        title: L10n.string("task.validation"),
                        value: validationText
                    )
                    TaskMetadataLabel(
                        title: L10n.string("task.filesChanged"),
                        value: "\(task.review.changedFileCount ?? 0)"
                    )
                    Spacer(minLength: 8)

                    if task.isActive {
                        Button(role: .destructive, action: onCancel) {
                            Text(task.status == "cancelling"
                                ? L10n.string("task.status.cancelling")
                                : L10n.string("common.cancel"))
                        }
                        .buttonStyle(.bordered)
                        .chadexControlSize(.small)
                        .disabled(!task.canCancel)
                    }
                }
            }

            HStack(alignment: .top, spacing: 16) {
                ForEach(TaskPhase.allCases) { phase in
                    TaskPhaseView(
                        title: phase.title,
                        state: state(for: phase)
                    )
                }
            }
            .accessibilityElement(children: .contain)
        }
    }

    private func elapsedText(at date: Date) -> String {
        if let durationMs = task.durationMs {
            return formatDuration(durationMs)
        }
        let nowMs = UInt64(date.timeIntervalSince1970 * 1_000)
        return formatDuration(nowMs.saturatingSubtracting(task.startedAtMs))
    }

    private var validationText: String {
        switch task.validation.status {
        case "passed":
            return L10n.string("task.validation.passed", task.validation.checksPassed)
        case "failed":
            return L10n.string("task.validation.failed", task.validation.checksFailed)
        default:
            return L10n.string("task.validation.notRun")
        }
    }

    private var statusText: String {
        L10n.string("task.status.\(task.status)")
    }

    private var statusSymbol: String {
        switch task.status {
        case "completed": return "checkmark.circle.fill"
        case "failed", "failed_validation", "blocked": return "xmark.circle.fill"
        case "interrupted": return "arrow.clockwise.circle.fill"
        case "unknown": return "questionmark.circle.fill"
        case "cancelled": return "stop.circle.fill"
        case "running", "queued", "cancelling": return "clock"
        default: return "circle"
        }
    }

    private var statusColor: Color {
        switch task.status {
        case "completed": return .green
        case "failed", "failed_validation", "blocked": return .red
        case "interrupted", "unknown": return .orange
        default: return .secondary
        }
    }

    private func state(for phase: TaskPhase) -> TaskPhaseState {
        let plannedIndices = task.plan.enumerated().compactMap { index, kind in
            phase.matches(kind) ? index : nil
        }
        guard !plannedIndices.isEmpty else { return .pending }

        let summaries = task.steps.filter { plannedIndices.contains($0.index) }
        if summaries.contains(where: { $0.status == "failed" || $0.status == "blocked" }) {
            return .failed
        }
        if task.isActive && plannedIndices.contains(task.currentStep) {
            return .running
        }
        if (task.status == "failed" || task.status == "failed_validation" || task.status == "blocked")
            && plannedIndices.contains(task.currentStep)
        {
            return .failed
        }
        let completed = summaries.filter { $0.status == "completed" }.count
        return completed == plannedIndices.count ? .completed : .pending
    }

    private func formatDuration(_ milliseconds: UInt64) -> String {
        let duration = Duration.milliseconds(Int64(clamping: milliseconds))
        let locale = L10n.selectedLanguage.locale
        if milliseconds < 60_000 {
            return duration.formatted(
                .units(allowed: [.seconds], width: .narrow, fractionalPart: .show(length: 1))
                    .locale(locale)
            )
        }
        return duration.formatted(
            .units(allowed: [.hours, .minutes, .seconds], width: .narrow).locale(locale)
        )
    }
}

private struct TaskMetadataLabel: View {
    let title: String
    let value: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 5) {
            Text(title)
                .foregroundStyle(.secondary)
            Text(value)
                .foregroundStyle(.primary)
                .monospacedDigit()
        }
        .chadexFont(.caption)
        .accessibilityElement(children: .combine)
    }
}

private enum TaskPhase: String, CaseIterable, Identifiable {
    case search
    case read
    case edit
    case validate
    case review

    var id: String { rawValue }

    var title: String {
        L10n.string("task.phase.\(rawValue)")
    }

    func matches(_ kind: String) -> Bool {
        switch self {
        case .validate:
            return kind == "validate" || kind == "run_process"
        default:
            return kind == rawValue
        }
    }
}

private enum TaskPhaseState {
    case pending
    case running
    case completed
    case failed
}

private struct TaskPhaseView: View {
    @Environment(\.chadexLayout) private var layout
    let title: String
    let state: TaskPhaseState

    var body: some View {
        HStack(spacing: 6) {
            if state == .running {
                ProgressView()
                    .chadexControlSize(.mini)
                    .frame(width: layout.control(12), height: layout.control(12))
            } else {
                Image(systemName: symbol)
                    .chadexFont(.caption2, weight: .semibold)
                    .foregroundStyle(color)
                    .frame(width: layout.control(12))
            }
            // Pending reads as secondary (not tertiary, 1.9:1); the symbol and
            // weight carry the state difference.
            Text(title)
                .chadexFont(.caption, weight: state == .running || state == .failed ? .semibold : .regular)
                .foregroundStyle(state == .running || state == .failed ? Color.primary : Color.secondary)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title)
        .accessibilityValue(accessibilityState)
    }

    private var accessibilityState: String {
        switch state {
        case .pending: return L10n.string("a11y.step.inactive")
        case .running: return L10n.string("a11y.step.active")
        case .completed: return L10n.string("a11y.step.complete")
        case .failed: return L10n.string("a11y.step.error")
        }
    }

    private var symbol: String {
        switch state {
        case .pending: return "circle"
        case .running: return "circle"
        case .completed: return "checkmark.circle.fill"
        case .failed: return "xmark.circle.fill"
        }
    }

    private var color: Color {
        switch state {
        case .pending: return .secondary
        case .running: return .secondary
        case .completed: return .green
        case .failed: return .red
        }
    }
}

private extension UInt64 {
    func saturatingSubtracting(_ other: UInt64) -> UInt64 {
        self >= other ? self - other : 0
    }
}
