import Foundation

/// 临时播放地址和密钥只存在于内存，不写入任务记录。
enum WeChatChannels {
    enum Failure: LocalizedError {
        case invalid, unavailable, download, format, feedData, videoHost
        case feedHTTP(Int)
        var errorDescription: String? {
            switch self {
            case .invalid: return L("视频号解析结果无效，请重新解析")
            case .unavailable: return L("视频号播放地址已失效，请重新解析")
            case .feedHTTP(let code): return String(localized: "获取视频信息失败（HTTP \(code)），请稍后重新解析")
            case .feedData: return L("未获取到视频信息，请重新解析")
            case .videoHost: return L("视频地址不在腾讯域名内，已停止下载")
            case .download: return L("视频下载失败，请重新解析后重试")
            case .format: return L("视频格式无法识别，未保存文件")
            }
        }
    }
    static func trusted(_ url: URL, exact: String? = nil) -> Bool {
        guard url.scheme?.lowercased() == "https", let host = url.host?.lowercased(), url.user == nil, url.password == nil else { return false }
        if let exact { return host == exact }
        return host == "qq.com" || host.hasSuffix(".qq.com")
    }
    static func download(playable: String, title: String, directory: URL) async throws -> URL {
        guard let page = URL(string: playable), trusted(page, exact: "channels.weixin.qq.com"),
              let components = URLComponents(url: page, resolvingAgainstBaseURL: false),
              let token = components.queryItems?.first(where: { $0.name == "token" })?.value, !token.isEmpty,
              let eid = components.queryItems?.first(where: { $0.name == "eid" })?.value, !eid.isEmpty else { throw Failure.invalid }
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 60
        let session = URLSession(configuration: config, delegate: RedirectGuard(), delegateQueue: nil)
        defer { session.invalidateAndCancel() }
        var request = URLRequest(url: URL(string: "https://channels.weixin.qq.com/finder-preview/api/feed/get_feed_info")!)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("https://channels.weixin.qq.com", forHTTPHeaderField: "Origin")
        request.setValue(playable, forHTTPHeaderField: "Referer")
        request.setValue("Mozilla/5.0", forHTTPHeaderField: "User-Agent")
        request.httpBody = try JSONSerialization.data(withJSONObject: ["baseReq": ["generalToken": token], "exportId": eid])
        let (data, response) = try await session.data(for: request)
        guard let code = (response as? HTTPURLResponse)?.statusCode, (200..<300).contains(code) else { throw Failure.feedHTTP((response as? HTTPURLResponse)?.statusCode ?? 0) }
        guard let root = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let payload = root["data"] as? [String: Any], let feed = payload["feedInfo"] as? [String: Any],
              let rawURL = [(feed["videoUrl"] as? String), ((feed["h264VideoInfo"] as? [String: Any])?["videoUrl"] as? String), (feed["originVideoUrl"] as? String)].compactMap({ $0 }).first(where: { !$0.isEmpty }) else { throw Failure.feedData }
        guard let videoURL = URL(string: rawURL), trusted(videoURL) else { throw Failure.videoHost }
        let key: UInt64?
        if let number = feed["decodeKey"] as? NSNumber { key = UInt64(number.stringValue) }
        else if let string = feed["decodeKey"] as? String { key = UInt64(string) }
        else { key = nil }
        if feed["decodeKey"] != nil && key == nil { throw Failure.invalid }
        var videoRequest = URLRequest(url: videoURL)
        videoRequest.setValue("Mozilla/5.0", forHTTPHeaderField: "User-Agent")
        videoRequest.setValue("https://channels.weixin.qq.com/", forHTTPHeaderField: "Referer")
        let (temp, videoResponse) = try await session.download(for: videoRequest)
        guard (videoResponse as? HTTPURLResponse)?.statusCode == 200 else { throw Failure.download }
        let handle = try FileHandle(forUpdating: temp)
        defer { try? handle.close() }
        var head = try handle.read(upToCount: 131_072) ?? Data()
        if let key, key != 0 { var cipher = WeChatCipher(seed: key); cipher.apply(to: &head); try handle.seek(toOffset: 0); try handle.write(contentsOf: head) }
        guard head.count > 12, String(data: head[4..<8], encoding: .ascii) == "ftyp" else { throw Failure.format }
        try Task.checkCancellation()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let safe = String(title.prefix(60)).components(separatedBy: CharacterSet(charactersIn: "/:\\").union(.newlines)).joined(separator: " ").trimmingCharacters(in: .whitespacesAndNewlines)
        let base = safe.isEmpty ? L("微信视频号") : safe
        var destination = directory.appendingPathComponent(base + ".mp4")
        var suffix = 2
        while FileManager.default.fileExists(atPath: destination.path) { destination = directory.appendingPathComponent("\(base) (\(suffix)).mp4"); suffix += 1 }
        try FileManager.default.moveItem(at: temp, to: destination)
        return destination
    }
    private final class RedirectGuard: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
        func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
            completionHandler(request.url.map { WeChatChannels.trusted($0) } == true ? request : nil)
        }
    }
}
