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

    func testLegacyPreferencesDecodeWithoutComputerControlDefault() throws {
        let legacy = #"{"projects":[],"tunnelID":"","restoreServiceOnLaunch":false,"restoreConnectionOnLaunch":false,"backgroundCloseHintShown":false}"#.data(using: .utf8)!
        let decoded = try JSONDecoder().decode(ChadexPreferences.self, from: legacy)
        XCTAssertNil(decoded.computerControlDefaultMode)
    }

    func testComputerControlAlwaysAllowPersistsAsPreference() throws {
        var preferences = ChadexPreferences()
        preferences.computerControlDefaultMode = .alwaysAllow
        let data = try JSONEncoder().encode(preferences)
        let decoded = try JSONDecoder().decode(ChadexPreferences.self, from: data)
        XCTAssertEqual(decoded.computerControlDefaultMode, .alwaysAllow)
    }

    func testPrepareServiceAndRestoreConnectionPreferencesAreIndependent() {
        var preferences = ChadexPreferences()
        preferences.prepareServiceOnLaunch = true
        preferences.restoreConnectionOnLaunch = false
        XCTAssertTrue(preferences.prepareServiceOnLaunchEnabled)
        XCTAssertFalse(preferences.restoreConnectionOnLaunch)
        preferences.prepareServiceOnLaunch = false
        preferences.restoreConnectionOnLaunch = true
        XCTAssertFalse(preferences.prepareServiceOnLaunchEnabled)
        XCTAssertTrue(preferences.restoreConnectionOnLaunch)
    }

    func testPrepareServiceDefaultsOnForNewAndLegacyPreferences() throws {
        XCTAssertTrue(ChadexPreferences().prepareServiceOnLaunchEnabled)
        // Existing users carry the old toggle, stored false because it was
        // never touched. It must not carry over and disable the new default.
        for legacyValue in ["false", "true"] {
            let legacy = #"{"projects":[],"tunnelID":"","restoreServiceOnLaunch":\#(legacyValue),"restoreConnectionOnLaunch":false,"backgroundCloseHintShown":false}"#
                .data(using: .utf8)!
            let decoded = try JSONDecoder().decode(ChadexPreferences.self, from: legacy)
            XCTAssertNil(decoded.prepareServiceOnLaunch)
            XCTAssertTrue(decoded.prepareServiceOnLaunchEnabled)
        }
    }

    func testCursorOverlayDefaultsOnWhenNeverChosen() throws {
        XCTAssertNil(ChadexPreferences().computerCursorOverlay)
        XCTAssertTrue(ChadexPreferences().computerCursorOverlayEnabled)
        XCTAssertTrue(ChadexPreferences.cursorOverlayEnabled(for: nil))
        XCTAssertTrue(ChadexPreferences.cursorOverlayEnabled(for: true))
        XCTAssertFalse(ChadexPreferences.cursorOverlayEnabled(for: false))
        // Preferences written before the option existed decode as "never chosen".
        let legacy = #"{"projects":[],"tunnelID":"","restoreConnectionOnLaunch":false,"backgroundCloseHintShown":false}"#
            .data(using: .utf8)!
        let decoded = try JSONDecoder().decode(ChadexPreferences.self, from: legacy)
        XCTAssertNil(decoded.computerCursorOverlay)
        XCTAssertTrue(decoded.computerCursorOverlayEnabled)
    }

    func testCursorOverlayChoicePersistsAndSurvivesUnrelatedSaves() throws {
        var preferences = ChadexPreferences()
        preferences.computerCursorOverlay = false
        let decoded = try JSONDecoder().decode(
            ChadexPreferences.self,
            from: JSONEncoder().encode(preferences)
        )
        XCTAssertEqual(decoded.computerCursorOverlay, false)
        XCTAssertFalse(decoded.computerCursorOverlayEnabled)

        // Through the real store, in an isolated directory.
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = ProjectStore(environment: ["CHADEX_PREFERENCES_DIR": root.path])
        try store.save(preferences)
        var loaded = store.load()
        XCTAssertEqual(loaded.computerCursorOverlay, false)
        loaded.backgroundCloseHintShown = true
        try store.save(loaded)
        XCTAssertEqual(store.load().computerCursorOverlay, false)
        XCTAssertTrue(FileManager.default.fileExists(atPath: root.appendingPathComponent("preferences.json").path))
    }

    func testPrepareServiceChoicePersists() throws {
        var preferences = ChadexPreferences()
        preferences.prepareServiceOnLaunch = false
        let decoded = try JSONDecoder().decode(
            ChadexPreferences.self,
            from: JSONEncoder().encode(preferences)
        )
        XCTAssertEqual(decoded.prepareServiceOnLaunch, false)
        XCTAssertFalse(decoded.prepareServiceOnLaunchEnabled)
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

    func testGlobalInstructionsAreAppOwnedAndIndependentOfSelectedProject() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let store = ProjectStore(environment: ["CHADEX_PREFERENCES_DIR": root.path])
        XCTAssertEqual(try store.loadGlobalInstructions(), "")
        try store.saveGlobalInstructions("# Global\nShared rule\n")

        let first = ProjectRecord(name: "A", path: "/tmp/a")
        let second = ProjectRecord(name: "B", path: "/tmp/b")
        try store.save(ChadexPreferences(projects: [first, second], selectedProjectID: first.id))
        XCTAssertEqual(try store.loadGlobalInstructions(), "# Global\nShared rule\n")
        try store.save(ChadexPreferences(projects: [first, second], selectedProjectID: second.id))
        XCTAssertEqual(try store.loadGlobalInstructions(), "# Global\nShared rule\n")
        XCTAssertTrue(FileManager.default.fileExists(atPath: root.appendingPathComponent("global-instructions.md").path))
    }

    @MainActor
    func testGlobalInstructionsCanBeEditedWithoutAnyProjectOrConnection() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = ProjectStore(environment: ["CHADEX_PREFERENCES_DIR": root.path])
        let model = AppModel(store: store, autostart: false)
        XCTAssertTrue(model.projects.isEmpty)
        XCTAssertNil(model.selectedProject)
        XCTAssertTrue(model.saveGlobalInstructions("offline global preference"))
        XCTAssertEqual(model.globalInstructions, "offline global preference")
        XCTAssertEqual(try store.loadGlobalInstructions(), "offline global preference")
    }

    func testGlobalInstructionsPathNeverUsesAmbientDocumentsAgents() {
        let path = ProjectStore.globalInstructionsURL(
            environment: [:],
            homeDirectory: URL(fileURLWithPath: "/Users/test")
        )
        XCTAssertEqual(path.path, "/Users/test/Library/Application Support/Chadex/global-instructions.md")
        XCTAssertFalse(path.path.contains("Documents/ChatGPT"))
        XCTAssertNotEqual(path.lastPathComponent, "AGENTS.md")
    }

    func testGlobalInstructionsRejectSymlinkStorageWithoutTouchingTarget() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let target = root.appendingPathComponent("outside.txt")
        let link = root.appendingPathComponent("global-instructions.md")
        try "ambient target".write(to: target, atomically: true, encoding: .utf8)
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: target)
        let store = ProjectStore(environment: ["CHADEX_PREFERENCES_DIR": root.path])

        XCTAssertThrowsError(try store.loadGlobalInstructions())
        XCTAssertThrowsError(try store.saveGlobalInstructions("replacement"))
        XCTAssertEqual(try String(contentsOf: target, encoding: .utf8), "ambient target")
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

