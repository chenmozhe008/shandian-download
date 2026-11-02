import XCTest
@testable import Shandian

final class DoubanTrailerTests: XCTestCase {
    func testTrailerLinksRouteToVideoDownloader() {
        let url = "https://movie.douban.com/trailer/326598/#content"
        XCTAssertEqual(LinkKind.of(url), .videoSite)
        XCTAssertTrue(LinkKind.worthPopup(url))
        XCTAssertEqual(VideoSiteRunner.siteName(url), "豆瓣")
        XCTAssertEqual(LinkKind.of("https://movie.douban.com/trailer/12345?x=1"), .videoSite)
    }

    func testOtherPagesAndLookalikeHostsAreNotVideo() {
        for url in ["https://movie.douban.com/subject/1292052/", "https://movie.douban.com/trailer/", "https://movie.douban.com/trailer/abc/", "https://movie.douban.com/trailer/123/other", "https://movie.douban.com.example.org/trailer/123/"] {
            XCTAssertEqual(LinkKind.of(url), .http)
        }
        XCTAssertFalse(LinkKind.worthPopup("https://movie.douban.com/subject/1292052/"))
    }
}
