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
        guard let code = language.localizationCode,
              let path = Bundle.module.path(forResource: code, ofType: "lproj"),
              let bundle = Bundle(path: path)
        else {
            return Bundle.module
        }
        return bundle
    }
}
