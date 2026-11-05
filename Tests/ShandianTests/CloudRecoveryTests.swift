import XCTest
@testable import Shandian

final class CloudRecoveryTests: XCTestCase {
    func testCloudFolderRejectsIncompleteOrPaginatedResults() throws {
        let valid: [String: Any] = ["id": "f1", "name": "video.mp4", "kind": "drive#file", "size": "12"]
        let entries = try PikPak.folderEntries(["files": [valid]])
        XCTAssertEqual(entries.first?.name, "video.mp4")
        XCTAssertEqual(entries.first?.size, 12)
        XCTAssertThrowsError(try PikPak.folderEntries([:]))
        XCTAssertThrowsError(try PikPak.folderEntries(["files": [["name": "missing id"]]]))
        XCTAssertThrowsError(try PikPak.folderEntries(["files": [valid], "next_page_token": "more"]))
        XCTAssertThrowsError(try PikPak.folderEntries(["files": Array(repeating: valid, count: 500)]))
    }
    @MainActor func testRecoveryIdentifiesOnlyUnrecordedTasksInIsolatedDirectory() throws {
        var job = DownloadStore.CloudJob(id: "cloud-1", title: "test", link: "magnet:test", dir: "/tmp/PikPak 下载/cloud-1", engineID: "original", partial: nil, total: 10)
        job.pulling = ["recorded"]
        func task(_ id: String, _ path: String) throws -> EngineTask {
            let object: [String: Any] = ["id": id, "status": "running", "meta": ["req": ["url": "https://example.com/file"], "opts": ["path": path]], "progress": ["speed": 0, "downloaded": 0]]
            return try JSONDecoder().decode(EngineTask.self, from: JSONSerialization.data(withJSONObject: object))
        }
        let tasks = try [task("recorded", job.dir), task("unrecorded", job.dir), task("other", "/tmp/other"), task("original", job.dir)]
        XCTAssertEqual(DownloadStore.unrecordedCloudPulls(job, tasks: tasks), ["unrecorded"])
        job.dir = "/tmp/ordinary-folder"
        XCTAssertTrue(DownloadStore.unrecordedCloudPulls(job, tasks: tasks).isEmpty)
    }
    @MainActor func testInterruptedPreparationRestoresCheckpointAndDeletesOnlyKnownTasks() async throws {
        var job = DownloadStore.CloudJob(id: "cloud-1", title: "test", link: "magnet:test", dir: "/tmp", engineID: "original", partial: nil, total: 10)
        job.preparingPulls = true
        job.pulling = ["new-1", "new-2"]
        let restored = try JSONDecoder().decode(DownloadStore.CloudJob.self, from: JSONEncoder().encode(job))
        XCTAssertEqual(restored.preparingPulls, true)
        var deleted: [String] = []
        try await DownloadStore.rollbackCloudPulls(restored.pulling ?? [], existing: ["original", "new-1"]) { deleted.append($0) }
        XCTAssertEqual(deleted, ["new-1"])
    }

