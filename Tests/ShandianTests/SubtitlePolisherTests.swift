import Foundation
import Testing
@testable import Shandian

struct SubtitlePolisherTests {
    @Test func rejectsMalformedAndOverflowTimestampsWithoutCrashing() {
        #expect(SubtitlePolisher.parseTimestamp("") == nil)
        #expect(SubtitlePolisher.parseTimestamp("00:00:") == nil)
        #expect(SubtitlePolisher.parseTimestamp("00:00:.") == nil)
        #expect(SubtitlePolisher.parseTimestamp("999999999999999999:00:00.000") == nil)
        #expect(SubtitlePolisher.parseTimestamp("00:61:00.000") == nil)
        #expect(SubtitlePolisher.parseTimestamp("01:02:03.456") == 3_723_456)
    }

    @Test func cleansRollingYouTubeVTTAndMergesFragments() {
        let vtt = """
        WEBVTT

        00:00:01.000 --> 00:00:02.000
        <c>Hello</c> world

        00:00:02.000 --> 00:00:03.500
        Hello world this is a test

        00:00:03.500 --> 00:00:05.000
        , and it works.
        """
        let parsed = SubtitlePolisher.parse(vtt)
        #expect(parsed.count == 3)
        let cleaned = SubtitlePolisher.cleanRollingSubtitles(parsed)
        #expect(cleaned.count == 1)
        #expect(cleaned[0].text == "Hello world this is a test, and it works.")
        #expect(cleaned[0].startMs == 1000)
        #expect(cleaned[0].endMs == 5000)
    }

    @Test func mergesAIResponseSafelyWithFallback() {
        let originals = ["First line", "Second line", "Third line"]
        let aiOut = """
        [1] 第一句
        [3] 第三句
        """
        let bilingual = SubtitlePolisher.mergeAIResponse(aiOut, originals: originals, bilingual: true).texts
        #expect(bilingual[0] == "第一句\nFirst line")
        #expect(bilingual[1] == "Second line")
        #expect(bilingual[2] == "第三句\nThird line")

        let mono = SubtitlePolisher.mergeAIResponse(aiOut, originals: originals, bilingual: false).texts
        #expect(mono[0] == "第一句")
        #expect(mono[1] == "Second line")
        #expect(mono[2] == "第三句")
    }

    @Test func duplicateAndMissingTranslationNumbersPreserveOriginalCue() {
        let result = SubtitlePolisher.mergeAIResponse(
            "[3] 第三句。\n[1] 第一句。\n[1] 重复编号。\n[4] 无效编号。",
            originals: ["First.", "Second.", "Third."], bilingual: true
        )
        #expect(result.accepted == 1)
        #expect(result.texts == ["First.", "Second.", "第三句。\nThird."])
    }

    @Test func normalizesAndDetectsLanguages() {
        #expect(SubtitlePolisher.normalizeLanguageTag("en-US") == "en")
        #expect(SubtitlePolisher.normalizeLanguageTag("en-orig") == "en")
        #expect(SubtitlePolisher.normalizeLanguageTag("zh-Hans") == "zh")
        #expect(SubtitlePolisher.normalizeLanguageTag("zh-CN") == "zh")
        #expect(SubtitlePolisher.normalizeLanguageTag("ai-zh") == "zh")
        #expect(SubtitlePolisher.normalizeLanguageTag("ja-JP") == "ja")
        #expect(SubtitlePolisher.normalizeLanguageTag("ko") == "ko")
        #expect(SubtitlePolisher.normalizeLanguageTag("fr-FR") == "fr")
        #expect(SubtitlePolisher.normalizeLanguageTag("zh-en") == nil)

        let dir = URL(fileURLWithPath: "/tmp")
        let video = dir.appendingPathComponent("demo [abc].mp4")
        #expect(SubtitlePolisher.extractLanguageTag(subtitleURL: dir.appendingPathComponent("demo [abc].en-US.vtt"), videoURL: video) == "en")
        #expect(SubtitlePolisher.extractLanguageTag(subtitleURL: dir.appendingPathComponent("demo [abc].zh-Hans.vtt"), videoURL: video) == "zh")
        #expect(SubtitlePolisher.extractLanguageTag(subtitleURL: dir.appendingPathComponent("demo [abc].ja.vtt"), videoURL: video) == "ja")
        #expect(SubtitlePolisher.extractLanguageTag(subtitleURL: dir.appendingPathComponent("demo [abc].srt"), videoURL: video) == nil)

        // Content fallback: CJK ratio >= 30% -> zh; English -> nil; Japanese with kana -> not zh
        let zhCues = [SubtitlePolisher.Cue(startMs: 0, endMs: 1000, text: "这是一段测试中文字幕")]
        let enCues = [SubtitlePolisher.Cue(startMs: 0, endMs: 1000, text: "This is an English subtitle")]
        let jaCues = [SubtitlePolisher.Cue(startMs: 0, endMs: 1000, text: "これは日本語の字幕テストです")]
        #expect(SubtitlePolisher.detectLanguage(subtitleURL: dir.appendingPathComponent("demo [abc].srt"), videoURL: video, cues: zhCues) == "zh")
        #expect(SubtitlePolisher.detectLanguage(subtitleURL: dir.appendingPathComponent("demo [abc].srt"), videoURL: video, cues: enCues) == nil)
        #expect(SubtitlePolisher.detectLanguage(subtitleURL: dir.appendingPathComponent("demo [abc].srt"), videoURL: video, cues: jaCues) == nil)
    }

