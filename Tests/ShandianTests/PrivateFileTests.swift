import Foundation
import XCTest
@testable import Shandian

final class PrivateFileTests: XCTestCase {
    func testNewLogIsPrivateAndAppendPreservesContents() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let log = directory.appendingPathComponent("engine.log")
        let first = try PrivateFile.openForAppending(log)
        try first.write(contentsOf: Data("first".utf8)); try first.close()
        let second = try PrivateFile.openForAppending(log)
        try second.write(contentsOf: Data("second".utf8)); try second.close()
        XCTAssertEqual(try Data(contentsOf: log), Data("firstsecond".utf8))
        let permissions = try XCTUnwrap(FileManager.default.attributesOfItem(atPath: log.path)[.posixPermissions] as? NSNumber).intValue
        XCTAssertEqual(permissions & 0o077, 0)
    }

    func testOverwriteUsesPrivatePermissionsAndRetainsContent() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let target = directory.appendingPathComponent("pending.json")
        try Data("old".utf8).write(to: target)
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: target.path)
        try PrivateFile.write(Data("new".utf8), to: target)
        XCTAssertEqual(try Data(contentsOf: target), Data("new".utf8))
        let permissions = try XCTUnwrap(FileManager.default.attributesOfItem(atPath: target.path)[.posixPermissions] as? NSNumber).intValue
        XCTAssertEqual(permissions & 0o077, 0)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: directory.path), ["pending.json"])
    }

    func testFailedReplacementLeavesExistingDirectoryAndNoTemporaryFile() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let target = directory.appendingPathComponent("target")
        try FileManager.default.createDirectory(at: target, withIntermediateDirectories: true)
        XCTAssertThrowsError(try PrivateFile.write(Data("new".utf8), to: target))
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: directory.path), ["target"])
    }
}
