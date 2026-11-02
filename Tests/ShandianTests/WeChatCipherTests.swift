import CryptoKit
import XCTest
@testable import Shandian

final class WeChatCipherTests: XCTestCase {
    func testKeystreamMatchesReferenceWASM() {
        var bytes = Data(repeating: 0, count: 131_072)
        var cipher = WeChatCipher(seed: 12345)
        cipher.apply(to: &bytes)
        let hash = SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
        XCTAssertEqual(hash, "932e7fed86b329540fd72561f3401d0f11412b1fe976217efa016e2a9a17fcbc")
    }
    func testOnlyFirst128KiBAreChanged() {
        var bytes = Data(repeating: 0x55, count: 131_100)
        var cipher = WeChatCipher(seed: 54321)
        cipher.apply(to: &bytes)
        XCTAssertEqual(bytes.suffix(28), Data(repeating: 0x55, count: 28))
        var second = WeChatCipher(seed: 54321)
        second.apply(to: &bytes)
        XCTAssertEqual(bytes, Data(repeating: 0x55, count: 131_100))
    }
}
