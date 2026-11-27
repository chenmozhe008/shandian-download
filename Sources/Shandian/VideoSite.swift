import Foundation
import Darwin

/// 视频网站下载选项（参考 Downie：默认直接下最高画质，选项记住后 ⌘V 也按它来）
struct VideoOptions: Codable, Equatable {
    var maxHeight = 0          // 0 = 最高；否则不超过这个分辨率
    var subtitles = false      // 下载字幕并嵌入视频
    var audioOnly = false      // 只下音频（m4a）
    var subtitleMode: SubtitleMode? // 老任务和无确认框下载默认只下载原字幕

    private static let key = "videoOptions"
    static var current: VideoOptions {
        get { (UserDefaults.standard.data(forKey: key)).flatMap { try? JSONDecoder().decode(VideoOptions.self, from: $0) } ?? VideoOptions() }
        set { UserDefaults.standard.set(try? JSONEncoder().encode(newValue), forKey: key) }
    }

    /// 对应的 yt-dlp 参数
    var arguments: [String] { arguments(for: "") }

    func arguments(for rawURL: String) -> [String] {
        if audioOnly { return ["-f", "ba/b", "-x", "--audio-format", "m4a"] }
        // 先比分辨率，同分辨率优先 H.264/AAC。
        var args = ["-f", "bv*+ba/b", "-S", (maxHeight > 0 ? "res:\(maxHeight)" : "res") + ",vcodec:h264,acodec:aac", "--merge-output-format", "mp4"]
        if subtitles && !VideoSiteRunner.isYouTube(rawURL) {
            args += ["--write-subs", "--write-auto-subs", "--sub-format", "srt/vtt/best", "--sub-langs", "zh-Hans,zh-CN,zh,en,ja,ko,ai-zh"]
            if subtitleMode != .translate { args += ["--embed-subs"] }
        }
        return args
    }

    /// 勾选字幕时为视频建独立文件夹，视频与各语种字幕集中存放；未勾选时平铺存放。
    var outputTemplate: String {
        (subtitles && !audioOnly)
            ? "%(title).80s [%(id)s]/%(title).80s [%(id)s].%(ext)s"
            : "%(title).80s [%(id)s].%(ext)s"
    }
}

/// 视频网站的一条下载（yt-dlp 进程）。完成的记录存到 videos.json，重开 App 还在。
struct VideoJob: Codable, Identifiable {
    var id = UUID()
    var url: String
    var title: String
    var site: String
    var thumbnailURL: String?
    var filePath: String?
    var downloaded: Int64 = 0
    var total: Int64 = 0
    var progress: Double?
    var speed: Int64 = 0
    var state: State = .running
    var error: String?
    var createdAt = Date()
    var options: VideoOptions?     // 旧记录没有这一项，按默认处理
    var outputDirectory: String?    // 重试沿用创建任务时的目录
    var subtitleNotice: String?
    var subtitleProcessing: Bool?
    var stage: Stage?              // 下载中处在哪一步，界面据此显示（旧记录没有）

    enum State: String, Codable { case running, done, failed }
    /// 解析网页 → 下视频 → 下音频 → 合并（或只下音频 → 转换）→ 翻译字幕。解析、合并、翻译时没有字节进度，界面不能停在 0
    enum Stage: String, Codable { case preparing, video, audio, merging, converting, translating }
}

/// App 自带一份 yt-dlp，放在配置目录、每周检查新版。视频站改版快，旧版会直接失效
/// （2026-10-27 实测：brew 里 2025-04 版下 B 站报 412，最新版不登录可下 1080P）。
/// 用官方「目录版」yt-dlp_macos.zip：单文件版 yt-dlp_macos 每次启动都要解包到临时目录、系统再扫一遍，
/// 10-29 实测每条视频白等 16 秒；目录版只有装好后第一次 16 秒，之后 0.3 秒。目录版不支持 -U，更新靠重新下载。
enum YtDlp {
    /// yt-dlp 报的是没连上（超时、解析不了域名、握手失败等）
    static func isConnectionError(_ error: String?) -> Bool {
        let lower = (error ?? "").lowercased()
        return ["timed out", "connection refused", "connection reset", "connection aborted", "network is unreachable", "nodename nor servname",
                "name or service not known", "failed to resolve", "unable to connect", "handshake", "remote end closed"].contains { lower.contains($0) }
    }

