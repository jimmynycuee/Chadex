import Foundation
import XCTest
@testable import ChadexApp

final class SkillArchiveStagingTests: XCTestCase {
    private var scratch: URL!
    private var project: URL!
    private var downloads: URL!

    override func setUpWithError() throws {
        let fm = FileManager.default
        scratch = fm.temporaryDirectory.appendingPathComponent("chadex-staging-\(UUID().uuidString)", isDirectory: true)
        project = scratch.appendingPathComponent("project", isDirectory: true)
        downloads = scratch.appendingPathComponent("Downloads", isDirectory: true)
        try fm.createDirectory(at: project, withIntermediateDirectories: true)
        try fm.createDirectory(at: downloads, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        // Directories made read-only by a test must be writable again to be removed.
        let imports = project.appendingPathComponent(".chadex/skill-imports")
        try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: imports.path)
        try? FileManager.default.removeItem(at: scratch)
    }

    private func makeZip(_ name: String = "skill.zip", bytes: Int = 64) throws -> URL {
        let url = downloads.appendingPathComponent(name)
        try Data((0..<bytes).map { UInt8($0 % 251) }).write(to: url)
        return url
    }

    private var importsDirectory: URL { project.appendingPathComponent(".chadex/skill-imports", isDirectory: true) }

    private func importsContents() -> [String] {
        (try? FileManager.default.contentsOfDirectory(atPath: importsDirectory.path)) ?? []
    }

    // MARK: - Staging

    func testStageCopiesArchiveFromOutsideProjectToUniqueRelativePath() throws {
        let zip = try makeZip(bytes: 1000)
        let first = try SkillArchiveStaging.stage(source: zip, projectRoot: project)
        let second = try SkillArchiveStaging.stage(source: zip, projectRoot: project)

        XCTAssertTrue(first.relativePath.hasPrefix(".chadex/skill-imports/"))
        XCTAssertTrue(first.relativePath.hasSuffix(".zip"))
        XCTAssertNotEqual(first.relativePath, second.relativePath)
        XCTAssertEqual(try Data(contentsOf: first.url), try Data(contentsOf: zip))
        XCTAssertEqual(first.url.path, project.appendingPathComponent(first.relativePath).path)
        // The original is left alone.
        XCTAssertTrue(FileManager.default.fileExists(atPath: zip.path))
        // Private to the user.
        let mode = try FileManager.default.attributesOfItem(atPath: first.url.path)[.posixPermissions] as? NSNumber
        XCTAssertEqual(mode?.intValue, 0o600)
    }

    func testStageDoesNotTouchExistingFilesInStagingDirectory() throws {
        try FileManager.default.createDirectory(at: importsDirectory, withIntermediateDirectories: true)
        let existing = importsDirectory.appendingPathComponent("keep.zip")
        try Data("keep".utf8).write(to: existing)
        let staged = try SkillArchiveStaging.stage(source: try makeZip(), projectRoot: project)
        XCTAssertEqual(try String(contentsOf: existing, encoding: .utf8), "keep")
        XCTAssertNotEqual(staged.url.lastPathComponent, "keep.zip")
    }

    func testRemoveDeletesCopyKeepsEmptyDirectoryAndIsIdempotent() throws {
        let staged = try SkillArchiveStaging.stage(source: try makeZip(), projectRoot: project)
        staged.remove()
        staged.remove()
        XCTAssertEqual(importsContents(), [])
        XCTAssertTrue(FileManager.default.fileExists(atPath: importsDirectory.path))
    }

    func testWithStagedArchiveRemovesCopyOnSuccess() async throws {
        let zip = try makeZip()
        var seen: String?
        let result: Int = try await SkillArchiveStaging.withStagedArchive(source: zip, projectRoot: project) { relative in
            seen = relative
            XCTAssertTrue(FileManager.default.fileExists(atPath: self.project.appendingPathComponent(relative).path))
            return 7
        }
        XCTAssertEqual(result, 7)
        XCTAssertNotNil(seen)
        XCTAssertEqual(importsContents(), [])
    }

    func testWithStagedArchiveRemovesCopyWhenBodyThrows() async throws {
        struct Boom: Error {}
        let zip = try makeZip()
        do {
            _ = try await SkillArchiveStaging.withStagedArchive(source: zip, projectRoot: project) { _ -> Int in
                throw Boom()
            }
            XCTFail("expected throw")
        } catch is Boom {}
        XCTAssertEqual(importsContents(), [])
    }

    // MARK: - Rejections

    func testRejectsNonZipExtension() throws {
        let file = try makeZip("skill.tar")
        XCTAssertThrowsError(try SkillArchiveStaging.stage(source: file, projectRoot: project)) {
            XCTAssertEqual($0 as? SkillArchiveStagingError, .notZip)
        }
        XCTAssertFalse(FileManager.default.fileExists(atPath: importsDirectory.path))
    }

