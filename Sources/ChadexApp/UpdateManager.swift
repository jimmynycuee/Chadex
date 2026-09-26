import AppKit
import Combine
import CryptoKit
import Foundation

struct ChadexAppVersion: Comparable, Equatable, CustomStringConvertible, Sendable {
    let components: [Int]
    let description: String

    init?(_ rawValue: String) {
        var value = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
        if value.hasPrefix("v") || value.hasPrefix("V") {
            value.removeFirst()
        }
        guard !value.isEmpty, !value.contains("-") else { return nil }

        let pieces = value.split(separator: ".", omittingEmptySubsequences: false)
        guard !pieces.isEmpty else { return nil }

        var parsed: [Int] = []
        parsed.reserveCapacity(pieces.count)
        for piece in pieces {
            guard !piece.isEmpty,
                  piece.allSatisfy({ $0.isNumber }),
                  let number = Int(piece)
            else {
                return nil
            }
            parsed.append(number)
        }

        components = parsed
        description = value
    }

    static func == (lhs: ChadexAppVersion, rhs: ChadexAppVersion) -> Bool {
        let count = max(lhs.components.count, rhs.components.count)
        for index in 0..<count {
            if component(lhs, at: index) != component(rhs, at: index) {
                return false
            }
        }
        return true
    }

    static func < (lhs: ChadexAppVersion, rhs: ChadexAppVersion) -> Bool {
        let count = max(lhs.components.count, rhs.components.count)
        for index in 0..<count {
            let left = component(lhs, at: index)
            let right = component(rhs, at: index)
            if left != right {
                return left < right
            }
        }
        return false
    }

    private static func component(_ version: ChadexAppVersion, at index: Int) -> Int {
        index < version.components.count ? version.components[index] : 0
    }
}

struct GitHubReleaseAsset: Decodable, Equatable, Sendable {
    let name: String
    let browserDownloadURL: URL
    let size: Int64

    enum CodingKeys: String, CodingKey {
        case name
        case browserDownloadURL = "browser_download_url"
        case size
    }
}

struct GitHubReleasePayload: Decodable, Equatable, Sendable {
    let tagName: String
    let htmlURL: URL
    let body: String?
    let draft: Bool
    let prerelease: Bool
    let assets: [GitHubReleaseAsset]

    enum CodingKeys: String, CodingKey {
        case tagName = "tag_name"
        case htmlURL = "html_url"
        case body
        case draft
        case prerelease
        case assets
    }
}

struct ChadexAvailableUpdate: Equatable, Sendable {
    let tagName: String
    let version: String
    let releasePageURL: URL
    let releaseNotes: String?
    let dmgAsset: GitHubReleaseAsset
    let checksumAsset: GitHubReleaseAsset
}

enum ChadexReleaseResolver {
    static func availableUpdate(
        from payload: GitHubReleasePayload,
        currentVersion: ChadexAppVersion
    ) throws -> ChadexAvailableUpdate? {
        guard !payload.draft, !payload.prerelease else { return nil }
        guard let releaseVersion = ChadexAppVersion(payload.tagName) else {
            throw ChadexUpdateError.invalidReleaseVersion(payload.tagName)
        }
        guard releaseVersion > currentVersion else { return nil }

        let dmgName = "Chadex-\(payload.tagName)-macos-arm64.dmg"
        guard let dmg = payload.assets.first(where: { $0.name == dmgName }) else {
            throw ChadexUpdateError.missingReleaseAsset(dmgName)
        }

        let checksumName = "\(dmgName).sha256"
        guard let checksum = payload.assets.first(where: { $0.name == checksumName }) else {
            throw ChadexUpdateError.missingReleaseAsset(checksumName)
        }

        return ChadexAvailableUpdate(
            tagName: payload.tagName,
            version: releaseVersion.description,
            releasePageURL: payload.htmlURL,
            releaseNotes: payload.body,
            dmgAsset: dmg,
            checksumAsset: checksum
        )
    }
}

enum ChadexChecksumParser {
    static func expectedSHA256(in text: String, filename: String) -> String? {
        for rawLine in text.split(whereSeparator: { $0.isNewline }) {
            let fields = rawLine.split(whereSeparator: { $0.isWhitespace })
            guard fields.count >= 2 else { continue }

            let digest = String(fields[0]).lowercased()
            let recordedFilename = String(fields[fields.count - 1])
            guard recordedFilename == filename,
                  digest.count == 64,
                  digest.allSatisfy({ $0.isHexDigit })
            else {
                continue
            }
            return digest
        }
        return nil
    }
}

