import XCTest
import CryptoKit
@testable import Shandian

@MainActor final class ThunderSafetyTests: XCTestCase {
    func testQuietPartialFileIsNotCompleted() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try Data(repeating: 1, count: 4096).write(to: dir.appendingPathComponent("partial"))
        var job = ThunderJob(id: "1", title: "test", link: "magnet:test", dir: dir.path, folder: dir.path,
                             hash: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", total: 100_000, createdAt: Date())
        ThunderBridge.measure(&job)
        job.lastChangeAt = Date(timeIntervalSinceNow: -60)
        ThunderBridge.measure(&job)
        XCTAssertFalse(job.done)
    }

    func testProgressMustMatchTaskHashAndCompleteExactly() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let info = Data("d6:lengthi1000e4:name4:teste".utf8)
        let hash = Insecure.SHA1.hash(data: info).map { String(format: "%02X", $0) }.joined()
        let torrent = Data("d4:info".utf8) + info + Data("e".utf8)
        try torrent.write(to: dir.appendingPathComponent(".magent_test.torrent"))
        let progress = try JSONSerialization.data(withJSONObject: ["subinfo0": ["checkedsize": 999, "dlsize": 1000]])
        try Data(progress.base64EncodedString().utf8).write(to: dir.appendingPathComponent(".\(hash).js"))
        XCTAssertNil(ThunderBridge.session(in: dir, hash: String(repeating: "B", count: 40)))
        var job = ThunderJob(id: "1", title: "test", link: "magnet:test", dir: dir.path, folder: dir.path,
                             hash: hash, total: 1000, createdAt: Date())
        ThunderBridge.measure(&job)
        XCTAssertEqual(job.checked, 999)
        XCTAssertFalse(job.done)
        let done = try JSONSerialization.data(withJSONObject: ["subinfo0": ["checkedsize": 1000, "dlsize": 1000]])
        try Data(done.base64EncodedString().utf8).write(to: dir.appendingPathComponent(".\(hash).js"))
        ThunderBridge.measure(&job)
        XCTAssertTrue(job.done)
    }

    func testUnchangedOrMissingDirectoryDoesNotProvePaused() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let job = ThunderJob(id: "1", title: "test", link: "magnet:test", dir: dir.path, folder: dir.path,
                             hash: nil, total: 0, createdAt: Date())
        let state = await ThunderBridge.state(of: job)
        XCTAssertEqual(state, .unknown)
    }
    func testPendingReclaimMetadataSurvivesRestart() throws {
        let pending = DownloadStore.PendingRecord(link: "magnet:test", headers: [:], name: nil, dir: "/tmp", selectFiles: nil,
                                                  originLink: "magnet:original", carryDownloaded: 500, carryTotal: 1000,
                                                  title: "test", verifyExisting: true, replacingThunderID: "thunder-1")
        let restored = try JSONDecoder().decode(DownloadStore.PendingRecord.self, from: JSONEncoder().encode(pending))
        XCTAssertEqual(restored, pending)
        XCTAssertEqual(restored.verifyExisting, true)
        XCTAssertEqual(restored.replacingThunderID, "thunder-1")
    }

    func testCloudJobPreservesThunderRecord() throws {
        let thunder = ThunderJob(id: "thunder-1", title: "test", link: "magnet:test", dir: "/tmp", hash: nil, total: 0, createdAt: Date())
        let cloud = DownloadStore.CloudJob(id: "cloud-1", title: "test", link: thunder.link, dir: "/tmp/cloud", engineID: nil,
                                           partial: nil, total: 0, thunderJob: thunder)
        let restored = try JSONDecoder().decode(DownloadStore.CloudJob.self, from: JSONEncoder().encode(cloud))
        XCTAssertEqual(restored.thunderJob, thunder)
        XCTAssertNil(restored.partial)
    }

    func testCompletedSmallFileDoesNotCompleteWholeTorrent() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let info = Data("d5:filesld6:lengthi200eed6:lengthi800eeee".utf8)
        let hash = Insecure.SHA1.hash(data: info).map { String(format: "%02X", $0) }.joined()
        try (Data("d4:info".utf8) + info + Data("e".utf8)).write(to: dir.appendingPathComponent(".magent_test.torrent"))
        func writeProgress(_ values: [[String: Int]]) throws {
            var object: [String: Any] = [:]
            for (index, value) in values.enumerated() { object["subinfo\(index)"] = value }
            let data = try JSONSerialization.data(withJSONObject: object)
            try Data(data.base64EncodedString().utf8).write(to: dir.appendingPathComponent(".\(hash).js"))
        }
        var job = ThunderJob(id: "1", title: "test", link: "magnet:test", dir: dir.path, folder: dir.path,
                             hash: hash, total: 0, createdAt: Date())
        try writeProgress([["checkedsize": 200, "dlsize": 200]])
        ThunderBridge.measure(&job)
        XCTAssertEqual(job.verifiedTotal, 1000)
        XCTAssertFalse(job.done)
        try writeProgress([["checkedsize": 200, "dlsize": 200], ["checkedsize": 800, "dlsize": 800]])
        ThunderBridge.measure(&job)
        XCTAssertTrue(job.done)
    }

    func testRelocatesReusedThunderTaskInEarlierHandoffFolder() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let first = root.appendingPathComponent("迅雷下载/thunder-first")
        let dataFolder = first.appendingPathComponent("actual-task")
        let latest = first.appendingPathComponent("迅雷下载/thunder-second/迅雷下载/thunder-third")
        try FileManager.default.createDirectory(at: dataFolder, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: latest, withIntermediateDirectories: true)
        let hash = String(repeating: "A", count: 40)
        let progress = try JSONSerialization.data(withJSONObject: ["subinfo0": ["checkedsize": 123, "dlsize": 1000]])
        try Data(progress.base64EncodedString().utf8).write(to: dataFolder.appendingPathComponent(".\(hash).js"))
        var job = ThunderJob(id: UUID().uuidString, title: "test", link: "magnet:test", dir: latest.path,
                             hash: hash, total: 1000, createdAt: Date())
        XCTAssertEqual(ThunderBridge.handoffBase(latest).standardizedFileURL, root.standardizedFileURL)
        XCTAssertEqual(ThunderBridge.locateNow(job).map { URL(fileURLWithPath: $0).resolvingSymlinksInPath() }, dataFolder.resolvingSymlinksInPath())
        job.id = UUID().uuidString
        ThunderBridge.measure(&job)
        XCTAssertEqual(job.folder.map { URL(fileURLWithPath: $0).resolvingSymlinksInPath() }, dataFolder.resolvingSymlinksInPath())
        XCTAssertEqual(job.checked, 123)
    }

    func testReclaimConfirmationMakesContinueThePrimaryChoice() {
        let alert = ThunderBridge.reclaimAlert(.unknown)
        XCTAssertEqual(alert.buttons.first?.title, "我已暂停，继续收回")
        XCTAssertEqual(alert.buttons.dropFirst().first?.title, "再等等")
        XCTAssertEqual(alert.buttons.first?.keyEquivalent, "\r")
    }

}
