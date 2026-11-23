import XCTest
@testable import Shandian

final class BaiduShareTests: XCTestCase {
    func testSharePagesKeepTheirAccessCodeAndShowGuidance() {
        let shares = [
            "https://pan.baidu.com/s/1testshare?pwd=abcd",
            "https://pan.baidu.com/share/init?surl=testshare",
            "https://pan.baidu.com/share/link?shareid=123&uk=456",
            "https://yun.baidu.com/s/1testshare"
        ]
        for share in shares {
            XCTAssertEqual(LinkKind.baiduShareURL(share)?.absoluteString, share)
            XCTAssertTrue(LinkKind.worthPopup(share))
        }
    }

    func testBaiduDirectLinksKeepTheirDownloadRoute() {
        let directLinks = [
            "https://d.pcs.baidu.com/rest/2.0/pcs/file?method=download&path=%2Ffile.zip",
            "https://example.baidupcs.com/file/test",
            "https://pan.baidu.com/files/test.zip"
        ]
        for link in directLinks {
            XCTAssertNil(LinkKind.baiduShareURL(link))
            XCTAssertEqual(LinkKind.of(link), .http)
            XCTAssertTrue(LinkKind.worthPopup(link))
        }
    }

    func testBaiduHomePageAndLookalikeHostsDoNotTriggerShareGuidance() {
        for link in [
            "https://pan.baidu.com/disk/main",
            "https://www.baidu.com/s?wd=test",
            "https://pan.baidu.com.example.org/s/1testshare",
            "https://pan.baidu.com@example.org/s/1testshare"
        ] {
            XCTAssertNil(LinkKind.baiduShareURL(link))
            XCTAssertFalse(LinkKind.worthPopup(link))
        }
    }

    func testPastingShareTextAndAnotherFileExtractsBothURLs() {
        let share = "https://pan.baidu.com/s/1testshare?pwd=abcd"
        let file = "https://example.org/file.zip"
        XCTAssertEqual(LinkKind.pastedLinks(from: "\(share) 提取码：abcd\n\(file)"), [share, file])
        XCTAssertEqual(LinkKind.pastedLinks(from: "链接：\(share)\n提取码：abcd"), [share])
    }

    func testPastedBaiduSharesCarryTheirOwnAccessCodes() {
        let first = "https://pan.baidu.com/s/1test"
        let second = "https://pan.baidu.com/s/2test?pwd=5678"
        let direct = "https://example.org/test.zip"
        let pasted = "链接：\(first) 提取码：aB12\n\(second) 提取码：old1\n\(direct) 提取码：zZ89"
        XCTAssertEqual(LinkKind.pastedLinks(from: pasted), [first + "?pwd=aB12", second, direct])
    }

    @MainActor
    func testOfficialDownloaderArgumentsKeepShareCodeAndSeparateDestination() {
        let share = URL(string: "https://pan.baidu.com/s/test?pwd=aB12")!
        let destination = URL(fileURLWithPath: "/tmp/百度网盘 测试", isDirectory: true)
        let id = UUID()
        XCTAssertEqual(BaiduDownloads.arguments(share: share, local: destination, id: id), [
            "download", share.absoluteString, destination.path + "/", "-t", "Shandian/\(id.uuidString)", "--no-check-update", "--json"
        ])
    }

    func testFinishedFileKeepsItsNameWithoutReplacingExistingDownload() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let stage = root.appendingPathComponent(".stage")
        let content = Data([0x01, 0x02, 0x03])
        try FileManager.default.createDirectory(at: stage, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let existing = root.appendingPathComponent("sample.zip")
        try Data([0x04]).write(to: existing)
        try content.write(to: stage.appendingPathComponent("sample.zip"))

        let result = try BaiduDownloads.commit(staging: stage, to: root)

        XCTAssertEqual(result.file.lastPathComponent, "sample (2).zip")
        XCTAssertEqual(result.bytes, 3)
        XCTAssertEqual(try Data(contentsOf: result.file), content)
        XCTAssertEqual(try Data(contentsOf: existing), Data([0x04]))
    }

