import Foundation
import XCTest
@testable import Shandian

final class VerificationStateTests: XCTestCase {
    func testEngineVerificationStateAndOldEngineCompatibility() throws {
        func task(_ field: String) throws -> EngineTask {
            let json = """
            {"id":"test","status":"running","meta":{"req":{"url":"magnet:?xt=test"}},
             "progress":{"speed":0,"downloaded":0}\(field)}
            """
            return try JSONDecoder().decode(EngineTask.self, from: Data(json.utf8))
        }
        XCTAssertEqual(try task(",\"shandianVerification\":\"checking\"").shandianVerification, "checking")
        XCTAssertEqual(try task(",\"shandianVerification\":\"complete\"").shandianVerification, "complete")
        XCTAssertEqual(try task(",\"shandianVerification\":\"failed\"").shandianVerification, "failed")
        XCTAssertNil(try task("").shandianVerification)
    }
}
