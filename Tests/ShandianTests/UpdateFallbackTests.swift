import Foundation
import Testing
@testable import Shandian

struct UpdateFallbackTests {
    @Test func onlyAcceptsOfficialVersionedReleasePage() {
        let page = URL(string: "https://github.com/chenmozhe008/shandian-download/releases/tag/v0.18.21")!
        #expect(UpdateChecker.releaseFromPage(page)?.version == "0.18.21")
        let chip = UpdateChecker.isAppleSilicon ? "AppleSilicon" : "Intel"
        #expect(UpdateChecker.releaseFromPage(page)?.asset?.absoluteString
                == "https://github.com/chenmozhe008/shandian-download/releases/download/v0.18.21/Shandian-0.18.21-\(chip).zip")
        #expect(UpdateChecker.releaseFromPage(URL(string: "https://example.com/chenmozhe008/shandian-download/releases/tag/v0.18.21")!) == nil)
    }

    @Test func fallbackCacheIsNotUsedAfterNetworkRecovers() throws {
        let page = URL(string: "https://github.com/chenmozhe008/shandian-download/releases/tag/v0.18.21")!
        let fallback = UpdateChecker.Release(version: "0.18.21", page: page, asset: nil, digest: nil, notes: [])
        #expect(UpdateChecker.usableCachedRelease(try JSONEncoder().encode(fallback)) == nil)

        let asset = URL(string: "https://github.com/chenmozhe008/shandian-download/releases/download/v0.18.21/Shandian-0.18.21-AppleSilicon.zip")!
        let complete = UpdateChecker.Release(version: fallback.version, page: page, asset: asset, digest: "sha256:abc", notes: [])
        #expect(UpdateChecker.usableCachedRelease(try JSONEncoder().encode(complete)) == complete)
        #expect(UpdateChecker.usableCachedRelease(Data("invalid".utf8)) == nil)
    }

    @Test func translocatedAppInstallsIntoApplications() {
        let moved = URL(fileURLWithPath: "/private/var/folders/x/AppTranslocation/ABC/d/闪电全能下载.app")
        #expect(UpdateChecker.installTarget(for: moved).path == "/Applications/闪电全能下载.app")
        let normal = URL(fileURLWithPath: "/Applications/闪电全能下载.app")
        #expect(UpdateChecker.installTarget(for: normal) == normal)
    }

    @Test func permissionErrorsTriggerAdministratorInstall() {
        #expect(UpdateChecker.isPermissionDenied(CocoaError(.fileWriteNoPermission)))
        #expect(UpdateChecker.isPermissionDenied(NSError(domain: NSPOSIXErrorDomain, code: Int(EPERM))))
        #expect(UpdateChecker.isPermissionDenied(UpdateChecker.UpdateError("x", permissionDenied: true)))
        #expect(!UpdateChecker.isPermissionDenied(UpdateChecker.UpdateError("x")))
        #expect(!UpdateChecker.isPermissionDenied(CocoaError(.fileNoSuchFile)))
    }

    private func runReplace(_ command: String) throws -> Int32 {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", command]
        try process.run(); process.waitUntilExit()
        return process.terminationStatus
    }

    @Test func administratorCommandHandlesSpecialPathsAndRestoresOnFailure() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("闪电 update's test-\(UUID())")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let target = folder.appendingPathComponent("闪电 全能.app")
        let new = folder.appendingPathComponent("new's.app")
        let backup = folder.appendingPathComponent("旧版 backup.app")
        try Data("old".utf8).write(to: target)
        try Data("new".utf8).write(to: new)
        #expect(try runReplace(UpdateChecker.replaceCommand(target: target, app: new, backup: backup)) == 0)
        #expect(try Data(contentsOf: target) == Data("new".utf8))
        #expect(try Data(contentsOf: backup) == Data("old".utf8))

        try FileManager.default.removeItem(at: backup)
        let missing = folder.appendingPathComponent("missing.app")
        #expect(try runReplace(UpdateChecker.replaceCommand(target: target, app: missing, backup: backup)) != 0)
        #expect(try Data(contentsOf: target) == Data("new".utf8))
        #expect(!FileManager.default.fileExists(atPath: backup.path))
    }

    @Test func failedReplacementRestoresOldApp() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("shandian-update-test-\(UUID())")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let old = folder.appendingPathComponent("old.app")
        let backup = folder.appendingPathComponent("backup.app")
        let missing = folder.appendingPathComponent("missing.app")
        try Data("old".utf8).write(to: old)
        #expect(throws: (any Error).self) {
            try UpdateChecker.replaceApp(at: old, with: missing, backup: {
                try FileManager.default.moveItem(at: old, to: backup); return backup
            }, scheduleRelaunch: {})
        }
        #expect(try Data(contentsOf: old) == Data("old".utf8))
        #expect(!FileManager.default.fileExists(atPath: backup.path))
    }

    @Test func successfulReplacementKeepsBackupAndSchedulesRelaunch() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("shandian-update-test-\(UUID())")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let target = folder.appendingPathComponent("old.app")
        let new = folder.appendingPathComponent("new.app")
        let backup = folder.appendingPathComponent("backup.app")
        try Data("old".utf8).write(to: target)
        try Data("new".utf8).write(to: new)
        var scheduled = false
        try UpdateChecker.replaceApp(at: target, with: new, backup: {
            try FileManager.default.moveItem(at: target, to: backup); return backup
        }, scheduleRelaunch: { scheduled = true })
        #expect(scheduled)
        #expect(try Data(contentsOf: target) == Data("new".utf8))
        #expect(try Data(contentsOf: backup) == Data("old".utf8))
    }

    @Test func failedRelaunchRestoresOldAndKeepsNewForRetry() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("shandian-update-test-\(UUID())")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let old = folder.appendingPathComponent("old.app")
        let new = folder.appendingPathComponent("new.app")
        let backup = folder.appendingPathComponent("backup.app")
        try Data("old".utf8).write(to: old)
        try Data("new".utf8).write(to: new)
        #expect(throws: (any Error).self) {
            try UpdateChecker.replaceApp(at: old, with: new, backup: {
                try FileManager.default.moveItem(at: old, to: backup); return backup
            }, scheduleRelaunch: { throw UpdateChecker.UpdateError("test") })
        }
        #expect(try Data(contentsOf: old) == Data("old".utf8))
        #expect(try Data(contentsOf: new) == Data("new".utf8))
    }
}
