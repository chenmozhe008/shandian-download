import Foundation
import Testing
@testable import Shandian

struct VideoSiteTests {
    @Test func youtubeSelectsCreatorTrackOrOriginalAutomaticTrack() {
        let manual = Data(#"{"subtitles":{"en":[{}],"de":[{}]}}"#.utf8)
        let automatic = Data(#"{"language":"ja-JP","subtitles":{"en":[{}]},"automatic_captions":{"en":[{}],"en-orig":[{}],"ja-orig":[{}],"zh-Hans":[{}]}}"#.utf8)
        let both = Data(#"{"subtitles":{"en":[{}],"de":[{}]},"automatic_captions":{"en-orig":[{}]}}"#.utf8)
        let translatedOnly = Data(#"{"automatic_captions":{"zh-Hans":[{}]},"subtitles":{"live_chat":[{}]}}"#.utf8)

        let manualArgs = VideoSiteRunner.youtubeSubtitleArguments(metadata: manual)
        #expect(manualArgs.contains("--write-subs"))
        #expect(manualArgs.last == "en")
        let automaticArgs = VideoSiteRunner.youtubeSubtitleArguments(metadata: automatic)
        #expect(automaticArgs.contains("--write-auto-subs"))
        #expect(automaticArgs.contains("--write-subs"))
        #expect(automaticArgs.contains("ja-orig"))
        #expect(VideoSiteRunner.youtubeSubtitleArguments(metadata: both).contains("--write-subs"))
        #expect(VideoSiteRunner.youtubeSubtitleArguments(metadata: translatedOnly).isEmpty)
    }

    @Test func youtubeRecognizesOnlyItsOwnHosts() {
        #expect(VideoSiteRunner.isYouTube("https://m.youtube.com/watch?v=test"))
        #expect(VideoSiteRunner.isYouTube("https://youtu.be/test"))
        #expect(!VideoSiteRunner.isYouTube("https://notyoutube.com/test"))
    }

    @Test func completionRequiresNonemptyMediaFile() throws {
        let state = JobState(VideoJob(url: "https://youtu.be/test", title: "Test", site: "YouTube"))
        #expect(state.finish(ok: true).state == .failed)
        let file = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".mp4")
        defer { try? FileManager.default.removeItem(at: file) }
        try Data([1, 2, 3]).write(to: file)
        _ = state.apply("SDFILE " + file.path)
        #expect(state.finish(ok: false).state == .failed)
        #expect(state.finish(ok: true).state == .done)
    }

    @Test func youtubeRetryIsLimitedToTransientNetworkAndExpiredMediaAddresses() {
        let state = JobState(VideoJob(url: "https://youtu.be/test", title: "Test", site: "YouTube"))

        state.recordDiagnostic("ERROR: unable to download video data: HTTP Error 403: Forbidden")
        #expect(state.shouldRetryYouTube)
        state.beginRetry()
        #expect(!state.shouldRetryYouTube)

        state.recordDiagnostic("ERROR: Connection reset by peer")
        #expect(state.shouldRetryYouTube)
        state.recordDiagnostic("ERROR: HTTP Error 429: Too Many Requests")
        #expect(!state.shouldRetryYouTube)
        state.recordDiagnostic("ERROR: [youtube] Private video. Sign in if you have been granted access.")
        #expect(!state.shouldRetryYouTube)
        state.recordDiagnostic("ERROR: video data failed: HTTP Error 403: Forbidden")
        _ = state.apply("SDFILE /tmp/already-finished.mp4")
        #expect(!state.shouldRetryYouTube)
    }

    @Test func existingVideoOptionsDecodeWithoutEnablingAI() throws {
        let saved = Data(#"{"maxHeight":720,"subtitles":true,"audioOnly":false}"#.utf8)
        var options = try JSONDecoder().decode(VideoOptions.self, from: saved)
        #expect(options.subtitleMode == nil)
        options.subtitleMode = .proofread
        let roundTrip = try JSONDecoder().decode(VideoOptions.self, from: JSONEncoder().encode(options))
        #expect(roundTrip.subtitleMode == .proofread)
    }

    @Test func newerPackagedDownloaderSupersedesOldCache() {
        #expect(YtDlp.isOlder("2026.10.26", than: "2026.11.23"))
        #expect(!YtDlp.isOlder("2026.11.23", than: "2026.11.23"))
        #expect(!YtDlp.isOlder("2026.12.01", than: "2026.11.23"))
    }

    @Test func youtubeProgressNeverShrinksBackwardAcrossEstimateJumpsAndTwoParts() {
        let state = JobState(VideoJob(url: "https://www.youtube.com/watch?v=test", title: "Test", site: "YouTube"))
        _ = state.apply("SDFMT 399+251")
        let p1 = state.apply("SDPROG 8000000 NA 10000000 1000000 downloading")?.progress ?? 0
        #expect(p1 > 0.7 && p1 <= 0.90)
        // HLS 估算总大小突然从 10MB 涨到 40MB，进度条不能往回缩
        let p2 = state.apply("SDPROG 9000000 NA 40000000 1000000 downloading")?.progress ?? p1
        #expect(p2 >= p1)
        // 视频段完成进入音频段，进度条平滑衔接不回跳
        let p3 = state.apply("SDPROG 40000000 40000000 40000000 0 finished")?.progress ?? 0
        #expect(p3 >= p2)
        let p4 = state.apply("SDPROG 1000000 5000000 5000000 500000 downloading")?.progress ?? p3
        #expect(p4 >= p3 && p4 <= 0.99)
    }
}
