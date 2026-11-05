import XCTest
@testable import Shandian

final class AIToolTests: XCTestCase {
    private let path = "/Applications/闪电 全能's.app/Contents/MacOS/shandian-mcp"

    func testConfigsAreValidJSONWithExactPath() throws {
        for tool in AITool.allCases where !tool.isCommand {
            let object = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(tool.snippet(path: path).utf8)) as? [String: Any], tool.rawValue)
            let servers = try XCTUnwrap(object[tool == .vscode ? "servers" : "mcpServers"] as? [String: Any], tool.rawValue)
            let entry = try XCTUnwrap(servers["shandian"] as? [String: Any], tool.rawValue)
            XCTAssertEqual(entry["command"] as? String, path)
            if tool == .vscode { XCTAssertEqual(entry["type"] as? String, "stdio") }
        }
    }

    func testCommandsSurviveShellQuoting() throws {
        for tool in AITool.allCases where tool.isCommand {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/bin/sh")
            // 把工具名换成 printf，看 shell 拆出来的参数里路径是否完整
            let name = tool.snippet(path: path).components(separatedBy: " ")[0]
            let script = "\(name)() { printf '%s\\n' \"$@\"; }; " + tool.snippet(path: path)
            process.arguments = ["-c", script]
            let pipe = Pipe(); process.standardOutput = pipe
            try process.run(); process.waitUntilExit()
            let output = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            XCTAssertTrue(output.components(separatedBy: "\n").contains(path), tool.rawValue)
        }
    }

    @MainActor func testNoticeAttentionWorksInBothLanguages() {
        XCTAssertTrue(DownloadStore.noticeNeedsAttention("添加失败：x"))
        XCTAssertTrue(DownloadStore.noticeNeedsAttention("Couldn't add: x"))
        XCTAssertFalse(DownloadStore.noticeNeedsAttention("Added: x"))
    }
}
