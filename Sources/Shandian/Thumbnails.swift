import AppKit
import Foundation

/// 缩略图缓存在 Application Support/ShandianDownload/thumbs/<任务 id>.jpg。
/// 电影：用 ffmpeg 从视频里截一帧（下到一部分也能截，失败过一会再试）；视频网站：下载站点给的封面。
@MainActor
final class Thumbnails: ObservableObject {
    @Published private(set) var images: [String: NSImage] = [:]
    /// 豆瓣搜索返回的一条
struct PosterHit: Decodable { var id: String?; var img: String?; var type: String?; var year: String?; var title: String?; var sub_title: String? }

/// 电影海报（竖版），和截帧分开存：卡片上海报要整张显示，截帧铺满
    @Published private(set) var posters: [String: NSImage] = [:]
    private var posterTried = Set<String>()
    private var attempts: [String: (count: Int, last: Date)] = [:]
    private var inFlight = Set<String>()
    private(set) var remoteFailed = Set<String>()   // 站点封面取不到的，改从视频里截帧
    private let dir = EngineConfig.supportDir.appendingPathComponent("thumbs")
    static let ffmpeg = ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"].first { FileManager.default.isExecutableFile(atPath: $0) }

    init() {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        // 旧版本只按年份挑海报，不同影片可能配到同一张。没有匹配记录的旧海报一律丢掉重查。
        let files = (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)) ?? []
        for f in files where f.lastPathComponent.hasSuffix(".poster.jpg") {
            if !FileManager.default.fileExists(atPath: Self.metaURL(id: String(f.lastPathComponent.dropLast(11)), dir: dir).path) {
                try? FileManager.default.removeItem(at: f)
            }
        }
    }

    nonisolated private static func metaURL(id: String, dir: URL) -> URL {
        dir.appendingPathComponent("\(id).poster.json")
    }

    func image(for id: String) -> NSImage? {
        if let img = images[id] { return img }
        let file = dir.appendingPathComponent("\(id).jpg")
        if let img = NSImage(contentsOf: file) { images[id] = img; return img }
        return nil
    }

    /// 从本地视频文件截帧。progress 太低时文件头可能还没下到，跳过；失败最多重试 6 次、间隔 30 秒
    func requestFrame(id: String, video: URL, progress: Double) {
        guard images[id] == nil, image(for: id) == nil, !inFlight.contains(id), progress > 0.08 || progress >= 1 else { return }
        // BT 乱序下载，没下完的文件常截不出画面；下载中最多试 6 次，下完后另外再给 2 次
        let key = progress >= 1 ? id + "#done" : id
        if let a = attempts[key], a.count >= (progress >= 1 ? 2 : 6) || Date().timeIntervalSince(a.last) < 30 { return }
        guard let ffmpeg = Self.ffmpeg, FileManager.default.fileExists(atPath: video.path) else { return }
        inFlight.insert(id)
        attempts[key] = ((attempts[key]?.count ?? 0) + 1, Date())
        let out = dir.appendingPathComponent("\(id).jpg")
        Task.detached {
            // 片头常是黑场或片商标，取第 3 分钟左右；短片取第 10 秒
            var ok = false
            for ss in ["180", "10"] {
                let p = Process()
                p.executableURL = URL(fileURLWithPath: ffmpeg)
                p.arguments = ["-v", "error", "-y", "-ss", ss, "-i", video.path, "-frames:v", "1", "-vf", "scale=480:-2", "-q:v", "4", out.path]
                p.standardError = FileHandle.nullDevice
                try? p.run(); p.waitUntilExit()
                if p.terminationStatus == 0, let s = try? FileManager.default.attributesOfItem(atPath: out.path)[.size] as? Int, s > 2000 { ok = true; break }
            }
            let img = ok ? NSImage(contentsOf: out) : nil
            await MainActor.run {
                self.inFlight.remove(id)
                if let img { self.images[id] = img }
            }
        }
    }

    func poster(for id: String) -> NSImage? {
        if let img = posters[id] { return img }
        if let img = NSImage(contentsOf: dir.appendingPathComponent("\(id).poster.jpg")) { posters[id] = img; return img }
        return nil
    }

    /// 电影海报：用片名查豆瓣，取中号海报。每次启动每部只查一次。
    /// 片名必须对得上（标题里带年份时年份也要对上）—— 只按年份挑会给不同的电影配同一张海报
    func requestPoster(id: String, title: String) {
        // 片名变了（用户改名、或者以前识别错）：旧海报作废，重新查
        let metaOut = Self.metaURL(id: id, dir: dir)
        if let old = try? String(contentsOf: metaOut, encoding: .utf8), old != title {
            try? FileManager.default.removeItem(at: dir.appendingPathComponent("\(id).poster.jpg"))
            posters[id] = nil
        }
        guard poster(for: id) == nil, !posterTried.contains(id) else { return }
        posterTried.insert(id)
        let year = title.range(of: #"(19|20)\d{2}"#, options: .regularExpression).map { String(title[$0]) }
        let query = title.replacingOccurrences(of: #"\s*(19|20)\d{2}\s*"#, with: " ", options: .regularExpression)
            .trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return }
        // 完整片名（中文名 + 英文名）豆瓣常常搜不到，搜不到就再拿纯中文名试一次
        var queries = [query]
        if let cjk = Self.cjkPart(query), cjk != query, cjk.count >= 2 { queries.append(cjk) }
        let out = dir.appendingPathComponent("\(id).poster.jpg")
        Task.detached {
            for q in queries {
                guard let hits = await Self.search(q), let hit = Self.pick(hits, query: q, year: year) else { continue }
                guard let src = hit.img?.replacingOccurrences(of: "s_ratio_poster", with: "m_ratio_poster"),
                      let imgURL = URL(string: src) else { continue }
                var ir = URLRequest(url: imgURL, timeoutInterval: 15)
                ir.setValue("https://movie.douban.com/", forHTTPHeaderField: "Referer")   // 豆瓣图片要带来源页
                guard let (img, resp) = try? await URLSession.shared.data(for: ir),
                      (resp as? HTTPURLResponse)?.statusCode == 200, let image = NSImage(data: img) else { continue }
                try? img.write(to: out)
                // 记下这张海报对的是哪个片名：下次片名变了要重查，旧版本按年份错配的海报也不会再被当成有效
                try? title.write(to: metaOut, atomically: true, encoding: .utf8)
                await MainActor.run { self.posters[id] = image }
                return
            }
        }
    }

    nonisolated private static func search(_ query: String) async -> [PosterHit]? {
        guard var c = URLComponents(string: "https://movie.douban.com/j/subject_suggest") else { return nil }
        c.queryItems = [URLQueryItem(name: "q", value: query)]
        guard let api = c.url else { return nil }
        var req = URLRequest(url: api, timeoutInterval: 10)
        req.setValue("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36", forHTTPHeaderField: "User-Agent")
        req.setValue("https://movie.douban.com/", forHTTPHeaderField: "Referer")
        guard let (data, _) = try? await URLSession.shared.data(for: req) else { return nil }
        return try? JSONDecoder().decode([PosterHit].self, from: data)
    }

    /// 宁缺毋滥：片名要对得上（查询串里含这个片名，或者片名含查询串），标题带年份就要求年份也对得上；
    /// 豆瓣的默认占位图和同名多部都跳过。以前只按年份挑，可能将不同影片配到同一张占位海报
    nonisolated private static func pick(_ hits: [PosterHit], query: String, year: String?) -> PosterHit? {
        let q = key(query)
        func names(_ h: PosterHit) -> [String] { [h.title, h.sub_title].compactMap { $0 } }
        func sameName(_ h: PosterHit) -> Bool {
            names(h).contains { name in
                let n = key(name)
                guard n.count >= 2, q.count >= 2 else { return false }
                return q.contains(n) || n == q || (n.count >= 4 && n.contains(q))
            }
        }
        let usable = hits.filter {
            $0.img != nil && !($0.img ?? "").contains("default") && ($0.type == "movie" || $0.type == "tv")
        }
        let named = usable.filter(sameName)
        // 有年份：年份对上的优先，年份信息缺失的只接受片名完全一致的；没年份：同名的不止一部就不猜
        if let year {
            return named.first { $0.year == year }
                ?? named.first { h in (h.year ?? "").isEmpty && names(h).contains { key($0) == q } }
        }
        return named.count == 1 ? named[0] : nil
    }

    /// 从名称中提取中文部分
    nonisolated private static func cjkPart(_ s: String) -> String? {
        let cjk = s.unicodeScalars.filter { (0x4E00...0x9FFF).contains($0.value) || (0x3400...0x4DBF).contains($0.value) }
        guard !cjk.isEmpty else { return nil }
        return String(String.UnicodeScalarView(cjk))
    }

    /// 片名归一化：去掉空格、标点、全角符号，只留字母数字和汉字，用来判断豆瓣返回的是不是同一部片子
    nonisolated private static func key(_ s: String) -> String {
        String(String.UnicodeScalarView(s.lowercased().unicodeScalars.filter { CharacterSet.alphanumerics.contains($0) }))
    }

    func requestRemote(id: String, url: String?) {
        // 有的站（小红书）给的封面是 http://，系统默认拦不加密的地址，换成 https
        guard let raw = url, let u = URL(string: raw.hasPrefix("http://") ? "https://" + raw.dropFirst(7) : raw),
              images[id] == nil, image(for: id) == nil, !inFlight.contains(id), !remoteFailed.contains(id) else { return }
        inFlight.insert(id)
        let out = dir.appendingPathComponent("\(id).jpg")
        Task.detached {
            var img: NSImage?
            if let (data, _) = try? await URLSession.shared.data(from: u), let src = NSImage(data: data),
               let tiff = src.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
               let jpg = rep.representation(using: .jpeg, properties: [.compressionFactor: 0.8]) {
                try? jpg.write(to: out); img = src
            }
            await MainActor.run {
                self.inFlight.remove(id)
                if let img { self.images[id] = img } else { self.remoteFailed.insert(id) }
            }
        }
    }
}
