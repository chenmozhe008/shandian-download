import XCTest
@testable import Shandian

final class EngineFilePathsTests: XCTestCase {
    func testRenamedHTTPFileIsIncludedWithoutChangingTorrentPaths() {
        let dir = URL(fileURLWithPath: "/tmp/downloads", isDirectory: true)
        let renamed = EngineFilePaths.candidates(directory: dir, responseName: "", relativeFile: "source.mp4", overrideName: "自定义.mp4", singleHTTP: true)
        XCTAssertEqual(renamed.first?.path, "/tmp/downloads/自定义.mp4")
        let torrent = EngineFilePaths.candidates(directory: dir, responseName: "folder", relativeFile: "movie.mp4", overrideName: "folder", singleHTTP: false)
        XCTAssertEqual(torrent.map(\.path), ["/tmp/downloads/folder/movie.mp4", "/tmp/downloads/movie.mp4"])
    }
}