    func testAcceptsUppercaseZipExtension() throws {
        let staged = try SkillArchiveStaging.stage(source: try makeZip("SKILL.ZIP"), projectRoot: project)
        staged.remove()
    }

    func testRejectsMissingSource() {
        let missing = downloads.appendingPathComponent("nope.zip")
        XCTAssertThrowsError(try SkillArchiveStaging.stage(source: missing, projectRoot: project)) {
            XCTAssertEqual($0 as? SkillArchiveStagingError, .unreadable)
        }
    }

    func testRejectsSymlinkAndDirectorySources() throws {
        let real = try makeZip("real.zip")
        let link = downloads.appendingPathComponent("link.zip")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: real)
        XCTAssertThrowsError(try SkillArchiveStaging.stage(source: link, projectRoot: project)) {
            XCTAssertEqual($0 as? SkillArchiveStagingError, .notRegularFile)
        }
        let dir = downloads.appendingPathComponent("folder.zip", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        XCTAssertThrowsError(try SkillArchiveStaging.stage(source: dir, projectRoot: project)) {
            XCTAssertEqual($0 as? SkillArchiveStagingError, .notRegularFile)
        }
        XCTAssertFalse(FileManager.default.fileExists(atPath: importsDirectory.path))
    }

    func testSizeLimitIsCheckedBeforeCopyingAndAllowsExactLimit() throws {
        XCTAssertEqual(SkillArchiveStaging.maxArchiveBytes, 8 * 1024 * 1024)
        let zip = try makeZip(bytes: 101)
        XCTAssertThrowsError(try SkillArchiveStaging.stage(source: zip, projectRoot: project, limit: 100)) {
            XCTAssertEqual($0 as? SkillArchiveStagingError, .tooLarge)
        }
        // Rejected before any directory or copy is created.
        XCTAssertFalse(FileManager.default.fileExists(atPath: importsDirectory.path))
        let exact = try makeZip("exact.zip", bytes: 100)
        let staged = try SkillArchiveStaging.stage(source: exact, projectRoot: project, limit: 100)
        XCTAssertEqual(try Data(contentsOf: staged.url).count, 100)
    }

    func testRealLimitRejectsArchiveJustOverEightMiB() throws {
        let zip = try makeZip(bytes: SkillArchiveStaging.maxArchiveBytes + 1)
        XCTAssertThrowsError(try SkillArchiveStaging.preflight(zip)) {
            XCTAssertEqual($0 as? SkillArchiveStagingError, .tooLarge)
        }
    }

    func testRefusesSymlinkedStagingDirectories() throws {
        let outside = scratch.appendingPathComponent("outside", isDirectory: true)
        try FileManager.default.createDirectory(at: outside, withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(
            at: project.appendingPathComponent(".chadex"), withDestinationURL: outside
        )
        XCTAssertThrowsError(try SkillArchiveStaging.stage(source: try makeZip(), projectRoot: project)) {
            XCTAssertEqual($0 as? SkillArchiveStagingError, .unsafeStagingDirectory)
        }
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: outside.path), [])

