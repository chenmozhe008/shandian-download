import XCTest
@testable import Shandian

final class WeChatShareTests: XCTestCase {
    func testOnlyWeChatChannelShareLinksAreRecognized() {
        XCTAssertTrue(LinkKind.isWeChatChannelShare("https://weixin.qq.com/sph/AWgwLbQM41"))
        XCTAssertTrue(LinkKind.worthPopup("https://weixin.qq.com/sph/AWgwLbQM41"))
        XCTAssertFalse(LinkKind.worthPopup("https://weixin.qq.com/"))
        XCTAssertFalse(LinkKind.worthPopup("https://example.com/article"))
        XCTAssertTrue(LinkKind.isWeChatChannelShare("https://weixin.qq.com/sph/example?x=1"))
        XCTAssertFalse(LinkKind.isWeChatChannelShare("https://weixin.qq.com/other/example"))
        XCTAssertFalse(LinkKind.isWeChatChannelShare("https://example.com/sph/example"))
        XCTAssertFalse(LinkKind.isWeChatChannelShare("https://weixin.qq.com.example.com/sph/example"))
    }
}
