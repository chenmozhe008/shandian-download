import XCTest
@testable import Shandian

final class AIToolTests: XCTestCase {
    private let path = "/Applications/闪电 全能's.app/Contents/MacOS/shandian-mcp"

    func testConfigsAreValidJSONWithExactPath() throws {
        for tool in AITool.allCases where !tool.isCommand && !tool.isPath {
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

    func testChatGPTCopiesThePathAsIs() {
        XCTAssertEqual(AITool.chatGPT.snippet(path: path), path)
    }

    /// 翻译结果不能进判断：英文界面下会对不上，迅雷按钮也找不到（11-05 差点发出去）
    func testLocalizedTextIsNeverUsedInLogic() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let folder = root.appendingPathComponent("Sources/Shandian")
        for name in try FileManager.default.contentsOfDirectory(atPath: folder.path) where name.hasSuffix(".swift") {
            let text = try String(contentsOf: folder.appendingPathComponent(name), encoding: .utf8)
            for pattern in [#"[!=]= L\("#, #"(contains|hasPrefix|hasSuffix)\(L\("#, #"findButton\([^)]*L\("#, #"origin: [^,]*L\("#, #"site: L\("#] {
                XCTAssertNil(text.range(of: pattern, options: .regularExpression), "\(name): \(pattern)")
            }
        }
    }

    @MainActor func testNoticeAttentionWorksInBothLanguages() {
        XCTAssertTrue(DownloadStore.noticeNeedsAttention("添加失败：x"))
        XCTAssertTrue(DownloadStore.noticeNeedsAttention("Couldn't add: x"))
        XCTAssertFalse(DownloadStore.noticeNeedsAttention("Added: x"))
    }
}
