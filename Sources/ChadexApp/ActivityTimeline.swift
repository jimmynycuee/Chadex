import AppKit
import SwiftUI

/// Activity as a vertical day timeline: time on the left, a colored event
/// node on one continuous line, the message on the right. The overview card
/// and the Activity page share it so history reads the same everywhere.
struct ActivityTimeline: View {
    @Environment(\.chadexLayout) private var layout
    let entries: [ActivityEntry]
    /// Day headers ("Today", "Yesterday", a date) above each group.
    var showsDayHeaders = true

    var body: some View {
        LazyVStack(alignment: .leading, spacing: 0) {
            ForEach(Array(days.enumerated()), id: \.element.day) { index, group in
                if showsDayHeaders {
                    dayHeader(group.day)
                        .chadexPadding(.top, index == 0 ? 0 : 14)
                        .chadexPadding(.bottom, 6)
                }
                ForEach(group.entries) { entry in
                    ActivityTimelineRow(
                        entry: entry,
                        isFirst: entry.id == group.entries.first?.id,
                        isLast: entry.id == group.entries.last?.id
                    )
                }
            }
        }
    }

    private var days: [(day: Date, entries: [ActivityEntry])] {
        let calendar = Calendar.current
        var result: [(day: Date, entries: [ActivityEntry])] = []
        for entry in entries {
            let day = calendar.startOfDay(for: entry.date)
            if let last = result.indices.last, result[last].day == day {
                result[last].entries.append(entry)
            } else {
                result.append((day, [entry]))
            }
        }
        return result
    }

    @ViewBuilder
    private func dayHeader(_ day: Date) -> some View {
        Group {
            if Calendar.current.isDateInToday(day) {
                Text(L10n.string("activity.today"))
            } else if Calendar.current.isDateInYesterday(day) {
                Text(L10n.string("activity.yesterday"))
            } else {
                Text(day, format: .dateTime.month(.abbreviated).day().weekday(.wide))
            }
        }
        .chadexFont(.callout, weight: .semibold)
        .foregroundStyle(.primary)
        .padding(.leading, ActivityTimelineRow.messageInset(layout))
        .accessibilityAddTraits(.isHeader)
    }
}

struct ActivityTimelineRow: View {
    @Environment(\.chadexLayout) private var layout
    let entry: ActivityEntry
    var isFirst = false
    var isLast = false

    static let nodeDiameter: CGFloat = 22
    static let timeWidth: CGFloat = 70
    static let columnSpacing: CGFloat = 12
    static let verticalPadding: CGFloat = 8

    static func messageInset(_ layout: ChadexLayoutMetrics) -> CGFloat {
        layout.control(timeWidth) + layout.spacing(columnSpacing) * 2 + layout.control(nodeDiameter)
    }

    var body: some View {
        let node = layout.control(Self.nodeDiameter)
        let padding = layout.spacing(Self.verticalPadding)

        HStack(alignment: .top, spacing: layout.spacing(Self.columnSpacing)) {
            Text(entry.date, format: .dateTime.hour().minute())
                .chadexFont(.caption)
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .minimumScaleFactor(0.85)
                .frame(width: layout.control(Self.timeWidth), alignment: .trailing)
                .frame(minHeight: node)
                .help(entry.date.formatted(date: .abbreviated, time: .standard))

            Image(systemName: ActivityPresentation.symbol(for: entry))
                .font(.system(size: layout.control(10), weight: .bold))
                .foregroundStyle(.white)
                .frame(width: node, height: node)
                .background(tint, in: Circle())
                .overlay(Circle().strokeBorder(ChadexBrand.cardFill, lineWidth: 2))
                .accessibilityHidden(true)

            Text(ActivityPresentation.message(for: entry))
                .chadexFont(.callout)
                .foregroundStyle(.primary)
                .fixedSize(horizontal: false, vertical: true)
                .frame(minHeight: node)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, padding)
        // The spine runs through the row's padding so consecutive nodes join
        // into one line; it stops at the first and last node of a day.
        .background(alignment: .topLeading) {
            GeometryReader { proxy in
                let x = layout.control(Self.timeWidth) + layout.spacing(Self.columnSpacing) + node / 2 - 1
                let center = padding + node / 2
                let top = isFirst ? center : 0
                let bottom = isLast ? center : proxy.size.height
                Rectangle()
                    .fill(Color.secondary.opacity(0.25))
                    .frame(width: 2, height: max(0, bottom - top))
                    .offset(x: x, y: top)
            }
            .accessibilityHidden(true)
        }
        .help("\(entry.source) · \(entry.eventKind)")
        .contentShape(Rectangle())
        .contextMenu {
            Button(L10n.string("activity.copyMessage")) {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(ActivityPresentation.message(for: entry), forType: .string)
            }
            Button(L10n.string("activity.copyDetails")) {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(
                    "\(entry.source) · \(entry.eventKind) · \(entry.date.formatted(date: .abbreviated, time: .standard))",
                    forType: .string
                )
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(ActivityPresentation.message(for: entry))
        .accessibilityValue(accessibilityValue)
    }

    /// Color says what kind of event it was; the symbol and the spoken level
    /// carry the same information without color.
    private var tint: Color {
        switch entry.level {
        case .info:
            return entry.eventKind == "local_runtime_ready" ? .green : ChadexBrand.signal
        case .warning: return .orange
        case .error: return .red
        }
    }

    private var accessibilityValue: String {
        let time = entry.date.formatted(date: .omitted, time: .shortened)
        switch entry.level {
        case .info: return time
        case .warning: return "\(L10n.string("activity.level.warning")), \(time)"
        case .error: return "\(L10n.string("activity.level.error")), \(time)"
        }
    }
}