enum ChadexUpdatePhase: Equatable {
    case idle
    case checking
    case upToDate
    case available(String)
    case preparing(String)
    case installing(String)
    case failed(String)

    var isBusy: Bool {
        switch self {
        case .checking, .preparing, .installing:
            return true
        case .idle, .upToDate, .available, .failed:
            return false
        }
    }
}

struct ChadexUpdateNotice: Identifiable {
    enum Kind {
        case available(String)
        case blocked(String)
        case upToDate(String)
        case error(String)
    }

    let id = UUID()
    let kind: Kind
}

@MainActor
final class UpdateManager: ObservableObject {
    @Published private(set) var phase: ChadexUpdatePhase = .idle
    @Published private(set) var availableRelease: ChadexAvailableUpdate?
    @Published var notice: ChadexUpdateNotice?

    private static let automaticCheckInterval: TimeInterval = 6 * 60 * 60

    private let service: ChadexUpdateService
    private let bundle: Bundle
    private let defaults: UserDefaults
    private let now: () -> Date
    private var scheduledAutomaticCheck = false

    init(
        service: ChadexUpdateService = ChadexUpdateService(),
        bundle: Bundle = .main,
        defaults: UserDefaults = .standard,
        now: @escaping () -> Date = Date.init
    ) {
        self.service = service
        self.bundle = bundle
        self.defaults = defaults
        self.now = now
    }

    var currentVersion: String {
        bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.0.0"
    }

    var currentBuildNumber: String {
        bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "0"
    }

    var isBusy: Bool {
        phase.isBusy
    }

    func scheduleAutomaticCheckIfNeeded() {
        guard !scheduledAutomaticCheck else { return }
        scheduledAutomaticCheck = true

        let enabled = defaults.object(forKey: ChadexPreferenceKey.autoCheckUpdates) as? Bool ?? true
        guard enabled else { return }

        let lastCheck = defaults.double(forKey: ChadexPreferenceKey.lastUpdateCheckAt)
        if lastCheck > 0,
           now().timeIntervalSince1970 - lastCheck < Self.automaticCheckInterval {
            return
        }

        Task { [weak self] in
            try? await Task.sleep(for: .seconds(2))
            guard let self else { return }
            await self.checkForUpdates(userInitiated: false)
        }
    }

    func checkForUpdates(userInitiated: Bool) async {
        guard !phase.isBusy else { return }
        guard let parsedCurrentVersion = ChadexAppVersion(currentVersion) else {
            let message = ChadexUpdateError.invalidCurrentVersion(currentVersion).localizedDescription
            phase = .failed(message)
            if userInitiated {
                notice = ChadexUpdateNotice(kind: .error(message))
            }
            return
        }

        phase = .checking

        do {
            let release = try await service.fetchLatestRelease(currentVersion: parsedCurrentVersion)
            defaults.set(now().timeIntervalSince1970, forKey: ChadexPreferenceKey.lastUpdateCheckAt)

            if let release {
                availableRelease = release
                phase = .available(release.version)
                notice = ChadexUpdateNotice(kind: .available(release.version))
            } else {
                availableRelease = nil
                phase = .upToDate
                if userInitiated {
                    notice = ChadexUpdateNotice(kind: .upToDate(currentVersion))
                }
            }
        } catch {
            let message = error.localizedDescription
            phase = .failed(message)
            if userInitiated {
                notice = ChadexUpdateNotice(kind: .error(message))
            }
        }
    }

    func installAvailableUpdate(canInstallNow: () -> Bool = { true }) async {
        guard let release = availableRelease, !phase.isBusy else { return }
        guard canInstallNow() else {
            presentInstallBlocked(version: release.version)
            return
        }

        phase = .preparing(release.version)

        do {
            let currentAppURL = bundle.bundleURL.resolvingSymlinksInPath()
            let bundleIdentifier = bundle.bundleIdentifier ?? "app.chadex.Chadex"
            let prepared = try await service.prepareUpdate(
                release,
                currentAppURL: currentAppURL,
                currentBundleIdentifier: bundleIdentifier
            )

            guard canInstallNow() else {
                await service.discardPreparedUpdate(prepared)
                phase = .available(release.version)
                presentInstallBlocked(version: release.version)
                return
            }

            phase = .installing(release.version)
            try await service.launchInstaller(
                prepared,
                currentPID: ProcessInfo.processInfo.processIdentifier
            )
            NSApplication.shared.terminate(nil)
        } catch {
            let message = error.localizedDescription
            phase = .failed(message)
            notice = ChadexUpdateNotice(kind: .error(message))
        }
    }

