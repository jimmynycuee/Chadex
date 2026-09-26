import Foundation

enum L10n {
    private static let resourceBundleName = "Chadex_ChadexApp.bundle"

    static func string(_ key: String, _ arguments: CVarArg...) -> String {
        let language = selectedLanguage
        let bundle = localizedBundle(for: language)
        let format = bundle.localizedString(forKey: key, value: key, table: nil)
        guard !arguments.isEmpty else { return format }
        return String(format: format, locale: language.locale, arguments: arguments)
    }

    static func resourceBundle(mainBundle: Bundle = .main) -> Bundle? {
        var candidates: [URL] = []

#if DEBUG
        if let override = ProcessInfo.processInfo.environment["PACKAGE_RESOURCE_BUNDLE_PATH"]
            ?? ProcessInfo.processInfo.environment["PACKAGE_RESOURCE_BUNDLE_URL"],
           !override.isEmpty {
            candidates.append(URL(fileURLWithPath: override, isDirectory: true))
        }
#endif

        appendResourceCandidates(for: mainBundle, to: &candidates)

        let codeBundle = Bundle(for: ChadexResourceBundleLocator.self)
        if codeBundle.bundleURL != mainBundle.bundleURL {
            appendResourceCandidates(for: codeBundle, to: &candidates)
        }

        var visited = Set<String>()
        for candidate in candidates {
            let normalized = candidate.standardizedFileURL
            guard visited.insert(normalized.path).inserted else { continue }
            if let bundle = Bundle(url: normalized) {
                return bundle
            }
        }
        return nil
    }

    private static var selectedLanguage: ChadexLanguage {
        let raw = UserDefaults.standard.string(forKey: ChadexPreferenceKey.language)
            ?? ChadexLanguage.system.rawValue
        return ChadexLanguage(rawValue: raw) ?? .system
    }

    private static func localizedBundle(for language: ChadexLanguage) -> Bundle {
        guard let resources = resourceBundle() else {
            return .main
        }

        guard let code = language.localizationCode else {
            return resources
        }

        let names = code == "zh-Hant" ? [code, code.lowercased()] : [code]
        if let resourceURL = resources.resourceURL {
            for name in names {
                let localizationURL = resourceURL
                    .appendingPathComponent("\(name).lproj", isDirectory: true)
                if let localized = Bundle(url: localizationURL) {
                    return localized
                }
            }
        }

        return resources
    }

    private static func appendResourceCandidates(for bundle: Bundle, to candidates: inout [URL]) {
        func append(from root: URL?) {
            guard let root else { return }
            if root.lastPathComponent == resourceBundleName {
                candidates.append(root)
            } else {
                candidates.append(root.appendingPathComponent(resourceBundleName, isDirectory: true))
            }
        }

        append(from: bundle.resourceURL)
        append(from: bundle.bundleURL)
        append(from: bundle.bundleURL.appendingPathComponent("Contents/Resources", isDirectory: true))

        if let executableURL = bundle.executableURL {
            let executableDirectory = executableURL.deletingLastPathComponent()
            append(from: executableDirectory)
            append(from: executableDirectory
                .deletingLastPathComponent()
                .appendingPathComponent("Resources", isDirectory: true))
        }
    }
}

private final class ChadexResourceBundleLocator {}
