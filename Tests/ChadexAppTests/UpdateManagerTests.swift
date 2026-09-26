import AppKit
import Foundation
import XCTest
@testable import ChadexApp

final class UpdateManagerTests: XCTestCase {
    func testVersionComparisonHandlesMultipleDigitsAndTrailingZeroes() throws {
        let older = try XCTUnwrap(ChadexAppVersion("0.1.9"))
        let newer = try XCTUnwrap(ChadexAppVersion("v0.1.10"))
        let normalized = try XCTUnwrap(ChadexAppVersion("0.1.10.0"))
        let major = try XCTUnwrap(ChadexAppVersion("1.0.0"))
        let shorthandMajor = try XCTUnwrap(ChadexAppVersion("1"))

        XCTAssertLessThan(older, newer)
        XCTAssertEqual(newer, normalized)
        XCTAssertEqual(major, shorthandMajor)
        XCTAssertEqual(major.description, "1.0.0")
        XCTAssertNil(ChadexAppVersion("v0.1.0-beta"))
        XCTAssertNil(ChadexAppVersion("0..1"))
    }

    func testReleaseResolverSelectsExactDMGAndChecksumAssets() throws {
        let payload = GitHubReleasePayload(
            tagName: "v0.1.2",
            htmlURL: URL(string: "https://github.com/jimmynycuee/Chadex/releases/tag/v0.1.2")!,
            body: "Release notes",
            draft: false,
            prerelease: false,
            assets: [
                GitHubReleaseAsset(
                    name: "Chadex-v0.1.2-macos-arm64.dmg",
                    browserDownloadURL: URL(string: "https://example.invalid/Chadex.dmg")!,
                    size: 123
                ),
                GitHubReleaseAsset(
                    name: "Chadex-v0.1.2-macos-arm64.dmg.sha256",
                    browserDownloadURL: URL(string: "https://example.invalid/Chadex.dmg.sha256")!,
                    size: 99
                )
            ]
        )

        let update = try XCTUnwrap(
            ChadexReleaseResolver.availableUpdate(
                from: payload,
                currentVersion: try XCTUnwrap(ChadexAppVersion("0.1.1"))
            )
        )

        XCTAssertEqual(update.version, "0.1.2")
        XCTAssertEqual(update.dmgAsset.name, "Chadex-v0.1.2-macos-arm64.dmg")
        XCTAssertEqual(update.checksumAsset.name, "Chadex-v0.1.2-macos-arm64.dmg.sha256")
    }

    func testReleaseResolverIgnoresSameAndPrereleaseVersions() throws {
        let current = try XCTUnwrap(ChadexAppVersion("0.1.2"))
        let assets = [
            GitHubReleaseAsset(
                name: "Chadex-v0.1.2-macos-arm64.dmg",
                browserDownloadURL: URL(string: "https://example.invalid/a")!,
                size: 1
            ),
            GitHubReleaseAsset(
                name: "Chadex-v0.1.2-macos-arm64.dmg.sha256",
                browserDownloadURL: URL(string: "https://example.invalid/b")!,
                size: 1
            )
        ]

        let same = GitHubReleasePayload(
            tagName: "v0.1.2",
            htmlURL: URL(string: "https://example.invalid/release")!,
            body: nil,
            draft: false,
            prerelease: false,
            assets: assets
        )
        XCTAssertNil(try ChadexReleaseResolver.availableUpdate(from: same, currentVersion: current))

        let prerelease = GitHubReleasePayload(
            tagName: "v0.1.3",
            htmlURL: URL(string: "https://example.invalid/prerelease")!,
            body: nil,
            draft: false,
            prerelease: true,
            assets: []
        )
        XCTAssertNil(try ChadexReleaseResolver.availableUpdate(from: prerelease, currentVersion: current))
    }

    func testChecksumParserRequiresMatchingFilenameAndHexDigest() {
        let digest = String(repeating: "a", count: 64)
        let text = "\(digest)  Chadex-v0.1.2-macos-arm64.dmg\n"

        XCTAssertEqual(
            ChadexChecksumParser.expectedSHA256(
                in: text,
                filename: "Chadex-v0.1.2-macos-arm64.dmg"
            ),
            digest
        )
        XCTAssertNil(
            ChadexChecksumParser.expectedSHA256(
                in: text,
                filename: "Other.dmg"
            )
        )
        XCTAssertNil(
            ChadexChecksumParser.expectedSHA256(
                in: "not-a-digest  Chadex-v0.1.2-macos-arm64.dmg",
                filename: "Chadex-v0.1.2-macos-arm64.dmg"
            )
        )
    }

    func testInstallerScriptDoesNotForceDuplicateAppInstances() {
        XCTAssertFalse(ChadexUpdateService.installerScript.contains("/usr/bin/open -n"))
    }

    @MainActor
    func testPreparedUpdateTerminationBypassesDeferredShutdown() {
        let delegate = ChadexAppDelegate()
        delegate.shutdownHandler = {}
        delegate.markShutdownCompletedForUpdate()

        XCTAssertEqual(
            delegate.applicationShouldTerminate(NSApplication.shared),
            .terminateNow
        )
    }

    func testInstallerScriptIsValidPOSIXShellSyntax() throws {
        let process = Process()
        let input = Pipe()
        let output = Pipe()

        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-n"]
        process.standardInput = input
        process.standardOutput = output
        process.standardError = output

        try process.run()
        input.fileHandleForWriting.write(Data(ChadexUpdateService.installerScript.utf8))
        try input.fileHandleForWriting.close()
        let diagnostics = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()

        XCTAssertEqual(
            process.terminationStatus,
            0,
            String(decoding: diagnostics, as: UTF8.self)
        )
    }
}
