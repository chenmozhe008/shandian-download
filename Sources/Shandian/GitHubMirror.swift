import Foundation

/// GitHub 与 Hugging Face 公开文件加速站测速与失败换站。
/// 部分链接识别、请求头过滤与两轮测速思路参考自 Ghost Downloader (GPL-3.0) 及本项目 Windows 版 `ghmirror.rs`。
///
/// - GitHub 公开文件：走「加速站 + / + 原链接」
/// - Hugging Face 公开 resolve 直链：替换域名为 `https://hf-mirror.com`
/// - 两轮测速：先并发发 `Range: bytes=0-0` 筛出返回 206 且非 HTML 的候选站，
///   大文件（≥ 8 MB）再让最先响应的至多 5 个站各拉取 512 KB 比真实吞吐速度。
/// - 界面与查重始终保留原始链接，发往第三方镜像站时剥离 Cookie / Authorization。
enum GitHubMirror {
    enum Source: Hashable, Sendable {
        case github
        case huggingFace

        var probeTimeout: TimeInterval {
            switch self {
            case .github: 5
            case .huggingFace: 12
            }
        }

        var sampleTimeout: TimeInterval {
            switch self {
            case .github: 3
            case .huggingFace: 8
            }
        }

        var mirrors: [String] {
            switch self {
            case .github: GitHubMirror.githubMirrors
            case .huggingFace: GitHubMirror.hfMirrors
            }
        }
    }

    static let githubMirrors: [String] = [
        "https://gh-proxy.com",
        "https://edgeone.gh-proxy.org",
        "https://hk.gh-proxy.org",
        "https://cdn.gh-proxy.org",
        "https://gh-proxy.org",
        "https://fastgit.cc",
        "https://github.ednovas.xyz",
        "https://gh.monlor.com",
        "https://ghfast.top",
        "https://gh.nxnow.top",
    ]

    static let hfMirrors: [String] = [
        "https://hf-mirror.com",
    ]

    private static let githubHosts: Set<String> = [
        "codeload.github.com",
        "gist.githubusercontent.com",
        "github-releases.githubusercontent.com",
        "media.githubusercontent.com",
        "objects.githubusercontent.com",
        "raw.githubusercontent.com",
        "raw.github.com",
        "release-assets.githubusercontent.com",
    ]

    private static let finalistsLimit = 5
    private static let sampleBytes: Int64 = 512 * 1024
    private static let sampleMinSize: Int64 = 8 * 1024 * 1024
    private static let cacheTTL: TimeInterval = 600
    static let maxSwitches = 3

    // MARK: - URL 识别与拆解

    /// 是否是 GitHub 公开文件直链（Release 附件、源码包、raw 文件等；不含普通网页、API、带登录或临时签名的链接）。
    static func isGitHubFile(_ raw: String) -> Bool {
        guard let url = URL(string: raw),
              let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
              url.user == nil, url.password == nil,
              var host = url.host?.lowercased() else { return false }
        // 带查询参数的直链往往含临时签名或鉴权令牌，只走原站，绝不发给第三方加速站。
        if let query = url.query, !query.isEmpty { return false }
        if host.hasPrefix("www.") { host.removeFirst(4) }
        if githubHosts.contains(host) { return true }
        guard host == "github.com" else { return false }
        let path = url.path.lowercased()
        return ["/archive/", "/raw/", "/releases/download/", "/releases/latest/download/"].contains { path.contains($0) }
    }

    /// 是否是 Hugging Face 公开模型或数据集的 resolve 文件直链（不含带私有 query 或登录态的链接）。
    static func isHuggingFaceFile(_ raw: String) -> Bool {
        guard let url = URL(string: raw),
              let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
              url.host?.lowercased() == "huggingface.co",
              url.user == nil, url.password == nil else { return false }
        if let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
           items.contains(where: { $0.name != "download" }) {
            return false
        }
        let segments = url.path.split(separator: "/").map(String.init)
        let file = segments.first == "datasets" ? Array(segments.dropFirst()) : segments
        guard file.count >= 5, file[2] == "resolve",
              file.allSatisfy({ !$0.isEmpty }),
              !["api", "spaces", "datasets"].contains(file[0]) else { return false }
        return true
    }

    static func source(of raw: String) -> Source? {
        if isGitHubFile(raw) { return .github }
        if isHuggingFaceFile(raw) { return .huggingFace }
        return nil
    }

    static func isSupported(_ raw: String) -> Bool {
        source(of: original(raw)) != nil
    }

