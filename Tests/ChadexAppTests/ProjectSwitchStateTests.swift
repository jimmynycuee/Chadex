import XCTest
@testable import ChadexApp

/// The sidebar follows a project change made outside a sidebar click (adding
/// a project, the memory inspector, removing the current project); otherwise
/// its row stays on the old project and shows "Switching to …" forever.
final class ProjectSwitchStateTests: XCTestCase {
    func testSidebarFollowsAnActiveProjectChangedElsewhere() {
        let row = UUID()
        let active = UUID()
        XCTAssertEqual(
            SidebarProjectFollow.projectToSelect(rowProjectID: row, activeProjectID: active, sidebarSwitchInFlight: false),
            active
        )
    }

    func testSidebarIsLeftAloneWhenNothingToFollow() {
        let id = UUID()
        XCTAssertNil(SidebarProjectFollow.projectToSelect(rowProjectID: id, activeProjectID: id, sidebarSwitchInFlight: false))
        XCTAssertNil(SidebarProjectFollow.projectToSelect(rowProjectID: nil, activeProjectID: id, sidebarSwitchInFlight: false))
        XCTAssertNil(SidebarProjectFollow.projectToSelect(rowProjectID: id, activeProjectID: nil, sidebarSwitchInFlight: false))
        XCTAssertNil(
            SidebarProjectFollow.projectToSelect(rowProjectID: id, activeProjectID: UUID(), sidebarSwitchInFlight: true),
            "the sidebar's own switch request resolves the row itself"
        )
    }
}