    static let dir = EngineConfig.supportDir.appendingPathComponent("bin/yt-dlp_macos")
    static let bundled = dir.appendingPathComponent("yt-dlp_macos")
    static var packaged: URL? { Bundle.main.resourceURL?.appendingPathComponent("yt-dlp_macos/yt-dlp_macos") }
    static let legacy = EngineConfig.supportDir.appendingPathComponent("bin/yt-dlp")   // 0.14 及以前的单文件版
    static let latestRelease = URL(string: "https://github.com/yt-dlp/yt-dlp/releases/latest")!

    private static let packagedVersion = Bundle.main.resourceURL
        .flatMap { try? String(contentsOf: $0.appendingPathComponent("yt-dlp-version.txt"), encoding: .utf8) }?
        .trimmingCharacters(in: .whitespacesAndNewlines)

    static var path: String? {
        let fm = FileManager.default
        let embedded = packaged.flatMap { fm.isExecutableFile(atPath: $0.path) ? $0.path : nil }
        if fm.isExecutableFile(atPath: bundled.path) {
            if let embedded, let packagedVersion,
               run(["--version"]).map({ isOlder($0, than: packagedVersion) }) ?? true {
                return embedded
            }
            return bundled.path
        }
        return embedded ?? ([legacy.path, "/opt/homebrew/bin/yt-dlp", "/usr/local/bin/yt-dlp"].first {
            fm.isExecutableFile(atPath: $0)
        })
    }

    static func isOlder(_ current: String, than candidate: String) -> Bool {
        current.compare(candidate, options: .numeric) == .orderedAscending
    }

    /// 只有查到有效版本时才记录检查时间，避免 GitHub 请求失败后继续用旧版等一周。
    static func ensureFresh() async {
        let fm = FileManager.default
        let installedVersion = run(["--version"])
        let modified = (try? fm.attributesOfItem(atPath: bundled.path)[.modificationDate] as? Date) ?? .distantPast
        let embeddedIsNewer = packagedVersion.map { version in
            installedVersion.map { isOlder($0, than: version) } ?? true
        } ?? false
        guard embeddedIsNewer || Date().timeIntervalSince(modified) > 7 * 86400 else { return }
        let cfg = URLSessionConfiguration.ephemeral
        cfg.connectionProxyDictionary = LocalProxy.sessionDictionary
        let session = URLSession(configuration: cfg)
        defer { session.finishTasksAndInvalidate() }
        var request = URLRequest(url: latestRelease, timeoutInterval: 15)
        request.httpMethod = "HEAD"
        guard let (_, response) = try? await session.data(for: request),
              let http = response as? HTTPURLResponse, http.statusCode == 200,
              let release = http.url, release.host == "github.com",
              release.deletingLastPathComponent().path == "/yt-dlp/yt-dlp/releases/tag",
              release.lastPathComponent.range(of: #"^\d{4}\.\d{2}\.\d{2}$"#, options: .regularExpression) != nil else { return }
        let tag = release.lastPathComponent
        if let installedVersion, !isOlder(installedVersion, than: tag) {
            try? fm.setAttributes([.modificationDate: Date()], ofItemAtPath: bundled.path)
        } else if packagedVersion.map({ !isOlder(tag, than: $0) }) ?? true {
            await install(version: tag)
        }
    }

    /// 下载目录版、解压后整个换上；成功后删掉旧的单文件版，并空跑一次让系统先扫描完（否则第一条视频要多等 16 秒）
    private static func install(version: String) async {
        let fm = FileManager.default
        let cfg = URLSessionConfiguration.ephemeral
        cfg.connectionProxyDictionary = LocalProxy.sessionDictionary
        let download = URL(string: "https://github.com/yt-dlp/yt-dlp/releases/download/\(version)/yt-dlp_macos.zip")!
        guard let (zip, resp) = try? await URLSession(configuration: cfg).download(from: download),
              (resp as? HTTPURLResponse)?.statusCode == 200 else { return }
        let staging = dir.deletingLastPathComponent().appendingPathComponent(".yt-dlp-new")
        try? fm.removeItem(at: staging)
        let unzip = Process()
        unzip.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
        unzip.arguments = ["-x", "-k", zip.path, staging.path]
        defer { try? fm.removeItem(at: zip) }
        guard (try? unzip.run()) != nil else { return }
        unzip.waitUntilExit()
        let exe = staging.appendingPathComponent("yt-dlp_macos")
        guard unzip.terminationStatus == 0, fm.fileExists(atPath: exe.path) else { try? fm.removeItem(at: staging); return }
        try? fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: exe.path)
        try? fm.removeItem(at: dir)
        guard (try? fm.moveItem(at: staging, to: dir)) != nil else { return }
        try? fm.removeItem(at: legacy)
        _ = run(["--version"])
    }