    static func isArchive(_ raw: String) -> Bool {
        let lower = raw.lowercased()
        return lower.contains("codeload.github.com") || lower.contains("/archive/")
    }

    /// 若 `raw` 是加速站链接，拆成 `(加速站前缀, 原链接)`；否则返回 nil。
    static func split(_ raw: String) -> (mirror: String, original: String)? {
        for m in hfMirrors {
            let prefix = m + "/"
            if raw.hasPrefix(prefix) {
                let rest = String(raw.dropFirst(prefix.count))
                let orig = "https://huggingface.co/" + rest
                if isHuggingFaceFile(orig) { return (m, orig) }
            }
        }
        for m in githubMirrors {
            let prefix = m + "/"
            if raw.hasPrefix(prefix) {
                let rest = String(raw.dropFirst(prefix.count))
                if isGitHubFile(rest) { return (m, rest) }
            }
        }
        return nil
    }

    /// 返回用户看到的原始链接。
    static func original(_ raw: String) -> String {
        split(raw)?.original ?? raw
    }

    /// 发往第三方加速站的请求头：移除 Authorization 和 Cookie，避免泄露登录凭据。
    static func mirrorHeaders(_ headers: [String: String]) -> [String: String] {
        headers.filter { k, _ in
            let lower = k.lowercased()
            return lower != "authorization" && lower != "cookie"
        }
    }

    static func candidateURL(mirror: String?, original: String) -> String {
        guard let mirror else { return original }
        if hfMirrors.contains(mirror), let url = URL(string: original) {
            let query = url.query.map { "?\($0)" } ?? ""
            return mirror + url.path + query
        }
        return "\(mirror)/\(original)"
    }

    // MARK: - 测速缓存

    private actor Cache {
        private var winners: [Source: (at: Date, mirror: String?)] = [:]

        func winner(for source: Source) -> (found: Bool, mirror: String?) {
            guard let entry = winners[source], Date().timeIntervalSince(entry.at) < GitHubMirror.cacheTTL else {
                return (false, nil)
            }
            return (true, entry.mirror)
        }

        func record(_ mirror: String?, for source: Source) {
            winners[source] = (Date(), mirror)
        }
    }

    private static let cache = Cache()

    private static func makeSession(timeout: TimeInterval, direct: Bool) -> URLSession {
        let cfg = URLSessionConfiguration.ephemeral
        cfg.timeoutIntervalForRequest = timeout
        cfg.timeoutIntervalForResource = timeout
        if direct { cfg.connectionProxyDictionary = [:] }
        return URLSession(configuration: cfg)
    }

    // MARK: - 两轮测速

    /// 第一轮：发送 `Range: bytes=0-0`，只读响应头（不缓冲响应体），验证可用并返回文件总大小。
    private static func probe(url raw: String, headers: [String: String], archive: Bool, timeout: TimeInterval, direct: Bool) async -> Int64? {
        guard let url = URL(string: raw) else { return nil }
        var req = URLRequest(url: url, timeoutInterval: timeout)
        req.setValue("bytes=0-0", forHTTPHeaderField: "Range")
        for (k, v) in headers { req.setValue(v, forHTTPHeaderField: k) }
        let session = makeSession(timeout: timeout, direct: direct)
        defer { session.finishTasksAndInvalidate() }
        guard let (bytes, resp) = try? await session.bytes(for: req),
              let http = resp as? HTTPURLResponse else { return nil }
        let ctype = (http.value(forHTTPHeaderField: "Content-Type") ?? "").lowercased()
        if ctype.hasPrefix("text/html") { return nil }
        if http.statusCode == 206 {
            if let range = http.value(forHTTPHeaderField: "Content-Range"),
               let last = range.split(separator: "/").last,
               let total = Int64(last.trimmingCharacters(in: .whitespaces)), total > 0 {
                // 只确认能读出 1 字节即刻断开，绝不整文件缓冲
                var it = bytes.makeAsyncIterator()
                guard (try? await it.next()) != nil else { return nil }
                return total
            }
            return nil
        }
        if http.statusCode == 200, archive, !ctype.hasPrefix("text/") {
            return max(0, http.expectedContentLength)
        }
        return nil
    }

