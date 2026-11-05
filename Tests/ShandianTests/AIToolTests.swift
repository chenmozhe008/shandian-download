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

    func testUnreachableDetectionSeparatesConnectionFromServerErrors() {
        XCTAssertTrue(LocalProxy.isUnreachable(URLError(.timedOut)))
        XCTAssertTrue(LocalProxy.isUnreachable(URLError(.cannotFindHost)))
        XCTAssertFalse(LocalProxy.isUnreachable(URLError(.badServerResponse)))
        XCTAssertFalse(LocalProxy.isUnreachable(PikPak.PikPakError.notLoggedIn))
        XCTAssertTrue(LocalProxy.isOverseas("https://www.youtube.com/watch?v=1"))
        XCTAssertFalse(LocalProxy.isOverseas("https://www.bilibili.com/video/1"))
        XCTAssertFalse(LocalProxy.isOverseas("magnet:?xt=urn:btih:abc"))
        XCTAssertTrue(YtDlp.isConnectionError("Unable to download webpage: The read operation timed out"))
        XCTAssertFalse(YtDlp.isConnectionError("HTTP Error 403: Forbidden"))
    }

    func testThunderLogTellsWhetherTheTaskWasCreated() {
        let ok = #"11/5/26 14:00:53 - [report] taskevent/createtask, data: ["taskid": "1284", "createfrom": "scheme", "result": "success"]"#
        let refused = #"11/5/26 14:11:50 - [report] taskevent/createtask, data: ["result": "fail", "taskid": "0", "createfrom": "scheme"]"#
        XCTAssertEqual(ThunderBridge.submitResult(inLog: ok), .submitted)
        XCTAssertEqual(ThunderBridge.submitResult(inLog: refused), .alreadyExists)
        XCTAssertEqual(ThunderBridge.submitResult(inLog: "11/5/26 14:01:24 - [log] NSWindow >> didBecomeKeyNotification: title=下载任务已存在"), .alreadyExists)
        XCTAssertNil(ThunderBridge.submitResult(inLog: "11/5/26 14:00:52 - [log] NSWindow >> didBecomeKeyNotification: title=新建任务"))
    }

    /// 收回时文件夹会先搬回保存目录，种子路径要跟着变，否则任务建不出来、一直停在「请查看迅雷」
    func testReclaimFindsTorrentAfterFolderMoves() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("reclaim 测试-\(UUID().uuidString)")
        let old = root.appendingPathComponent("迅雷下载/thunder-1/影片")
        try FileManager.default.createDirectory(at: old, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let torrent = old.appendingPathComponent(".magent_1.torrent").path
        try Data("d".utf8).write(to: URL(fileURLWithPath: torrent))
        XCTAssertEqual(LinkKind.of(torrent), .torrentFile)
        let moved = try XCTUnwrap(HandoffFiles.moveThunderFolder(old, to: root.appendingPathComponent("影片"), partial: nil, keepName: true))
        XCTAssertNil(LinkKind.of(torrent))   // 旧路径确实失效了，这就是原来的 bug
        XCTAssertEqual(LinkKind.of(ThunderBridge.pathAfterMove(torrent, from: old.path, to: moved.path)), .torrentFile)
        XCTAssertEqual(ThunderBridge.pathAfterMove("magnet:?xt=1", from: old.path, to: moved.path), "magnet:?xt=1")
    }

    /// 再次交给迅雷：文件夹回到迅雷记着的位置，种子还在；中转目录被清掉了也能重建
    func testHandBackRestoresThunderFolderWithItsTorrent() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("handback 测试-\(UUID().uuidString)")
        let home = root.appendingPathComponent("影片")
        let original = root.appendingPathComponent("迅雷下载/thunder-1/影片")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try Data("d".utf8).write(to: home.appendingPathComponent(".magent_1.torrent"))
        try HandoffFiles.moveBackForThunder(home, to: original)
        XCTAssertTrue(FileManager.default.fileExists(atPath: original.appendingPathComponent(".magent_1.torrent").path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: home.path))
        XCTAssertNoThrow(try HandoffFiles.moveBackForThunder(original, to: original))
    }

    func testThunderRowReadsNameFromRunningAndPausedRows() {
        XCTAssertEqual(ThunderBridge.taskRow(texts: ["已暂停", "影片"])?.name, "影片")
        let running = ThunderBridge.taskRow(texts: ["1.2MB/s", "  剩余10分33秒", "影片"])
        XCTAssertEqual(running?.name, "影片")
        XCTAssertEqual(running?.status, "1.2MB/s")
        XCTAssertNil(ThunderBridge.taskRow(texts: ["只有一段"]))
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
            for pattern in [#"[!=]= L\("#, #"(contains|hasPrefix|hasSuffix)\(L\("#, #"findButton\([^)]*L\("#, #"origin: [^,\n]*L\("#, #"site: L\("#] {
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