    @Test func backsUpAsHiddenDotfileAndRestoresOriginalSubtitle() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("shandian-sub-test-\(UUID())")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let video = dir.appendingPathComponent("demo [abc].mp4")
        let rawVTT = dir.appendingPathComponent("demo [abc].en-US.vtt")
        let originalVTT = """
        WEBVTT

        00:00:01.000 --> 00:00:02.000
        Hello world

        00:00:02.000 --> 00:00:03.000
        Hello world again.
        """
        try Data().write(to: video)
        try Data(originalVTT.utf8).write(to: rawVTT)

        var cfg = SubtitlePolishConfig()
        cfg.aiEnabled = false
        let summary = await SubtitlePolisher.process(videoURL: video, config: cfg, mode: .original)
        #expect(summary != nil)
        #expect(SubtitlePolisher.hasBackup(for: video))

        // Hidden backup exists as .<original>.original.bak and is invisible to findSubtitle
        let hiddenBak = dir.appendingPathComponent(".demo [abc].en-US.vtt.original.bak")
        #expect(FileManager.default.fileExists(atPath: hiddenBak.path))
        // Clean standard BCP-47 .en.srt produced; raw .vtt removed once backed up
        let cleanSRT = dir.appendingPathComponent("demo [abc].en.srt")
        #expect(FileManager.default.fileExists(atPath: cleanSRT.path))
        #expect(!FileManager.default.fileExists(atPath: rawVTT.path))
        #expect(SubtitlePolisher.findSubtitle(for: video)?.lastPathComponent == "demo [abc].en.srt")

        let processed = try String(contentsOf: cleanSRT, encoding: .utf8)
        #expect(processed.contains("Hello world again."))

