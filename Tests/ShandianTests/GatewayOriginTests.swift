import XCTest
@testable import Shandian

final class GatewayOriginTests: XCTestCase {
    func testOnlyExplicitlyAllowedExtensionOriginIsAccepted() {
        XCTAssertTrue(Gateway.allowsExtensionOrigin(Gateway.extensionOrigin))
        XCTAssertTrue(Gateway.allowsExtensionOrigin("chrome-extension://onnpjcddpnfmaahjjdcibcofebfgopei"))
        XCTAssertTrue(Gateway.allowsExtensionOrigin("chrome-extension://achpnnoffkoeomkmofbcjpnmifaeanjg"))
        XCTAssertFalse(Gateway.allowsExtensionOrigin(nil))
        XCTAssertFalse(Gateway.allowsExtensionOrigin("http://127.0.0.1:6800"))
        XCTAssertFalse(Gateway.allowsExtensionOrigin("https://example.com"))
        XCTAssertFalse(Gateway.allowsExtensionOrigin("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))
        XCTAssertFalse(Gateway.allowsExtensionOrigin(Gateway.extensionOrigin + ".example.com"))
        XCTAssertFalse(Gateway.allowsExtensionOrigin(Gateway.edgeStoreOrigin + ".example.com"))
        XCTAssertFalse(Gateway.allowsExtensionOrigin(Gateway.chromeStoreOrigin + ".example.com"))
    }

    func testStoreExtensionOnlyHasAccessToItsTwoPostEndpoints() {
        for origin in [Gateway.edgeStoreOrigin, Gateway.chromeStoreOrigin] {
        XCTAssertTrue(Gateway.allowsExtensionRequest(origin: origin, method: "POST", path: "/extension/ping"))
        XCTAssertTrue(Gateway.allowsExtensionRequest(origin: origin, method: "POST", path: "/extension/add"))
        for path in ["/extension/pikpak", "/extension/wechat", "/extension/batch", "/extension/known", "/extension/status", "/extension/add/extra"] {
            XCTAssertFalse(Gateway.allowsExtensionRequest(origin: origin, method: "POST", path: path))
        }
        XCTAssertFalse(Gateway.allowsExtensionRequest(origin: origin, method: "GET", path: "/extension/add"))
        }
        XCTAssertTrue(Gateway.allowsExtensionRequest(origin: Gateway.extensionOrigin, method: "POST", path: "/extension/pikpak"))
    }

    func testStorePayloadRejectsOtherCapabilitiesAndUnsupportedLinks() {
        XCTAssertTrue(Gateway.validStorePayload(["url": "https://example.org/file.zip?token=abc"]))
        XCTAssertTrue(Gateway.validStorePayload(["url": "https://example.org/sintel.torrent"]))
        XCTAssertFalse(Gateway.validStorePayload(["url": "https://example.org/file.zip", "cookies": []]))
        for url in ["magnet:?xt=urn:btih:123", "ftp://example.org/file.zip", "https://user:pass@example.org/file.zip", "https://youtube.com/file.zip", "https://example.org/video.m3u8", "https://example.org/" + String(repeating: "a", count: 8193)] {
            XCTAssertFalse(Gateway.validStorePayload(["url": url]))
        }
    }
}
