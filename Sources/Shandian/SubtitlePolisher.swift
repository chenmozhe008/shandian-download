import Foundation
import CryptoKit
import os

enum SubtitleMode: String, Codable, CaseIterable, Sendable {
    case original
    case proofread
    case translate

    /// 界面展示给用户的字幕选项（精简为原字幕 + AI 翻译，保留 proofread 枚举仅用于兼容旧记录）。
    static var userChoices: [SubtitleMode] { [.original, .translate] }

    var label: String {
        label(bilingual: SubtitlePolishConfig.current.bilingual)
    }

    func label(bilingual: Bool) -> String {
        switch self {
        case .original: L("只下载原字幕")
        case .proofread: L("AI 校对原字幕")
        case .translate: bilingual ? L("AI 翻译（中外双语）") : L("AI 翻译成中文")
        }
    }

    var isAI: Bool { self != .original }

    var progressTitle: String {
        switch self {
        case .original: L("正在修整字幕结构…")
        case .proofread: L("正在 AI 校对字幕…")
        case .translate: L("正在 AI 翻译字幕…")
        }
    }
}

/// 视频字幕修整与可选 AI 润色翻译。
///
/// 两层设计：
/// 1. 纯算法结构修整（默认开启、零配置、离线）：
///    解析 VTT / SRT，清除 HTML 标签与 YouTube 自动字幕的滚动重复行，合并碎片成完整句子，修正行首标点。
/// 2. 可选 AI 润色与翻译（兼容 OpenAI `/v1/chat/completions`）：
///    支持本机 Ollama（`http://127.0.0.1:11434/v1`）、DeepSeek、OpenAI 及任意兼容接口。
///    时间轴由程序严格锁定，模型只返回译文/润色文本；条数不匹配时自动回退保留原文，绝不串行错位。
///    原字幕自动隐藏备份为 `.<原文件>.original.bak`，支持随时一键恢复；
///    输出严格遵循标准 BCP-47 语言后缀（如 `.en.srt` / `.zh.srt` / `.zh-en.srt`），绝不污染文件名。
struct SubtitlePolishConfig: Codable, Equatable, Sendable {
    /// 服务清单对齐 SuiTone / 电影下载。模型名各家常改，这里只给最新默认值，点「测试连接」后自动从服务商拉取最新列表（零费用）。
    enum Provider: String, Codable, CaseIterable, Identifiable, Sendable {
        case deepseek
        case kimi
        case qwen
        case doubao
        case zhipu
        case openai
        case gemini
        case anthropic
        case ollama
        case custom

        var id: String { rawValue }

        var displayName: String {
            switch self {
            case .deepseek: "DeepSeek"
            case .kimi: "Kimi"
            case .qwen: "通义千问"
            case .doubao: "豆包"
            case .zhipu: "智谱 GLM"
            case .openai: "OpenAI"
            case .gemini: "Gemini"
            case .anthropic: "Claude"
            case .ollama: "本机 Ollama（免费离线）"
            case .custom: "其他 OpenAI 兼容服务"
            }
        }

        var defaultBaseURL: String {
            switch self {
            case .deepseek: "https://api.deepseek.com"
            case .kimi: "https://api.moonshot.cn/v1"
            case .qwen: "https://dashscope.aliyuncs.com/compatible-mode/v1"
            case .doubao: "https://ark.cn-beijing.volces.com/api/v3"
            case .zhipu: "https://open.bigmodel.cn/api/paas/v4"
            case .openai: "https://api.openai.com/v1"
            case .gemini: "https://generativelanguage.googleapis.com/v1beta/openai"
            case .anthropic: "https://api.anthropic.com/v1"
            case .ollama: "http://127.0.0.1:11434/v1"
            case .custom: ""
            }
        }

        var presetModels: [String] {
            switch self {
            case .deepseek: ["deepseek-flash", "deepseek-chat", "deepseek-reasoner"]
            case .kimi: ["kimi-latest"]
            case .qwen: ["qwen-plus", "qwen-turbo", "qwen-max"]
            case .doubao: ["doubao-seed-1-6-flash-250615", "doubao-seed-1-6-250615"]
            case .zhipu: ["glm-4.5-flash", "glm-4.5"]
            case .openai: ["gpt-5.5-mini", "gpt-5.5"]
            case .gemini: ["gemini-3-flash", "gemini-3-pro"]
            case .anthropic: ["claude-haiku-5-5", "claude-sonnet-5-5"]
            case .ollama: ["qwen2.5:7b", "qwen2.5:14b"]
            case .custom: []
            }
        }

        var defaultModel: String { presetModels.first ?? "" }
        var needsAPIKey: Bool { self != .ollama }
        var needsBaseURLInput: Bool { self == .custom }
    }

    var autoCleanStructure = true
    var aiEnabled = false
    var provider: Provider = .deepseek
    var baseURL = Provider.deepseek.defaultBaseURL
    var apiKey = ""
    var apiKeys: [String: String] = [:]
    var fetchedModels: [String: [String]] = [:]
    var selectedModels: [String: String] = [:]
    var model = Provider.deepseek.defaultModel
    var bilingual = true

    var effectiveBaseURL: String {
        provider.needsBaseURLInput ? baseURL : provider.defaultBaseURL
    }

    var effectiveAPIKey: String {
        if let saved = apiKeys[provider.rawValue], !saved.isEmpty { return saved }
        return ""
    }

    var hasKeyForCurrentProvider: Bool {
        !effectiveAPIKey.isEmpty
    }

    /// 严格只返回当前服务商自己的模型列表，绝不混入其他服务商的旧模型。
    var availableModels: [String] {
        let fetched = fetchedModels[provider.rawValue] ?? []
        return fetched.isEmpty ? provider.presetModels : fetched
    }

    var isConfiguredForAI: Bool {
        guard aiEnabled, !model.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              let url = URL(string: effectiveBaseURL.trimmingCharacters(in: .whitespacesAndNewlines)),
              let scheme = url.scheme, ["http", "https"].contains(scheme), url.host != nil else { return false }
        return !provider.needsAPIKey || provider == .custom || hasKeyForCurrentProvider
    }

    var usesLocalEndpoint: Bool {
        guard let host = URL(string: effectiveBaseURL.trimmingCharacters(in: .whitespacesAndNewlines))?.host?.lowercased() else { return false }
        let octets = host.split(separator: ".")
        let ipv4Loopback = octets.count == 4 && octets.first == "127" && octets.allSatisfy { UInt8($0) != nil }
        return host == "localhost" || host == "[::1]" || host == "::1" || ipv4Loopback
    }

    var usageNotice: String {
        usesLocalEndpoint
            ? L("在本机处理字幕，耗时取决于模型和电脑性能。")
            : L("字幕文本会发送到所选 API，服务商可能按用量计费。")
    }

