import CryptoKit
import Foundation

enum FeishuMedia {
    static let setupURL = URL(string: "https://github.com/larksuite/cli")!

    static var executable: URL? {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return [home.appendingPathComponent(".npm-global/bin/lark-cli"),
                home.appendingPathComponent(".local/bin/lark-cli"),
                URL(fileURLWithPath: "/opt/homebrew/bin/lark-cli"),
                URL(fileURLWithPath: "/usr/local/bin/lark-cli")]
            .first { FileManager.default.isExecutableFile(atPath: $0.path) }
    }

    static func documentURL(_ raw: String) -> URL? {
        guard let url = URL(string: raw), url.scheme == "https", url.user == nil, url.password == nil,
              url.port == nil, let host = url.host?.lowercased(), host == "feishu.cn" || host.hasSuffix(".feishu.cn") else { return nil }
        let path = url.path.split(separator: "/")
        guard path.count == 2, ["wiki", "docx"].contains(path[0]),
              path[1].range(of: #"^[A-Za-z0-9]+$"#, options: .regularExpression) != nil else { return nil }
        var components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        components?.fragment = nil
        components?.query = nil
        return components?.url
    }

    struct Attachment: Equatable {
        let token: String
        let name: String
        let size: Int64?

        var selectionID: String {
            SHA256.hash(data: Data(token.utf8)).map { String(format: "%02x", $0) }.joined()
        }
    }

    private final class DocumentReader: NSObject, XMLParserDelegate {
        var title = ""
        var attachments: [Attachment] = []
        private var inTitle = false
        private var seen = Set<String>()

        func parser(_ parser: XMLParser, didStartElement element: String, namespaceURI: String?,
                    qualifiedName: String?, attributes: [String: String]) {
            if element == "title" { inTitle = true }
            guard element == "source", let mime = attributes["mime"]?.lowercased(),
                  mime.hasPrefix("audio/") || mime.hasPrefix("video/"),
                  let token = attributes["token"], !token.isEmpty, seen.insert(token).inserted,
                  let name = attributes["name"], !name.isEmpty else { return }
            let size = attributes["size"].flatMap(Int64.init).flatMap { $0 > 0 ? $0 : nil }
            attachments.append(Attachment(token: token, name: name, size: size))
        }

        func parser(_ parser: XMLParser, foundCharacters string: String) {
            if inTitle { title += string }
        }

        func parser(_ parser: XMLParser, didEndElement element: String, namespaceURI: String?, qualifiedName: String?) {
            if element == "title" { inTitle = false }
        }
    }

    private struct Envelope<Payload: Decodable>: Decodable {
        let ok: Bool
        let data: Payload?
    }

    private struct DocumentResponse: Decodable {
        struct Document: Decodable { let content: String }
        let document: Document
    }

    private struct MediaResponse: Decodable {
        struct Link: Decodable {
            let token: String
            let url: URL
            enum CodingKeys: String, CodingKey {
                case token = "file_token"
                case url = "tmp_download_url"
            }
        }
        let links: [Link]
        enum CodingKeys: String, CodingKey { case links = "tmp_download_urls" }
    }

    enum Failure: LocalizedError {
        case cli, noMedia, incomplete, selectionChanged

        var errorDescription: String? {
            switch self {
            case .cli: L("无法读取飞书文档，请检查 lark-cli 授权和文档权限。")
            case .noMedia: L("这篇飞书文档没有可下载的原始音视频附件。")
            case .incomplete: L("飞书未提供全部音视频的下载链接，请检查附件下载权限后重试。")
            case .selectionChanged: L("飞书附件列表已变化，请在浏览器扩展里重新扫描后再下载。")
            }
        }
    }

    static func attachments(in xml: String) throws -> (title: String, files: [Attachment]) {
        let reader = DocumentReader()
        let parser = XMLParser(data: Data("<document>\(xml)</document>".utf8))
        parser.delegate = reader
        guard parser.parse() else { throw Failure.cli }
        return (reader.title.trimmingCharacters(in: .whitespacesAndNewlines), reader.attachments)
    }

    static func list(_ url: URL, executable: URL) throws -> (title: String, files: [Attachment]) {
        let document: DocumentResponse = try run(executable, ["docs", "+fetch", "--as", "user", "--doc", url.absoluteString])
        return try attachments(in: document.document.content)
    }

    static func selectedAttachments(_ files: [Attachment], ids: Set<String>?) throws -> [Attachment] {
        guard let ids else { return files }
        let selected = files.filter { ids.contains($0.selectionID) }
        guard !ids.isEmpty, selected.count == ids.count else { throw Failure.selectionChanged }
        return selected
    }

    static func prepare(_ url: URL, executable: URL, selectedIDs: Set<String>? = nil) throws -> (name: String, entries: [ArchiveDownloads.Entry]) {
        let (title, attachments) = try list(url, executable: executable)
        let files = try selectedAttachments(attachments, ids: selectedIDs)
        guard !files.isEmpty else { throw Failure.noMedia }

        var links: [String: URL] = [:]
        // The Feishu endpoint limits the number of file tokens in one request.
        for start in stride(from: 0, to: files.count, by: 5) {
            let group = files[start..<min(start + 5, files.count)]
            let params = try JSONEncoder().encode(["file_tokens": group.map(\.token)])
            let response: MediaResponse = try run(executable, ["api", "GET", "/open-apis/drive/v1/medias/batch_get_tmp_download_url",
                                                                  "--as", "user", "--params", String(decoding: params, as: UTF8.self)])
            let tokens = Set(group.map(\.token))
            for link in response.links where tokens.contains(link.token) && trustedDownload(link.url) {
                links[link.token] = link.url
            }
        }
        guard links.count == files.count else { throw Failure.incomplete }
        let entries = files.compactMap { file in
            links[file.token].map {
                ArchiveDownloads.Entry(url: $0, name: file.name, headers: [:], expectedBytes: file.size,
                                        stableID: url.absoluteString + "\n" + file.token)
            }
        }
        return (title.isEmpty ? L("飞书文档音视频") : title, entries)
    }

    private static func trustedDownload(_ url: URL) -> Bool {
        guard url.scheme == "https", url.user == nil, url.password == nil, url.port == nil,
              let host = url.host?.lowercased() else { return false }
        return host.hasSuffix(".feishu.cn") || host.hasSuffix(".feishucdn.com")
    }

    private static func run<T: Decodable>(_ executable: URL, _ args: [String]) throws -> T {
        let process = Process()
        let output = Pipe()
        process.executableURL = executable
        process.arguments = args
        var environment = ProcessInfo.processInfo.environment
        environment["PATH"] = "/opt/homebrew/bin:/usr/local/bin:" + (environment["PATH"] ?? "/usr/bin:/bin")
        process.environment = environment
        process.standardInput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        process.standardOutput = output
        try process.run()
        let timeout = DispatchWorkItem {
            if process.isRunning { process.terminate() }
        }
        DispatchQueue.global().asyncAfter(deadline: .now() + 30, execute: timeout)
        defer { timeout.cancel() }
        let limit = 24 * 1024 * 1024
        var data = Data()
        while let chunk = try? output.fileHandleForReading.read(upToCount: 32 * 1024), !chunk.isEmpty {
            if data.count <= limit { data.append(chunk) }
        }
        process.waitUntilExit()
        guard process.terminationStatus == 0, data.count <= limit,
              let response = try? JSONDecoder().decode(Envelope<T>.self, from: data),
              response.ok, let payload = response.data else { throw Failure.cli }
        return payload
    }
}
