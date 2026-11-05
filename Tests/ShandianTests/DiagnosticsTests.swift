import XCTest
@testable import Shandian

final class DiagnosticsTests: XCTestCase {
    func testPrivateDataIsRemoved() {
        let log = #"""
        open /Volumes/Jack Feng/movies/私人片名.2026/a.mkv failed
        path=/Users/someone/Downloads/私人片名 2.mp4
        url magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10&dn=私人片名
        hash 08ada5a7a6183aae1e09d831df6748d566095a10
        fetch https://cdn.example.com/private/file.zip?sign=abc
        data:application/x-bittorrent;base64,AAAABBBB
        "Authorization": Bearer123 token=abcdef
        engine-secret-token shows here
        tracker announce error code 500
        """#
        let out = Diagnostics.sanitize(log, secrets: ["engine-secret-token"])
        for leaked in ["私人片名", "someone", "Jack Feng", "08ada5a7", "sign=abc", "AAAABBBB", "Bearer123", "abcdef", "engine-secret-token"] {
            XCTAssertFalse(out.contains(leaked), leaked + "\n" + out)
        }
        XCTAssertTrue(out.contains("cdn.example.com"))
        XCTAssertTrue(out.contains("tracker announce error code 500"))
    }

    func testLastLines() {
        XCTAssertEqual(Diagnostics.lastLines("a\nb\nc", count: 2), "b\nc")
    }
}
