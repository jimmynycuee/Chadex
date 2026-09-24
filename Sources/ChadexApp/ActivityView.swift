import SwiftUI

struct ActivityView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Group {
            if model.filteredActivities.isEmpty {
                VStack(spacing: 10) {
                    Image(systemName: "clock.arrow.circlepath")
                        .chadexFont(.title, weight: .light)
                        .foregroundStyle(.tertiary)
                    Text(L10n.string("activity.empty"))
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List(model.filteredActivities) { entry in
                    ActivityRow(entry: entry)
                        .chadexPadding(.vertical, 4)
                }
                .listStyle(.inset)
            }
        }
        .navigationTitle(L10n.string("sidebar.activity"))
        .searchable(text: $model.activitySearch, placement: .toolbar, prompt: L10n.string("activity.search"))
        .toolbar {
            ToolbarItem(placement: .automatic) {
                Picker(L10n.string("activity.filter"), selection: $model.activityFilter) {
                    Text(L10n.string("activity.all")).tag(AppModel.ActivityFilter.all)
                    Text(L10n.string("activity.warningsErrors")).tag(AppModel.ActivityFilter.warningsAndErrors)
                }
                .pickerStyle(.menu)
                .controlSize(.small)
                .labelsHidden()
                .frame(width: 140)
            }
        }
    }
}