        // Restore restores original .vtt and cleans up generated .en.srt + hidden backup
        #expect(SubtitlePolisher.restoreOriginal(for: video))
        #expect(!SubtitlePolisher.hasBackup(for: video))
        #expect(FileManager.default.fileExists(atPath: rawVTT.path))
        #expect(!FileManager.default.fileExists(atPath: cleanSRT.path))
        #expect(!FileManager.default.fileExists(atPath: hiddenBak.path))
        #expect(try String(contentsOf: rawVTT, encoding: .utf8) == originalVTT)
    }

    @Test func foreignBilingualOutputsStandardNamesWithChineseTopOriginalBottom() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("shandian-sub-bi-\(UUID())")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        // English source -> keeps .en.srt + writes .zh-en.srt
        let enVideo = dir.appendingPathComponent("talk [en1].mp4")
        let enSub = dir.appendingPathComponent("talk [en1].en.vtt")
        try Data().write(to: enVideo)
        try Data("WEBVTT\n\n00:00:01.000 --> 00:00:03.000\nWelcome everyone.\n".utf8).write(to: enSub)

        var cfg = SubtitlePolishConfig()
        cfg.aiEnabled = true
        cfg.bilingual = true

        let summaryEN = await SubtitlePolisher.process(videoURL: enVideo, config: cfg, mode: .translate) { batch, isZh, bilingual in
            #expect(!isZh)
            #expect(bilingual)
            let merged = SubtitlePolisher.mergeAIResponse("[1] 欢迎大家。", originals: batch, bilingual: bilingual).texts
            return (merged, true)
        }
        #expect(summaryEN != nil)
        let enClean = dir.appendingPathComponent("talk [en1].en.srt")
        let enBi = dir.appendingPathComponent("talk [en1].zh-en.ass")
        let enBiDuplicateSRT = dir.appendingPathComponent("talk [en1].zh-en.srt")
        // 翻译成功后只保留这一份双语 ASS，避免与原英文字幕冲突导致播放器默认挂载英文
        #expect(!FileManager.default.fileExists(atPath: enClean.path))
        #expect(!FileManager.default.fileExists(atPath: enSub.path))
        #expect(FileManager.default.fileExists(atPath: enBi.path))
        #expect(!FileManager.default.fileExists(atPath: enBiDuplicateSRT.path))
        let biText = try String(contentsOf: enBi, encoding: .utf8)
        #expect(biText.contains(#"欢迎大家。\N{\rSecondary}Welcome everyone."#))
        #expect(SubtitlePolisher.findSubtitle(for: enVideo)?.lastPathComponent == "talk [en1].zh-en.ass")

        // Restore cleans up .zh-en.ass and restores original .en.vtt
        #expect(SubtitlePolisher.restoreOriginal(for: enVideo))
        #expect(!FileManager.default.fileExists(atPath: enBi.path))
        #expect(FileManager.default.fileExists(atPath: enSub.path))

        // Japanese source -> only keeps .zh-ja.ass
        let jaVideo = dir.appendingPathComponent("anime [jp1].mp4")
        let jaSub = dir.appendingPathComponent("anime [jp1].ja.vtt")
        try Data().write(to: jaVideo)
        try Data("WEBVTT\n\n00:00:02.000 --> 00:00:04.000\nおはようございます。\n".utf8).write(to: jaSub)

        _ = await SubtitlePolisher.process(videoURL: jaVideo, config: cfg, mode: .translate) { batch, isZh, bilingual in
            #expect(!isZh)
            #expect(bilingual)
            let merged = SubtitlePolisher.mergeAIResponse("[1] 早上好。", originals: batch, bilingual: bilingual).texts
            return (merged, true)
        }
        let jaClean = dir.appendingPathComponent("anime [jp1].ja.srt")
        let jaBi = dir.appendingPathComponent("anime [jp1].zh-ja.ass")
        #expect(!FileManager.default.fileExists(atPath: jaClean.path))
        #expect(FileManager.default.fileExists(atPath: jaBi.path))
        let jaBiText = try String(contentsOf: jaBi, encoding: .utf8)
        #expect(jaBiText.contains(#"早上好。\N{\rSecondary}おはようございます。"#))
    }

    @Test func chineseSourceProofreadsToSingleZhSRTWithoutDuplicatingLines() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("shandian-sub-zh-\(UUID())")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let video = dir.appendingPathComponent("vlog [zh1].mp4")
        let rawSub = dir.appendingPathComponent("vlog [zh1].zh-Hans.vtt")
        try Data().write(to: video)
        try Data("WEBVTT\n\n00:00:01.000 --> 00:00:03.000\n今天天气真好\n".utf8).write(to: rawSub)

        var cfg = SubtitlePolishConfig()
        cfg.aiEnabled = true
        cfg.bilingual = true   // Even with bilingual enabled, Chinese source must NOT duplicate lines

        let summary = await SubtitlePolisher.process(videoURL: video, config: cfg, mode: .translate) { batch, isZh, bilingual in
            #expect(isZh)
            #expect(!bilingual)
            let merged = SubtitlePolisher.mergeAIResponse("[1] 今天天气真好。", originals: batch, bilingual: bilingual).texts
            return (merged, true)
        }
        #expect(summary != nil)
        let zhOut = dir.appendingPathComponent("vlog [zh1].zh.srt")
        #expect(FileManager.default.fileExists(atPath: zhOut.path))
        #expect(!FileManager.default.fileExists(atPath: dir.appendingPathComponent("vlog [zh1].zh-zh.srt").path))
        let content = try String(contentsOf: zhOut, encoding: .utf8)
        #expect(content.contains("00:00:01,000 --> 00:00:03,000\n今天天气真好。\n"))
        #expect(!content.contains("今天天气真好。\n今天天气真好"))
    }

    @Test func originalAndProofreadingDoNotCreateTranslation() async throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let video = folder.appendingPathComponent("sample.mp4")
        let english = folder.appendingPathComponent("sample.en.srt")
        try Data("1\n00:00:01,000 --> 00:00:03,000\nhello wrld.\n".utf8).write(to: english)

        var config = SubtitlePolishConfig()
        config.aiEnabled = true
        _ = await SubtitlePolisher.process(videoURL: video, config: config, mode: .original) { batch, _, _ in
            Issue.record("Original subtitles should use no AI")
            return (batch, false)
        }
        let result = await SubtitlePolisher.process(videoURL: video, config: config, mode: .proofread) { batch, isChinese, bilingual in
            #expect(!isChinese)
            #expect(!bilingual)
            #expect(batch == ["hello wrld."])
            return (["Hello world."], true)
        }
        #expect(result == L("AI 字幕校对完成"))
        #expect(try String(contentsOf: english, encoding: .utf8).contains("00:00:01,000 --> 00:00:03,000\nHello world."))
        #expect(!FileManager.default.fileExists(atPath: folder.appendingPathComponent("sample.zh-en.srt").path))
        #expect(SubtitlePolisher.restoreOriginal(for: video))
        #expect(try String(contentsOf: english, encoding: .utf8).contains("hello wrld."))
    }

    @Test func videoOptionsOutputTemplateAndTrashTarget() {
        let withSubs = VideoOptions(maxHeight: 0, subtitles: true, audioOnly: false)
        #expect(withSubs.outputTemplate == "%(title).80s [%(id)s]/%(title).80s [%(id)s].%(ext)s")

        let noSubs = VideoOptions(maxHeight: 0, subtitles: false, audioOnly: false)
        #expect(noSubs.outputTemplate == "%(title).80s [%(id)s].%(ext)s")

        let root = URL(fileURLWithPath: "/Users/test/Downloads/Shandian")
        let dedicatedVideo = root
            .appendingPathComponent("Demo [abc]")
            .appendingPathComponent("Demo [abc].mp4")
        #expect(DownloadStore.videoTrashTarget(for: dedicatedVideo, rootDownloadDir: root).path == root.appendingPathComponent("Demo [abc]").path)

        let flatVideo = root.appendingPathComponent("Demo [abc].mp4")
        #expect(DownloadStore.videoTrashTarget(for: flatVideo, rootDownloadDir: root).path == flatVideo.path)

        // Even if a flat video happens to share the root download folder's name, never trash the root folder
        let sameAsRootVideo = root.appendingPathComponent("Shandian.mp4")
        #expect(DownloadStore.videoTrashTarget(for: sameAsRootVideo, rootDownloadDir: root).path == sameAsRootVideo.path)
    }

    @Test func resegmentsFragmentsSanitizesChineseAndRendersStyledASS() {
        let fragmented: [SubtitlePolisher.Cue] = [
            .init(startMs: 0, endMs: 1640, text: "So, I've teamed up with Anthropic, who"),
            .init(startMs: 1640, endMs: 3480, text: "is sponsoring this video."),
            .init(startMs: 3480, endMs: 5200, text: "Now let's build your first agent!"),
        ]
        let merged = SubtitlePolisher.resegmentSentences(fragmented)
        #expect(merged.count == 2)
        #expect(merged[0].text == "So, I've teamed up with Anthropic, who is sponsoring this video.")
        #expect(merged[0].endMs <= merged[1].startMs)

        let sanitized = SubtitlePolisher.sanitizeChineseSubtitleLine("所以，我与 Anthropic 合作。版本 2.5 很棒，对吗？")
        #expect(sanitized == "所以 我与 Anthropic 合作 版本 2.5 很棒 对吗？")

        let ass = SubtitlePolisher.renderASS([
            .init(startMs: 0, endMs: 3480, text: "我这次和 Anthropic 合作\nSo, I've teamed up with Anthropic")
        ], bilingual: true)
        #expect(ass.contains("PlayResX: 1920"))
        #expect(ass.contains("Style: Default,PingFang SC,52"))
        #expect(ass.contains("Style: Secondary,SF Pro Text,34"))
        #expect(ass.contains(#"我这次和 Anthropic 合作\N{\rSecondary}So, I've teamed up with Anthropic"#))
    }
}