    var usageLabel: String {
        usesLocalEndpoint ? L("本机模型") : L("API 可能计费")
    }

    private static let key = "subtitlePolishConfig"

    static var current: SubtitlePolishConfig {
        get {
            guard let data = UserDefaults.standard.data(forKey: key),
                  let cfg = try? JSONDecoder().decode(SubtitlePolishConfig.self, from: data) else {
                return SubtitlePolishConfig()
            }
            return cfg
        }
        set {
            if let data = try? JSONEncoder().encode(newValue) {
                UserDefaults.standard.set(data, forKey: key)
            }
        }
    }
}

enum SubtitlePolisher {
    struct Cue: Equatable, Sendable {
        var startMs: Int
        var endMs: Int
        var text: String
    }

    private static let subtitleExts: Set<String> = ["vtt", "srt", "ass"]
    private static let batchSize = 20
    private static let activeVideos = OSAllocatedUnfairLock(initialState: Set<String>())

    // MARK: - 字幕文件定位、语言识别与隐藏备份

    /// 判断文件名尾部（去掉 `<videoStem>.` 之后）是否为生成的双语轨道（如 `zh-en.srt`、`zh-ja.srt`、`zh-orig.srt`）。
    private static func isBilingualTail(_ tail: String) -> Bool {
        let lower = tail.lowercased()
        guard lower.hasPrefix("zh-") else { return false }
        let tag = lower.split(separator: ".").first.map(String.init) ?? lower
        let zhOnly: Set<String> = ["zh-hans", "zh-hant", "zh-cn", "zh-tw", "zh-hk", "zh-sg", "zh-mo", "zh-orig"]
        if zhOnly.contains(tag) || tag.hasPrefix("zh-hans-") || tag.hasPrefix("zh-hant-") {
            return false
        }
        return true
    }

    /// 将字幕文件名中的语言标签归一为标准 BCP-47 主语言代码（如 `en-US` → `en`，`zh-Hans`/`ai-zh` → `zh`，`ja-JP` → `ja`）。
    static func normalizeLanguageTag(_ raw: String) -> String? {
        let cleaned = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
            .replacingOccurrences(of: "_", with: "-")
        guard !cleaned.isEmpty else { return nil }
        let stripped = cleaned.hasPrefix("ai-") ? String(cleaned.dropFirst(3)) : cleaned

        let zhVariants: Set<String> = [
            "zh", "chi", "zho", "chs", "cht",
            "zh-hans", "zh-hant", "zh-cn", "zh-tw", "zh-hk", "zh-sg", "zh-mo", "zh-orig",
        ]
        if zhVariants.contains(stripped) || stripped.hasPrefix("zh-hans-") || stripped.hasPrefix("zh-hant-") {
            return "zh"
        }
        // 双语标签（如 zh-en）不视为单一源语言
        if stripped.hasPrefix("zh-") {
            return nil
        }

        let primary = stripped.split(separator: "-").first.map(String.init) ?? stripped
        guard (2...3).contains(primary.count), primary.allSatisfy({ $0.isASCII && $0.isLetter }) else {
            return nil
        }
        switch primary {
        case "eng": return "en"
        case "jpn": return "ja"
        case "kor": return "ko"
        case "fra", "fre": return "fr"
        case "deu", "ger": return "de"
        case "spa": return "es"
        case "rus": return "ru"
        case "por": return "pt"
        case "ita": return "it"
        default: return primary
        }
    }

    /// 从字幕文件名中提取并归一语言标签（例如 `demo [id].en-US.vtt` → `en`）。
    static func extractLanguageTag(subtitleURL: URL, videoURL: URL) -> String? {
        let videoStem = videoURL.deletingPathExtension().lastPathComponent
        let subStem = subtitleURL.deletingPathExtension().lastPathComponent
        let prefix = videoStem.lowercased() + "."
        guard subStem.lowercased().hasPrefix(prefix) else { return nil }
        let rest = String(subStem.dropFirst(videoStem.count + 1))
        let firstSegment = rest.split(separator: ".").first.map(String.init) ?? rest
        return normalizeLanguageTag(firstSegment)
    }

    /// 内容启发式判断字幕正文是否为中文（排除含明显日文假名的情形；CJK 汉字占比 >= 30% 判为中文）。
    static func isChineseContent(_ cues: [Cue]) -> Bool {
        var total = 0
        var cjk = 0
        var kana = 0
        for cue in cues {
            for scalar in cue.text.unicodeScalars {
                if CharacterSet.whitespacesAndNewlines.contains(scalar) || CharacterSet.punctuationCharacters.contains(scalar) {
                    continue
                }
                total += 1
                let v = scalar.value
                if (0x4E00...0x9FFF).contains(v) || (0x3400...0x4DBF).contains(v) {
                    cjk += 1
                } else if (0x3040...0x309F).contains(v) || (0x30A0...0x30FF).contains(v) {
                    kana += 1
                }
            }
        }
        guard total > 0 else { return false }
        if Double(kana) / Double(total) >= 0.05 { return false }
        return Double(cjk) / Double(total) >= 0.30
    }

    /// 结合文件名标签与正文启发式识别字幕源语言（无法判断时返回 nil）。
    static func detectLanguage(subtitleURL: URL, videoURL: URL, cues: [Cue]) -> String? {
        if let tag = extractLanguageTag(subtitleURL: subtitleURL, videoURL: videoURL) {
            return tag
        }
        if isChineseContent(cues) {
            return "zh"
        }
        return nil
    }