    /// 同步跑一次，返回去掉首尾空白的输出
    @discardableResult
    static func run(_ args: [String]) -> String? {
        let p = Process()
        p.executableURL = bundled
        p.arguments = args
        let out = Pipe(); p.standardOutput = out; p.standardError = FileHandle.nullDevice
        guard (try? p.run()) != nil else { return nil }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

final class VideoSiteRunner: @unchecked Sendable {
    private final class Transfer {
        var process: Process?
        var cancelled = false
    }

    private var transfers: [UUID: Transfer] = [:]
    private let lock = NSLock()
    private let downloader: @Sendable () -> String?

    init(downloader: @escaping @Sendable () -> String? = { YtDlp.path }) {
        self.downloader = downloader
    }

    static func siteName(_ url: String) -> String {
        let host = URL(string: url)?.host?.lowercased() ?? ""
        let names: [(String, String)] = [("movie.douban.com", "豆瓣"), ("bilibili", "B 站"), ("b23.tv", "B 站"), ("youtu", "YouTube"), ("douyin", "抖音"),
                                         ("xiaohongshu", "小红书"), ("xhslink", "小红书"), ("kuaishou", "快手"), ("weibo", "微博"),
                                         ("ixigua", "西瓜视频"), ("vimeo", "Vimeo"), ("x.com", "X"), ("twitter", "X")]
        return names.first { host.contains($0.0) }?.1 ?? host
    }

    static func isYouTube(_ rawURL: String) -> Bool {
        guard let host = URL(string: rawURL)?.host?.lowercased() else { return false }
        return host == "youtube.com" || host.hasSuffix(".youtube.com") || host == "youtu.be"
    }

    /// 优先选与原声自动字幕同语种的人工轨，纯机翻轨不参与选择。
    static func youtubeSubtitleArguments(metadata data: Data) -> [String] {
        guard let info = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return [] }
        let auto = (info["automatic_captions"] as? [String: [[String: Any]]] ?? [:])
            .keys.filter { $0.hasSuffix("-orig") }.sorted()
        let manual = (info["subtitles"] as? [String: [[String: Any]]] ?? [:])
            .keys.filter { $0 != "live_chat" }.sorted()
        let language = (info["language"] as? String)?.lowercased()
        let matchingTrack = language.flatMap { lang -> String? in
            if let exact = manual.first(where: { $0.lowercased() == lang }) { return exact }
            if let exact = auto.first(where: { $0.lowercased() == lang + "-orig" }) { return exact }
            guard let primary = SubtitlePolisher.normalizeLanguageTag(lang) else { return nil }
            return manual.first { SubtitlePolisher.normalizeLanguageTag($0) == primary }
                ?? auto.first { SubtitlePolisher.normalizeLanguageTag(String($0.dropLast(5))) == primary }
        }
        let original = auto.first { $0 == "en-orig" } ?? auto.first
        let matchingManual = original.flatMap { fallback in
            let lang = String(fallback.dropLast(5))
            return manual.first { $0 == lang }
        }
        let track = matchingTrack ?? matchingManual ?? original ?? manual.first { $0 == "en" } ?? manual.first
        guard let track, track.range(of: #"^[A-Za-z0-9][A-Za-z0-9-]*$"#, options: .regularExpression) != nil else { return [] }
        // yt-dlp 嵌入后用 --write-subs 决定是否保留外挂文件，自动轨也需要此开关。
        var args = ["--ignore-errors", "--write-subs", "--sub-format", "srt/vtt", "--sub-langs", track]
        if !manual.contains(track) { args += ["--write-auto-subs"] }
        return args
    }

    /// 启动下载；进度、标题、封面、成品路径通过 update 回调（在后台线程）
    func start(_ job: VideoJob, into dir: URL, headers: [String: String] = [:], subtitleConfig: SubtitlePolishConfig = .current, update: @escaping @Sendable (VideoJob) -> Void) {
        let started = lock.withLock {
            guard transfers[job.id] == nil else { return false }
            transfers[job.id] = Transfer()
            return true
        }
        guard started else { return }
        DispatchQueue.global(qos: .utility).async {
            defer { self.lock.withLock { self.transfers[job.id] = nil } }
            self.download(job, into: dir, headers: headers, subtitleConfig: subtitleConfig, update: update)
        }
    }

    private func download(_ job: VideoJob, into dir: URL, headers: [String: String], subtitleConfig: SubtitlePolishConfig, update: @escaping @Sendable (VideoJob) -> Void) {
        var first = job
        first.filePath = nil
        first.subtitleNotice = nil
        first.subtitleProcessing = nil
        first.error = nil
        first.stage = .preparing
        update(first)
        let state = JobState(first)
        guard let bin = downloader() else {
            update(state.fail(L("视频下载组件不可用，请重新安装应用后重试")))
            return
        }

        let options = job.options ?? VideoOptions()
        let youtube = Self.isYouTube(job.url)
        let temporaryCookies = Self.temporaryCookieFile(headers, for: job.url)
        defer { if let temporaryCookies { try? FileManager.default.removeItem(at: temporaryCookies) } }
        var common = ["--no-playlist", "--newline", "--no-colors", "--encoding", "utf-8"]
        if youtube { common += ["--ignore-config", "--abort-on-unavailable-fragments"] }
        common += SiteCookies.ytdlpArguments(for: job.url)
        common += Self.safeRequestArguments(headers)
        if let temporaryCookies { common += ["--cookies", temporaryCookies.path] }
        if let node = ["/opt/homebrew/bin/node", "/usr/local/bin/node"].first(where: { FileManager.default.isExecutableFile(atPath: $0) }) {
            common += ["--js-runtimes", "node:" + node]
        }

        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            for attempt in 0..<(youtube ? 2 : 1) {
                let canRetry = youtube && attempt == 0
                let proxy = youtube ? LocalProxy.youtubeYtdlpArguments() : LocalProxy.ytdlpArguments(for: job.url)
                let completed = try downloadAttempt(bin, job: job, into: dir, options: options, common: common + proxy,
                                                    state: state, subtitleConfig: subtitleConfig, canRetry: canRetry, update: update)
                if completed { return }
                state.beginRetry()
            }
        } catch is CancellationError {
            update(state.fail(L("下载已取消")))
        } catch {
            update(state.fail(error.localizedDescription))
        }
    }

    /// 返回 false 时由调用者重新提取一次签名地址；成品由正常完成流程处理。
    private func downloadAttempt(_ bin: String, job: VideoJob, into dir: URL, options: VideoOptions,
                                 common: [String], state: JobState, subtitleConfig: SubtitlePolishConfig,
                                 canRetry: Bool, update: @escaping @Sendable (VideoJob) -> Void) throws -> Bool {
        var arguments = options.arguments(for: job.url)
        let input = ["--", job.url]
        let output = ["-P", dir.path, "-o", options.outputTemplate]
        let wantsSubtitles = Self.isYouTube(job.url) && options.subtitles && !options.audioOnly
        if wantsSubtitles {
            let metadata = FileManager.default.temporaryDirectory.appendingPathComponent("shandian-video-\(job.id).json")
            defer { try? FileManager.default.removeItem(at: metadata) }
            let status = try run(bin, common + arguments + ["--dump-json", "--write-auto-subs", "--sub-langs", "__none__"] + input,
                                 id: job.id, state: state, output: nil)
            guard status.code == 0 else {
                if canRetry && state.shouldRetryYouTube { return false }
                update(state.finish(ok: false))
                return true
            }
            try PrivateFile.write(status.data, to: metadata)
            let subtitles = Self.youtubeSubtitleArguments(metadata: status.data)
            arguments += subtitles
            // AI 翻译使用外挂字幕，避免播放器优先使用内嵌的英文轨。
            if !subtitles.isEmpty && options.subtitleMode != .translate && Thumbnails.ffmpeg != nil {
                arguments += ["--embed-subs"]
            }
            return try downloadMedia(bin, common + arguments + output + ["--load-info-json", metadata.path],
                                     id: job.id, state: state, subtitleConfig: subtitleConfig,
                                     wantsYouTubeSubtitles: true, hasTrack: !subtitles.isEmpty, canRetry: canRetry, update: update)
        }
        return try downloadMedia(bin, common + arguments + output + input, id: job.id, state: state,
                                 subtitleConfig: subtitleConfig, wantsYouTubeSubtitles: false, hasTrack: false,
                                 canRetry: canRetry, update: update)
    }

    private func downloadMedia(_ bin: String, _ args: [String], id: UUID, state: JobState, subtitleConfig: SubtitlePolishConfig,
                               wantsYouTubeSubtitles: Bool, hasTrack: Bool, canRetry: Bool,
                               update: @escaping @Sendable (VideoJob) -> Void) throws -> Bool {
        let progress = [
            "--progress", "--concurrent-fragments", "4",
            "--print", "before_dl:SDTITLE %(title)s",
            "--print", "before_dl:SDFMT %(format_id)s",
            "--print", "before_dl:SDSIZE %(filesize,filesize_approx)s",
            "--print", "before_dl:SDTHUMB %(thumbnail)s",
            "--print", "after_move:SDFILE %(filepath)s",
            "--progress-template", "download:SDPROG %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(progress.speed)s %(progress.status)s",
        ]
        let result = try run(bin, progress + args, id: id, state: state, output: { line in
            if let job = state.apply(line) { update(job) }
        })
        if result.code != 0 && canRetry && state.shouldRetryYouTube { return false }
        var finished = state.finish(ok: result.code == 0)
        guard finished.state == .done, finished.options?.subtitles == true,
              finished.options?.audioOnly == false, let path = finished.filePath else {
            update(finished)
            return true
        }
        let video = URL(fileURLWithPath: path)
        let subtitle = SubtitlePolisher.findSubtitle(for: video)
        if wantsYouTubeSubtitles && subtitle == nil {
            finished.subtitleNotice = hasTrack
                ? L("视频已下载，字幕下载失败")
                : L("视频已下载，未获取到原文字幕轨")
        }
        let config = subtitleConfig
        let mode = finished.options?.subtitleMode ?? .original
        if subtitle != nil && mode.isAI {
            // 翻译字幕是选了 AI 翻译后的最后一步：任务留在「正在下载」里显示实时进度，
            // 等双语字幕全部写入文件夹后再标记完成移入「已完成」，避免点开文件夹时还没字幕。
            var translating = finished
            translating.state = .running
            translating.stage = .translating
            translating.speed = 0
            translating.progress = 0.05
            translating.subtitleProcessing = true
            translating.subtitleNotice = L("提取术语中…")
            update(translating)

            let baseTranslating = translating
            let finalJob = finished
            Task.detached(priority: .utility) {
                let summary = await SubtitlePolisher.process(videoURL: video, config: config, mode: mode, progress: { completed, total in
                    var job = baseTranslating
                    job.progress = max(0.05, min(0.99, Double(completed) / Double(max(1, total))))
                    job.subtitleNotice = "\(completed)/\(total) " + L("条")
                    update(job)
                })
                var done = finalJob
                done.state = .done
                done.stage = nil
                done.progress = 1.0
                done.subtitleProcessing = nil
                done.subtitleNotice = summary ?? L("字幕文件无法处理，已保留原文")
                update(done)
            }
            return true
        }
        update(finished)
        if subtitle != nil && config.autoCleanStructure {
            let downloaded = finished
            Task.detached(priority: .utility) {
                let summary = await SubtitlePolisher.process(videoURL: video, config: config, mode: mode)
                var processed = downloaded
                processed.subtitleProcessing = nil
                processed.subtitleNotice = summary ?? L("字幕文件无法处理，已保留原文")
                update(processed)
            }
        }
        return true
    }

    private func run(_ bin: String, _ args: [String], id: UUID, state: JobState,
                     output: (@Sendable (String) -> Void)?) throws -> (code: Int32, data: Data) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: bin)
        process.arguments = args
        var env = ProcessInfo.processInfo.environment
        env["PATH"] = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
        process.environment = env
        process.standardInput = FileHandle.nullDevice
        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr
        try lock.withLock {
            guard let transfer = transfers[id], !transfer.cancelled else { throw CancellationError() }
            try process.run()
            transfer.process = process
        }

