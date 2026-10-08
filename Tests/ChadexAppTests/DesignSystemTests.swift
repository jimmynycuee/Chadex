import XCTest
@testable import ChadexApp

final class DesignSystemTests: XCTestCase {
    func testTypeScaleNeverDropsBelowMacOSMinimum() {
        let styles: [ChadexFontStyle] = [
            .largeTitle, .title, .title2, .title3, .headline, .body,
            .callout, .subheadline, .footnote, .caption, .caption2
        ]
        for style in styles {
            // HIG › Typography: macOS minimum text size is 10 pt.
            XCTAssertGreaterThanOrEqual(style.baseSize, 10, "\(style) is below the macOS minimum")
        }
        // The size the font modifier renders is clamped at every interface
        // size, including compact (80%), and still scales up above it.
        for size in ChadexInterfaceSize.allCases {
            let scale = ChadexLayoutMetrics(interfaceSize: size).fontScale
            for style in styles {
                XCTAssertGreaterThanOrEqual(style.size(at: scale), 10, "\(style) at \(size)")
            }
        }
        XCTAssertEqual(ChadexFontStyle.caption2.size(at: 0.8), 10)
        XCTAssertEqual(ChadexFontStyle.body.size(at: 1.4), 13 * 1.4, accuracy: 0.001)
        XCTAssertEqual(ChadexFontStyle.body.baseSize, 13, "macOS default body size is 13 pt")
    }

    func testSettingsReopensLastUsedPane() throws {
        let suite = "chadex.tests.settings.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }

        XCTAssertEqual(SettingsView.openingTab(nil, defaults: defaults), .general)
        defaults.set(SettingsTab.advanced.rawValue, forKey: SettingsView.lastTabKey)
        XCTAssertEqual(SettingsView.openingTab(nil, defaults: defaults), .advanced)
        XCTAssertEqual(SettingsView.openingTab(.connection, defaults: defaults), .connection)
        defaults.set("unknown", forKey: SettingsView.lastTabKey)
        XCTAssertEqual(SettingsView.openingTab(nil, defaults: defaults), .general)
    }

    func testEveryLocalizationKeyExistsInBothLanguages() throws {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/ChadexApp/Resources")
        func keys(_ language: String) throws -> Set<String> {
            let url = root.appendingPathComponent("\(language).lproj/Localizable.strings")
            let dictionary = try XCTUnwrap(NSDictionary(contentsOf: url) as? [String: String])
            return Set(dictionary.keys)
        }
        let english = try keys("en")
        let chinese = try keys("zh-Hant")
        XCTAssertEqual(english.subtracting(chinese), [], "keys missing from zh-Hant")
        XCTAssertEqual(chinese.subtracting(english), [], "keys missing from en")
    }

    func testChineseCopyHasNoStraySpacesBetweenHanCharacters() throws {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/ChadexApp/Resources/zh-Hant.lproj/Localizable.strings")
        let strings = try XCTUnwrap(NSDictionary(contentsOf: url) as? [String: String])
        let pattern = try NSRegularExpression(pattern: "\\p{Han} \\p{Han}")
        for (key, value) in strings {
            let range = NSRange(value.startIndex..., in: value)
            XCTAssertNil(pattern.firstMatch(in: value, range: range), "stray space in \(key): \(value)")
        }
    }
}