    @MainActor
    func testCLIDownloadBecomesCompletedAndKeepsCodeOutOfHistory() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let helper = root.appendingPathComponent("bdpan")
        let fixture = """
        #!/bin/sh
        if [ "$1" = transfer ] && [ "$2" = list ]; then
            printf '%s' '{"has_more":false,"items":[{"name":"sample.txt","size":20,"is_dir":false}]}'
            exit 0
        fi
        [ "$1" = download ] && [ "$2" = "https://pan.baidu.com/s/test?pwd=aB12" ] || exit 1
        printf 'downloaded' > "${3}sample.txt.part"
        while [ ! -f "${3}../finish" ]; do sleep 0.05; done
        printf 'downloaded' >> "${3}sample.txt.part"
        mv "${3}sample.txt.part" "${3}sample.txt"
        printf '%s' '{"code":0}'
        """
        try Data(fixture.utf8).write(to: helper)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: helper.path)
        let storage = root.appendingPathComponent("history.json")
        let store = BaiduDownloads(storage: storage)
        defer { store.shutdown() }

        store.start(share: URL(string: "https://pan.baidu.com/s/test?pwd=aB12")!, directory: root, executable: helper)
        let id = try XCTUnwrap(store.items.first?.id)
        let partial = root.appendingPathComponent(".shandian-baidu-\(id)/sample.txt.part")
        for _ in 0..<40 where store.items.first?.total != 20 || !FileManager.default.fileExists(atPath: partial.path) {
            try await Task.sleep(for: .milliseconds(25))
        }
        let sampledAt = ProcessInfo.processInfo.systemUptime + 1
        store.refreshProgress(now: sampledAt)
        XCTAssertEqual(store.items.first?.status, .running)
        XCTAssertEqual(store.items.first?.title, "sample.txt")
        XCTAssertEqual(store.items.first?.downloaded, 10)
        XCTAssertEqual(store.items.first?.total, 20)
        XCTAssertEqual(store.items.first?.progress, 0.5)
        XCTAssertGreaterThan(store.items.first?.avgSpeed ?? 0, 0)
        store.refreshProgress(now: sampledAt + 1)
        XCTAssertGreaterThan(store.items.first?.avgSpeed ?? 0, 0)
        store.refreshProgress(now: sampledAt + 5)
        XCTAssertEqual(store.items.first?.avgSpeed, 0)

        try Data().write(to: root.appendingPathComponent("finish"))
        for _ in 0..<40 where store.items.first?.status != .done {
            try await Task.sleep(for: .milliseconds(25))
        }

        XCTAssertEqual(store.items.first?.status, .done)
        XCTAssertEqual(store.items.first?.file, root.appendingPathComponent("sample.txt"))
        XCTAssertEqual(try String(contentsOf: root.appendingPathComponent("sample.txt"), encoding: .utf8), "downloadeddownloaded")
        XCTAssertFalse(try String(contentsOf: storage, encoding: .utf8).contains("aB12"))
        XCTAssertEqual(BaiduDownloads(storage: storage).items.first?.status, .done)
    }

    @MainActor
    func testCLIFailureWithZeroExitAndPartialFileStaysFailed() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let helper = root.appendingPathComponent("bdpan")
        let fixture = """
        #!/bin/sh
        [ "$1" = download ] || exit 1
        printf 'partial' > "${3}sample.txt"
        printf '%s' '{"code":1,"error":"errno=13002, msg=share link is expired"}'
        """
        try Data(fixture.utf8).write(to: helper)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: helper.path)
        let storage = root.appendingPathComponent("history.json")
        let store = BaiduDownloads(storage: storage)

        store.start(share: URL(string: "https://pan.baidu.com/s/test?pwd=aB12")!, directory: root, executable: helper)
        for _ in 0..<40 where store.items.first?.status == .running {
            try await Task.sleep(for: .milliseconds(25))
        }

        let expected = Item.Status.failed(L("百度网盘分享已过期，请向分享者索取新链接。"))
        XCTAssertEqual(store.items.first?.status, expected)
        XCTAssertEqual(BaiduDownloads(storage: storage).items.first?.status, expected)
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("sample.txt").path))
        XCTAssertFalse(try FileManager.default.contentsOfDirectory(atPath: root.path).contains { $0.hasPrefix(".shandian-baidu-") })
    }
}
