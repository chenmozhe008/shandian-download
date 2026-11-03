import XCTest
@testable import Shandian

final class DownloadFailureTests: XCTestCase {
    func testEngineMustReturnTaskID() throws {
        XCTAssertThrowsError(try EngineClient.validateTaskID(nil))
        XCTAssertThrowsError(try EngineClient.validateTaskID(" \n"))
        XCTAssertEqual(try EngineClient.validateTaskID("task-1"), "task-1")
    }
    private func message(_ error: String?) throws -> String {
        var object: [String: Any] = [
            "id": "test", "status": "error",
            "meta": ["req": ["url": "https://example.com/file"]],
            "progress": ["speed": 0, "downloaded": 0]
        ]
        if let error { object["shandianError"] = error }
        return try JSONDecoder().decode(EngineTask.self, from: JSONSerialization.data(withJSONObject: object)).failureText
    }

    func testHTTPFailuresHaveActionableExplanations() throws {
        XCTAssertEqual(try message("http request fail, code:429"), "服务器限制了下载频率，请稍后重试")
        XCTAssertEqual(try message("connection 2 failed: retries=3, status=403"), "服务器拒绝下载（链接可能已过期，或需要登录）")
        XCTAssertEqual(try message("status=401"), "服务器要求登录，请登录后重新获取下载链接")
    }

    func testLocalAndNetworkFailuresAndUnknownError() throws {
        XCTAssertEqual(try message("write: no space left on device"), "磁盘空间不足，请清理空间或更换保存位置后重试")
        XCTAssertEqual(try message("permission denied"), "无法写入保存位置，请检查权限或更换保存位置")
        XCTAssertEqual(try message("context deadline exceeded"), "连接服务器超时，请检查网络后重试")
        XCTAssertEqual(try message(nil), "下载出错，请检查网络和链接后重试")
    }
}