    @MainActor func testRecoveryStopsWhenDeletingNewTaskFails() async throws {
        enum Failure: Error { case unavailable }
        var attempted: [String] = []
        do {
            try await DownloadStore.rollbackCloudPulls(["new-1", "new-2"], existing: ["new-1", "new-2"]) { id in
                attempted.append(id)
                throw Failure.unavailable
            }
            XCTFail("Recovery should wait when the engine cannot remove a task")
        } catch Failure.unavailable { }
        XCTAssertEqual(attempted, ["new-1"])
    }
    @MainActor func testCloudCopyRequiresExistingFullSizedRegularFile() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = dir.appendingPathComponent("file")
        try Data([1, 2, 3]).write(to: file)
        XCTAssertTrue(DownloadStore.cloudCopyCanBeRemoved(file: file, expectedSize: 3))
        XCTAssertFalse(DownloadStore.cloudCopyCanBeRemoved(file: file, expectedSize: 4))
        XCTAssertFalse(DownloadStore.cloudCopyCanBeRemoved(file: file, expectedSize: 0))
        XCTAssertFalse(DownloadStore.cloudCopyCanBeRemoved(file: dir, expectedSize: 3))
        XCTAssertFalse(DownloadStore.cloudCopyCanBeRemoved(file: dir.appendingPathComponent("missing"), expectedSize: 3))
    }

    @MainActor func testCloudCleanupRequiresSameAccountAndUnchangedLocalFile() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = dir.appendingPathComponent("copy")
        try Data("abc".utf8).write(to: file)
        XCTAssertTrue(DownloadStore.cloudCleanupAllowed(account: "a", currentAccount: "a", path: file.path, size: 3))
        XCTAssertFalse(DownloadStore.cloudCleanupAllowed(account: "a", currentAccount: "b", path: file.path, size: 3))
        XCTAssertFalse(DownloadStore.cloudCleanupAllowed(account: nil, currentAccount: "a", path: file.path, size: 3))
        XCTAssertFalse(DownloadStore.cloudCleanupAllowed(account: "a", currentAccount: "a", path: file.path, size: 4))
        try FileManager.default.removeItem(at: file)
        XCTAssertFalse(DownloadStore.cloudCleanupAllowed(account: "a", currentAccount: "a", path: file.path, size: 3))
    }
    @MainActor func testMultiFileCleanupRequiresEveryLocalFile() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let a = dir.appendingPathComponent("a"), b = dir.appendingPathComponent("b")
        try Data("abc".utf8).write(to: a)
        try Data("abcd".utf8).write(to: b)
        let files = [DownloadStore.CloudLocalFile(path: a.path, size: 3), DownloadStore.CloudLocalFile(path: b.path, size: 4)]
        XCTAssertTrue(DownloadStore.cloudCleanupAllowed(account: "x", currentAccount: "x", files: files))
        XCTAssertFalse(DownloadStore.cloudCleanupAllowed(account: "x", currentAccount: "x", files: []))
        try FileManager.default.removeItem(at: b)
        XCTAssertFalse(DownloadStore.cloudCleanupAllowed(account: "x", currentAccount: "x", files: files))
    }

    func testCloudContentMustContainEveryPulledFile() {
        // 文件夹里多出没拉回的小附件可以清理；拉回的文件对不上不行；同样大小的两个文件要对上两个
        XCTAssertTrue(PikPak.cloudHolds(pulled: [1000], cloud: [71, 1000]))
        XCTAssertTrue(PikPak.cloudHolds(pulled: [1000, 1000], cloud: [1000, 1000, 5]))
        XCTAssertFalse(PikPak.cloudHolds(pulled: [1000, 1000], cloud: [1000, 5]))
        XCTAssertFalse(PikPak.cloudHolds(pulled: [1000], cloud: [999]))
        XCTAssertFalse(PikPak.cloudHolds(pulled: [], cloud: [1000]))
        XCTAssertFalse(PikPak.cloudHolds(pulled: [0], cloud: [0]))
    }

    func testOnlyPreSendConnectionErrorsAreRetried() {
        XCTAssertTrue(PikPak.isRetryableConnectionError(URLError(.secureConnectionFailed)))
        XCTAssertTrue(PikPak.isRetryableConnectionError(URLError(.cannotConnectToHost)))
        XCTAssertFalse(PikPak.isRetryableConnectionError(URLError(.timedOut)))
        XCTAssertFalse(PikPak.isRetryableConnectionError(PikPak.PikPakError.notLoggedIn))
    }

    @MainActor func testAutoHandSkipsHalfDoneAndJustLaunched() {
        XCTAssertTrue(DownloadStore.canAutoHand(progress: 0, sinceLaunch: 300))
        XCTAssertTrue(DownloadStore.canAutoHand(progress: 0.49, sinceLaunch: 300))
        XCTAssertFalse(DownloadStore.canAutoHand(progress: 0.5, sinceLaunch: 300))
        XCTAssertFalse(DownloadStore.canAutoHand(progress: 0, sinceLaunch: 30))
    }

    @MainActor func testHandoffLabelLeavesOriginForMenuLogic() {
        var item = Item(id: "1", source: .engine("1"), title: "t", rawTitle: "t", origin: "种子文件", category: .movie, status: .running,
                        downloaded: 1, total: 2, speed: 0, file: nil, createdAt: Date(), thumbRemote: nil, linkKey: "k", location: nil)
        XCTAssertNil(DownloadStore.handoffLabel(item, pulling: false))
        item.fromThunder = true
        XCTAssertEqual(DownloadStore.handoffLabel(item, pulling: false), "迅雷素材已取回 · 继续下载")
        XCTAssertEqual(item.origin, "种子文件")
        item.status = .paused
        XCTAssertEqual(DownloadStore.handoffLabel(item, pulling: false), "迅雷素材已取回")
        XCTAssertEqual(DownloadStore.handoffLabel(item, pulling: true), "PikPak 中转 · 取回本机")
    }

    func testScheduleWindowHandlesOvernight() {
        XCTAssertTrue(DownloadStore.insideSchedule(hour: 2, from: 1, to: 8))
        XCTAssertFalse(DownloadStore.insideSchedule(hour: 8, from: 1, to: 8))
        XCTAssertTrue(DownloadStore.insideSchedule(hour: 23, from: 22, to: 6))
        XCTAssertTrue(DownloadStore.insideSchedule(hour: 3, from: 22, to: 6))
        XCTAssertFalse(DownloadStore.insideSchedule(hour: 12, from: 22, to: 6))
        XCTAssertTrue(DownloadStore.insideSchedule(hour: 12, from: 5, to: 5))
    }

    func testCloudBaseOnlyMatchesHandoffFolders() {
        XCTAssertEqual(HandoffFiles.cloudBase(of: URL(fileURLWithPath: "/m/PikPak 下载/cloud-1"))?.path, "/m")
        XCTAssertNil(HandoffFiles.cloudBase(of: URL(fileURLWithPath: "/m/movie")))
        XCTAssertNil(HandoffFiles.cloudBase(of: URL(fileURLWithPath: "/m/other/cloud-1")))
    }

    func testOnlySmallerPartialInsideBaseIsDisposable() throws {
        let base = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let finished = base.appendingPathComponent("迅雷下载/thunder-1/片名")
        try FileManager.default.createDirectory(at: finished, withIntermediateDirectories: true)
        try Data(repeating: 1, count: 300_000).write(to: finished.appendingPathComponent("a.mp4"))
        let partial = base.appendingPathComponent("片名")
        try FileManager.default.createDirectory(at: partial, withIntermediateDirectories: true)
        try Data(repeating: 1, count: 8192).write(to: partial.appendingPathComponent("a.mp4"))
        let size = HandoffFiles.allocatedSize(of: finished)
        XCTAssertTrue(HandoffFiles.isDisposablePartial(partial, base: base, finished: finished, finishedSize: size))
        XCTAssertFalse(HandoffFiles.isDisposablePartial(base, base: base, finished: finished, finishedSize: size))
        XCTAssertFalse(HandoffFiles.isDisposablePartial(finished, base: base, finished: finished, finishedSize: size))
        XCTAssertFalse(HandoffFiles.isDisposablePartial(base.appendingPathComponent("迅雷下载"), base: base, finished: finished, finishedSize: size))
        XCTAssertFalse(HandoffFiles.isDisposablePartial(partial, base: base, finished: finished, finishedSize: 1))
        XCTAssertFalse(HandoffFiles.isDisposablePartial(URL(fileURLWithPath: "/tmp/elsewhere"), base: base, finished: finished, finishedSize: size))
    }

    func testMoveThunderFolderClearsPartialAndKeepsLargerConflict() throws {
        let base = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let folder = base.appendingPathComponent("迅雷下载/thunder-1/片名")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try Data(repeating: 1, count: 300_000).write(to: folder.appendingPathComponent("a.mp4"))
        let target = base.appendingPathComponent("片名")
        try FileManager.default.createDirectory(at: target, withIntermediateDirectories: true)
        try Data(repeating: 1, count: 900_000).write(to: target.appendingPathComponent("big.mp4"))
        // 同名的比迅雷这份还大：收回时不能动；下完时另起名字，两份都保留
        var trashed: [URL] = []
        XCTAssertNil(try HandoffFiles.moveThunderFolder(folder, to: target, partial: target, keepName: true) { trashed.append($0) })
        XCTAssertTrue(trashed.isEmpty)
        let renamed = try XCTUnwrap(HandoffFiles.moveThunderFolder(folder, to: target, partial: target, keepName: false) { trashed.append($0) })
        XCTAssertEqual(renamed.lastPathComponent, "片名 (2)")
        XCTAssertTrue(FileManager.default.fileExists(atPath: target.appendingPathComponent("big.mp4").path))
        // 同名的是更小的半截：清掉后用原名
        let second = base.appendingPathComponent("迅雷下载/thunder-2/别的片")
        try FileManager.default.createDirectory(at: second, withIntermediateDirectories: true)
        try Data(repeating: 1, count: 300_000).write(to: second.appendingPathComponent("a.mp4"))
        let small = base.appendingPathComponent("别的片")
        try FileManager.default.createDirectory(at: small, withIntermediateDirectories: true)
        try Data(repeating: 1, count: 4096).write(to: small.appendingPathComponent("a.mp4"))
        let home = try XCTUnwrap(HandoffFiles.moveThunderFolder(second, to: small, partial: small, keepName: true) { try FileManager.default.removeItem(at: $0); trashed.append($0) })
        XCTAssertEqual(home.lastPathComponent, "别的片")
        XCTAssertEqual(trashed.count, 1)
    }

    func testOfflinesTodayCountsOnlyAcceptedTasksFromToday() {
        let now = Date()
        let yesterday = now.addingTimeInterval(-86_400)
        XCTAssertEqual(PikPak.offlinesToday([
            (now, "PHASE_TYPE_ERROR", "云盘文件已删除"),
            (now, "PHASE_TYPE_ERROR", "云盘文件已删除"),
            (now, "PHASE_TYPE_ERROR", "空间不足，请清理后再试"),
            (now, "PHASE_TYPE_RUNNING", ""),
            (now, "PHASE_TYPE_COMPLETE", ""),
            (yesterday, "PHASE_TYPE_COMPLETE", ""),
        ], now: now), 4)
    }

    func testTrashEmptiedOnlyWhenRoomIsNeeded() {
        let q = PikPak.Quota(limit: 6000, used: 5000, inTrash: 4800)
        XCTAssertTrue(PikPak.shouldEmptyTrash(q, need: 3000))
        XCTAssertTrue(PikPak.shouldEmptyTrash(q, need: 0))      // 磁力还没解析出大小
        XCTAssertFalse(PikPak.shouldEmptyTrash(q, need: 500))
        XCTAssertFalse(PikPak.shouldEmptyTrash(PikPak.Quota(limit: 6000, used: 5000, inTrash: 0), need: 3000))
    }

    func testOnlyOfflineLeftoversAreDeletedFromTrash() {
        XCTAssertEqual(PikPak.offlineLeftovers(trash: ["tutorial", "movie", "bunny"], taskFiles: ["movie", "bunny", "other"]), ["movie", "bunny"])
        XCTAssertEqual(PikPak.offlineLeftovers(trash: ["mine"], taskFiles: []), [])
    }

    func testSpaceCountsAsFreedOnlyWhenUsageDrops() {
        XCTAssertTrue(PikPak.spaceFreed(before: 5000, after: 4000, size: 1000))
        XCTAssertTrue(PikPak.spaceFreed(before: 5000, after: 4400, size: 1000))
        XCTAssertFalse(PikPak.spaceFreed(before: 5000, after: 5000, size: 1000))
        XCTAssertFalse(PikPak.spaceFreed(before: 5000, after: 6000, size: 1000))
    }

    func testMissingCloudDownloadURLIsAnError() throws {
        XCTAssertThrowsError(try PikPak.downloadURL([:]))
        XCTAssertEqual(try PikPak.downloadURL(["web_content_link": "", "medias": [["link": ["url": "https://example.com/file"]]]]), "https://example.com/file")
    }
    @MainActor func testPartialCreationRollsBackOnlyNewTasks() async throws {
        enum Failure: Error { case create }
        let files = [PikPak.RemoteFile(name: "one", size: 1, url: "https://example.com/1"),
                     PikPak.RemoteFile(name: "two", size: 2, url: "https://example.com/2")]
        var rolledBack: [String] = []
        do {
            _ = try await DownloadStore.createCloudPulls(files, create: { file in
                if file.name == "two" { throw Failure.create }
                return "new-task"
            }, rollback: { rolledBack.append($0) })
            XCTFail("Expected creation to fail")
        } catch Failure.create { }
        XCTAssertEqual(rolledBack, ["new-task"])
    }

    @MainActor func testSuccessfulCreationKeepsAllNewTasks() async throws {
        let files = [PikPak.RemoteFile(name: "one", size: 1, url: "https://example.com/1"),
                     PikPak.RemoteFile(name: "two", size: 2, url: "https://example.com/2")]
        var rolledBack: [String] = []
        let created = try await DownloadStore.createCloudPulls(files, create: { $0.name }, rollback: { rolledBack.append($0) })
        XCTAssertEqual(created, ["one", "two"])
        XCTAssertTrue(rolledBack.isEmpty)
    }

    @MainActor func testPendingTaskSurvivesCloudJobSerialization() throws {
        let pending = DownloadStore.PendingRecord(link: "magnet:?xt=urn:btih:example", headers: ["Example": "value"], name: "name", dir: "/tmp/download", selectFiles: [1, 3])
        let job = DownloadStore.CloudJob(id: "cloud-1", title: "example", link: pending.link, dir: pending.dir, engineID: nil, partial: nil, total: 0, pendingRecord: pending)
        let decoded = try JSONDecoder().decode(DownloadStore.CloudJob.self, from: JSONEncoder().encode(job))
        XCTAssertEqual(decoded.pendingRecord, pending)
        XCTAssertTrue(decoded.taskID.isEmpty)
    }

    @MainActor func testExistingCloudJobWithoutPendingRecordStillLoads() throws {
        let old = """
        {"id":"cloud-1","title":"example","link":"magnet:test","dir":"/tmp","total":0,"taskID":"task","fileID":"file","progress":0,"createdAt":0}
        """
        let decoded = try JSONDecoder().decode(DownloadStore.CloudJob.self, from: Data(old.utf8))
        XCTAssertNil(decoded.pendingRecord)
        XCTAssertNil(decoded.includesAllCloudFiles)
        XCTAssertNil(decoded.preparingPulls)
        XCTAssertEqual(decoded.taskID, "task")
    }
}
