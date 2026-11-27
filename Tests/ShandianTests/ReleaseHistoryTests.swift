import XCTest
@testable import Shandian

final class ReleaseHistoryTests: XCTestCase {
    func testPublishedAndDevelopmentSameVersionRemainDistinct() throws {
        let json = """
        [{"tag_name":"v0.18.5","draft":false,"prerelease":false,"html_url":"https://github.com/example/releases/v0.18.5","published_at":"2026-11-02T00:00:00Z","body":"## 更新内容\n\n- 正式版内容"}, {"tag_name":"win-v1.0","draft":false,"prerelease":true}]
        """.replacingOccurrences(of: "\n\n- 正式版内容", with: "\\n\\n- 正式版内容")
        let online = try XCTUnwrap(ReleaseHistory.online(Data(json.utf8)))
        let local = ReleaseHistory.local("## 0.18.5 · 2026-11-02（开发版）\n- 本机改动", includeDevelopment: true)
        let rows = ReleaseHistory.merge(local: local, online: online)
        XCTAssertEqual(rows.count, 2)
        XCTAssertTrue(rows[0].isDevelopment)
        XCTAssertEqual(rows[0].notes, ["本机改动"])
        XCTAssertEqual(rows[1].notes, ["正式版内容"])
        XCTAssertNotNil(rows[1].page)
    }

    func testOfflineLocalHistoryAndPublishedMerge() {
        let local = ReleaseHistory.local("## 0.18.4 · 2026-11-01\n- 本地记录", includeDevelopment: false)
        XCTAssertEqual(ReleaseHistory.merge(local: local, online: []), local)
        let published = HistoryEntry(version: "0.18.4", date: "2026-11-02", notes: ["线上"], page: URL(string: "https://github.com/example")!, isDevelopment: false)
        let merged = ReleaseHistory.merge(local: local, online: [published])
        XCTAssertEqual(merged.count, 1)
        XCTAssertEqual(merged[0].notes, ["线上"])
        XCTAssertNotNil(merged[0].page)
    }

    func testReleaseWithoutOnlineNotesKeepsLocalNotes() {
        let local = ReleaseHistory.local("## 0.18.4 · 2026-11-01\n- 本地记录", includeDevelopment: false)
        let empty = HistoryEntry(version: "0.18.4", date: "2026-11-02", notes: [], page: nil, isDevelopment: false)
        XCTAssertEqual(ReleaseHistory.merge(local: local, online: [empty])[0].notes, ["本地记录"])
    }

    func testInvalidResponseDoesNotReplaceCache() {
        XCTAssertNil(ReleaseHistory.online(Data("{}".utf8)))
    }

    func testDevelopmentBuildDoesNotRelabelBundledReleaseHistory() {
        let text = """
        ## 0.22.7 · 2026-11-23（开发版）
        - 首页标签顺序
        ## 0.22.6 · 2026-11-23
        - 飞书下载
        - 百度网盘
        """
        let dev = ReleaseHistory.local(text, includeDevelopment: true)
        XCTAssertEqual(dev.filter(\.isDevelopment).map(\.notes), [["首页标签顺序"]])
        XCTAssertEqual(dev.map(\.date), ["2026-11-23", "2026-11-23"])
        let publishedOnly = ReleaseHistory.local(text, includeDevelopment: false)
        XCTAssertEqual(publishedOnly, Array(dev.dropFirst()))
        XCTAssertEqual(publishedOnly.map(\.id), ["0.22.6-release"])
    }

    func testBilingualReleaseNotesAcceptCurlyApostrophe() {
        let text = """
        ## 更新内容
        - 新功能
        ## What’s New
        - New feature
        ## 安装
        - 下载
        """
        XCTAssertEqual(UpdateChecker.notes(text, isEnglish: false), ["新功能"])
        XCTAssertEqual(UpdateChecker.notes(text, isEnglish: true), ["New feature"])
    }
}
