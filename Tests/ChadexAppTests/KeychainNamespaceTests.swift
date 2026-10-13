import XCTest
@testable import ChadexApp

final class KeychainNamespaceTests: XCTestCase {
    func testProductionServiceStaysStable() {
        XCTAssertEqual(KeychainStore.defaultService(environment: [:]), "app.chadex.credentials")
        XCTAssertEqual(KeychainStore.defaultService(environment: ["CHADEX_KEYCHAIN_SERVICE": "  "]), "app.chadex.credentials")
    }

    func testCandidateUsesIsolatedService() {
        let candidate = KeychainStore.defaultService(environment: ["CHADEX_KEYCHAIN_SERVICE": " app.chadex.v071-candidate.credentials "])
        XCTAssertEqual(candidate, "app.chadex.v071-candidate.credentials")
        XCTAssertNotEqual(candidate, KeychainStore.defaultService(environment: [:]))
    }
}
