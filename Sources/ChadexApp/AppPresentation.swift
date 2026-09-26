import SwiftUI

enum ChadexPreferenceKey {
    static let language = "appearance.language"
    static let interfaceSize = "appearance.interfaceSize"
    static let appearance = "appearance.colorScheme"
    static let guideOpenAIProjectConfirmed = "guide.openAIProjectConfirmed"
    static let guideChatGPTPluginConfirmed = "guide.chatGPTPluginConfirmed"
    static let autoCheckUpdates = "updates.autoCheck"
    static let lastUpdateCheckAt = "updates.lastCheckAt"
}

enum ChadexLanguage: String, CaseIterable, Identifiable {
    case system
    case traditionalChinese = "zh-Hant"
    case english = "en"

    var id: String { rawValue }

    var localizationCode: String? {
        switch self {
        case .system: return nil
        case .traditionalChinese: return "zh-Hant"
        case .english: return "en"
        }
    }

    var locale: Locale {
        localizationCode.map(Locale.init(identifier:)) ?? .current
    }
}

enum ChadexInterfaceSize: String, CaseIterable, Identifiable {
    case compact = "80"
    case standard = "100"
    case comfortable = "120"
    case large = "140"
    case extraLarge = "160"

    var id: String { rawValue }

    var percent: Int {
        switch self {
        case .compact: return 80
        case .standard: return 100
        case .comfortable: return 120
        case .large: return 140
        case .extraLarge: return 160
        }
    }

    var scaleFactor: CGFloat {
        CGFloat(percent) / 100
    }

    func scaled(_ value: CGFloat) -> CGFloat {
        value * scaleFactor
    }

    static func resolve(storedValue: String?) -> ChadexInterfaceSize {
        guard let storedValue else { return .comfortable }
        if let size = ChadexInterfaceSize(rawValue: storedValue) {
            return size
        }

        // Legacy builds stored semantic names instead of percentages.
        // Map those values to the nearest new step while preserving 140% exactly.
        switch storedValue {
        case "compact": return .standard
        case "standard": return .standard
        case "comfortable": return .comfortable
        case "large": return .comfortable
        case "extraLarge": return .large
        default: return .comfortable
        }
    }

    static func zoomedIn(from value: ChadexInterfaceSize) -> ChadexInterfaceSize {
        let values = allCases
        guard let index = values.firstIndex(of: value) else { return .comfortable }
        return values[min(index + 1, values.count - 1)]
    }

    static func zoomedOut(from value: ChadexInterfaceSize) -> ChadexInterfaceSize {
        let values = allCases
        guard let index = values.firstIndex(of: value) else { return .comfortable }
        return values[max(index - 1, 0)]
    }
}

struct ChadexLayoutMetrics {
    let interfaceSize: ChadexInterfaceSize

    var fontScale: CGFloat { interfaceSize.scaleFactor }

    // Reading size and layout density are related, but should not grow at the
    // same rate. This keeps larger type crisp without turning whitespace and
    // window chrome into a 140% geometric zoom.
    var spacingScale: CGFloat { 1 + (fontScale - 1) * 0.55 }
    var controlScale: CGFloat { 1 + (fontScale - 1) * 0.35 }
    var sidebarScale: CGFloat { 1 + (fontScale - 1) * 0.45 }

    func spacing(_ value: CGFloat) -> CGFloat { value * spacingScale }
    func control(_ value: CGFloat) -> CGFloat { value * controlScale }
    func sidebar(_ value: CGFloat) -> CGFloat { value * sidebarScale }

    func windowSize(base: CGSize) -> CGSize {
        CGSize(width: control(base.width), height: control(base.height))
    }
}

enum ChadexAppearance: String, CaseIterable, Identifiable {
    case system
    case light
    case dark

    var id: String { rawValue }

    var colorScheme: ColorScheme? {
        switch self {
        case .system: return nil
        case .light: return .light
        case .dark: return .dark
        }
    }
}

struct ChadexPresentationModifier: ViewModifier {
    let language: ChadexLanguage
    let interfaceSize: ChadexInterfaceSize
    let appearance: ChadexAppearance

    func body(content: Content) -> some View {
        let layout = ChadexLayoutMetrics(interfaceSize: interfaceSize)
        return content
            .environment(\.locale, language.locale)
            .environment(\.chadexUIScale, interfaceSize.scaleFactor)
            .environment(\.chadexLayout, layout)
            .environment(
                \.font,
                .system(size: ChadexFontStyle.body.baseSize * layout.fontScale)
            )
            .preferredColorScheme(appearance.colorScheme)
    }
}

private struct ChadexLayoutKey: EnvironmentKey {
    static let defaultValue = ChadexLayoutMetrics(interfaceSize: .standard)
}

private struct ChadexUIScaleKey: EnvironmentKey {
    static let defaultValue: CGFloat = 1
}

extension EnvironmentValues {
    var chadexUIScale: CGFloat {
        get { self[ChadexUIScaleKey.self] }
        set { self[ChadexUIScaleKey.self] = newValue }
    }
}

extension EnvironmentValues {
    var chadexLayout: ChadexLayoutMetrics {
        get { self[ChadexLayoutKey.self] }
        set { self[ChadexLayoutKey.self] = newValue }
    }
}

struct ChadexWindowZoomModifier: ViewModifier {
    let interfaceSize: ChadexInterfaceSize

    func body(content: Content) -> some View {
        let layout = ChadexLayoutMetrics(interfaceSize: interfaceSize)
        return content
            .environment(\.chadexUIScale, interfaceSize.scaleFactor)
            .environment(\.chadexLayout, layout)
    }
}

struct ChadexFixedWindowZoomModifier: ViewModifier {
    let interfaceSize: ChadexInterfaceSize
    let baseSize: CGSize

    func body(content: Content) -> some View {
        let layout = ChadexLayoutMetrics(interfaceSize: interfaceSize)
        let windowSize = layout.windowSize(base: baseSize)
        return content
            .frame(width: windowSize.width, height: windowSize.height)
            .environment(\.chadexUIScale, interfaceSize.scaleFactor)
            .environment(\.chadexLayout, layout)
    }
}

extension View {
    func chadexWindowZoom(_ interfaceSize: ChadexInterfaceSize) -> some View {
        modifier(ChadexWindowZoomModifier(interfaceSize: interfaceSize))
    }

    func chadexFixedWindowZoom(
        _ interfaceSize: ChadexInterfaceSize,
        baseSize: CGSize
    ) -> some View {
        modifier(
            ChadexFixedWindowZoomModifier(
                interfaceSize: interfaceSize,
                baseSize: baseSize
            )
        )
    }

    func chadexPresentation(
        language: ChadexLanguage,
        interfaceSize: ChadexInterfaceSize,
        appearance: ChadexAppearance
    ) -> some View {
        modifier(
            ChadexPresentationModifier(
                language: language,
                interfaceSize: interfaceSize,
                appearance: appearance
            )
        )
    }
}
