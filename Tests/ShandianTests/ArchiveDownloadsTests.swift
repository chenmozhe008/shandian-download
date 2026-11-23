import Foundation
import XCTest
@testable import Shandian

@MainActor
final class ArchiveDownloadsTests: XCTestCase {
    func testBatchFolderKeepsOriginalFilesAndExistingFolder() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let stage = root.appendingPathComponent(".stage")
        let existing = root.appendingPathComponent("素材")
        try FileManager.default.createDirectory(at: stage, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: existing, withIntermediateDirectories: false)
        try Data([1]).write(to: stage.appendingPathComponent("视频.mp4"))
        try Data([2]).write(to: stage.appendingPathComponent("音频.wav"))

        let output = try ArchiveDownloads.saveOutput(stage, to: root, folderName: "素材", singleFileName: nil)

        XCTAssertEqual(output.lastPathComponent, "素材 (2)")
        XCTAssertEqual(Set(try FileManager.default.contentsOfDirectory(atPath: output.path)), ["视频.mp4", "音频.wav"])
        XCTAssertTrue(try FileManager.default.contentsOfDirectory(atPath: existing.path).isEmpty)
    }

    func testSingleOutputSavesOriginalBesideExistingFile() throws {
        let fm = FileManager.default
        let root = fm.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let stage = root.appendingPathComponent(".stage")
        try fm.createDirectory(at: stage, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: root) }
        let existing = root.appendingPathComponent("视频.mp4")
        try Data([1]).write(to: existing)
        try Data([2]).write(to: stage.appendingPathComponent("视频.mp4"))

        let output = try ArchiveDownloads.saveOutput(stage, to: root, folderName: "文档", singleFileName: "视频.mp4")
        XCTAssertEqual(output.lastPathComponent, "视频 (2).mp4")
        XCTAssertEqual(try Data(contentsOf: output), Data([2]))
        XCTAssertEqual(try Data(contentsOf: existing), Data([1]))
        XCTAssertEqual(Set(try fm.contentsOfDirectory(atPath: root.path)), ["视频.mp4", "视频 (2).mp4"])
    }

    func testCompletedAndInterruptedArchivesAreRecoveredAsOrdinaryItems() async throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let completed = folder.appendingPathComponent("已完成.zip")
        let archiveID = UUID()
        let activeID = UUID()
        let history: [[String: Any]] = [
            ["id": archiveID.uuidString, "signature": "completed-digest", "title": completed.lastPathComponent,
             "directory": folder.absoluteString, "count": 2, "expectedBytes": 40, "createdAt": 1.0,
             "state": "done", "files": ["0": 10, "1": 30], "file": completed.absoluteString, "archiveBytes": 96],
            ["id": activeID.uuidString, "signature": "interrupted-digest", "title": "未完成.zip",
             "directory": folder.absoluteString, "count": 3, "expectedBytes": 200, "createdAt": 2.0,
             "state": "running", "files": ["0": 40], "archiveBytes": 0]
        ]
        try JSONSerialization.data(withJSONObject: history).write(to: folder.appendingPathComponent("archive-downloads.json"))
        let downloads = ArchiveDownloads(directory: folder)
        let items = downloads.items
        XCTAssertEqual(items.count, 2)
        XCTAssertEqual(items[0].source, .archive(archiveID))
        XCTAssertEqual(items[0].status, .done)
        XCTAssertEqual(items[0].file, completed)
        XCTAssertEqual(items[0].total, 96)
        XCTAssertFalse(items[0].isActive)
        XCTAssertEqual(items[1].source, .archive(activeID))
        if case .failed = items[1].status {} else { XCTFail("Interrupted archive should be recoverable from the archive list") }
        XCTAssertEqual(items[1].downloaded, 40)
        XCTAssertEqual(items[1].archiveProgress?.saved, 1)
        var notice: String?
        downloads.onNotice = { notice = $0 }
        downloads.resume(activeID)
        XCTAssertNotNil(notice)
        downloads.remove(archiveID)
        XCTAssertEqual(ArchiveDownloads(directory: folder).items.map(\.id), [activeID.uuidString])
    }
}