    /// 查找与视频文件同目录、同前缀的字幕文件（忽略隐藏文件，优先原始单语主轨）。
    static func findSubtitle(for videoURL: URL) -> URL? {
        let dir = videoURL.deletingLastPathComponent()
        let stem = videoURL.deletingPathExtension().lastPathComponent.lowercased()
        let prefix = stem + "."
        guard let entries = try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil) else {
            return nil
        }
        var candidates: [(rank: Int, url: URL)] = []
        for url in entries {
            let rawName = url.lastPathComponent
            guard !rawName.hasPrefix(".") else { continue }
            let ext = url.pathExtension.lowercased()
            guard subtitleExts.contains(ext) else { continue }
            let name = rawName.lowercased()
            guard name.hasPrefix(prefix), isRegularSubtitle(url) else { continue }
            let tail = String(name.dropFirst(prefix.count))
            let rank: Int
            if isBilingualTail(tail) {
                rank = 3
            } else if tail.hasPrefix("zh.") || tail.hasPrefix("zh-hans.") || tail.hasPrefix("zh-cn.") || tail.hasPrefix("zh-hant.") || tail.hasPrefix("zh-tw.") || tail.hasPrefix("ai-zh.") {
                rank = 0
            } else if tail.hasPrefix("en.") || tail.hasPrefix("en-") {
                rank = 1
            } else {
                rank = 2
            }
            candidates.append((rank, url))
        }
        return candidates.sorted { $0.rank != $1.rank ? $0.rank < $1.rank : $0.url.lastPathComponent < $1.url.lastPathComponent }.first?.url
    }

    private static func isRegularSubtitle(_ url: URL) -> Bool {
        let values = try? url.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey])
        return values?.isRegularFile == true && values?.isSymbolicLink == false && (values?.fileSize ?? 0) > 0
    }

    /// 原字幕隐藏备份路径：`同目录/.<原字幕文件名>.original.bak`
    static func backupURL(for subtitleURL: URL) -> URL {
        let dir = subtitleURL.deletingLastPathComponent()
        let name = subtitleURL.lastPathComponent
        let hiddenName = name.hasPrefix(".") ? "\(name).original.bak" : ".\(name).original.bak"
        return dir.appendingPathComponent(hiddenName)
    }

    private static func findBackup(for videoURL: URL) -> (original: URL, backup: URL)? {
        let dir = videoURL.deletingLastPathComponent()
        let stem = videoURL.deletingPathExtension().lastPathComponent.lowercased()
        let prefix = stem + "."
        let hiddenPrefix = "." + prefix
        guard let entries = try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil) else {
            return nil
        }
        var legacyMatch: (original: URL, backup: URL)?
        for bak in entries {
            let name = bak.lastPathComponent
            let lower = name.lowercased()
            guard lower.hasSuffix(".original.bak"), isRegularSubtitle(bak) else { continue }
            if lower.hasPrefix(hiddenPrefix) {
                let origName = String(name.dropFirst(1).dropLast(".original.bak".count))
                return (dir.appendingPathComponent(origName), bak)
            } else if lower.hasPrefix(prefix), legacyMatch == nil {
                let origName = String(name.dropLast(".original.bak".count))
                legacyMatch = (dir.appendingPathComponent(origName), bak)
            }
        }
        return legacyMatch
    }

    static func hasBackup(for videoURL: URL) -> Bool {
        findBackup(for: videoURL) != nil
    }

    /// 只清理本次处理写入、并且之后未被编辑的字幕文件。
    private struct SubtitleOutputs {
        let manifest: URL
        var hashes: [String: String]

        init(backup: URL) {
            manifest = backup.appendingPathExtension("outputs.json")
            hashes = (try? Data(contentsOf: manifest)).flatMap {
                try? JSONDecoder().decode([String: String].self, from: $0)
            } ?? [:]
        }

        func owns(_ url: URL) -> Bool {
            guard let fingerprint = hashes[url.lastPathComponent],
                  SubtitlePolisher.isRegularSubtitle(url),
                  let data = try? Data(contentsOf: url) else { return false }
            return fingerprint == Data(SHA256.hash(data: data)).base64EncodedString()
        }

        mutating func write(_ data: Data, to preferred: URL, source: URL) throws {
            let fm = FileManager.default
            var target = preferred
            if target != source, fm.fileExists(atPath: target.path), !owns(target) {
                let ext = target.pathExtension.isEmpty ? "srt" : target.pathExtension
                let base = target.deletingPathExtension()
                target = base.appendingPathExtension("processed.\(ext)")
                while fm.fileExists(atPath: target.path) && !owns(target) {
                    target = base.appendingPathExtension("processed-\(UUID().uuidString).\(ext)")
                }
            }
            try data.write(to: target, options: .atomic)
            guard target != source else { return }
            hashes[target.lastPathComponent] = Data(SHA256.hash(data: data)).base64EncodedString()
            try JSONEncoder().encode(hashes).write(to: manifest, options: .atomic)
        }
    }

    @discardableResult
    static func restoreOriginal(for videoURL: URL) -> Bool {
        let path = videoURL.standardizedFileURL.path
        guard activeVideos.withLock({ $0.insert(path).inserted }) else { return false }
        defer { activeVideos.withLock { _ = $0.remove(path) } }
        guard let pair = findBackup(for: videoURL),
              let data = try? Data(contentsOf: pair.backup) else { return false }
        let fm = FileManager.default
        let dir = videoURL.deletingLastPathComponent()
        let prefix = videoURL.deletingPathExtension().lastPathComponent.lowercased() + "."
        let outputs = SubtitleOutputs(backup: pair.backup)
        do {
            try data.write(to: pair.original, options: .atomic)
            for name in outputs.hashes.keys where name.lowercased().hasPrefix(prefix) {
                let file = dir.appendingPathComponent(name)
                guard file.lastPathComponent == name,
                      ["srt", "ass"].contains(file.pathExtension.lowercased()),
                      file != pair.original, outputs.owns(file) else { continue }
                try? fm.removeItem(at: file)
            }
            try? fm.removeItem(at: outputs.manifest)
            try? fm.removeItem(at: pair.backup)
            return true
        } catch {
            return false
        }
    }

    // MARK: - VTT / SRT 解析与格式化

    static func parseTimestamp(_ raw: String) -> Int? {
        let cleaned = raw.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: ",", with: ".")
        let parts = cleaned.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
        guard parts.count == 2 || parts.count == 3 else { return nil }
        let h = parts.count == 3 ? (Int(parts[0]) ?? -1) : 0
        let m = Int(parts[parts.count - 2]) ?? -1
        let secParts = parts[parts.count - 1].split(separator: ".", maxSplits: 1, omittingEmptySubsequences: false).map(String.init)
        guard let secStr = secParts.first, !secStr.isEmpty, let s = Int(secStr) else { return nil }
        var ms = 0
        if secParts.count == 2 {
            let digits = secParts[1].prefix(while: \.isNumber).prefix(3)
            guard !digits.isEmpty else { return nil }
            let padded = digits + String(repeating: "0", count: max(0, 3 - digits.count))
            ms = Int(padded) ?? -1
        }
        guard (0...999).contains(h), (0..<60).contains(m), (0..<60).contains(s), (0..<1000).contains(ms) else { return nil }
        return ((h * 3600 + m * 60 + s) * 1000) + ms
    }

    static func formatSRTTimestamp(_ ms: Int) -> String {
        let clamped = max(0, ms)
        let h = clamped / 3_600_000
        let m = (clamped % 3_600_000) / 60_000
        let s = (clamped % 60_000) / 1000
        let r = clamped % 1000
        return String(format: "%02d:%02d:%02d,%03d", h, m, s, r)
    }

    static func stripTags(_ text: String) -> String {
        var out = text.replacingOccurrences(of: #"<[^>]+>"#, with: "", options: .regularExpression)
        out = out.replacingOccurrences(of: "&amp;", with: "&")
            .replacingOccurrences(of: "&lt;", with: "<")
            .replacingOccurrences(of: "&gt;", with: ">")
            .replacingOccurrences(of: "&nbsp;", with: " ")
            .replacingOccurrences(of: "&#39;", with: "'")
            .replacingOccurrences(of: "&quot;", with: "\"")
        return out.replacingOccurrences(of: #"\s+"#, with: " ", options: .regularExpression)
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: #"^(?:>{2,}|＞{2,})(?: +|$)"#, with: "", options: .regularExpression)
    }

    /// 解析 VTT 或 SRT 字幕为 Cue 数组。
    static func parse(_ content: String) -> [Cue] {
        let normalized = content.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
        let lines = normalized.components(separatedBy: "\n")
        var cues: [Cue] = []
        var i = 0
        while i < lines.count {
            let line = lines[i].trimmingCharacters(in: .whitespaces)
            if line.contains("-->") {
                let halves = line.components(separatedBy: "-->")
                if halves.count >= 2,
                   let start = parseTimestamp(halves[0]),
                   let end = parseTimestamp(halves[1].split(separator: " ").first.map(String.init) ?? halves[1]) {
                    i += 1
                    var textLines: [String] = []
                    while i < lines.count {
                        let t = lines[i].trimmingCharacters(in: .whitespaces)
                        if t.isEmpty { break }
                        let clean = stripTags(t)
                        if !clean.isEmpty { textLines.append(clean) }
                        i += 1
                    }
                    let joined = textLines.joined(separator: " ").trimmingCharacters(in: .whitespacesAndNewlines)
                    if !joined.isEmpty {
                        cues.append(Cue(startMs: start, endMs: max(end, start), text: joined))
                    }
                    continue
                }
            }
            i += 1
        }
        return cues
    }

    /// 去除 YouTube 自动字幕的滚动重复行、合并碎片短句、清理行首孤立标点。
    static func cleanRollingSubtitles(_ raw: [Cue]) -> [Cue] {
        guard !raw.isEmpty else { return [] }
        // 第 1 步：去掉完全重复以及滚动前缀重叠（例如 cue1 = "Hello world", cue2 = "Hello world how are"）
        var deduped: [Cue] = []
        for cue in raw {
            let text = cue.text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty else { continue }
            if let last = deduped.last {
                if text == last.text {
                    deduped[deduped.count - 1].endMs = max(last.endMs, cue.endMs)
                    continue
                }
                if text.hasPrefix(last.text) {
                    let remainder = String(text.dropFirst(last.text.count)).trimmingCharacters(in: .whitespaces)
                    if remainder.isEmpty {
                        deduped[deduped.count - 1].endMs = max(last.endMs, cue.endMs)
                        continue
                    }
                    // 极短的过渡帧直接替换成完整那句，否则仅保留新增部分
                    if last.endMs - last.startMs <= 300 || cue.startMs <= last.startMs + 250 {
                        deduped[deduped.count - 1].text = text
                        deduped[deduped.count - 1].endMs = max(last.endMs, cue.endMs)
                    } else {
                        deduped.append(Cue(startMs: cue.startMs, endMs: cue.endMs, text: remainder))
                    }
                    continue
                }
                if let overlap = suffixPrefixOverlap(last.text, text), overlap >= 4 {
                    let remainder = String(text.dropFirst(overlap)).trimmingCharacters(in: .whitespaces)
                    if remainder.isEmpty {
                        deduped[deduped.count - 1].endMs = max(last.endMs, cue.endMs)
                    } else {
                        deduped.append(Cue(startMs: cue.startMs, endMs: cue.endMs, text: remainder))
                    }
                    continue
                }
            }
            deduped.append(Cue(startMs: cue.startMs, endMs: cue.endMs, text: text))
        }

        // 第 2 步：把行首孤立标点归位到上一条末尾，并合并过短的相邻碎片
        var merged: [Cue] = []
        let leadingPunct = CharacterSet(charactersIn: ",.;:!?，。；：！？、")
        let sentenceEnd = CharacterSet(charactersIn: ".!?。！？")
        for var cue in deduped {
            if let firstScalar = cue.text.unicodeScalars.first, leadingPunct.contains(firstScalar), !merged.isEmpty {
                let punct = String(firstScalar)
                merged[merged.count - 1].text += punct
                cue.text = String(cue.text.dropFirst()).trimmingCharacters(in: .whitespaces)
                if cue.text.isEmpty {
                    merged[merged.count - 1].endMs = max(merged[merged.count - 1].endMs, cue.endMs)
                    continue
                }
            }
            if let last = merged.last,
               cue.startMs - last.endMs <= 600,
               last.endMs - last.startMs < 6000,
               last.text.count + cue.text.count <= 72,
               last.text.unicodeScalars.last.map({ !sentenceEnd.contains($0) }) ?? true {
                let needsSpace = !(last.text.last?.isCJK ?? false) && !(cue.text.first?.isCJK ?? false)
                merged[merged.count - 1].text = last.text + (needsSpace ? " " : "") + cue.text
                merged[merged.count - 1].endMs = max(last.endMs, cue.endMs)
            } else {
                merged.append(cue)
            }
        }
        // 第 3 步：消除相邻时间轴重叠，防止播放器/剪辑软件双条叠屏闪烁
        return clampOverlaps(merged)
    }

    private static func clampOverlaps(_ cues: [Cue]) -> [Cue] {
        guard cues.count > 1 else { return cues }
        var out = cues
        for i in 0..<(out.count - 1) {
            let nextStart = out[i + 1].startMs
            if nextStart > out[i].startMs, out[i].endMs > nextStart {
                out[i].endMs = nextStart
            }
        }
        return out
    }

    /// 语义重断句（借鉴 VideoLingo / VideoCaptioner 管线）：
    /// 先把 YouTube 切碎的跨条半句按句末标点或明显停顿拼回完整自然句，
    /// 再按适合单屏阅读的长度在逗号/词边界重新切分，按字数比例精确分配时间轴，彻底消灭断头句。
    static func resegmentSentences(_ cues: [Cue]) -> [Cue] {
        guard cues.count > 1 else { return cues }
        let sentenceEnders = CharacterSet(charactersIn: ".!?。！？")
        var sentences: [Cue] = []
        for cue in cues {
            let text = cue.text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty else { continue }
            if let last = sentences.last {
                let lastEndsSentence = last.text.unicodeScalars.last.map { sentenceEnders.contains($0) } ?? false
                let gap = cue.startMs - last.endMs
                let combinedDuration = cue.endMs - last.startMs
                let combinedLen = last.text.count + text.count
                if !lastEndsSentence && gap <= 900 && combinedDuration <= 14_000 && combinedLen <= 220 {
                    let needsSpace = !(last.text.last?.isCJK ?? false) && !(text.first?.isCJK ?? false)
                    sentences[sentences.count - 1].text = last.text + (needsSpace ? " " : "") + text
                    sentences[sentences.count - 1].endMs = max(last.endMs, cue.endMs)
                    continue
                }
            }
            sentences.append(Cue(startMs: cue.startMs, endMs: cue.endMs, text: text))
        }

        // 将过长的完整句按自然语义停顿（逗号/分号/词边界）拆分为适合单行展示的片段，按字符长度比例分配时间轴
        var result: [Cue] = []
        for sentence in sentences {
            let isCJK = sentence.text.contains(where: \.isCJK)
            let maxChars = isCJK ? 24 : 76
            if sentence.text.count <= maxChars {
                result.append(sentence)
                continue
            }
            let parts = splitTextNaturally(sentence.text, maxChars: maxChars, isCJK: isCJK)
            let totalChars = max(1, parts.reduce(0) { $0 + $1.count })
            let totalMs = max(1, sentence.endMs - sentence.startMs)
            var cursor = sentence.startMs
            for (idx, part) in parts.enumerated() {
                let end: Int
                if idx == parts.count - 1 {
                    end = sentence.endMs
                } else {
                    let share = Int((Double(part.count) / Double(totalChars)) * Double(totalMs))
                    end = min(sentence.endMs, max(cursor + 300, cursor + share))
                }
                result.append(Cue(startMs: cursor, endMs: max(end, cursor + 1), text: part))
                cursor = end
            }
        }
        return clampOverlaps(result)
    }

    private static func splitTextNaturally(_ text: String, maxChars: Int, isCJK: Bool) -> [String] {
        var remaining = text.trimmingCharacters(in: .whitespacesAndNewlines)
        var chunks: [String] = []
        let softBreaks: Set<Character> = [",", ";", ":", "，", "；", "：", "、"]
        while remaining.count > maxChars {
            let chars = Array(remaining)
            let target = min(maxChars, max(maxChars / 2, chars.count / ((chars.count + maxChars - 1) / maxChars)))
            let minIdx = max(1, Int(Double(target) * 0.55))
            let maxIdx = min(chars.count - 1, maxChars)
            var splitAt: Int? = nil
            // 优先在目标点附近的逗号/分号处断句
            var bestDist = Int.max
            for idx in minIdx...maxIdx {
                if softBreaks.contains(chars[idx - 1]) {
                    let dist = abs(idx - target)
                    if dist < bestDist { bestDist = dist; splitAt = idx }
                }
            }
            // 其次在英文空格边界断句，绝不劈开单词
            if splitAt == nil && !isCJK {
                for idx in stride(from: maxIdx, through: minIdx, by: -1) {
                    if chars[idx] == " " { splitAt = idx; break }
                }
                if splitAt == nil {
                    for idx in maxIdx..<chars.count where chars[idx] == " " {
                        splitAt = idx; break
                    }
                }
            }
            let cut = splitAt ?? maxIdx
            let head = String(chars[..<cut]).trimmingCharacters(in: .whitespacesAndNewlines)
            let tail = String(chars[cut...]).trimmingCharacters(in: .whitespacesAndNewlines)
            if !head.isEmpty { chunks.append(head) }
            if tail.isEmpty { break }
            remaining = tail
        }
        if !remaining.isEmpty { chunks.append(remaining) }
        return chunks
    }

    /// 影视中文字幕规范：
    /// 折叠内部换行为单行；将句中及行尾的中文句号、逗号、顿号、分号替换为自然空格（保留问号、感叹号、省略号、引号与小数点）。
    static func sanitizeChineseSubtitleLine(_ text: String) -> String {
        var line = text.split(whereSeparator: \.isNewline).joined(separator: " ")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard line.contains(where: \.isCJK) else { return line }
        for mark in ["。", "，", "、", "；"] {
            line = line.replacingOccurrences(of: mark, with: " ")
        }
        line = line.replacingOccurrences(of: #"\s+"#, with: " ", options: .regularExpression)
            .trimmingCharacters(in: .whitespaces)
        let trailingRemovable: Set<Character> = [",", ";"]
        while let last = line.last, trailingRemovable.contains(last) {
            line.removeLast()
        }
        return line.trimmingCharacters(in: .whitespaces)
    }

    private static func suffixPrefixOverlap(_ a: String, _ b: String) -> Int? {
        let aChars = Array(a)
        let bChars = Array(b)
        let maxLen = min(aChars.count, bChars.count, 40)
        guard maxLen >= 4 else { return nil }
        for len in stride(from: maxLen, through: 4, by: -1) {
            if aChars[(aChars.count - len)...].elementsEqual(bChars[..<len]) {
                return len
            }
        }
        return nil
    }

    static func renderSRT(_ cues: [Cue]) -> String {
        cues.enumerated().map { idx, cue in
            "\(idx + 1)\n\(formatSRTTimestamp(cue.startMs)) --> \(formatSRTTimestamp(cue.endMs))\n\(cue.text)"
        }.joined(separator: "\n\n") + "\n"
    }

    static func formatASSTimestamp(_ ms: Int) -> String {
        let clamped = max(0, ms)
        let h = clamped / 3_600_000
        let m = (clamped % 3_600_000) / 60_000
        let s = (clamped % 60_000) / 1000
        let cs = (clamped % 1000) / 10
        return String(format: "%d:%02d:%02d.%02d", h, m, s, cs)
    }

    private static func escapeASS(_ text: String) -> String {
        text.replacingOccurrences(of: "\\", with: "＼")
            .replacingOccurrences(of: "{", with: "（")
            .replacingOccurrences(of: "}", with: "）")
    }

    /// 生成影视规范样式的 ASS 字幕（1920×1080 基准）：
    /// 中文主行 52px 纯白细黑边，英文副行 34px 柔和浅灰紧贴下方；在 IINA / VLC / Infuse 打开即得舒适观感，无需手动调字号。
    static func renderASS(_ cues: [Cue], bilingual: Bool) -> String {
        let header = """
        [Script Info]
        ScriptType: v4.00+
        PlayResX: 1920
        PlayResY: 1080
        ScaledBorderAndShadow: yes
        WrapStyle: 2

        [V4+ Styles]
        Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
        Style: Default,PingFang SC,52,&H00FFFFFF,&H000000FF,&H00181818,&H64000000,0,0,0,0,100,100,1,0,1,2.2,1.0,2,60,60,42,1
        Style: Secondary,SF Pro Text,34,&H00DCDCDC,&H000000FF,&H00181818,&H64000000,0,0,0,0,100,100,0,0,1,1.6,0.8,2,60,60,42,1

        [Events]
        Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
        """
        let lines = cues.map { cue -> String in
            let start = formatASSTimestamp(cue.startMs)
            let end = formatASSTimestamp(max(cue.endMs, cue.startMs + 100))
            let body: String
            if bilingual, let nl = cue.text.firstIndex(of: "\n") {
                let top = escapeASS(String(cue.text[..<nl]))
                let bottom = escapeASS(String(cue.text[cue.text.index(after: nl)...]))
                body = "\(top)\\N{\\rSecondary}\(bottom)"
            } else {
                body = escapeASS(cue.text.replacingOccurrences(of: "\n", with: "\\N"))
            }
            return "Dialogue: 0,\(start),\(end),Default,,0,0,0,,\(body)"
        }
        return header + "\n" + lines.joined(separator: "\n") + "\n"
    }

    // MARK: - AI 批量润色与翻译

    /// 解析模型返回的编号行（如 `[1] 译文`），严格保证长度与输入一致；缺失项回退原文。
    static func mergeAIResponse(_ response: String, originals: [String], bilingual: Bool) -> (texts: [String], accepted: Int) {
        var map: [Int: String] = [:]
        var duplicates: Set<Int> = []
        for rawLine in response.components(separatedBy: .newlines) {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            guard line.hasPrefix("["), let close = line.firstIndex(of: "]") else { continue }
            let idxStr = line[line.index(after: line.startIndex)..<close]
            guard let num = Int(idxStr), num >= 1, num <= originals.count else { continue }
            let text = String(line[line.index(after: close)...]).trimmingCharacters(in: .whitespaces)
            guard !text.isEmpty else { continue }
            if map.updateValue(text, forKey: num - 1) != nil { duplicates.insert(num - 1) }
        }
        for index in duplicates { map[index] = nil }
        let texts = originals.enumerated().map { i, orig in
            guard let polished = map[i], !polished.isEmpty else { return orig }
            if bilingual, polished.lowercased() != orig.lowercased() {
                return "\(polished)\n\(orig)"
            }
            return polished
        }
        return (texts, map.count)
    }

    /// 翻译前抽取全片统一术语表（3–8 条核心专有名词/术语），确保前后批次译名 100% 一致。
    private static func extractGlossary(title: String, cues: [Cue], config: SubtitlePolishConfig) async -> [String] {
        let base = config.effectiveBaseURL.trimmingCharacters(in: .whitespacesAndNewlines).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let key = config.effectiveAPIKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !base.isEmpty, !config.model.isEmpty,
              let url = URL(string: base + (config.provider == .anthropic ? "/messages" : "/chat/completions")) else { return [] }
        let head = cues.prefix(15).map(\.text)
        let midStart = cues.count > 40 ? cues.count / 2 : cues.count
        let mid = cues.dropFirst(midStart).prefix(10).map(\.text)
        let sample = (head + mid).joined(separator: "\n")
        let prompt = """
        从下面视频标题与字幕片段中提取 3 到 8 个需要全片统一译法的专有名词或核心技术术语（品牌/产品名如 Claude Code、Anthropic 保留英文，AI agent 译为 AI 智能体等）。
        每行输出一条，格式严格为「原文 = 译文」，不要输出任何解释或编号。
        """
        let userContent = "标题：\(title)\n\n片段：\n\(sample)"
        var payload: [String: Any] = ["model": config.model]
        var req = URLRequest(url: url, timeoutInterval: 20)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if config.provider == .anthropic {
            req.setValue(key, forHTTPHeaderField: "x-api-key")
            req.setValue("2023-06-01", forHTTPHeaderField: "anthropic-version")
            payload["system"] = prompt
            payload["max_tokens"] = 400
            payload["messages"] = [["role": "user", "content": userContent]]
        } else {
            if !key.isEmpty { req.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization") }
            payload["temperature"] = 0.1
            payload["messages"] = [
                ["role": "system", "content": prompt],
                ["role": "user", "content": userContent],
            ]
            if config.provider == .deepseek { payload["thinking"] = ["type": "disabled"] }
        }
        req.httpBody = try? JSONSerialization.data(withJSONObject: payload)
        let cfg = URLSessionConfiguration.ephemeral
        if config.usesLocalEndpoint { cfg.connectionProxyDictionary = [:] }
        let session = URLSession(configuration: cfg)
        defer { session.finishTasksAndInvalidate() }
        guard let (data, response) = try? await session.data(for: req),
              (response as? HTTPURLResponse)?.statusCode == 200,
              let content = responseText(data, provider: config.provider) else { return [] }
        return content.components(separatedBy: .newlines)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { $0.contains("=") && $0.count <= 80 }
            .prefix(10)
            .map { $0 }
    }

    private static func callAI(batch: [String], config: SubtitlePolishConfig, translate: Bool, bilingual: Bool, glossary: [String] = []) async -> (texts: [String], accepted: Int, error: String?) {
        let base = config.effectiveBaseURL.trimmingCharacters(in: .whitespacesAndNewlines).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let key = config.effectiveAPIKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !base.isEmpty, !config.model.isEmpty,
              let url = URL(string: base + (config.provider == .anthropic ? "/messages" : "/chat/completions")) else { return (batch, 0, L("AI 字幕服务尚未配置")) }
        let numbered = batch.enumerated().map { index, text in
            "[\(index + 1)] " + text.split(whereSeparator: \.isNewline).joined(separator: " ")
        }.joined(separator: "\n")
        let glossaryBlock = glossary.isEmpty ? "" : "\n全片统一术语表（务必严格遵守）：\n" + glossary.joined(separator: "\n")
        let systemPrompt = translate
            ? """
            你是一名专业影视字幕组翻译。请将下面带编号的外语字幕逐条意译为地道、自然、简练的简体中文口语，结合上下文理顺语序，去掉生硬翻译腔与无意义口水词，保留数字与原意。
            严格遵守国内影视中文字幕规范：中文不用句号、逗号、顿号，句中停顿直接用空格隔开（仅保留问号、感叹号、引号），每条译文保持单行简洁，不换行。\(glossaryBlock)
            严格按原编号输出，每条占一行，格式为「[编号] 译文」。每个编号必须恰好输出一次，不合并、拆分或解释。
            """
            : """
            你是一名专业影视字幕校对。逐条检查带编号的字幕，修正错别字、确定的听写错误、标点、大小写和明显断句问题。原语种、口语节奏、术语和原意照留，正确的条目照原样返回。
            严格按原编号输出，每条占一行，格式为「[编号] 校对后的字幕」。每个编号必须恰好输出一次，不翻译、合并、拆分或解释。
            """
        var payload: [String: Any] = ["model": config.model]
        var req = URLRequest(url: url, timeoutInterval: config.usesLocalEndpoint ? 180 : 90)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if config.provider == .anthropic {
            req.setValue(key, forHTTPHeaderField: "x-api-key")
            req.setValue("2023-06-01", forHTTPHeaderField: "anthropic-version")
            payload["system"] = systemPrompt
            payload["max_tokens"] = 3000
            payload["messages"] = [["role": "user", "content": numbered]]
        } else {
            if !key.isEmpty { req.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization") }
            payload["temperature"] = 0.2
            payload["messages"] = [
                ["role": "system", "content": systemPrompt],
                ["role": "user", "content": numbered],
            ]
            if config.provider == .deepseek { payload["thinking"] = ["type": "disabled"] }
        }
        req.httpBody = try? JSONSerialization.data(withJSONObject: payload)
        let cfg = URLSessionConfiguration.ephemeral
        if config.usesLocalEndpoint { cfg.connectionProxyDictionary = [:] }
        let session = URLSession(configuration: cfg)
        defer { session.finishTasksAndInvalidate() }
        do {
            let (data, response) = try await session.data(for: req)
            guard let http = response as? HTTPURLResponse else {
                return (batch, 0, L("AI 字幕服务响应无效"))
            }
            guard http.statusCode == 200 else {
                let message: String
                switch http.statusCode {
                case 401, 403: message = L("AI 字幕服务拒绝授权，请检查密钥和权限")
                case 402: message = L("AI 字幕服务余额不足")
                case 429: message = L("AI 字幕服务请求过于频繁")
                default: message = L("AI 字幕服务返回错误") + " (HTTP \(http.statusCode))"
                }
                return (batch, 0, message)
            }
            guard let content = responseText(data, provider: config.provider), !content.isEmpty else {
                return (batch, 0, L("AI 字幕服务未返回可用文本"))
            }
            let result = mergeAIResponse(content, originals: batch, bilingual: bilingual)
            let formatted = translate ? result.texts.map { entry -> String in
                if bilingual, let splitIdx = entry.firstIndex(of: "\n") {
                    let top = sanitizeChineseSubtitleLine(String(entry[..<splitIdx]))
                    let bottom = String(entry[entry.index(after: splitIdx)...])
                    return "\(top)\n\(bottom)"
                }
                return sanitizeChineseSubtitleLine(entry)
            } : result.texts
            return (formatted, result.accepted, nil)
        } catch let error as URLError where error.code == .timedOut {
            return (batch, 0, L("AI 字幕服务响应超时"))
        } catch {
            return (batch, 0, L("连不上 AI 字幕服务，请检查网络"))
        }
    }

    static func responseText(_ data: Data, provider: SubtitlePolishConfig.Provider) -> String? {
        guard let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        if provider == .anthropic {
            let blocks = obj["content"] as? [[String: Any]] ?? []
            return blocks.compactMap { $0["type"] as? String == "text" ? $0["text"] as? String : nil }
                .joined(separator: "\n")
        }
        let choices = obj["choices"] as? [[String: Any]]
        let message = choices?.first?["message"] as? [String: Any]
        return message?["content"] as? String
    }

    /// 设置窗口「测试连接」：只读 `GET /models` 拉取最新模型列表，不产生任何费用。
    static func testAndFetchModels(config: SubtitlePolishConfig, overrideKey: String? = nil) async -> (ok: Bool, models: [String], message: String) {
        let base = config.effectiveBaseURL.trimmingCharacters(in: .whitespacesAndNewlines).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let key = (overrideKey?.isEmpty == false ? overrideKey! : config.effectiveAPIKey).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !base.isEmpty, let url = URL(string: base + "/models"), url.host != nil else {
            return (false, [], L("请先填写有效的接口地址"))
        }
        if config.provider.needsAPIKey && config.provider != .custom && key.isEmpty {
            return (false, [], L("请先粘贴 API 密钥"))
        }
        var req = URLRequest(url: url, timeoutInterval: 12)
        req.httpMethod = "GET"
        if !key.isEmpty {
            if config.provider == .anthropic {
                req.setValue(key, forHTTPHeaderField: "x-api-key")
                req.setValue("2023-06-01", forHTTPHeaderField: "anthropic-version")
            } else {
                req.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
            }
        }
        let cfg = URLSessionConfiguration.ephemeral
        if config.usesLocalEndpoint { cfg.connectionProxyDictionary = [:] }
        let session = URLSession(configuration: cfg)
        defer { session.finishTasksAndInvalidate() }
        do {
            let (data, resp) = try await session.data(for: req)
            let code = (resp as? HTTPURLResponse)?.statusCode ?? 0
            switch code {
            case 200:
                let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
                let arr = obj?["data"] as? [[String: Any]] ?? []
                let excluded = ["embed", "whisper", "tts", "dall-e", "imagen", "moderation", "audio", "realtime", "transcribe", "rerank", "bge-"]
                var ids: [String] = []
                var seen = Set<String>()
                for item in arr {
                    guard var id = item["id"] as? String, !id.isEmpty else { continue }
                    if id.hasPrefix("models/") { id = String(id.dropFirst("models/".count)) }
                    let lower = id.lowercased()
                    if excluded.contains(where: { lower.contains($0) }) { continue }
                    if seen.insert(id).inserted { ids.append(id) }
                }
                let sorted = ids.sorted { $0.localizedStandardCompare($1) == .orderedDescending }
                let msg = sorted.isEmpty ? L("连接成功") : String(localized: "连接成功，已同步 \(sorted.count) 个最新模型")
                return (true, sorted, msg)
            case 401: return (false, [], L("服务拒绝了这把密钥（401），请确认密钥有效且已复制完整"))
            case 403: return (false, [], L("没有权限（403），请确认密钥权限或当前代理节点"))
            case 402: return (false, [], L("账户余额不足（402）"))
            case 404: return (false, [], L("这个地址读不到模型列表（404），可选择「手动填写…」"))
            case 429: return (false, [], L("请求太频繁，稍后再试"))
            default: return (false, [], String(localized: "服务返回错误（\(code)）"))
            }
        } catch {
            return (false, [], L("连接不上，请检查接口地址和网络"))
        }
    }

    /// 对指定视频的字幕执行结构修整与可选 AI 润色。返回处理结果摘要（nil 表示无字幕可处理）。
    static func process(
        videoURL: URL,
        config: SubtitlePolishConfig = .current,
        mode: SubtitleMode,
        aiTranslator: (@Sendable (_ batch: [String], _ isChineseSource: Bool, _ bilingual: Bool) async -> (texts: [String], succeeded: Bool))? = nil,
        progress: (@Sendable (_ completed: Int, _ total: Int) -> Void)? = nil
    ) async -> String? {
        let path = videoURL.standardizedFileURL.path
        guard activeVideos.withLock({ $0.insert(path).inserted }) else { return L("此视频的字幕正在处理") }
        defer { activeVideos.withLock { _ = $0.remove(path) } }
        let fm = FileManager.default
        let dir = videoURL.deletingLastPathComponent()
        let videoStem = videoURL.deletingPathExtension().lastPathComponent

        let existingBackup = findBackup(for: videoURL)
        guard let subURL = existingBackup?.original ?? findSubtitle(for: videoURL) else { return nil }
        let bakURL = existingBackup?.backup ?? backupURL(for: subURL)
        // 重复润色始终从原字幕备份开始
        let sourceURL = fm.fileExists(atPath: bakURL.path) ? bakURL : subURL
        guard let rawContent = try? String(contentsOf: sourceURL, encoding: .utf8), !rawContent.isEmpty else { return nil }

        // 备份必须先写成功，绝不在备份失败的情况下覆盖或移除原字幕
        if !fm.fileExists(atPath: bakURL.path) {
            do { try Data(rawContent.utf8).write(to: bakURL, options: .atomic) }
            catch { return nil }
        }

        var outputs = SubtitleOutputs(backup: bakURL)
        let parsedCues = parse(rawContent)
        guard !parsedCues.isEmpty else { return nil }
        let dedupedCues = cleanRollingSubtitles(parsedCues)
        guard !dedupedCues.isEmpty else { return nil }
        // 翻译模式先按语义重断句，彻底消除 YouTube 碎片半句
        let cleanedCues = mode == .translate ? resegmentSentences(dedupedCues) : dedupedCues

        let srcLang = detectLanguage(subtitleURL: subURL, videoURL: videoURL, cues: cleanedCues)
        let isChineseSource = (srcLang == "zh")
        let cleanedSourceURL = srcLang.map { dir.appendingPathComponent("\(videoStem).\($0).srt") }
            ?? dir.appendingPathComponent("\(videoStem).srt")

        let translate = mode == .translate && !isChineseSource
        let canRunAI = mode.isAI && (aiTranslator != nil || config.isConfiguredForAI)
        if !canRunAI {
            do {
                try outputs.write(Data(renderSRT(cleanedCues).utf8), to: cleanedSourceURL, source: subURL)
                if subURL.standardizedFileURL != cleanedSourceURL.standardizedFileURL {
                    try? fm.removeItem(at: subURL)
                }
                return mode.isAI ? L("AI 字幕服务尚未配置，已保留原文") : L("已自动修整字幕结构")
            } catch {
                return nil
            }
        }

        let effectiveBilingual = translate && config.bilingual
        let glossary: [String] = (translate && aiTranslator == nil)
            ? await extractGlossary(title: videoStem, cues: cleanedCues, config: config)
            : []
        var aiCuesAccepted = 0
        var serviceError: String?
        var updated: [Cue] = []
        var offset = 0
        while offset < cleanedCues.count {
            let end = min(offset + batchSize, cleanedCues.count)
            let slice = Array(cleanedCues[offset..<end])
            let result: (texts: [String], accepted: Int, error: String?)
            if let aiTranslator {
                let testResult = await aiTranslator(slice.map(\.text), isChineseSource, effectiveBilingual)
                result = (testResult.texts, testResult.succeeded ? min(testResult.texts.count, slice.count) : 0, nil)
            } else {
                result = await callAI(batch: slice.map(\.text), config: config, translate: translate, bilingual: effectiveBilingual, glossary: glossary)
            }
            aiCuesAccepted += result.accepted
            for (i, cue) in slice.enumerated() {
                let text = i < result.texts.count ? result.texts[i] : cue.text
                updated.append(Cue(startMs: cue.startMs, endMs: cue.endMs, text: text))
            }
            offset = end
            if result.error == nil && end < cleanedCues.count { progress?(end, cleanedCues.count) }
            if let error = result.error {
                serviceError = error
                updated.append(contentsOf: cleanedCues.dropFirst(offset))
                break
            }
        }

        let outputTargetURL: URL
        if !translate {
            outputTargetURL = cleanedSourceURL
        } else if effectiveBilingual {
            // 双语字幕只输出带影视字号样式的 .ass，避免文件夹同时出现 .srt 和 .ass 让人不知选哪个
            outputTargetURL = dir.appendingPathComponent("\(videoStem).zh-\(srcLang ?? "orig").ass")
        } else {
            outputTargetURL = dir.appendingPathComponent("\(videoStem).zh.srt")
        }

        do {
            if !translate {
                try outputs.write(Data(renderSRT(updated).utf8), to: outputTargetURL, source: subURL)
                if subURL.standardizedFileURL != outputTargetURL.standardizedFileURL {
                    try? fm.removeItem(at: subURL)
                }
            } else if aiCuesAccepted > 0 {
                // 选翻译时文件夹内只保留这一份最终字幕（避免和原英文字幕并存导致播放器默认挂错轨）；原字幕仍在隐藏备份中可随时恢复
                if effectiveBilingual {
                    try outputs.write(Data(renderASS(updated, bilingual: true).utf8), to: outputTargetURL, source: subURL)
                    let legacySRT = outputTargetURL.deletingPathExtension().appendingPathExtension("srt")
                    try? fm.removeItem(at: legacySRT)
                } else {
                    try outputs.write(Data(renderSRT(updated).utf8), to: outputTargetURL, source: subURL)
                }
                if subURL.standardizedFileURL != outputTargetURL.standardizedFileURL {
                    try? fm.removeItem(at: subURL)
                }
                if cleanedSourceURL.standardizedFileURL != outputTargetURL.standardizedFileURL {
                    try? fm.removeItem(at: cleanedSourceURL)
                }
            } else {
                // AI 未返回有效译文时兜底保留清理后的原语种字幕
                try outputs.write(Data(renderSRT(cleanedCues).utf8), to: cleanedSourceURL, source: subURL)
                if subURL.standardizedFileURL != cleanedSourceURL.standardizedFileURL {
                    try? fm.removeItem(at: subURL)
                }
            }

            if aiCuesAccepted == cleanedCues.count { return translate ? L("已生成中文字幕") : L("AI 字幕校对完成") }
            let summary = aiCuesAccepted > 0
                ? L("部分字幕已完成 AI 处理，其余保留原文")
                : L("未生成 AI 字幕，已保留原文")
            return serviceError.map { summary + " · " + $0 } ?? summary
        } catch {
            return nil
        }
    }
}

private extension Character {
    var isCJK: Bool {
        unicodeScalars.contains { scalar in
            (0x4E00...0x9FFF).contains(scalar.value) || (0x3400...0x4DBF).contains(scalar.value)
        }
    }
}
