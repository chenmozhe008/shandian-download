import Foundation
import Testing
@testable import Shandian

struct UpdateFallbackTests {
    @Test func onlyAcceptsOfficialVersionedReleasePage() {
        let page = URL(string: "https://github.com/chenmozhe008/shandian-download/releases/tag/v0.18.21")!
        #expect(UpdateChecker.releaseFromPage(page)?.version == "0.18.21")
        #expect(UpdateChecker.releaseFromPage(page)?.asset == nil)
        #expect(UpdateChecker.releaseFromPage(URL(string: "https://example.com/chenmozhe008/shandian-download/releases/tag/v0.18.21")!) == nil)
    }
}