    func presentInstallBlocked(version: String? = nil) {
        let version = version ?? availableRelease?.version ?? currentVersion
        notice = ChadexUpdateNotice(kind: .blocked(version))
    }

    func markCurrentLaunchHealthy() async {
        await service.markCurrentLaunchHealthy(currentVersion: currentVersion)
    }
}

actor ChadexUpdateService {
    private struct PendingMarker: Codable {
        let nonce: String
        let targetVersion: String
    }

    struct PreparedUpdate: Sendable {
        let currentAppURL: URL
        let candidateAppURL: URL
        let backupAppURL: URL
        let pendingMarkerURL: URL
        let healthyMarkerURL: URL
        let logURL: URL
        let installerScriptURL: URL
        let workDirectoryURL: URL
    }

    private static let latestReleaseURL = URL(
        string: "https://api.github.com/repos/jimmynycuee/Chadex/releases/latest"
    )!

    private let fileManager: FileManager
    private let session: URLSession
    private let environment: [String: String]

    init(
        fileManager: FileManager = .default,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) {
        self.fileManager = fileManager
        self.environment = environment

        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 30
        configuration.timeoutIntervalForResource = 240
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        self.session = URLSession(configuration: configuration)
    }

    func fetchLatestRelease(currentVersion: ChadexAppVersion) async throws -> ChadexAvailableUpdate? {
        var request = URLRequest(url: Self.latestReleaseURL)
        request.httpMethod = "GET"
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        request.setValue("Chadex/\(currentVersion.description)", forHTTPHeaderField: "User-Agent")
        request.setValue("2022-11-28", forHTTPHeaderField: "X-GitHub-Api-Version")

        let (data, response) = try await session.data(for: request)
        try Self.requireSuccessfulHTTPResponse(response)

        let payload: GitHubReleasePayload
        do {
            payload = try JSONDecoder().decode(GitHubReleasePayload.self, from: data)
        } catch {
            throw ChadexUpdateError.invalidReleaseResponse
        }

        return try ChadexReleaseResolver.availableUpdate(
            from: payload,
            currentVersion: currentVersion
        )
    }

    func prepareUpdate(
        _ release: ChadexAvailableUpdate,
        currentAppURL: URL,
        currentBundleIdentifier: String
    ) async throws -> PreparedUpdate {
        guard currentAppURL.pathExtension == "app",
              fileManager.fileExists(atPath: currentAppURL.path)
        else {
            throw ChadexUpdateError.unsupportedInstallLocation
        }

        let parentURL = currentAppURL.deletingLastPathComponent()
        guard fileManager.isWritableFile(atPath: parentURL.path) else {
            throw ChadexUpdateError.installLocationNotWritable(parentURL.path)
        }

        let nonce = UUID().uuidString.lowercased()
        let workDirectory = fileManager.temporaryDirectory
            .appendingPathComponent("ChadexUpdate-\(nonce)", isDirectory: true)
        let mountURL = workDirectory.appendingPathComponent("mount", isDirectory: true)
        let dmgURL = workDirectory.appendingPathComponent(release.dmgAsset.name)
        let checksumURL = workDirectory.appendingPathComponent(release.checksumAsset.name)
        let stagedAppURL = workDirectory.appendingPathComponent("Chadex.app", isDirectory: true)

        let appStem = currentAppURL.deletingPathExtension().lastPathComponent
        let candidateAppURL = parentURL.appendingPathComponent(
            ".\(appStem)-update-\(nonce).app",
            isDirectory: true
        )
        let backupAppURL = parentURL.appendingPathComponent(
            ".\(appStem)-backup-\(nonce).app",
            isDirectory: true
        )

        var keepPreparedArtifacts = false
        var pendingMarkerURL: URL?
        var healthyMarkerURL: URL?

        defer {
            if !keepPreparedArtifacts {
                try? fileManager.removeItem(at: candidateAppURL)
                try? fileManager.removeItem(at: backupAppURL)
                if let pendingMarkerURL {
                    try? fileManager.removeItem(at: pendingMarkerURL)
                }
                if let healthyMarkerURL {
                    try? fileManager.removeItem(at: healthyMarkerURL)
                }
                try? fileManager.removeItem(at: workDirectory)
            }
        }

        try fileManager.createDirectory(at: mountURL, withIntermediateDirectories: true)

        try await download(release.checksumAsset, to: checksumURL)
        try await download(release.dmgAsset, to: dmgURL)

        let checksumText = try String(contentsOf: checksumURL, encoding: .utf8)
        guard let expectedChecksum = ChadexChecksumParser.expectedSHA256(
            in: checksumText,
            filename: release.dmgAsset.name
        ) else {
            throw ChadexUpdateError.invalidChecksumFile
        }

        let actualChecksum = try Self.sha256(of: dmgURL)
        guard actualChecksum == expectedChecksum else {
            throw ChadexUpdateError.checksumMismatch
        }

        try Self.runProcess(
            executable: "/usr/bin/hdiutil",
            arguments: [
                "attach",
                "-nobrowse",
                "-readonly",
                "-mountpoint",
                mountURL.path,
                dmgURL.path
            ]
        )
        var mounted = true
        defer {
            if mounted {
                _ = try? Self.runProcess(
                    executable: "/usr/bin/hdiutil",
                    arguments: ["detach", mountURL.path, "-quiet"]
                )
            }
        }

        let mountedAppURL = mountURL.appendingPathComponent("Chadex.app", isDirectory: true)
        try Self.validateApp(
            at: mountedAppURL,
            expectedVersion: release.version,
            expectedBundleIdentifier: currentBundleIdentifier
        )

        try Self.runProcess(
            executable: "/usr/bin/ditto",
            arguments: [mountedAppURL.path, stagedAppURL.path]
        )

        try Self.runProcess(
            executable: "/usr/bin/hdiutil",
            arguments: ["detach", mountURL.path, "-quiet"]
        )
        mounted = false

        try Self.validateApp(
            at: stagedAppURL,
            expectedVersion: release.version,
            expectedBundleIdentifier: currentBundleIdentifier
        )

        try Self.runProcess(
            executable: "/usr/bin/ditto",
            arguments: [stagedAppURL.path, candidateAppURL.path]
        )
        try Self.validateApp(
            at: candidateAppURL,
            expectedVersion: release.version,
            expectedBundleIdentifier: currentBundleIdentifier
        )

        let updateSupportURL = try updateSupportDirectory()
        let markerURL = updateSupportURL.appendingPathComponent("pending.json")
        let healthURL = updateSupportURL.appendingPathComponent("healthy-\(nonce)")
        let logURL = updateSupportURL.appendingPathComponent("last-update.log")
        pendingMarkerURL = markerURL
        healthyMarkerURL = healthURL

        try? fileManager.removeItem(at: healthURL)
        let marker = PendingMarker(nonce: nonce, targetVersion: release.version)
        let markerData = try JSONEncoder().encode(marker)
        try markerData.write(to: markerURL, options: .atomic)
        try Data().write(to: logURL, options: .atomic)

        let installerScriptURL = workDirectory.appendingPathComponent("install-update.sh")
        try Self.installerScript.write(
            to: installerScriptURL,
            atomically: true,
            encoding: .utf8
        )
        try fileManager.setAttributes(
            [.posixPermissions: 0o700],
            ofItemAtPath: installerScriptURL.path
        )

        keepPreparedArtifacts = true
        return PreparedUpdate(
            currentAppURL: currentAppURL,
            candidateAppURL: candidateAppURL,
            backupAppURL: backupAppURL,
            pendingMarkerURL: markerURL,
            healthyMarkerURL: healthURL,
            logURL: logURL,
            installerScriptURL: installerScriptURL,
            workDirectoryURL: workDirectory
        )
    }

    func launchInstaller(_ prepared: PreparedUpdate, currentPID: Int32) throws {
        let logHandle = try FileHandle(forWritingTo: prepared.logURL)
        try logHandle.seekToEnd()

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = [
            prepared.installerScriptURL.path,
            String(currentPID),
            prepared.currentAppURL.path,
            prepared.candidateAppURL.path,
            prepared.backupAppURL.path,
            prepared.pendingMarkerURL.path,
            prepared.healthyMarkerURL.path,
            prepared.logURL.path,
            prepared.workDirectoryURL.path
        ]
        process.standardOutput = logHandle
        process.standardError = logHandle

        do {
            try process.run()
            try? logHandle.close()
        } catch {
            try? logHandle.close()
            try? fileManager.removeItem(at: prepared.candidateAppURL)
            try? fileManager.removeItem(at: prepared.pendingMarkerURL)
            try? fileManager.removeItem(at: prepared.healthyMarkerURL)
            try? fileManager.removeItem(at: prepared.workDirectoryURL)
            throw ChadexUpdateError.installerLaunchFailed
        }
    }

    func discardPreparedUpdate(_ prepared: PreparedUpdate) {
        try? fileManager.removeItem(at: prepared.candidateAppURL)
        try? fileManager.removeItem(at: prepared.backupAppURL)
        try? fileManager.removeItem(at: prepared.pendingMarkerURL)
        try? fileManager.removeItem(at: prepared.healthyMarkerURL)
        try? fileManager.removeItem(at: prepared.workDirectoryURL)
    }

    func markCurrentLaunchHealthy(currentVersion: String) {
        guard let updateSupportURL = try? updateSupportDirectory() else { return }
        let pendingURL = updateSupportURL.appendingPathComponent("pending.json")
        guard let data = try? Data(contentsOf: pendingURL),
              let marker = try? JSONDecoder().decode(PendingMarker.self, from: data),
              let launchedVersion = ChadexAppVersion(currentVersion),
              let targetVersion = ChadexAppVersion(marker.targetVersion),
              launchedVersion == targetVersion
        else {
            return
        }

        let healthURL = updateSupportURL.appendingPathComponent("healthy-\(marker.nonce)")
        try? Data(marker.nonce.utf8).write(to: healthURL, options: .atomic)
    }

    private func download(_ asset: GitHubReleaseAsset, to destinationURL: URL) async throws {
        var request = URLRequest(url: asset.browserDownloadURL)
        request.setValue("application/octet-stream", forHTTPHeaderField: "Accept")
        request.setValue("Chadex-Updater", forHTTPHeaderField: "User-Agent")

        let (temporaryURL, response) = try await session.download(for: request)
        try Self.requireSuccessfulHTTPResponse(response)

        do {
            try fileManager.moveItem(at: temporaryURL, to: destinationURL)
        } catch {
            throw ChadexUpdateError.downloadFailed(asset.name)
        }
    }

    private func updateSupportDirectory() throws -> URL {
        let baseURL: URL
        if let override = environment["CHADEX_PREFERENCES_DIR"]?
            .trimmingCharacters(in: .whitespacesAndNewlines),
           !override.isEmpty {
            baseURL = URL(fileURLWithPath: override, isDirectory: true)
        } else {
            baseURL = fileManager.urls(
                for: .applicationSupportDirectory,
                in: .userDomainMask
            )[0].appendingPathComponent("Chadex", isDirectory: true)
        }

        let updateURL = baseURL.appendingPathComponent("Update", isDirectory: true)
        try fileManager.createDirectory(at: updateURL, withIntermediateDirectories: true)
        return updateURL
    }

    private static func requireSuccessfulHTTPResponse(_ response: URLResponse) throws {
        guard let http = response as? HTTPURLResponse else {
            throw ChadexUpdateError.invalidHTTPResponse
        }
        guard (200..<300).contains(http.statusCode) else {
            throw ChadexUpdateError.httpStatus(http.statusCode)
        }
    }

    private static func sha256(of fileURL: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: fileURL)
        defer { try? handle.close() }

        var hasher = SHA256()
        while true {
            let data = try handle.read(upToCount: 1024 * 1024) ?? Data()
            if data.isEmpty { break }
            hasher.update(data: data)
        }

        return hasher.finalize()
            .map { String(format: "%02x", $0) }
            .joined()
    }

    private static func validateApp(
        at appURL: URL,
        expectedVersion: String,
        expectedBundleIdentifier: String
    ) throws {
        let plistURL = appURL
            .appendingPathComponent("Contents", isDirectory: true)
            .appendingPathComponent("Info.plist")

        guard let plistData = try? Data(contentsOf: plistURL),
              let plist = try? PropertyListSerialization.propertyList(
                from: plistData,
                options: [],
                format: nil
              ) as? [String: Any],
              let bundleIdentifier = plist["CFBundleIdentifier"] as? String,
              let versionString = plist["CFBundleShortVersionString"] as? String,
              let candidateVersion = ChadexAppVersion(versionString),
              let expectedParsedVersion = ChadexAppVersion(expectedVersion)
        else {
            throw ChadexUpdateError.invalidAppBundle
        }

        guard bundleIdentifier == expectedBundleIdentifier else {
            throw ChadexUpdateError.bundleIdentifierMismatch
        }
        guard candidateVersion == expectedParsedVersion else {
            throw ChadexUpdateError.versionMismatch(expected: expectedVersion, actual: versionString)
        }

        do {
            try runProcess(
                executable: "/usr/bin/codesign",
                arguments: ["--verify", "--deep", "--strict", appURL.path]
            )
        } catch {
            throw ChadexUpdateError.invalidCodeSignature
        }

        let executableURL = appURL
            .appendingPathComponent("Contents", isDirectory: true)
            .appendingPathComponent("MacOS", isDirectory: true)
            .appendingPathComponent("Chadex")

        let architectures: String
        do {
            architectures = try runProcess(
                executable: "/usr/bin/lipo",
                arguments: ["-archs", executableURL.path]
            )
        } catch {
            throw ChadexUpdateError.unsupportedArchitecture
        }

        guard architectures
            .split(whereSeparator: { $0.isWhitespace })
            .contains(where: { $0 == "arm64" })
        else {
            throw ChadexUpdateError.unsupportedArchitecture
        }
    }

    @discardableResult
    private static func runProcess(
        executable: String,
        arguments: [String]
    ) throws -> String {
        let process = Process()
        let outputPipe = Pipe()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        process.standardOutput = outputPipe
        process.standardError = outputPipe

        try process.run()
        let outputData = outputPipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()

        let output = String(decoding: outputData, as: UTF8.self)
        guard process.terminationStatus == 0 else {
            throw ChadexUpdateError.commandFailed(
                URL(fileURLWithPath: executable).lastPathComponent,
                process.terminationStatus,
                String(output.suffix(1200))
            )
        }
        return output
    }

    static let installerScript = #"""
