import Foundation
import Testing
@testable import Shandian

struct GitHubMirrorTests {
    @Test func detectsSupportedGitHubAndHuggingFaceFiles() {
        #expect(GitHubMirror.isGitHubFile("https://github.com/chenmozhe008/shandian-download/releases/download/v0.22.6/Shandian-0.22.6-AppleSilicon.zip"))
        #expect(GitHubMirror.isGitHubFile("https://raw.githubusercontent.com/user/repo/main/file.bin"))
        #expect(GitHubMirror.isGitHubFile("https://codeload.github.com/GopeedLab/gopeed/tar.gz/refs/tags/v1.9.3"))
        #expect(!GitHubMirror.isGitHubFile("https://github.com/chenmozhe008/shandian-download"))
        #expect(!GitHubMirror.isGitHubFile("https://api.github.com/repos/chenmozhe008/shandian-download/releases/latest"))
        #expect(!GitHubMirror.isGitHubFile("https://release-assets.githubusercontent.com/file.zip?X-Amz-Signature=secret"))
        #expect(!GitHubMirror.isGitHubFile("https://user:pass@raw.githubusercontent.com/a/b/main/c.bin"))

        #expect(GitHubMirror.isHuggingFaceFile("https://huggingface.co/Qwen/Qwen2.5-7B/resolve/main/model.safetensors"))
        #expect(GitHubMirror.isHuggingFaceFile("https://huggingface.co/datasets/org/ds/resolve/main/data.parquet?download=true"))
        #expect(!GitHubMirror.isHuggingFaceFile("https://huggingface.co/Qwen/Qwen2.5-7B/blob/main/README.md"))
        #expect(!GitHubMirror.isHuggingFaceFile("https://huggingface.co/Qwen/Qwen2.5-7B/resolve/main/model.bin?token=secret"))
    }

    @Test func splitsAndRestoresOriginalMirrorURLs() {
        let ghOrig = "https://github.com/chenmozhe008/shandian-download/releases/download/v0.22.6/Shandian-0.22.6-AppleSilicon.zip"
        let ghMirrored = GitHubMirror.candidateURL(mirror: "https://gh-proxy.com", original: ghOrig)
        #expect(ghMirrored == "https://gh-proxy.com/\(ghOrig)")
        #expect(GitHubMirror.original(ghMirrored) == ghOrig)
        #expect(GitHubMirror.split(ghMirrored)?.mirror == "https://gh-proxy.com")

        let hfOrig = "https://huggingface.co/Qwen/Qwen2.5-7B/resolve/main/model.safetensors?download=true"
        let hfMirrored = GitHubMirror.candidateURL(mirror: "https://hf-mirror.com", original: hfOrig)
        #expect(hfMirrored == "https://hf-mirror.com/Qwen/Qwen2.5-7B/resolve/main/model.safetensors?download=true")
        #expect(GitHubMirror.original(hfMirrored) == hfOrig)
    }

    @Test func stripsCredentialsBeforeSendingToMirrors() {
        let headers = [
            "Authorization": "Bearer secret",
            "Cookie": "session=123",
            "User-Agent": "Shandian",
            "Referer": "https://github.com/",
        ]
        let clean = GitHubMirror.mirrorHeaders(headers)
        #expect(clean["Authorization"] == nil)
        #expect(clean["Cookie"] == nil)
        #expect(clean["User-Agent"] == "Shandian")
        #expect(clean["Referer"] == "https://github.com/")
    }
}