    /// 第二轮：流式拉取至多 512 KB 实测真实吞吐速率（字节/秒），遇到忽略 Range 的站点也会在 512 KB 处硬停。
    private static func sample(url raw: String, headers: [String: String], timeout: TimeInterval, direct: Bool) async -> Double {
        guard let url = URL(string: raw) else { return 0 }
        var req = URLRequest(url: url, timeoutInterval: timeout)
        req.setValue("bytes=0-\(sampleBytes - 1)", forHTTPHeaderField: "Range")
        for (k, v) in headers { req.setValue(v, forHTTPHeaderField: k) }
        let session = makeSession(timeout: timeout, direct: direct)
        defer { session.finishTasksAndInvalidate() }
        let start = Date()
        guard let (bytes, resp) = try? await session.bytes(for: req),
              let http = resp as? HTTPURLResponse,
              http.statusCode == 206 else { return 0 }
        var got: Int64 = 0
        do {
            for try await _ in bytes {
                got += 1
                if got >= sampleBytes || Date().timeIntervalSince(start) >= timeout { break }
            }
        } catch {}
        guard got > 0 else { return 0 }
        let elapsed = max(0.001, Date().timeIntervalSince(start))
        return Double(got) / elapsed
    }

    /// 挑出当前最快的下载地址（支持加速站与原站同场竞速）。
    /// - Parameters:
    ///   - original: 原始 GitHub / Hugging Face 链接
    ///   - headers: 请求头（发往加速站时自动剔除凭据）
    ///   - skip: 已经失败过的候选完整 URL（换站时传入）
    ///   - wantSize: 续传换站时要求文件总大小严格一致（0 表示不限制）
    /// - Returns: 选中的最终 URL（可能是加速站也可能是原站），全部不可用返回 nil。
    static func pick(original rawOriginal: String, headers: [String: String] = [:], skip: Set<String> = [], wantSize: Int64 = 0) async -> String? {
        let orig = original(rawOriginal)
        guard let src = source(of: orig) else { return nil }
        // Hugging Face 仅支持匿名公开文件，带凭据时不走第三方镜像。
        if src == .huggingFace, mirrorHeaders(headers).count != headers.count { return nil }

        let archive = src == .github && isArchive(orig)
        var candidates: [String?] = src.mirrors.map { Optional($0) } + [nil]
        candidates.removeAll { skip.contains(candidateURL(mirror: $0, original: orig)) }
        guard !candidates.isEmpty else { return nil }

        // 命中缓存的最快站：验证可用即直接复用
        if skip.isEmpty, wantSize == 0 {
            let cached = await cache.winner(for: src)
            if cached.found {
                let url = candidateURL(mirror: cached.mirror, original: orig)
                let h = cached.mirror != nil ? mirrorHeaders(headers) : headers
                if await probe(url: url, headers: h, archive: archive, timeout: src.probeTimeout, direct: cached.mirror != nil) != nil {
                    return url
                }
            }
        }

        // 第一轮：并发探测，收集最先通过的至多 finalistsLimit 个站
        var ok: [(mirror: String?, size: Int64)] = []
        await withTaskGroup(of: (String?, Int64?).self) { group in
            for m in candidates {
                let url = candidateURL(mirror: m, original: orig)
                let h = m != nil ? mirrorHeaders(headers) : headers
                let direct = m != nil
                group.addTask {
                    let size = await probe(url: url, headers: h, archive: archive, timeout: src.probeTimeout, direct: direct)
                    return (m, size)
                }
            }
            for await (m, size) in group {
                guard let size, wantSize == 0 || size == wantSize else { continue }
                ok.append((m, size))
                if ok.count >= finalistsLimit {
                    group.cancelAll()
                    break
                }
            }
        }

        guard let first = ok.first else { return nil }
        let winner: String?
        if ok.count == 1 || archive || first.size < sampleMinSize {
            winner = first.mirror
        } else {
            // 第二轮：并发测 512 KB 实际下载速度
            var best: (mirror: String?, speed: Double) = (first.mirror, -1)
            await withTaskGroup(of: (String?, Double).self) { group in
                for item in ok {
                    let url = candidateURL(mirror: item.mirror, original: orig)
                    let h = item.mirror != nil ? mirrorHeaders(headers) : headers
                    let direct = item.mirror != nil
                    group.addTask {
                        let speed = await sample(url: url, headers: h, timeout: src.sampleTimeout, direct: direct)
                        return (item.mirror, speed)
                    }
                }
                for await (m, speed) in group {
                    if speed > best.speed { best = (m, speed) }
                }
            }
            winner = best.mirror
        }

        if skip.isEmpty { await cache.record(winner, for: src) }
        return candidateURL(mirror: winner, original: orig)
    }
}