        try FileManager.default.removeItem(at: project.appendingPathComponent(".chadex"))
        try FileManager.default.createDirectory(
            at: project.appendingPathComponent(".chadex"), withIntermediateDirectories: true
        )
        try FileManager.default.createSymbolicLink(
            at: importsDirectory, withDestinationURL: outside
        )
        XCTAssertThrowsError(try SkillArchiveStaging.stage(source: try makeZip(), projectRoot: project)) {
            XCTAssertEqual($0 as? SkillArchiveStagingError, .unsafeStagingDirectory)
        }
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: outside.path), [])
    }

    func testRejectsMissingProjectDirectory() throws {
        let gone = scratch.appendingPathComponent("gone", isDirectory: true)
        XCTAssertThrowsError(try SkillArchiveStaging.stage(source: try makeZip(), projectRoot: gone)) {
            XCTAssertEqual($0 as? SkillArchiveStagingError, .projectUnavailable)
        }
    }

    func testCopyFailureLeavesNothingBehind() throws {
        try FileManager.default.createDirectory(at: importsDirectory, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o500], ofItemAtPath: importsDirectory.path)
        guard access(importsDirectory.path, W_OK) != 0 else {
            throw XCTSkip("directory permissions are not enforced for this user")
        }
        XCTAssertThrowsError(try SkillArchiveStaging.stage(source: try makeZip(), projectRoot: project)) {
            XCTAssertEqual($0 as? SkillArchiveStagingError, .copyFailed)
        }
        XCTAssertEqual(importsContents(), [])
    }

    func testErrorsHaveReadableDescriptions() {
        let all: [SkillArchiveStagingError] = [
            .notZip, .notRegularFile, .tooLarge, .unreadable, .projectUnavailable, .unsafeStagingDirectory, .copyFailed,
        ]
        for error in all {
            let text = error.localizedDescription
            XCTAssertFalse(text.isEmpty)
            XCTAssertFalse(text.hasPrefix("skills.archive.error"), "missing L10n string for \(error)")
        }
    }

    // MARK: - Stale purge

    func testStaleLeftoversArePurgedButFreshAndForeignFilesStay() throws {
        try FileManager.default.createDirectory(at: importsDirectory, withIntermediateDirectories: true)
        let stale = importsDirectory.appendingPathComponent("old.zip")
        let fresh = importsDirectory.appendingPathComponent("fresh.zip")
        let note = importsDirectory.appendingPathComponent("old.txt")
        for url in [stale, fresh, note] { try Data("x".utf8).write(to: url) }
        let old = Date().addingTimeInterval(-3 * SkillArchiveStaging.staleAge)
        try FileManager.default.setAttributes([.modificationDate: old], ofItemAtPath: stale.path)
        try FileManager.default.setAttributes([.modificationDate: old], ofItemAtPath: note.path)

        let staged = try SkillArchiveStaging.stage(source: try makeZip(), projectRoot: project)
        staged.remove()
        XCTAssertFalse(FileManager.default.fileExists(atPath: stale.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: fresh.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: note.path))
    }

    // MARK: - .gitignore

    private var gitignore: URL { project.appendingPathComponent(".gitignore") }

    func testStagingCreatesGitignoreWhenMissing() throws {
        let staged = try SkillArchiveStaging.stage(source: try makeZip(), projectRoot: project)
        staged.remove()
        XCTAssertEqual(try String(contentsOf: gitignore, encoding: .utf8), ".chadex/skill-imports/\n")
    }

    func testGitignoreAppendPreservesContentAndAddsMissingNewline() throws {
        try Data("node_modules/\n*.log".utf8).write(to: gitignore)
        try SkillArchiveStaging.ensureGitignoreEntry(projectRoot: project)
        XCTAssertEqual(
            try String(contentsOf: gitignore, encoding: .utf8),
            "node_modules/\n*.log\n.chadex/skill-imports/\n"
        )
    }

    func testGitignoreAppendKeepsExistingTrailingNewlineAndIsNotDuplicated() throws {
        try Data("dist/\n".utf8).write(to: gitignore)
        try SkillArchiveStaging.ensureGitignoreEntry(projectRoot: project)
        try SkillArchiveStaging.ensureGitignoreEntry(projectRoot: project)
        XCTAssertEqual(try String(contentsOf: gitignore, encoding: .utf8), "dist/\n.chadex/skill-imports/\n")
    }

    func testGitignoreUsesCRLFWhenFileDoes() throws {
        try Data("dist/\r\nbuild".utf8).write(to: gitignore)
        try SkillArchiveStaging.ensureGitignoreEntry(projectRoot: project)
        XCTAssertEqual(
            try String(contentsOf: gitignore, encoding: .utf8),
            "dist/\r\nbuild\r\n.chadex/skill-imports/\r\n"
        )
    }

    func testGitignoreIsLeftAloneWhenAlreadyCovered() throws {
        for line in [".chadex/skill-imports/", "/.chadex/skill-imports", ".chadex/", "  .chadex  "] {
            let original = "a\n\(line)\nb\n"
            try Data(original.utf8).write(to: gitignore)
            try SkillArchiveStaging.ensureGitignoreEntry(projectRoot: project)
            XCTAssertEqual(try String(contentsOf: gitignore, encoding: .utf8), original, "line: \(line)")
        }
    }

    func testGitignoreIgnoresCommentsAndUnrelatedChadexPaths() {
        XCTAssertFalse(SkillArchiveStaging.gitignoreCovers("# .chadex/skill-imports/\n.chadex-other/\n"))
        XCTAssertTrue(SkillArchiveStaging.gitignoreCovers("x\r\n.chadex/skill-imports/\r\n"))
    }

    func testGitignoreFailureDoesNotBlockStaging() throws {
        // A directory named .gitignore cannot be updated; the copy must still be staged.
        try FileManager.default.createDirectory(at: gitignore, withIntermediateDirectories: true)
        let staged = try SkillArchiveStaging.stage(source: try makeZip(), projectRoot: project)
        XCTAssertTrue(FileManager.default.fileExists(atPath: staged.url.path))
        staged.remove()
    }
}
