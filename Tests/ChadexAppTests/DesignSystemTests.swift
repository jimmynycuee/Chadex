import AppKit
import SwiftUI
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

    /// The symbol on a solid status fill is the non-color cue, so it must
    /// keep WCAG 1.4.11's 3:1 against that fill in both appearances.
    func testStatusGlyphsKeepNonTextContrast() {
        let fills: [Color] = [ChadexBrand.signal, .green, .orange, .red, .gray, .indigo]
        for name in [NSAppearance.Name.aqua, .darkAqua] {
            let appearance = NSAppearance(named: name)!
            for fill in fills {
                let ratio = contrast(ChadexBrand.glyph(on: fill), fill, in: appearance)
                XCTAssertGreaterThanOrEqual(ratio, 3, "\(fill) in \(name.rawValue): \(ratio)")
            }
        }
    }

    private func contrast(_ a: Color, _ b: Color, in appearance: NSAppearance) -> Double {
        var result = 0.0
        appearance.performAsCurrentDrawingAppearance {
            let la = luminance(NSColor(a)), lb = luminance(NSColor(b))
            result = (max(la, lb) + 0.05) / (min(la, lb) + 0.05)
        }
        return result
    }

    private func luminance(_ color: NSColor) -> Double {
        let c = color.usingColorSpace(.sRGB)!
        func channel(_ v: CGFloat) -> Double {
            let v = Double(v)
            return v <= 0.03928 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * channel(c.redComponent) + 0.7152 * channel(c.greenComponent) + 0.0722 * channel(c.blueComponent)
    }
}
