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
