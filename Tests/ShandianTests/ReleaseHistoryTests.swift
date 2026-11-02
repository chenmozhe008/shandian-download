import XCTest
@testable import Shandian

final class ReleaseHistoryTests: XCTestCase {
    func testPublishedAndDevelopmentSameVersionRemainDistinct() throws {
        let json = """
        [{"tag_name":"v0.18.5","draft":false,"prerelease":false,"html_url":"https://github.com/example/releases/v0.18.5","published_at":"2026-11-02T00:00:00Z","body":"## 更新内容\n\n- 正式版内容"}, {"tag_name":"win-v1.0","draft":false,"prerelease":true}]
        """.replacingOccurrences(of: "\n\n- 正式版内容", with: "\\n\\n- 正式版内容")
        let online = try XCTUnwrap(ReleaseHistory.online(Data(json.utf8)))
        let local = ReleaseHistory.local("## 0.18.5 · 2026-11-02\n- 本机改动", currentVersion: "0.18.5", isDevelopment: true)
        let rows = ReleaseHistory.merge(local: local, online: online)
        XCTAssertEqual(rows.count, 2)
        XCTAssertTrue(rows[0].isDevelopment)
        XCTAssertEqual(rows[0].notes, ["本机改动"])
        XCTAssertEqual(rows[1].notes, ["正式版内容"])
        XCTAssertNotNil(rows[1].page)
    }

    func testOfflineLocalHistoryAndPublishedMerge() {
        let local = ReleaseHistory.local("## 0.18.4 · 2026-11-01\n- 本地记录", currentVersion: "0.18.4", isDevelopment: false)
        XCTAssertEqual(ReleaseHistory.merge(local: local, online: []), local)
        let published = HistoryEntry(version: "0.18.4", date: "2026-11-02", notes: ["线上"], page: URL(string: "https://github.com/example")!, isDevelopment: false)
        let merged = ReleaseHistory.merge(local: local, online: [published])
        XCTAssertEqual(merged.count, 1)
        XCTAssertEqual(merged[0].notes, ["本地记录"])
        XCTAssertNotNil(merged[0].page)
    }

    func testInvalidResponseDoesNotReplaceCache() {
        XCTAssertNil(ReleaseHistory.online(Data("{}".utf8)))
    }
}
