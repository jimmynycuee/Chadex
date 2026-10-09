import XCTest
@testable import ChadexApp

/// The app must accept exactly the Tunnel IDs the helper accepts
/// (`validate_tunnel_id` in rust-helper). Before this, the app saved IDs the
/// helper rejects, so every launch failed `credential_push` and the
/// connection stayed unconfigured.
final class TunnelIDValidationTests: XCTestCase {
    func testAcceptsTheHelperFormat() {
        XCTAssertTrue(AppModel.isValidTunnelID("tunnel_6aa2855405988191b439e7c0860a51c7"))
        XCTAssertTrue(AppModel.isValidTunnelID("tunnel_00000000000000000000000000000000"))
    }

    func testRejectsIDsTheHelperRejects() {
        for value in [
            "",
            "tunnel_chadex_dev",
            "tunnel_6AA2855405988191B439E7C0860A51C7",
            "tunnel_6aa2855405988191b439e7c0860a51c",
            "tunnel_6aa2855405988191b439e7c0860a51c70",
            "tunnel_6aa2855405988191b439e7c0860a51cg",
            "tunnel-6aa2855405988191b439e7c0860a51c7",
            "6aa2855405988191b439e7c0860a51c7",
            "tunnel_6aa2855405988191b439e7c0860a51c7 ",
        ] {
            XCTAssertFalse(AppModel.isValidTunnelID(value), value)
        }
    }

    @MainActor
    func testSavingAnInvalidTunnelIDKeepsTheStoredPreferences() async throws {
        let temporaryRoot = FileManager.default.temporaryDirectory
            .appendingPathComponent("chadex-tunnel-id-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: temporaryRoot) }
        try FileManager.default.createDirectory(at: temporaryRoot, withIntermediateDirectories: true)

        let store = ProjectStore(environment: ["CHADEX_PREFERENCES_DIR": temporaryRoot.path])
        var preferences = ChadexPreferences()
        preferences.tunnelID = "tunnel_6aa2855405988191b439e7c0860a51c7"
        try store.save(preferences)

        let keychain = KeychainStore(
            service: "app.chadex.tests.\(UUID().uuidString)",
            account: "tunnel-id-validation"
        )
        let model = AppModel(keychain: keychain, store: store)

        let saved = await model.saveConnectionSettings(tunnelID: "tunnel_chadex_dev", apiKey: "")

        XCTAssertFalse(saved)
        XCTAssertEqual(model.presentedError?.code, "tunnel_config_invalid")
        XCTAssertEqual(store.load().tunnelID, "tunnel_6aa2855405988191b439e7c0860a51c7")
    }
}