        // 消费两个管道直到 EOF，确保退出前的成品路径和报错都已读取。
        let errors = DispatchGroup()
        errors.enter()
        DispatchQueue.global(qos: .utility).async {
            Self.readLines(stderr.fileHandleForReading, consume: state.recordDiagnostic)
            errors.leave()
        }
        let data: Data
        if let output {
            Self.readLines(stdout.fileHandleForReading, consume: output)
            data = Data()
        } else {
            data = stdout.fileHandleForReading.readDataToEndOfFile()
        }
        process.waitUntilExit()
        errors.wait()
        try lock.withLock {
            guard let transfer = transfers[id], !transfer.cancelled else { throw CancellationError() }
            transfer.process = nil
        }
        return (process.terminationStatus, data)
    }

    private static func readLines(_ handle: FileHandle, consume: (String) -> Void) {
        var pending = Data()
        while true {
            let chunk = handle.availableData
            if chunk.isEmpty { break }
            pending.append(chunk)
            let lines = pending.split(separator: 10, omittingEmptySubsequences: false)
            for line in lines.dropLast() {
                consume(String(decoding: line, as: UTF8.self).trimmingCharacters(in: .newlines))
            }
            pending = Data(lines.last ?? Data())
        }
        if !pending.isEmpty { consume(String(decoding: pending, as: UTF8.self)) }
    }

    /// 仅转交浏览器标识及去掉参数的来源地址；Cookie 和授权头不写入进程参数或任务记录。
    static func safeRequestArguments(_ headers: [String: String]) -> [String] {
        var result: [String] = []
        if let value = headers.first(where: { $0.key.lowercased() == "referer" })?.value,
           !value.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }),
           var parts = URLComponents(string: value),
           ["http", "https"].contains(parts.scheme?.lowercased() ?? ""), parts.host != nil {
            parts.query = nil; parts.fragment = nil; parts.user = nil; parts.password = nil
            if let referer = parts.url?.absoluteString { result += ["--referer", referer] }
        }
        if let agent = headers.first(where: { $0.key.lowercased() == "user-agent" })?.value,
           !agent.isEmpty, agent.utf8.count <= 1024,
           !agent.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) {
            result += ["--user-agent", agent]
        }
        return result
    }

    /// Netscape Cookie 文件严格限定为当前 URL 的主机；文件只在下载期间存在，权限为 0600。
    /// YouTube 公开视频带浏览器 Cookie 但无 PO Token 时，YouTube 会屏蔽视频流导致 Requested format is not available，因此跳过 YouTube。
    static func temporaryCookieFile(_ headers: [String: String], for rawURL: String) -> URL? {
        guard let raw = headers.first(where: { $0.key.lowercased() == "cookie" })?.value,
              raw.utf8.count <= 65536, let url = URL(string: rawURL), let host = url.host,
              !isYouTube(rawURL),
              ["http", "https"].contains(url.scheme?.lowercased() ?? ""),
              !host.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else { return nil }
        let forbidden = CharacterSet.controlCharacters
        let allowedName = CharacterSet(charactersIn: "!#$%&'*+-.^_`|~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz")
        let secure = url.scheme?.lowercased() == "https" ? "TRUE" : "FALSE"
        var lines = ["# Netscape HTTP Cookie File"]
        for pair in raw.split(separator: ";") {
            let parts = pair.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
            guard parts.count == 2 else { continue }
            let name = parts[0].trimmingCharacters(in: .whitespaces)
            let value = parts[1].trimmingCharacters(in: .whitespaces)
            guard !name.isEmpty, name.unicodeScalars.allSatisfy({ allowedName.contains($0) }),
                  !value.unicodeScalars.contains(where: { forbidden.contains($0) }) else { continue }
            lines.append("\(host)\tFALSE\t/\t\(secure)\t0\t\(name)\t\(value)")
        }
        guard lines.count > 1 else { return nil }
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("shandian-cookies-\(UUID().uuidString).txt")
        let fd = open(path.path, O_WRONLY | O_CREAT | O_EXCL, 0o600)
        guard fd >= 0 else { return nil }
        let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        do {
            try handle.write(contentsOf: Data((lines.joined(separator: "\n") + "\n").utf8))
            try handle.close()
            return path
        } catch {
            try? handle.close(); try? FileManager.default.removeItem(at: path); return nil
        }
    }

    func cancel(_ id: UUID) {
        lock.withLock {
            transfers[id]?.cancelled = true
            if let process = transfers[id]?.process, process.isRunning { process.terminate() }
        }
    }

    func cancelAll() {
        lock.withLock {
            for transfer in transfers.values {
                transfer.cancelled = true
                if let process = transfer.process, process.isRunning { process.terminate() }
            }
        }
    }
}

