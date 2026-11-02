import XCTest
@testable import Shandian

final class WeChatChannelsTests: XCTestCase {
    func testOnlyHTTPSOnTencentHostsIsAccepted() {
        XCTAssertTrue(WeChatChannels.trusted(URL(string: "https://channels.weixin.qq.com/finder-preview")!, exact: "channels.weixin.qq.com"))
        XCTAssertTrue(WeChatChannels.trusted(URL(string: "https://video.qq.com/file")!))
        XCTAssertFalse(WeChatChannels.trusted(URL(string: "https://qq.com.evil.example/file")!))
        XCTAssertFalse(WeChatChannels.trusted(URL(string: "http://video.qq.com/file")!))
        XCTAssertFalse(WeChatChannels.trusted(URL(string: "https://user@channels.weixin.qq.com/file")!))
    }
    func testInvalidPlayableFailsWithoutNetworkRequest() async {
        do {
            _ = try await WeChatChannels.download(playable: "https://evil.example/", title: "test", directory: FileManager.default.temporaryDirectory)
            XCTFail("should reject an unexpected host")
        } catch { XCTAssertTrue(error is WeChatChannels.Failure) }
    }
}