#!/bin/sh
set -u

old_pid="$1"
current="$2"
candidate="$3"
backup="$4"
pending="$5"
healthy="$6"
log_file="$7"
work_dir="$8"

log_line() {
    printf '%s %s\n' "$(date '+%Y-%m-%dT%H:%M:%S%z')" "$*" >> "$log_file"
}

cleanup_prepared() {
    rm -rf "$candidate"
    rm -f "$pending" "$healthy"
    rm -rf "$work_dir"
}

reopen_old() {
    if [ -d "$current" ]; then
        /usr/bin/open -n "$current" >> "$log_file" 2>&1 || true
    fi
}

rollback() {
    log_line "New Chadex did not report a healthy launch; rolling back."

    for executable in \
        "$current/Contents/MacOS/Chadex" \
        "$current/Contents/Helpers/chadex-helper" \
        "$current/Contents/Resources/chadex-runtime/chadex-runtime-server" \
        "$current/Contents/Resources/chadex-runtime/chadex-runtime-runner"
    do
        if [ -e "$executable" ]; then
            pids=$(/usr/sbin/lsof -t "$executable" 2>/dev/null || true)
            if [ -n "$pids" ]; then
                kill $pids 2>/dev/null || true
            fi
        fi
    done

    count=0
    while [ "$count" -lt 50 ]; do
        if ! /usr/sbin/lsof -t "$current/Contents/MacOS/Chadex" >/dev/null 2>&1; then
            break
        fi
        sleep 0.1
        count=$((count + 1))
    done

    rm -rf "$current"
    if [ -d "$backup" ]; then
        if mv "$backup" "$current"; then
            log_line "Rollback restored the previous Chadex."
            rm -f "$pending" "$healthy"
            /usr/bin/open -n "$current" >> "$log_file" 2>&1 || true
        else
            log_line "ERROR: rollback could not restore $backup"
        fi
    else
        log_line "ERROR: rollback backup is missing: $backup"
    fi

    rm -rf "$candidate"
    rm -rf "$work_dir"
    exit 1
}

