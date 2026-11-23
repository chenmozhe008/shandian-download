import XCTest
@testable import Shandian

final class FeishuMediaTests: XCTestCase {
    func testCollectsAllOriginalMediaAndPreservesOrderAndNames() throws {
        let xml = """
        <title>项目 &amp; 测试</title>
        <figure><source token="video1" mime="video/mp4" name="镜头一.mp4" size="1024"/></figure>
        <table><tr><td><source token="audio1" mime="audio/wav" name="声音.wav" size="2048"/></td></tr></table>
        <source token="zip1" mime="application/zip" name="素材.zip" size="4096"/>
        <source token="video1" mime="video/mp4" name="重复.mp4"/>
        <figure><source token="video2" mime="video/mp4" name="镜头 &amp; 二.mp4"/></figure>
        """
        let document = try FeishuMedia.attachments(in: xml)
        XCTAssertEqual(document.title, "项目 & 测试")
        XCTAssertEqual(document.files.map(\.name), ["镜头一.mp4", "声音.wav", "镜头 & 二.mp4"])
        XCTAssertEqual(document.files.map(\.size), [1024, 2048, nil])
    }

    func testOnlyAcceptsFeishuDocumentURLs() {
        XCTAssertEqual(FeishuMedia.documentURL("https://example.feishu.cn/wiki/abc123?from=copy#point")?.absoluteString,
                       "https://example.feishu.cn/wiki/abc123")
        for url in ["https://example.feishu.cn/file/abc", "https://feishu.cn.example.com/docx/abc",
                    "https://user@feishu.cn/wiki/abc", "http://example.feishu.cn/wiki/abc"] {
            XCTAssertNil(FeishuMedia.documentURL(url), url)
        }
    }

    func testSelectionKeepsDocumentOrderAndRejectsMissingAttachments() throws {
        let first = FeishuMedia.Attachment(token: "video1", name: "同名.mp4", size: nil)
        let second = FeishuMedia.Attachment(token: "video2", name: "同名.mp4", size: nil)
        let third = FeishuMedia.Attachment(token: "audio1", name: "声音.wav", size: nil)
        let files = [first, second, third]
        XCTAssertEqual(try FeishuMedia.selectedAttachments(files, ids: [third.selectionID, second.selectionID]), [second, third])
        XCTAssertEqual(try FeishuMedia.selectedAttachments(files, ids: nil), files)
        XCTAssertThrowsError(try FeishuMedia.selectedAttachments([first], ids: [first.selectionID, second.selectionID]))
        XCTAssertThrowsError(try FeishuMedia.selectedAttachments(files, ids: []))
    }
}