/// 解析 yt-dlp 输出。视频和音频是分两段下的，前一段下完的字节累加进 finishedBytes。
final class JobState: @unchecked Sendable {
    private var job: VideoJob
    private var finishedBytes: Int64 = 0
    private var currentTotal: Int64 = 0
    private var parts = 1          // 视频+音频分开下时是 2
    private var finishedParts = 0
    private var knownTotal: Int64 = 0  // 开下前 yt-dlp 给的整体大小（估算），有它总大小就不会中途变大、进度条不往回跳
    private var maxProgress: Double = 0
    // 速度按最近 3 秒实际下到的字节自己算：yt-dlp 每个新分块都从头估，开头几个包能报出 200 MB/s（10-29 实测）
    private var samples: [(Date, Int64)] = []
    private var lastEmit = Date.distantPast
    private var lastError: String?
    private var challengeFailed = false
    private let lock = NSLock()

    init(_ job: VideoJob) { self.job = job }

    func apply(_ line: String) -> VideoJob? {
        lock.withLock {
            if line.hasPrefix("SDTITLE ") { job.title = String(line.dropFirst(8)) }
            else if line.hasPrefix("SDTHUMB ") { let t = String(line.dropFirst(8)); job.thumbnailURL = t == "NA" ? nil : t }
            else if line.hasPrefix("SDFILE ") { job.filePath = String(line.dropFirst(7)) }
            else if line.hasPrefix("SDFMT ") { parts = line.dropFirst(6).split(separator: "+").count }
            else if line.hasPrefix("SDSIZE ") { knownTotal = Int64(Double(line.dropFirst(7)) ?? 0); job.total = max(job.total, knownTotal) }
            else if line.hasPrefix("SDPROG ") {
                let f = line.split(separator: " ").map(String.init)
                guard f.count >= 6 else { return nil }
                let done = Int64(Double(f[1]) ?? 0)
                let total = Int64(Double(f[2]) ?? Double(f[3]) ?? 0)
                if f[5] == "finished" { finishedBytes += max(total, done); currentTotal = 0; finishedParts += 1 }
                else { currentTotal = total }
                job.downloaded = finishedBytes + (f[5] == "finished" ? 0 : done)
                job.total = max(knownTotal, finishedBytes + currentTotal, job.total)
                // 分段单调进度：YouTube 视频+音频分两段且 HLS 总大小估算会波动；
                // 视频段占 0–90%、音频段占 90–99%、合并后 100%，并做单调递增保护，杜绝进度条往回缩。
                let rawProgress: Double
                if finishedParts >= parts {
                    rawProgress = 0.99
                } else if parts > 1 {
                    let partFrac = currentTotal > 0 ? min(1, max(0, Double(done) / Double(currentTotal))) : 0
                    rawProgress = finishedParts == 0 ? (partFrac * 0.90) : (0.90 + partFrac * 0.09)
                } else {
                    rawProgress = job.total > 0 ? min(0.99, max(0, Double(job.downloaded) / Double(job.total))) : 0
                }
                maxProgress = max(maxProgress, rawProgress)
                job.progress = maxProgress
                let now = Date()
                samples.append((now, job.downloaded))
                samples.removeAll { now.timeIntervalSince($0.0) > 3 }
                if let first = samples.first, now.timeIntervalSince(first.0) > 0.5 {
                    job.speed = max(0, Int64(Double(job.downloaded - first.1) / now.timeIntervalSince(first.0)))
                }
                let stage = job.stage
                let audioOnly = job.options?.audioOnly ?? false
                if finishedParts >= parts { job.stage = audioOnly ? .converting : parts > 1 ? .merging : job.stage; job.speed = 0 }
                else { job.stage = audioOnly || finishedParts > 0 ? .audio : .video }
                // yt-dlp 每秒报几十次：只在换阶段时立即刷新，其余最多每 0.5 秒一次
                guard job.stage != stage || now.timeIntervalSince(lastEmit) >= 0.5 else { return nil }
                lastEmit = now
            } else { return nil }
            return job
        }
    }

