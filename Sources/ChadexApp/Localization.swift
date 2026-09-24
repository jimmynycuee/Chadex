import Foundation

enum L10n {
    static func string(_ key: String, _ arguments: CVarArg...) -> String {
        let language = selectedLanguage
        let bundle = localizedBundle(for: language)
        let format = bundle.localizedString(forKey: key, value: key, table: nil)
        guard !arguments.isEmpty else { return format }
        return String(format: format, locale: language.locale, arguments: arguments)
    }

    private static var selectedLanguage: ChadexLanguage {
        let raw = UserDefaults.standard.string(forKey: ChadexPreferenceKey.language)
            ?? ChadexLanguage.system.rawValue
        return ChadexLanguage(rawValue: raw) ?? .system
    }

    private static func localizedBundle(for language: ChadexLanguage) -> Bundle {
        guard let code = language.localizationCode else {
            return Bundle.module
        }

        if let stringsURL = Bundle.module.url(
            forResource: "Localizable",
            withExtension: "strings",
            subdirectory: nil,
            localization: code
        ),
           let bundle = Bundle(url: stringsURL.deletingLastPathComponent()) {
            return bundle
        }

        let directURL = Bundle.module.bundleURL
            .appendingPathComponent("\(code).lproj", isDirectory: true)
        return Bundle(url: directURL) ?? Bundle.module
    }
}
