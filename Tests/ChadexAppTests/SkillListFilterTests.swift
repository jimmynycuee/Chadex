import Foundation
import XCTest
@testable import ChadexApp

final class SkillListFilterTests: XCTestCase {
    private func managed(_ key: String, active: Bool) -> ManagedSkillInventoryEntry {
        ManagedSkillInventoryEntry(
            skillId: key, skillKey: key, stateRevision: "s",
            activePackageRevision: active ? "p" : nil, preferredPackageRevision: "p",
            definitionRevision: "r", name: key, description: "d", totalVersions: 1
        )
    }

    private func item(
        _ id: String,
        name: String,
        description: String = "",
        trust: String,
        managed: ManagedSkillInventoryEntry? = nil
    ) -> SkillCenterItem {
        SkillCenterItem(
            skillId: id, name: name, description: description, definitionRevision: "r",
            packageRevision: nil, sourceScope: "scope-\(id)", trust: trust, nameConflict: false,
            scriptsAllowed: nil, managed: managed
        )
    }

    private var items: [SkillCenterItem] {
        [
            item("a", name: "Alpha", description: "Writes reports", trust: "project_content"),
            item("b", name: "Beta", description: "Reviews code", trust: "operator_configured_guidance"),
            item("c", name: "Gamma", description: "Deploys services", trust: "operator_installed_guidance",
                 managed: managed("gamma-key", active: true)),
            item("d", name: "Delta", description: "Switched off", trust: "operator_installed_guidance",
                 managed: managed("delta-key", active: false)),
        ]
    }

    private func ids(_ list: [SkillCenterItem]) -> [String] { list.map(\.skillId) }

    func testScopesPartitionTheCatalog() {
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .all, query: "")), ["a", "b", "c", "d"])
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .enabled, query: "")), ["a", "b", "c"])
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .external, query: "")), ["b"])
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .installed, query: "")), ["c", "d"])
    }

    func testInstalledScopeIncludesManagedEntryEvenWithoutInstalledTrust() {
        let odd = item("x", name: "X", trust: "project_content", managed: managed("x", active: true))
        XCTAssertTrue(SkillListFilter.matches(odd, scope: .installed))
        XCTAssertFalse(SkillListFilter.matches(odd, scope: .external))
    }

    func testQueryIsCaseAndWhitespaceInsensitive() {
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .all, query: "  REPORT ")), ["a"])
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .all, query: "beta")), ["b"])
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .all, query: "   ")), ["a", "b", "c", "d"])
    }

    func testQueryMatchesDescriptionKeyAndSourceLabel() {
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .all, query: "services")), ["c"])
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .all, query: "delta-key")), ["d"])
        let label = items[1].sourceDisplayLabel
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .all, query: label)), ["b"])
    }

    func testMultipleTermsMustAllMatch() {
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .all, query: "reviews code")), ["b"])
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .all, query: "reviews deploys")), [])
    }

    func testDiacriticsAreIgnored() {
        let accented = [item("e", name: "Résumé helper", trust: "project_content")]
        XCTAssertEqual(ids(SkillListFilter.filter(accented, scope: .all, query: "resume")), ["e"])
    }

    func testScopeAndQueryCombine() {
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .installed, query: "off")), ["d"])
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .external, query: "alpha")), [])
        XCTAssertEqual(ids(SkillListFilter.filter(items, scope: .enabled, query: "off")), [])
    }

    func testEmptyCatalogStaysEmpty() {
        XCTAssertEqual(SkillListFilter.filter([], scope: .all, query: "x"), [])
    }

    func testEveryScopeHasLocalizedTitle() {
        for scope in SkillListScope.allCases {
            XCTAssertNotEqual(L10n.string(scope.titleKey), scope.titleKey)
        }
    }
}
