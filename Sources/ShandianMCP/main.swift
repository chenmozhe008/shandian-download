import Foundation

// 给 AI 工具用的 MCP 服务（stdio）：提交下载、查进度、暂停继续。只连本机：下载交给 shandian:// 走 App 的添加流程，
// 查询和暂停走本机内核接口，令牌读 App 生成的 engine.json。不删除任务、不删除文件

let enginePort = 9999
let protocolFallback = "2025-06-18"

func engineToken() -> String? {
    let url = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Application Support/ShandianDownload/engine.json")
    guard let data = try? Data(contentsOf: url),
          let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
    return json["token"] as? String
}

struct ToolFailure: Error { let message: String }

func engine(_ method: String, _ path: String) throws -> Any? {
    guard let token = engineToken() else { throw ToolFailure(message: "没有找到闪电全能下载的配置，请先打开一次应用") }
    var request = URLRequest(url: URL(string: "http://127.0.0.1:\(enginePort)/api/v1/\(path)")!, timeoutInterval: 10)
    request.httpMethod = method
    request.setValue(token, forHTTPHeaderField: "X-Api-Token")
    let box = ResultBox()
    let done = DispatchSemaphore(value: 0)
    URLSession.shared.dataTask(with: request) { data, _, error in
        box.data = data; box.failed = error != nil
        done.signal()
    }.resume()
    done.wait()
    guard !box.failed, let data = box.data else { throw ToolFailure(message: "连不上下载内核，请先打开闪电全能下载") }
    guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw ToolFailure(message: "下载内核返回了无法识别的结果") }
    if let code = json["code"] as? Int, code != 0 { throw ToolFailure(message: json["msg"] as? String ?? "下载内核报错（\(code)）") }
    return json["data"]
}

final class ResultBox: @unchecked Sendable { var data: Data?; var failed = false }

func isSupportedLink(_ link: String) -> Bool {
    let lower = link.lowercased()
    return ["magnet:?", "ed2k://", "http://", "https://"].contains { lower.hasPrefix($0) }
}

func addDownload(_ link: String) throws -> String {
    let trimmed = link.trimmingCharacters(in: .whitespacesAndNewlines)
    guard isSupportedLink(trimmed) else { throw ToolFailure(message: "只支持磁力、ed2k、http、https 链接") }
    var components = URLComponents()
    components.scheme = "shandian"; components.host = "add"
    components.queryItems = [URLQueryItem(name: "confirm", value: "0"), URLQueryItem(name: "url", value: trimmed)]
    guard let url = components.url?.absoluteString else { throw ToolFailure(message: "链接无法识别") }
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/open")
    process.arguments = ["-g", url]
    try process.run(); process.waitUntilExit()
    guard process.terminationStatus == 0 else { throw ToolFailure(message: "没能打开闪电全能下载") }
    return "已交给闪电全能下载。可以用 list_downloads 查看进度。"
}

func listDownloads(includeDone: Bool) throws -> String {
    let tasks = try engine("GET", "tasks") as? [[String: Any]] ?? []
    let rows: [[String: Any]] = tasks.compactMap { task in
        let status = task["status"] as? String ?? ""
        if status == "done", !includeDone { return nil }
        let progress = task["progress"] as? [String: Any]
        let resource = (task["meta"] as? [String: Any])?["res"] as? [String: Any]
        return ["id": task["id"] ?? "", "name": task["name"] ?? "", "status": status,
                "downloaded_bytes": progress?["downloaded"] ?? 0, "total_bytes": resource?["size"] ?? 0,
                "speed_bytes_per_second": progress?["speed"] ?? 0]
    }
    let data = try JSONSerialization.data(withJSONObject: rows, options: [.prettyPrinted, .sortedKeys])
    let list = String(data: data, encoding: .utf8) ?? "[]"
    return rows.isEmpty ? "当前没有任务。（交给迅雷、云盘和视频网站的任务不在这个列表里，请在应用中查看）" : list
}

let tools: [[String: Any]] = [
    ["name": "add_download", "description": "把磁力、ed2k 或 http(s) 链接交给闪电全能下载。视频网站和网盘直链也走这里，保存位置按应用设置。",
     "inputSchema": ["type": "object", "properties": ["url": ["type": "string", "description": "下载链接"]], "required": ["url"]]],
    ["name": "list_downloads", "description": "列出内核里的下载任务和进度。默认只看未完成的。",
     "inputSchema": ["type": "object", "properties": ["include_done": ["type": "boolean", "description": "是否包含已完成的任务"]]]],
    ["name": "pause_download", "description": "暂停一条任务。",
     "inputSchema": ["type": "object", "properties": ["id": ["type": "string", "description": "list_downloads 返回的 id"]], "required": ["id"]]],
    ["name": "resume_download", "description": "继续一条任务。",
     "inputSchema": ["type": "object", "properties": ["id": ["type": "string", "description": "list_downloads 返回的 id"]], "required": ["id"]]],
]

func taskID(_ arguments: [String: Any]) throws -> String {
    guard let id = arguments["id"] as? String, !id.isEmpty,
          id.allSatisfy({ $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" }) else { throw ToolFailure(message: "任务 id 不正确") }
    return id
}

func callTool(_ name: String, _ arguments: [String: Any]) throws -> String {
    switch name {
    case "add_download":
        guard let url = arguments["url"] as? String else { throw ToolFailure(message: "缺少 url") }
        return try addDownload(url)
    case "list_downloads":
        return try listDownloads(includeDone: arguments["include_done"] as? Bool ?? false)
    case "pause_download":
        _ = try engine("PUT", "tasks/\(try taskID(arguments))/pause")
        return "已暂停。"
    case "resume_download":
        _ = try engine("PUT", "tasks/\(try taskID(arguments))/continue")
        return "已继续。"
    default:
        throw ToolFailure(message: "没有这个工具：\(name)")
    }
}

func send(_ object: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: object) else { return }
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data([0x0A]))
}

while let line = readLine(strippingNewline: true) {
    guard let data = line.data(using: .utf8),
          let message = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let method = message["method"] as? String else { continue }
    guard let id = message["id"] else { continue } // 通知不用回
    let params = message["params"] as? [String: Any] ?? [:]
    var reply: [String: Any] = ["jsonrpc": "2.0", "id": id]
    switch method {
    case "initialize":
        reply["result"] = ["protocolVersion": params["protocolVersion"] as? String ?? protocolFallback,
                           "capabilities": ["tools": [:] as [String: Any]],
                           "serverInfo": ["name": "shandian", "version": "1.0.0"]] as [String: Any]
    case "ping":
        reply["result"] = [:] as [String: Any]
    case "tools/list":
        reply["result"] = ["tools": tools]
    case "tools/call":
        let name = params["name"] as? String ?? ""
        let arguments = params["arguments"] as? [String: Any] ?? [:]
        do {
            reply["result"] = ["content": [["type": "text", "text": try callTool(name, arguments)]]]
        } catch {
            let text = (error as? ToolFailure)?.message ?? error.localizedDescription
            reply["result"] = ["content": [["type": "text", "text": text]], "isError": true] as [String: Any]
        }
    default:
        reply["error"] = ["code": -32601, "message": "Method not found"] as [String: Any]
    }
    send(reply)
}
