import XCTest
@testable import Shandian

final class RemainingTimeTests: XCTestCase {
    func testUnreliableOrHugeEstimatesAreHidden() {
        XCTAssertEqual(remainingText(.infinity), "")
        XCTAssertEqual(remainingText(Double.greatestFiniteMagnitude), "")
        XCTAssertEqual(remainingText(8 * 24 * 3600), "")
        XCTAssertEqual(remainingText(120), "剩余 2 分钟")
    }
}