    func recordDiagnostic(_ line: String) {
        lock.withLock {
            if line.hasPrefix("ERROR: ") {
                lastError = String(line.dropFirst(7))
            } else if line.contains("challenge solving failed") || line.contains("No supported JavaScript runtime") {
                challengeFailed = true
            }
        }
    }

    var shouldRetryYouTube: Bool {
        lock.withLock {
            guard job.filePath == nil, let lastError else { return false }
            return YtDlp.isConnectionError(lastError) || lastError.localizedCaseInsensitiveContains("HTTP Error 403")
        }
    }

    func beginRetry() {
        lock.withLock {
            lastError = nil
            challengeFailed = false
            samples.removeAll(keepingCapacity: true)
            finishedBytes = 0
            finishedParts = 0
            currentTotal = 0
        }
    }

    func fail(_ message: String) -> VideoJob {
        lock.withLock {
            job.speed = 0
            job.stage = nil
            job.state = .failed
            job.error = message
            return job
        }
    }

    static func readableError(_ error: String?) -> String {
        guard let error else { return L("下载失败") }
        let lower = error.lowercased()
        if lower.contains("drm") || lower.contains("widevine") || lower.contains("fairplay") {
            return L("此视频受数字版权保护，无法下载")
        }
        if lower.contains("ffmpeg") && (lower.contains("not installed") || lower.contains("not found")) {
            return L("缺少 FFmpeg，无法合并音视频或转换格式")
        }
        if lower.contains("http error 429") {
            return L("网站限制了下载请求，请稍后重试")
        }
        if lower.contains("http error 403") || lower.contains("http error 401") {
            return L("视频资源拒绝访问，可能需要登录或链接已过期；请返回网页重新侦测")
        }
        // yt-dlp 错误可能包含带签名的 URL；不要把它写入任务记录。
        return error.replacingOccurrences(of: #"https?://[^\s]+"#, with: L("[链接]"), options: .regularExpression)
    }

    func finish(ok: Bool) -> VideoJob {
        lock.withLock {
            job.speed = 0; job.stage = nil
            let size = job.filePath.flatMap { (try? FileManager.default.attributesOfItem(atPath: $0))?[.size] as? Int64 }
            if ok, let size, size > 0 {
                job.state = .done
                job.total = size
                job.downloaded = size
                job.progress = 1.0
            }
            else {
                job.state = .failed
                // 抖音、小红书没带登录信息时 yt-dlp 的报错看不懂，直接告诉怎么办
                // 抖音只能由扩展从页面里取直链，yt-dlp 必然失败（要签名）：直接告诉怎么办
                job.error = (URL(string: job.url)?.host ?? "").hasSuffix("douyin.com")
                    ? L("请在浏览器里打开这个抖音视频，点扩展里的「下载这个视频」")
                    : SiteCookies.site(of: job.url) != nil && SiteCookies.ytdlpArguments(for: job.url).isEmpty
                    ? L("需要在浏览器里登录后，右键「用闪电全能下载这个视频」")
                    : YtDlp.isConnectionError(lastError) && LocalProxy.isOverseas(job.url)
                    ? L("连不上这个网站：需要科学上网，请检查网络设置")
                    : challengeFailed && (lastError?.localizedCaseInsensitiveContains("format is not available") ?? false)
                    ? L("YouTube 播放器校验失败，请检查 JavaScript 运行环境和视频下载组件版本")
                    : ok && lastError == nil
                    ? L("下载程序未生成可用的视频文件") : Self.readableError(lastError)
            }
            return job
        }
    }
}
