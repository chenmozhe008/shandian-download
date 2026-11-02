import Foundation
import XCTest
@testable import Shandian

final class StreamTests: XCTestCase {
    func testStreamClassificationAndSafeReferer() {
        XCTAssertEqual(LinkKind.of("https://example.com/a.M3U8?signature=redacted"), .videoSite)
        XCTAssertEqual(LinkKind.of("https://example.com/a.mpd?x=1"), .videoSite)
        XCTAssertEqual(LinkKind.of("https://example.com/a.mp4"), .http)
        XCTAssertEqual(VideoSiteRunner.safeRequestArguments(["Referer": "https://example.com/page?private=1#x", "Cookie": "secret"]), ["--referer", "https://example.com/page"])
        XCTAssertEqual(VideoSiteRunner.safeRequestArguments(["Referer": "https://example.com/\r\nInjected: x"]), [])
    }

    func testTemporaryCookieFileIsHostScopedAndPrivate() throws {
        let file = try XCTUnwrap(VideoSiteRunner.temporaryCookieFile(["Cookie": "fixture=allowed; second=value"], for: "https://media.example.com/watch"))
        defer { try? FileManager.default.removeItem(at: file) }
        let text = try String(contentsOf: file, encoding: .utf8)
        XCTAssertTrue(text.contains("media.example.com\tFALSE\t/\tTRUE\t0\tfixture\tallowed"))
        let attributes = try FileManager.default.attributesOfItem(atPath: file.path)
        XCTAssertEqual((attributes[.posixPermissions] as? NSNumber)?.intValue, 0o600)
        XCTAssertNil(VideoSiteRunner.temporaryCookieFile(["Cookie": "bad\nname=value"], for: "https://media.example.com/"))
    }

    func testAuthenticatedExtensionlessStreamWhenFixtureIsAvailable() throws {
        guard let base = ProcessInfo.processInfo.environment["SHANDIAN_TEST_STREAM_BASE"] else { throw XCTSkip("仅在显式提供本地流测试服务时执行") }
        let finished = expectation(description: "带 Cookie 的无后缀 HLS")
        let runner = VideoSiteRunner()
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("shandian-stream-auth-" + UUID().uuidString)
        let job = VideoJob(url: base + "/protected/stream", title: "fixture", site: "测试")
        let temp = FileManager.default.temporaryDirectory
        let before = Set((try? FileManager.default.contentsOfDirectory(atPath: temp.path)) ?? [])
        runner.start(job, into: directory, headers: ["Referer": base + "/page", "Cookie": "fixture=allowed"]) { result in
            guard result.state != .running else { return }
            XCTAssertEqual(result.state, .done, result.error ?? "")
            XCTAssertNotNil(result.filePath)
            if let path = result.filePath { XCTAssertTrue(FileManager.default.fileExists(atPath: path)) }
            finished.fulfill()
        }
        wait(for: [finished], timeout: 45)
        let after = Set((try? FileManager.default.contentsOfDirectory(atPath: temp.path)) ?? [])
        XCTAssertTrue(after.subtracting(before).filter { $0.hasPrefix("shandian-cookies-") }.isEmpty)
    }

    func testProtectedStreamWithoutCookieFailsWhenFixtureIsAvailable() throws {
        guard let base = ProcessInfo.processInfo.environment["SHANDIAN_TEST_STREAM_BASE"] else { throw XCTSkip("仅在显式提供本地流测试服务时执行") }
        let finished = expectation(description: "无登录信息不得误报成功")
        let runner = VideoSiteRunner()
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("shandian-stream-denied-" + UUID().uuidString)
        let job = VideoJob(url: base + "/protected/stream", title: "fixture", site: "测试")
        runner.start(job, into: directory) { result in
            guard result.state != .running else { return }
            XCTAssertEqual(result.state, .failed)
            XCTAssertNil(result.filePath)
            finished.fulfill()
        }
        wait(for: [finished], timeout: 45)
    }

    func testRealStreamDownloadWhenFixtureIsAvailable() throws {
        guard let base = ProcessInfo.processInfo.environment["SHANDIAN_TEST_STREAM_BASE"] else { throw XCTSkip("仅在显式提供本地流测试服务时执行") }
        for name in ["encrypted.m3u8", "stream.mpd"] {
            let finished = expectation(description: name)
            let runner = VideoSiteRunner()
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent("shandian-stream-" + UUID().uuidString)
            let job = VideoJob(url: base + "/" + name, title: name, site: "测试")
            runner.start(job, into: directory, headers: ["Referer": base + "/page"]) { result in
                guard result.state != .running else { return }
                XCTAssertEqual(result.state, .done, result.error ?? "")
                XCTAssertNotNil(result.filePath)
                if let path = result.filePath { XCTAssertTrue(FileManager.default.fileExists(atPath: path)) }
                finished.fulfill()
            }
            wait(for: [finished], timeout: 45)
        }
    }
}