log_line "Waiting for Chadex process $old_pid to exit."
count=0
while kill -0 "$old_pid" 2>/dev/null; do
    if [ "$count" -ge 300 ]; then
        log_line "ERROR: Chadex did not quit within 30 seconds."
        cleanup_prepared
        reopen_old
        exit 1
    fi
    sleep 0.1
    count=$((count + 1))
done

if ! /usr/bin/codesign --verify --deep --strict "$candidate" >> "$log_file" 2>&1; then
    log_line "ERROR: candidate code signature validation failed."
    cleanup_prepared
    reopen_old
    exit 1
fi

if [ -e "$backup" ]; then
    rm -rf "$backup"
fi

if ! mv "$current" "$backup"; then
    log_line "ERROR: could not move current Chadex to backup."
    cleanup_prepared
    reopen_old
    exit 1
fi

if ! mv "$candidate" "$current"; then
    log_line "ERROR: could not place the new Chadex bundle."
    mv "$backup" "$current" 2>> "$log_file" || true
    rm -f "$pending" "$healthy"
    rm -rf "$work_dir"
    reopen_old
    exit 1
fi

log_line "Installed new Chadex bundle; launching for health verification."
if ! /usr/bin/open -n "$current" >> "$log_file" 2>&1; then
    rollback
fi

