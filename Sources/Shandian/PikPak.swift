import CryptoKit
import Foundation

/// 云盘离线（PikPak）：BT 网络里几乎没人传的资源（国内热门片常见，都在迅雷用户之间传），
/// 交给 PikPak 的服务器下（热门的常常几秒就好，云端早有现成的），再从云盘按普通链接拉回本地。
/// 用的是用户自己的 PikPak 账号；接口照开源库 PikPakAPI（github.com/Quan666/PikPakAPI）写。
/// 只存登录后的令牌（pikpak.json，600 权限），不存密码。PikPak 在境外，请求走本机代理。
actor PikPak {
    static let shared = PikPak()
    nonisolated static let loginChanged = Notification.Name("PikPakLoginChanged")

    /// 签发 token 的客户端：安卓（账号密码登录）和网页（在 PikPak 网页里登录后取会话）。token 绑定签发它的客户端，刷新要用同一个。
    /// 网页端常量取自开源项目 peach 的 ADR-0093 与 AList（PikPak 改版网页时盐和固定时间戳可能会换）
    struct Profile {
        var clientID: String, clientSecret: String?, version: String, package: String, salts: [String]
        var signTimestamp: String?, refreshHeaders: [String: String] = [:]
    }
    static let android = Profile(clientID: "YNxT9w7GMdWvEOKa", clientSecret: "dbw2OtmVEeuUvIptb1Coyg", version: "1.47.1",
        package: "com.pikcloud.pikpak",
        salts: ["Gez0T9ijiI9WCeTsKSg3SMlx", "zQdbalsolyb1R/", "ftOjr52zt51JD68C3s", "yeOBMH0JkbQdEFNNwQ0RI9T3wU/v",
                "BRJrQZiTQ65WtMvwO", "je8fqxKPdQVJiy1DM6Bc9Nb1", "niV", "9hFCW2R1", "sHKHpe2i96",
                "p7c5E6AcXQ/IJUuAEC9W6", "", "aRv9hjc9P+Pbn+u3krN6", "BzStcgE8qVdqjEH16l4",
                "SqgeZvL5j9zoHP95xWHt", "zVof5yaJkPe3VFpadPof"])
    static let web = Profile(clientID: "YUMx5nI8ZU8Ap8pm", clientSecret: nil, version: "2.0.0", package: "mypikpak.com",
        salts: ["C9qPpZLN8ucRTaTiUMWYS9cQvWOE", "+r6CQVxjzJV6LCV", "F", "pFJRC", "9WXYIDGrwTCz2OiVlgZa90qpECPD6olt",
                "/750aCr4lm/Sly/c", "RB+DT/gZCrbV", "", "CyLsf7hdkIRxRm215hl", "7xHvLi2tOYP0Y92b",
                "ZGTXXxu8E/MIWaEDB+Sm/", "1UI3", "E7fP5Pfijd+7K+t6Tg/NhuLq0eEUVChpJSkrKxpO", "ihtqpG6FMt65+Xk+tWUH2",
                "NhXXU9rg4XXdzo7u5o"],
        signTimestamp: "1793092691562",   // 10-28 从 mypikpak.com/drive 的网页脚本取（模块里 clientId/clientVersion/timestamp/algorithms）
        refreshHeaders: ["X-Client-Id": "YUMx5nI8ZU8Ap8pm", "X-Sdk-Version": "8.1.4", "X-Protocol-Version": "301"])

    private var profile: Profile {
        guard session?.clientID == Self.web.clientID else { return Self.android }
        var p = Self.web
        // 网页改版后自动取到的新签名参数（见 refreshWebSign）
        if let ts = UserDefaults.standard.string(forKey: "pikpakWebSignTimestamp") { p.signTimestamp = ts }
        if let salts = UserDefaults.standard.stringArray(forKey: "pikpakWebSignSalts"), salts.count > 5 { p.salts = salts }
        return p
    }
    private var isWeb: Bool { session?.clientID == Self.web.clientID }

    /// 网页端签名失效时（PikPak 改版），从当前网页脚本里重新取时间戳和盐：
    /// 脚本里有一段 {clientId:"YUMx5nI8ZU8Ap8pm",clientVersion:"2.0.0",packageName:"mypikpak.com",timestamp:"…",algorithms:[{alg:"md5",salt:"…"},…]}
    private func refreshWebSign() async -> Bool {
        let cfg = URLSessionConfiguration.ephemeral
        cfg.connectionProxyDictionary = LocalProxy.sessionDictionary(for: "https://mypikpak.com")
        let session = URLSession(configuration: cfg)
        guard let (html, _) = try? await session.data(from: URL(string: "https://mypikpak.com/drive/")!),
              let page = String(data: html, encoding: .utf8) else { return false }
        let scripts = page.matches(of: /src="(\/drive\/[^"]+\.js)"/).map { String($0.1) }
        for path in scripts {
            guard let (d, _) = try? await session.data(from: URL(string: "https://mypikpak.com" + path)!),
                  let js = String(data: d, encoding: .utf8), let r = js.range(of: "clientId:\"\(Self.web.clientID)\",clientVersion") else { continue }
            let chunk = String(js[r.lowerBound...].prefix(3000))
            guard let ts = chunk.firstMatch(of: /timestamp:"(\d+)"/)?.1 else { continue }
            let salts = chunk.matches(of: /salt:"([^"]*)"/).map { String($0.1) }
            guard salts.count > 5 else { continue }
            UserDefaults.standard.set(String(ts), forKey: "pikpakWebSignTimestamp")
            UserDefaults.standard.set(salts, forKey: "pikpakWebSignSalts")
            return true
        }
        return false
    }
    private let userHost = "https://user.mypikpak.com"
    private let apiHost = "https://api-drive.mypikpak.com"

    struct Session: Codable {
        var username: String; var deviceID: String; var accessToken: String; var refreshToken: String; var userID: String
        var clientID: String?
    }
    private var session: Session?
    private var signRefreshed = false
    private static var file: URL { EngineConfig.supportDir.appendingPathComponent("pikpak.json") }

    init() {
        session = (try? Data(contentsOf: Self.file)).flatMap { try? JSONDecoder().decode(Session.self, from: $0) }
        // 早先关联时存的是网页原样的 wdi10.<…>，签名要最后一段的前 32 位
        if let d = session?.deviceID, d.contains(".") { session?.deviceID = String((d.split(separator: ".").last.map(String.init) ?? d).prefix(32)) }
    }

    /// 已登录的账号名（界面显示用）
    nonisolated static var loggedInUser: String? {
        (try? Data(contentsOf: file)).flatMap { try? JSONDecoder().decode(Session.self, from: $0) }?.username
    }

    enum PikPakError: LocalizedError {
        case server(String), notLoggedIn
        var errorDescription: String? {
            switch self { case .server(let m): m; case .notLoggedIn: "还没有登录 PikPak" }
        }
    }

    // MARK: 登录

    func login(username: String, password: String) async throws {
        let device = UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
        var meta: [String: Any] = [:]
        if username.contains("@") { meta["email"] = username }
        else if username.allSatisfy(\.isNumber), username.count >= 11 { meta["phone_number"] = username }
        else { meta["username"] = username }
        let signin = "\(userHost)/v1/auth/signin"
        session = nil
        let captcha = try await captchaInit(action: "POST:\(signin)", meta: meta, device: device)
        let r = try await send("POST", signin, body: ["client_id": Self.android.clientID, "client_secret": Self.android.clientSecret ?? "",
                                                      "username": username, "password": password, "captcha_token": captcha],
                               device: device, auth: false)
        guard let at = r["access_token"] as? String, let rt = r["refresh_token"] as? String else {
            throw PikPakError.server(Self.message(r) ?? "登录失败")
        }
        session = Session(username: username, deviceID: device, accessToken: at, refreshToken: rt, userID: r["sub"] as? String ?? "",
                          clientID: Self.android.clientID)
        save()
    }

    /// 网页登录：在 PikPak 网页里登录后取到的会话（见 PikPakLoginView）。之后只由闪电全能下载刷新
    func adoptWebSession(clientID: String, deviceID: String, accessToken: String, refreshToken: String, userID: String, name: String) throws {
        // 网页存的 deviceid 形如 wdi10.<…>，网页签名和请求头用的是最后一段的前 32 位
        let device = String((deviceID.split(separator: ".").last.map(String.init) ?? deviceID).prefix(32))
        let newSession = Session(username: name, deviceID: device, accessToken: accessToken, refreshToken: refreshToken,
                                 userID: userID, clientID: clientID)
        let data = try JSONEncoder().encode(newSession)
        try PrivateFile.write(data, to: Self.file)
        session = newSession
        UserDefaults.standard.removeObject(forKey: "pikpakSessionProblem")
    }

    func logout() {
        session = nil
        UserDefaults.standard.removeObject(forKey: "pikpakSessionProblem")
        try? FileManager.default.removeItem(at: Self.file)
    }

    private func save() {
        guard let s = session, let d = try? JSONEncoder().encode(s) else { return }
        try? PrivateFile.write(d, to: Self.file)
    }

    private func refresh() async throws {
        // 文件里的令牌比内存新（别的进程刷新过），直接用，不拿旧令牌去撞
        if let saved = (try? Data(contentsOf: Self.file)).flatMap({ try? JSONDecoder().decode(Session.self, from: $0) }),
           saved.userID == session?.userID, saved.refreshToken != session?.refreshToken {
            session?.accessToken = saved.accessToken; session?.refreshToken = saved.refreshToken
            return
        }
        guard var s = session else { throw PikPakError.notLoggedIn }
        let r = try await send("POST", "\(userHost)/v1/auth/token",
                               body: ["client_id": profile.clientID, "refresh_token": s.refreshToken, "grant_type": "refresh_token"],
                               device: s.deviceID, auth: false, extraHeaders: profile.refreshHeaders)
        guard let at = r["access_token"] as? String, let rt = r["refresh_token"] as? String else {
            if session?.refreshToken == s.refreshToken {
                UserDefaults.standard.set(true, forKey: "pikpakSessionProblem")
                DispatchQueue.main.async { NotificationCenter.default.post(name: Self.loginChanged, object: nil) }
            }
            // 令牌只能用一次：从浏览器关联时浏览器也拿着同一个，页面开着就可能先刷新掉（11-04 实测 invalid_grant）
            throw PikPakError.server("PikPak 登录已失效，请重新关联")
        }
        guard session?.refreshToken == s.refreshToken else { return }
        UserDefaults.standard.removeObject(forKey: "pikpakSessionProblem")
        s.accessToken = at; s.refreshToken = rt
        session = s; save()
    }

    // MARK: 离线任务

    struct Task { var id: String; var fileID: String; var phase: String; var progress: Int; var message: String? }

    /// 提交离线下载，返回任务（磁力、种子网址都行）
    func offline(_ link: String) async throws -> Task {
        let r = try await api("POST", "\(apiHost)/drive/v1/files",
                              body: ["kind": "drive#file", "upload_type": "UPLOAD_TYPE_URL", "url": ["url": link], "folder_type": "DOWNLOAD"])
        guard let t = r["task"] as? [String: Any] else { throw PikPakError.server(Self.message(r) ?? "云盘没有接受这个任务") }
        return Self.task(t)
    }

    /// 查任务进度；任务列表里找不到但文件在，就是下完了
    func status(_ taskID: String, fileID: String) async throws -> Task {
        let filters = #"{"phase":{"in":"PHASE_TYPE_RUNNING,PHASE_TYPE_ERROR,PHASE_TYPE_PENDING,PHASE_TYPE_COMPLETE"}}"#
        let q = "type=offline&limit=200&filters=" + (filters.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? "")
        let r = try await api("GET", "\(apiHost)/drive/v1/tasks?\(q)", body: nil)
        if let t = (r["tasks"] as? [[String: Any]])?.first(where: { $0["id"] as? String == taskID }) { return Self.task(t) }
        return Task(id: taskID, fileID: fileID, phase: "PHASE_TYPE_COMPLETE", progress: 100, message: nil)
    }

    struct RemoteFile { var name: String; var size: Int64; var url: String }

    nonisolated static func downloadURL(_ response: [String: Any]) throws -> String {
        let media = ((response["medias"] as? [[String: Any]])?.first?["link"] as? [String: Any])?["url"] as? String
        for candidate in [response["web_content_link"] as? String, media] {
            if let candidate, !candidate.isEmpty { return candidate }
        }
        throw PikPakError.server("云端文件缺少下载地址，已保留云端文件，请稍后重试")
    }

    /// 下好的文件和下载地址：文件夹就展开（只要视频和字幕，同电影里「只下正片」的规则）
    func files(of fileID: String) async throws -> (files: [RemoteFile], includesAll: Bool) {
        let info = try await api("GET", "\(apiHost)/drive/v1/files/\(fileID)", body: nil)
        var ids: [(String, String, Int64)] = []
        let isFolder = info["kind"] as? String == "drive#folder"
        if isFolder {
            ids = try await list(fileID)
        } else {
            ids = [(fileID, info["name"] as? String ?? "video", Int64(info["size"] as? String ?? "") ?? 0)]
        }
        let entries = ids.map { EngineTask.Meta.File(name: $0.1, path: nil, size: $0.2) }
        let keep = FileSelection.pick(entries) ?? Array(ids.indices)
        // 每个文件要一次验证码 + 一次查询，串行时 10 个文件要等约 1 分钟（11-04 实测），一起查
        let out = try await withThrowingTaskGroup(of: (Int, RemoteFile).self) { group in
            for (order, i) in keep.enumerated() {
                let (id, name, size) = ids[i]
                group.addTask { [self] in
                    let url = try await downloadURL(fileID: id)
                    return (order, RemoteFile(name: name, size: size, url: url))
                }
            }
            var found: [(Int, RemoteFile)] = []
            for try await item in group { found.append(item) }
            return found.sorted { $0.0 < $1.0 }.map(\.1)
        }
        // 文件夹列表目前没有验证分页完整性，不能据此判断整个目录都已拉回。
        return (out, info["kind"] as? String == "drive#file" && ids.count == 1 && keep == [0])
    }

    private func downloadURL(fileID: String) async throws -> String {
        let response = try await api("GET", "\(apiHost)/drive/v1/files/\(fileID)", body: nil, captchaAction: "GET:/drive/v1/files/\(fileID)")
        return try Self.downloadURL(response)
    }

    func trash(_ fileID: String) async throws {
        _ = try await api("POST", "\(apiHost)/drive/v1/files:batchTrash", body: ["ids": [fileID]])
    }

    func deleteForever(_ fileID: String) async throws {
        _ = try await api("POST", "\(apiHost)/drive/v1/files:batchDelete", body: ["ids": [fileID]])
    }

    struct Quota: Equatable { var limit: Int64; var used: Int64; var inTrash: Int64 }

    /// 云盘容量。回收站也算在已用里（11-04 实测：used 4.84 GB 全部是 inTrash，正常目录是空的，新任务照样报空间不足）
    func quota() async throws -> Quota {
        let r = try await api("GET", "\(apiHost)/drive/v1/about", body: nil)
        let q = r["quota"] as? [String: Any]
        guard let limit = Int64(q?["limit"] as? String ?? ""), let used = Int64(q?["usage"] as? String ?? "") else {
            throw PikPakError.server("无法读取云盘容量")
        }
        return Quota(limit: limit, used: used, inTrash: Int64(q?["usage_in_trash"] as? String ?? "") ?? 0)
    }

    func usedBytes() async throws -> Int64 { try await quota().used }

    /// 需要的空间不够、或大小还不知道（磁力没解析完）时才清回收站
    /// 登录已失效：自动转交时跳过云盘，别每次都白失败一轮
    nonisolated static var sessionInvalid: Bool { UserDefaults.standard.bool(forKey: "pikpakSessionProblem") }
    nonisolated static let relinkRequested = Notification.Name("PikPakRelinkRequested")

    nonisolated static func shouldEmptyTrash(_ quota: Quota, need: Int64) -> Bool {
        quota.inTrash > 0 && (need <= 0 || quota.limit - quota.used < need)
    }

    /// 回收站里哪些是离线任务留下的。用户自己扔进去的（比如 PikPak 自带的教程视频）对不上任务记录，不动
    nonisolated static func offlineLeftovers(trash: [String], taskFiles: Set<String>) -> [String] {
        trash.filter { taskFiles.contains($0) }
    }

    /// PikPak 只是中转：把回收站里离线任务留下的东西彻底删除，返回删掉的条数
    @discardableResult
    func emptyOfflineTrash() async throws -> Int {
        let trashed = #"{"trashed":{"eq":true}}"#.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? ""
        let listed = try await api("GET", "\(apiHost)/drive/v1/files?parent_id=*&limit=500&filters=\(trashed)", body: nil)
        let trash = try Self.folderEntries(listed).map(\.id)
        guard !trash.isEmpty else { return 0 }
        let phases = #"{"phase":{"in":"PHASE_TYPE_ERROR,PHASE_TYPE_COMPLETE"}}"#.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? ""
        let tasks = try await api("GET", "\(apiHost)/drive/v1/tasks?type=offline&limit=200&filters=\(phases)", body: nil)
        let files = Set(((tasks["tasks"] as? [[String: Any]]) ?? []).compactMap { $0["file_id"] as? String })
        let ids = Self.offlineLeftovers(trash: trash, taskFiles: files)
        guard !ids.isEmpty else { return 0 }
        _ = try await api("POST", "\(apiHost)/drive/v1/files:batchDelete", body: ["ids": ids])
        return ids.count
    }

    /// 免费账号每天大约几次离线。接口不给这个数（11-04 查过），是经验值，PikPak 改政策时只改这里
    nonisolated static let freeDailyOfflines = 3

    /// 今天真正用掉的离线次数：空间不足等被拒的不算（11-04 实测被拒 6 次后仍能成功离线），
    /// 下完后云端文件被删的任务会变成 ERROR「云盘文件已删除」，要算
    nonisolated static func offlinesToday(_ tasks: [(created: Date, phase: String, message: String)], now: Date = Date(), calendar: Calendar = .current) -> Int {
        tasks.filter { task in
            calendar.isDate(task.created, inSameDayAs: now) && (task.phase != "PHASE_TYPE_ERROR" || task.message.contains("已删除"))
        }.count
    }

    /// 今天用了几次（含网页端提交的）
    func offlinesUsedToday() async throws -> Int {
        let phases = #"{"phase":{"in":"PHASE_TYPE_RUNNING,PHASE_TYPE_ERROR,PHASE_TYPE_PENDING,PHASE_TYPE_COMPLETE"}}"#.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? ""
        let r = try await api("GET", "\(apiHost)/drive/v1/tasks?type=offline&limit=200&filters=\(phases)", body: nil)
        let tasks = ((r["tasks"] as? [[String: Any]]) ?? []).compactMap { t -> (created: Date, phase: String, message: String)? in
            guard let created = ISO8601DateFormatter.flexible.date(from: t["created_time"] as? String ?? "") else { return nil }
            return (created, t["phase"] as? String ?? "", t["message"] as? String ?? "")
        }
        return Self.offlinesToday(tasks)
    }

    /// 提交离线前先腾地方。容量读不到时照常提交，由任务报错后的重试兜底
    func makeRoom(need: Int64) async {
        guard let quota = try? await quota(), Self.shouldEmptyTrash(quota, need: need) else { return }
        _ = try? await emptyOfflineTrash()
    }

    /// 清理前再核对一次：拉回的每个文件，大小都能在这个云端条目里对上。
    /// 已在回收站说明上一次清理只完成了一半，继续删就行
    func holds(_ fileID: String, sizes: [Int64]) async throws -> Bool {
        let info = try await api("GET", "\(apiHost)/drive/v1/files/\(fileID)", body: nil)
        if info["trashed"] as? Bool == true { return true }
        let cloud = info["kind"] as? String == "drive#folder"
            ? try await list(fileID).map(\.2)
            : [Int64(info["size"] as? String ?? "") ?? 0]
        return Self.cloudHolds(pulled: sizes, cloud: cloud)
    }

    nonisolated static func cloudHolds(pulled: [Int64], cloud: [Int64]) -> Bool {
        guard !pulled.isEmpty, pulled.allSatisfy({ $0 > 0 }) else { return false }
        var remaining = cloud
        for size in pulled {
            guard let index = remaining.firstIndex(of: size) else { return false }
            remaining.remove(at: index)
        }
        return true
    }

    enum Reclaimed: Equatable {
        case freed(Int64)   // 容量确实降了
        case unconfirmed    // 已彻底删除，容量还没变或读不到，可能稍后更新
    }

    /// 容量至少降了文件大小的一半才算释放（容量统计取整、同时可能有别的任务在写）
    nonisolated static func spaceFreed(before: Int64, after: Int64, size: Int64) -> Bool {
        before - after >= size / 2
    }

    /// 彻底删除：回收站也占容量，只移进去腾不出地方（11-04 实测）
    func reclaim(_ fileID: String, size: Int64) async throws -> Reclaimed {
        let before = try? await usedBytes()
        try await trash(fileID)
        try await deleteForever(fileID)
        if let before, let freed = await waitForFreed(before: before, size: size) { return .freed(freed) }
        return .unconfirmed
    }

    private func waitForFreed(before: Int64, size: Int64) async -> Int64? {
        for _ in 0..<3 {
            try? await _Concurrency.Task.sleep(nanoseconds: 2_000_000_000)
            if let after = try? await usedBytes(), Self.spaceFreed(before: before, after: after, size: size) { return before - after }
        }
        return nil
    }

    struct FolderEntry {
        var id: String; var name: String; var isFolder: Bool; var size: Int64
    }

    nonisolated static func folderEntries(_ response: [String: Any]) throws -> [FolderEntry] {
        guard let files = response["files"] as? [[String: Any]] else {
            throw PikPakError.server("无法确认云盘文件列表，请稍后重试；云端文件已保留")
        }
        if files.count >= 500 || !(response["next_page_token"] as? String ?? "").isEmpty {
            throw PikPakError.server("云盘目录文件过多，暂无法确认完整列表；云端文件已保留")
        }
        return try files.map { file in
            guard let id = file["id"] as? String, !id.isEmpty,
                  let name = file["name"] as? String, !name.isEmpty,
                  let kind = file["kind"] as? String, kind == "drive#file" || kind == "drive#folder" else {
                throw PikPakError.server("云盘文件信息不完整，请稍后重试；云端文件已保留")
            }
            return FolderEntry(id: id, name: name, isFolder: kind == "drive#folder", size: Int64(file["size"] as? String ?? "") ?? 0)
        }
    }

    private func list(_ parent: String, prefix: String = "") async throws -> [(String, String, Int64)] {
        let filters = #"{"trashed":{"eq":false}}"#.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? ""
        let r = try await api("GET", "\(apiHost)/drive/v1/files?parent_id=\(parent)&limit=500&filters=\(filters)", body: nil)
        var out: [(String, String, Int64)] = []
        for file in try Self.folderEntries(r) {
            if file.isFolder { out += try await list(file.id, prefix: prefix + file.name + "/") }
            else { out.append((file.id, file.name, file.size)) }
        }
        return out
    }

    private static func task(_ t: [String: Any]) -> Task {
        Task(id: t["id"] as? String ?? "", fileID: t["file_id"] as? String ?? "", phase: t["phase"] as? String ?? "",
             progress: t["progress"] as? Int ?? Int(t["progress"] as? String ?? "") ?? 0, message: t["message"] as? String)
    }

    // MARK: 请求

    private func api(_ method: String, _ url: String, body: [String: Any]?, captchaAction: String? = nil, retried: Bool = false) async throws -> [String: Any] {
        guard let s = session else { throw PikPakError.notLoggedIn }
        var captcha: String?
        // 网页端每个云盘请求都要验证码令牌（action 是「方法:路径」，不带域名和参数）
        let action = captchaAction ?? (isWeb ? "\(method):" + (URL(string: url)?.path ?? "") : nil)
        if let a = action { captcha = try await captchaInit(action: a, meta: nil, device: s.deviceID) }
        let r = try await send(method, url, body: body, device: s.deviceID, auth: true, captcha: captcha)
        if (r["error_code"] as? Int) == 16, !retried {   // 令牌过期
            try await refresh()
            return try await api(method, url, body: body, captchaAction: captchaAction, retried: true)
        }
        if let e = r["error"] as? String, !e.isEmpty { throw PikPakError.server(Self.message(r) ?? e) }
        return r
    }

    private func captchaInit(action: String, meta: [String: Any]?, device: String) async throws -> String {
        var m = meta ?? [:]
        let p = session == nil ? Self.android : profile
        if meta == nil {
            let ts = p.signTimestamp ?? String(Int(Date().timeIntervalSince1970 * 1000))
            var sign = p.clientID + p.version + p.package + device + ts
            for salt in p.salts { sign = Insecure.MD5.hash(data: Data((sign + salt).utf8)).map { String(format: "%02x", $0) }.joined() }
            m = ["captcha_sign": "1." + sign, "client_version": p.version, "package_name": p.package,
                 "user_id": session?.userID ?? "", "timestamp": ts]
        }
        let r = try await send("POST", "\(userHost)/v1/shield/captcha/init",
                               body: ["client_id": p.clientID, "action": action, "device_id": device, "meta": m], device: device, auth: false)
        guard let t = r["captcha_token"] as? String, !t.isEmpty else {
            // 签名被拒多半是网页改版换了参数：自动重取一次再试
            if meta == nil, isWeb, (Self.message(r) ?? "").contains("captcha_sign"), !signRefreshed, await refreshWebSign() {
                signRefreshed = true
                return try await captchaInit(action: action, meta: nil, device: device)
            }
            throw PikPakError.server(Self.message(r) ?? "PikPak 要求人机验证，请先在 PikPak 官网或 App 里登录一次再试")
        }
        signRefreshed = false
        return t
    }

    private func send(_ method: String, _ url: String, body: [String: Any]?, device: String, auth: Bool, captcha: String? = nil,
                      extraHeaders: [String: String] = [:]) async throws -> [String: Any] {
        var req = URLRequest(url: URL(string: url)!, timeoutInterval: 30)
        req.httpMethod = method
        req.setValue("application/json; charset=utf-8", forHTTPHeaderField: "Content-Type")
        req.setValue("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36", forHTTPHeaderField: "User-Agent")
        req.setValue(device, forHTTPHeaderField: "X-Device-Id")
        if auth, let t = session?.accessToken { req.setValue("Bearer \(t)", forHTTPHeaderField: "Authorization") }
        if let captcha { req.setValue(captcha, forHTTPHeaderField: "X-Captcha-Token") }
        if auth, session?.clientID == Self.web.clientID { req.setValue(Self.web.clientID, forHTTPHeaderField: "X-Client-Id") }
        for (k, v) in extraHeaders { req.setValue(v, forHTTPHeaderField: k) }
        if let body { req.httpBody = try JSONSerialization.data(withJSONObject: body) }
        let cfg = URLSessionConfiguration.ephemeral
        cfg.connectionProxyDictionary = LocalProxy.sessionDictionary(for: url)   // 境外服务，走本机代理
        let request = req
        let (data, response) = try await Self.withConnectionRetry { try await URLSession(configuration: cfg).data(for: request) }
        if data.isEmpty, let http = response as? HTTPURLResponse, (200...299).contains(http.statusCode) { return [:] }
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw PikPakError.server("PikPak 返回无法识别的结果，请稍后重试")
        }
        if let http = response as? HTTPURLResponse, !(200...299).contains(http.statusCode), json["error"] == nil {
            throw PikPakError.server(Self.message(json) ?? "PikPak 请求失败（HTTP \(http.statusCode)）")
        }
        return json
    }

    /// 代理节点偶尔握手失败（11-04 实测同一接口时好时坏）。只重试请求发出之前的连接错误，不会重复提交离线任务
    nonisolated static func isRetryableConnectionError(_ error: Error) -> Bool {
        guard let error = error as? URLError else { return false }
        return [.secureConnectionFailed, .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed].contains(error.code)
    }

    private static func withConnectionRetry<T: Sendable>(_ work: @Sendable () async throws -> T) async throws -> T {
        for delay: UInt64 in [1, 3, 6] {
            do { return try await work() }
            catch where isRetryableConnectionError(error) { try await _Concurrency.Task.sleep(nanoseconds: delay * 1_000_000_000) }
        }
        return try await work()
    }

    private static func message(_ r: [String: Any]) -> String? {
        (r["error_description"] as? String) ?? (r["message"] as? String) ?? (r["error"] as? String)
    }
}
