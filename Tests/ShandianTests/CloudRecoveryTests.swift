import XCTest
@testable import Shandian

final class CloudRecoveryTests: XCTestCase {
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
        XCTAssertEqual(decoded.taskID, "task")
    }
}
