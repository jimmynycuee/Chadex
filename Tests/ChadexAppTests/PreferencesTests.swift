import XCTest
@testable import ChadexApp

final class PreferencesTests: XCTestCase {
    func testRegisteredProjectsRoundTrip() throws {
        let project = ProjectRecord(name: "測試 Project", path: "/tmp/測試 Project")
        let preferences = ChadexPreferences(projects: [project], selectedProjectID: project.id)
        let data = try JSONEncoder().encode(preferences)
        let decoded = try JSONDecoder().decode(ChadexPreferences.self, from: data)
        XCTAssertEqual(decoded.projects, [project])
        XCTAssertEqual(decoded.selectedProjectID, project.id)
    }

    func testRestorePreferencesAreIndependent() {
        var preferences = ChadexPreferences()
        preferences.restoreServiceOnLaunch = true
        preferences.restoreConnectionOnLaunch = false
        XCTAssertTrue(preferences.restoreServiceOnLaunch)
        XCTAssertFalse(preferences.restoreConnectionOnLaunch)
    }

    func testProjectStoreSupportsIsolatedPreferencesDirectory() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let store = ProjectStore(environment: ["CHADEX_PREFERENCES_DIR": root.path])
        let project = ProjectRecord(name: "Isolated", path: "/tmp/isolated")
        try store.save(ChadexPreferences(projects: [project], selectedProjectID: project.id))

        let loaded = store.load()
        XCTAssertEqual(loaded.projects, [project])
        XCTAssertEqual(loaded.selectedProjectID, project.id)
        XCTAssertTrue(FileManager.default.fileExists(atPath: root.appendingPathComponent("preferences.json").path))
    }

    func testInterfaceSizeZoomOrdering() {
        XCTAssertEqual(ChadexInterfaceSize.zoomedIn(from: .comfortable), .large)
        XCTAssertEqual(ChadexInterfaceSize.zoomedOut(from: .comfortable), .standard)
        XCTAssertEqual(ChadexInterfaceSize.zoomedOut(from: .compact), .compact)
        XCTAssertEqual(ChadexInterfaceSize.zoomedIn(from: .extraLarge), .extraLarge)
        XCTAssertEqual(ChadexInterfaceSize.compact.percent, 80)
        XCTAssertEqual(ChadexInterfaceSize.standard.percent, 100)
        XCTAssertEqual(ChadexInterfaceSize.comfortable.percent, 120)
        XCTAssertEqual(ChadexInterfaceSize.large.percent, 140)
        XCTAssertEqual(ChadexInterfaceSize.extraLarge.percent, 160)
        XCTAssertEqual(ChadexInterfaceSize.compact.scaleFactor, 0.80, accuracy: 0.0001)
        XCTAssertEqual(ChadexInterfaceSize.standard.scaleFactor, 1.00, accuracy: 0.0001)
        XCTAssertEqual(ChadexInterfaceSize.comfortable.scaleFactor, 1.20, accuracy: 0.0001)
        XCTAssertEqual(ChadexInterfaceSize.large.scaleFactor, 1.40, accuracy: 0.0001)
        XCTAssertEqual(ChadexInterfaceSize.extraLarge.scaleFactor, 1.60, accuracy: 0.0001)
        XCTAssertEqual(ChadexInterfaceSize.large.rawValue, "140")
        XCTAssertEqual(ChadexInterfaceSize.resolve(storedValue: "standard"), .standard)
        XCTAssertEqual(ChadexInterfaceSize.resolve(storedValue: "extraLarge"), .large)
        XCTAssertEqual(ChadexInterfaceSize.resolve(storedValue: "large"), .comfortable)
        XCTAssertEqual(ChadexInterfaceSize.resolve(storedValue: "140"), .large)

        let base: CGFloat = 13
        let scaledValues = ChadexInterfaceSize.allCases.map { $0.scaled(base) }
        XCTAssertEqual(Set(scaledValues.map { Int(($0 * 1_000).rounded()) }).count, 5)
        XCTAssertEqual(ChadexInterfaceSize.standard.scaled(base), base, accuracy: 0.0001)
        XCTAssertGreaterThan(
            ChadexInterfaceSize.comfortable.scaled(base),
            ChadexInterfaceSize.standard.scaled(base)
        )
        XCTAssertGreaterThan(
            ChadexInterfaceSize.extraLarge.scaled(base),
            ChadexInterfaceSize.large.scaled(base)
        )
        XCTAssertEqual(ChadexFontStyle.body.baseSize, 13, accuracy: 0.0001)

        let extraLargeLayout = ChadexLayoutMetrics(interfaceSize: .extraLarge)
        XCTAssertEqual(extraLargeLayout.fontScale, 1.60, accuracy: 0.0001)
        XCTAssertGreaterThan(extraLargeLayout.spacingScale, 1.0)
        XCTAssertGreaterThan(extraLargeLayout.controlScale, 1.0)
        XCTAssertGreaterThan(extraLargeLayout.sidebarScale, 1.0)
        XCTAssertLessThan(extraLargeLayout.spacingScale, extraLargeLayout.fontScale)
        XCTAssertLessThan(extraLargeLayout.controlScale, extraLargeLayout.fontScale)
        XCTAssertLessThan(extraLargeLayout.sidebarScale, extraLargeLayout.fontScale)
    }

    func testActivityPresentationLocalizesKnownTechnicalEvents() {
        let defaults = UserDefaults.standard
        let previous = defaults.object(forKey: ChadexPreferenceKey.language)
        defer {
            if let previous {
                defaults.set(previous, forKey: ChadexPreferenceKey.language)
            } else {
                defaults.removeObject(forKey: ChadexPreferenceKey.language)
            }
        }

        defaults.set(ChadexLanguage.traditionalChinese.rawValue, forKey: ChadexPreferenceKey.language)
        let entry = ActivityEntry(
            sequence: 1,
            timestampMs: 0,
            source: "runner",
            level: .info,
            eventKind: "process_started",
            message: "Desktop started the process (PID 22374)"
        )
        XCTAssertEqual(ActivityPresentation.message(for: entry), "Chadex 已啟動本機程序（PID 22374）")
    }

    func testExplicitLocalizationOverride() {
        let defaults = UserDefaults.standard
        let previous = defaults.object(forKey: ChadexPreferenceKey.language)
        defer {
            if let previous {
                defaults.set(previous, forKey: ChadexPreferenceKey.language)
            } else {
                defaults.removeObject(forKey: ChadexPreferenceKey.language)
            }
        }

        defaults.set(ChadexLanguage.english.rawValue, forKey: ChadexPreferenceKey.language)
        XCTAssertEqual(L10n.string("settings.general"), "General")

        defaults.set(ChadexLanguage.traditionalChinese.rawValue, forKey: ChadexPreferenceKey.language)
        XCTAssertEqual(L10n.string("settings.general"), "一般")
    }

    func testLocalizationResourcesResolveInSwiftPMTestRuntime() throws {
        let bundle = try XCTUnwrap(L10n.resourceBundle())
        let resourceURL = try XCTUnwrap(bundle.resourceURL)

        XCTAssertTrue(
            FileManager.default.fileExists(
                atPath: resourceURL
                    .appendingPathComponent("en.lproj", isDirectory: true)
                    .appendingPathComponent("Localizable.strings")
                    .path
            )
        )
        XCTAssertEqual(
            ChadexStartupPreflight.resourceExitStatus(
                arguments: ["Chadex", ChadexStartupPreflight.resourceArgument]
            ),
            0
        )
    }
}

