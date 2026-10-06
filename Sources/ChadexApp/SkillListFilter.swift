import Foundation

/// Scope bar segments on the Skills page (HIG search-fields: scope bars).
enum SkillListScope: String, CaseIterable, Identifiable, Sendable {
    case all
    case enabled
    case external
    case installed

    var id: String { rawValue }

    var titleKey: String {
        switch self {
        case .all: return "skills.scope.all"
        case .enabled: return "skills.scope.enabled"
        case .external: return "skills.scope.external"
        case .installed: return "skills.scope.installed"
        }
    }
}

extension SkillCenterItem {
    /// Skills the user connected from an external folder (Codex, Claude Code, shared agents).
    var isExternalSkill: Bool { trust == "operator_configured_guidance" }
    /// Skills imported into Chadex's own managed store.
    var isInstalledSkill: Bool { managed != nil || trust == "operator_installed_guidance" }

    var sourceDisplayLabel: String {
        switch trust {
        case "project_content": return L10n.string("skills.projectSource")
        case "operator_configured_guidance": return L10n.string("skills.configuredSource")
        case "operator_installed_guidance": return L10n.string("skills.installedSource")
        default: return sourceScope
        }
    }
}

/// Pure search and scope filtering for the Skills list, kept free of SwiftUI so it can be tested.
enum SkillListFilter {
    static func matches(_ item: SkillCenterItem, scope: SkillListScope) -> Bool {
        switch scope {
        case .all: return true
        case .enabled: return item.isActive
        case .external: return item.isExternalSkill
        case .installed: return item.isInstalledSkill
        }
    }

    /// Every whitespace-separated term must appear in the name, description, key, or source label.
    /// Matching ignores case and diacritics; a blank query matches everything.
    static func matches(_ item: SkillCenterItem, query: String) -> Bool {
        let terms = query.split(whereSeparator: { $0.isWhitespace }).map(String.init)
        guard !terms.isEmpty else { return true }
        var fields = [item.name, item.description, item.skillId, item.sourceDisplayLabel]
        if let managed = item.managed { fields.append(managed.skillKey) }
        return terms.allSatisfy { term in
            fields.contains { field in
                field.range(of: term, options: [.caseInsensitive, .diacriticInsensitive]) != nil
            }
        }
    }

    static func filter(_ items: [SkillCenterItem], scope: SkillListScope, query: String) -> [SkillCenterItem] {
        items.filter { matches($0, scope: scope) && matches($0, query: query) }
    }
}
