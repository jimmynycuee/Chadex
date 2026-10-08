import SwiftUI

struct ActivityView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Group {
            if model.filteredActivities.isEmpty {
                VStack(spacing: 10) {
                    Image(systemName: isFiltering ? "magnifyingglass" : "clock.arrow.circlepath")
                        .chadexFont(.title, weight: .light)
                        .foregroundStyle(.tertiary)
                        .accessibilityHidden(true)
                    Text(emptyMessage)
                        .chadexFont(.body, weight: .medium)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                    if isFiltering {
                        Button(L10n.string("activity.clearFilters")) {
                            model.activitySearch = ""
                            model.activityFilter = .all
                        }
                        .controlSize(.regular)
                        .padding(.top, 4)
                    } else {
                        Text(L10n.string("activity.emptyHint"))
                            .chadexFont(.callout)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                            .frame(maxWidth: 360)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ChadexPageColumn {
                    ActivityTimeline(entries: model.filteredActivities)
                        .chadexCard(padding: 18)
                }
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

    private var isFiltering: Bool {
        !model.activitySearch.trimmingCharacters(in: .whitespaces).isEmpty
            || model.activityFilter != .all
    }

    /// A filtered-out list says so, instead of claiming there is no history.
    private var emptyMessage: String {
        let query = model.activitySearch.trimmingCharacters(in: .whitespaces)
        if !query.isEmpty { return L10n.string("activity.noResults", query) }
        if model.activityFilter != .all { return L10n.string("activity.noFilteredResults") }
        return L10n.string("activity.empty")
    }
}