count=0
while [ "$count" -lt 300 ]; do
    if [ -f "$healthy" ]; then
        log_line "New Chadex reported a healthy launch. Update complete."
        rm -rf "$backup"
        rm -f "$pending" "$healthy"
        rm -rf "$work_dir"
        exit 0
    fi
    sleep 0.1
    count=$((count + 1))
done

rollback
"""#
}

enum ChadexUpdateError: LocalizedError {
    case invalidCurrentVersion(String)
    case invalidReleaseVersion(String)
    case invalidReleaseResponse
    case missingReleaseAsset(String)
    case invalidHTTPResponse
    case httpStatus(Int)
    case downloadFailed(String)
    case invalidChecksumFile
    case checksumMismatch
    case unsupportedInstallLocation
    case installLocationNotWritable(String)
    case invalidAppBundle
    case bundleIdentifierMismatch
    case versionMismatch(expected: String, actual: String)
    case invalidCodeSignature
    case unsupportedArchitecture
    case installerLaunchFailed
    case commandFailed(String, Int32, String)

    var errorDescription: String? {
        switch self {
        case .invalidCurrentVersion(let version):
            return L10n.string("updates.error.invalidCurrentVersion", version)
        case .invalidReleaseVersion(let version):
            return L10n.string("updates.error.invalidReleaseVersion", version)
        case .invalidReleaseResponse:
            return L10n.string("updates.error.invalidReleaseResponse")
        case .missingReleaseAsset(let name):
            return L10n.string("updates.error.missingAsset", name)
        case .invalidHTTPResponse:
            return L10n.string("updates.error.invalidHTTP")
        case .httpStatus(let status):
            return L10n.string("updates.error.httpStatus", status)
        case .downloadFailed(let name):
            return L10n.string("updates.error.download", name)
        case .invalidChecksumFile:
            return L10n.string("updates.error.invalidChecksum")
        case .checksumMismatch:
            return L10n.string("updates.error.checksumMismatch")
        case .unsupportedInstallLocation:
            return L10n.string("updates.error.installLocation")
        case .installLocationNotWritable(let path):
            return L10n.string("updates.error.installNotWritable", path)
        case .invalidAppBundle:
            return L10n.string("updates.error.invalidBundle")
        case .bundleIdentifierMismatch:
            return L10n.string("updates.error.bundleIdentifier")
        case .versionMismatch(let expected, let actual):
            return L10n.string("updates.error.versionMismatch", expected, actual)
        case .invalidCodeSignature:
            return L10n.string("updates.error.codeSignature")
        case .unsupportedArchitecture:
            return L10n.string("updates.error.architecture")
        case .installerLaunchFailed:
            return L10n.string("updates.error.installerLaunch")
        case .commandFailed(let command, let status, let output):
            let detail = output.trimmingCharacters(in: .whitespacesAndNewlines)
            return L10n.string("updates.error.command", command, status, detail)
        }
    }
}
