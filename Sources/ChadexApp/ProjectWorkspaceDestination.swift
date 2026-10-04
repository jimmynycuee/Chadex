import Foundation

enum ProjectWorkspaceDestination: String, CaseIterable, Hashable, Identifiable {
    case overview
    case computer
    case instructions
    case skills
    case memory

    var id: String { rawValue }

    var titleKey: String {
        switch self {
        case .overview: return "sidebar.overview"
        case .computer: return "sidebar.computer"
        case .instructions: return "sidebar.instructions"
        case .skills: return "sidebar.skills"
        case .memory: return "sidebar.memory"
        }
    }

    var systemImage: String {
        switch self {
        case .overview: return "rectangle.grid.1x2"
        case .computer: return "desktopcomputer"
        case .instructions: return "doc.text"
        case .skills: return "wrench.and.screwdriver"
        case .memory: return "memorychip"
        }
    }

    var persistedValue: String {
        "project.\(rawValue)"
    }

    init?(persistedValue: String) {
        guard persistedValue.hasPrefix("project.") else { return nil }
        self.init(rawValue: String(persistedValue.dropFirst("project.".count)))
    }
}
