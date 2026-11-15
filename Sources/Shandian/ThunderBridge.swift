import AppKit
@preconcurrency import ApplicationServices
import SQLite3

/// 迅雷通道：国内热门资源常在迅雷用户之间传，BT 网络里几乎没人（10-28 实测同一部片 BT 0、迅雷不开会员 3–5 MB/s）。
/// 慢了就交给本机装着的迅雷下。不碰迅雷的协议和会员：只像用户一样用迅雷打开链接、点「立即下载」（辅助功能），
/// 再看它写进硬盘的文件算进度，任务照样显示在闪电全能下载里。
struct ThunderJob: Codable, Identifiable, Equatable {
    var id: String
    var title: String
    var link: String
    var dir: String              // 交给迅雷时的保存位置
    var folder: String?          // 迅雷建的任务文件夹（找到后记下）
    var hash: String?            // 磁力的 infohash（大写），迅雷有时在文件夹里放 .<hash>.js，靠它认
    var name: String?            // 种子里的名字（迅雷建的文件夹通常就叫这个）
    var total: Int64
    var createdAt: Date
    var sourceEngineID: String? = nil
    var sourcePending: DownloadStore.PendingRecord? = nil
    var done = false
    var bytes: Int64 = 0
    var speed: Int64 = 0
    var measuredAt: Date?
    /// 迅雷写在任务文件夹里 `.<infohash>.js` 的进度：已完成字节 / 任务总字节。
    /// 文件实际占用按 4K 对齐，4 GB 的片子会比真实大小多出几 MB，靠它判「满」永远差一点（11-01 实测），
    /// 完成后迅雷也照旧留着这个文件，所以「.js 没了才算完」也不成立。这两个数是迅雷自己记的真值
    var checked: Int64?
    var dlSize: Int64?
    /// 下载中的已写入字节：迅雷 5.80 起边下边写在 `.<文件名>.js` 的 rqWrited 里。只用来显示进度，不作为完成依据
    var written: Int64? = nil
    /// 文件夹里最后一个字节都没变化的时间：判「老版本迅雷下完删了 .js」用，刚建文件夹时不至于立刻算完成
    var lastChangeAt: Date?
    var verifiedTotal: Int64? = nil
    var completionVerified: Bool? = nil
    var legacyCompletionPreserved: Bool? = nil
    var homeMoveTried: Bool? = nil   // 下完后试过移回保存目录（失败也不反复试）
    /// 迅雷里这条任务被删掉的时间。删任务时文件可能一起删、也可能留着，光看文件夹判断不了，
    /// 所以直接读迅雷自己的任务账本（见 `ThunderBridge.liveness`）。有值就说明它不会再下了
    var goneAt: Date? = nil
    /// 交给迅雷那一次的结果。`unconfirmed` / `needsAccessibility` 说明我们没看到迅雷真的建了任务，
    /// 隔两分钟账本里还是没有它，就是用户在迅雷的新建任务窗口里点了取消（11-15 用户反馈：取消了还一直显示「请在迅雷查看」）
    var submitState: SubmitState? = nil
    /// 迅雷里从来没建成这条任务（用户取消了）。和「被删掉」分开：提示和重试的说法不一样
    var neverStarted: Bool? = nil

    enum SubmitState: String, Codable { case submitted, unconfirmed, needsAccessibility }
}

@MainActor
enum ThunderBridge {
    /// 迅雷是第三方软件，不是本应用的依赖：没装也能正常下磁力、种子。装在哪由系统登记的信息查，
    /// 只认 /Applications 会漏掉装在别处的（比如 ~/Applications 或改名后的副本）
    private static var cache: (url: URL?, at: Date)?

    static var appURL: URL? {
        if let c = cache, Date().timeIntervalSince(c.at) < 3 { return c.url }
        let fallback = URL(fileURLWithPath: "/Applications/Thunder.app")
        let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.xunlei.Thunder")
            ?? (FileManager.default.fileExists(atPath: fallback.path) ? fallback : nil)
        cache = (url, Date())
        return url
    }

    static var installed: Bool { appURL != nil }

    /// 用户刚装完迅雷时，设置页/右键菜单要能立刻认出来
    static func recheck() { cache = nil }

    /// 官网（Mac 版页，intel 与 Apple 芯片同一个包），只在用户主动要用迅雷、但本机没装时给
    static let officialSite = URL(string: "https://mac.xunlei.com/")!

    static func openOfficialSite() {
        NSWorkspace.shared.open(officialSite)
    }

    /// 用户主动要用迅雷、但本机没装时的提示。说清它是什么、装不装都行，再给官网
    static func installAlert() -> NSAlert {
        let alert = NSAlert()
        alert.messageText = L("还没安装迅雷")
        alert.informativeText = """
        「用迅雷下载」要把任务交给本机的迅雷，没装就交不出去。

        不装也能照常下磁力、种子和直链，只是国内冷门资源多在迅雷用户之间传，装上通常更快。到官网下载 Mac 版（免费），装好后回来再点一次。
        """
        alert.addButton(withTitle: L("去迅雷官网"))
        alert.addButton(withTitle: L("继续用闪电全能下载"))
        return alert
    }

    static var store: URL { EngineConfig.supportDir.appendingPathComponent("thunder.json") }

    static func load() -> [ThunderJob] {
        guard let d = try? Data(contentsOf: store) else { return [] }
        let dec = JSONDecoder(); dec.dateDecodingStrategy = .iso8601
        return (try? dec.decode([ThunderJob].self, from: d)) ?? []
    }

    static func save(_ jobs: [ThunderJob]) {
        let enc = JSONEncoder(); enc.dateEncodingStrategy = .iso8601
        if let d = try? enc.encode(jobs) { try? PrivateFile.write(d, to: store) }
    }

    /// 磁力里的 infohash，统一成 40 位大写十六进制（base32 的也换算）
    static func infoHash(_ link: String) -> String? {
        guard let r = link.range(of: #"btih:([A-Za-z0-9]+)"#, options: .regularExpression) else { return nil }
        let h = String(link[r].dropFirst(5))
        if h.count == 40 { return h.uppercased() }
        guard h.count == 32 else { return nil }
        let alphabet = Array("ABCDEFGHIJKLMNOPQRSTUVWXYZ234567")
        var bits = "", out = ""
        for ch in h.uppercased() {
            guard let v = alphabet.firstIndex(of: ch) else { return nil }
            bits += String(v, radix: 2).leftPad(5)
        }
        for i in stride(from: 0, to: 160, by: 4) {
            let s = bits.index(bits.startIndex, offsetBy: i)
            out += String(Int(bits[s..<bits.index(s, offsetBy: 4)], radix: 2)!, radix: 16)
        }
        return out.uppercased()
    }

    /// 用迅雷打开链接（保存位置预设好），再替用户点迅雷新建任务窗口里的「立即下载」
    /// alreadyExists：迅雷认为任务已存在，拒绝新建。收回后旧任务还留在迅雷，回收站里的也算（11-05 迅雷日志确认）
    enum SubmitResult { case submitted, unconfirmed, alreadyExists, needsAccessibility }

    /// 迅雷自己的事件日志（11-05 实测）：建任务写 `taskevent/createtask` 带 result，判重写窗口名。
    /// 判重窗口的辅助功能标题读不到，要以日志为准；只读，不碰任务
    nonisolated static var accessibilityAllowed: Bool { AXIsProcessTrusted() }

    /// 打开系统设置的「辅助功能」页。自动点迅雷全靠这项授权，别人装上后也要先允许一次
    static func openAccessibilitySettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") { NSWorkspace.shared.open(url) }
    }

    private static var eventLog: URL {
        let f = DateFormatter(); f.dateFormat = "yyyyMMdd"; f.locale = Locale(identifier: "en_US_POSIX")
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/Thunder/.events/record_\(f.string(from: Date())).log")
    }

    nonisolated static func submitResult(inLog text: String) -> SubmitResult? {
        for line in text.components(separatedBy: .newlines).reversed() where line.contains("taskevent/createtask") {
            return line.contains("\"result\": \"success\"") ? .submitted : .alreadyExists
        }
        return text.contains("title=下载任务已存在") ? .alreadyExists : nil
    }

    private static func logResult(after offset: UInt64) -> SubmitResult? {
        guard let handle = try? FileHandle(forReadingFrom: eventLog) else { return nil }
        defer { try? handle.close() }
        try? handle.seek(toOffset: offset)
        return (try? handle.readToEnd()).flatMap { submitResult(inLog: String(decoding: $0, as: UTF8.self)) }
    }

    static func open(link: String, saveDir: String, onResult: @escaping (SubmitResult) -> Void) {
        guard let app = appURL, let url = URL(string: link) else { onResult(.unconfirmed); return }
        let logStart = ((try? FileManager.default.attributesOfItem(atPath: eventLog.path))?[.size] as? NSNumber)?.uint64Value ?? 0
        UserDefaults(suiteName: "com.xunlei.Thunder")?.set(saveDir, forKey: "LastSavePath")
        NSWorkspace.shared.open([url], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration(), completionHandler: nil)
        Task {
            // 只静默检查，不在交任务时弹系统授权框：系统框每调一次就弹一次，用户明明设过了还反复弹（11-15 用户反馈）。
            // 没授权就退回「让用户自己在迅雷里点立即下载」，授权入口放在设置和提示条里，由用户主动点
            guard AXIsProcessTrusted() else {
                onResult(.needsAccessibility); return
            }
            for _ in 0..<40 {
                if let result = logResult(after: logStart) { onResult(result); return }
                if hasDuplicateWindow() { onResult(.alreadyExists); return }
                _ = pressDownload()
                try? await Task.sleep(nanoseconds: 500_000_000)
            }
            onResult(.unconfirmed)
        }
    }

    private static func hasDuplicateWindow() -> Bool {
        guard let pid = NSRunningApplication.runningApplications(withBundleIdentifier: "com.xunlei.Thunder").first?.processIdentifier
        else { return false }
        var winsRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(AXUIElementCreateApplication(pid), kAXWindowsAttribute as CFString, &winsRef) == .success,
              let wins = winsRef as? [AXUIElement] else { return false }
        return wins.contains { attr($0, kAXTitleAttribute) == "下载任务已存在" }
    }

    /// 在迅雷的「新建下载任务 / 新建 BT 任务」窗口里找「立即下载」按钮按下
    private static func pressDownload() -> Bool {
        guard let pid = NSRunningApplication.runningApplications(withBundleIdentifier: "com.xunlei.Thunder").first?.processIdentifier
        else { return false }
        let appEl = AXUIElementCreateApplication(pid)
        var winsRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(appEl, kAXWindowsAttribute as CFString, &winsRef) == .success,
              let wins = winsRef as? [AXUIElement] else { return false }
        for w in wins where (attr(w, kAXTitleAttribute) ?? "").hasPrefix("新建") {
            if let b = findButton(w, title: "立即下载", depth: 0) {
                return AXUIElementPerformAction(b, kAXPressAction as CFString) == .success
            }
        }
        return false
    }

    private static func attr(_ e: AXUIElement, _ name: String) -> String? {
        var v: CFTypeRef?
        guard AXUIElementCopyAttributeValue(e, name as CFString, &v) == .success else { return nil }
        return v as? String
    }

    private static func findButton(_ e: AXUIElement, title: String, depth: Int) -> AXUIElement? {
        if attr(e, kAXRoleAttribute) == kAXButtonRole, attr(e, kAXTitleAttribute) == title { return e }
        guard depth < 8 else { return nil }
        var kids: CFTypeRef?
        guard AXUIElementCopyAttributeValue(e, kAXChildrenAttribute as CFString, &kids) == .success,
              let list = kids as? [AXUIElement] else { return nil }
        for k in list { if let b = findButton(k, title: title, depth: depth + 1) { return b } }
        return nil
    }

    /// 读迅雷的任务进度文件 `.<infohash>.js`：base64 包着的 JSON
    /// `{"infohash":"…","subinfo0":{"checkedsize":4030002163,"dlsize":4030002163}}`。
    /// 多文件任务是 subinfo0、subinfo1…，逐个加起来。checkedsize 追上 dlsize 就是下完了
    static func session(in folder: URL, hash: String?) -> (checked: Int64, total: Int64)? {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: folder.path)) ?? []
        guard let hash, let js = names.first(where: { $0.lowercased() == ".\(hash.lowercased()).js" }) else { return nil }
        guard let raw = FileManager.default.contents(atPath: folder.appendingPathComponent(js).path),
              var text = String(data: raw, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines),
              !text.isEmpty else { return nil }
        while text.count % 4 != 0 { text += "=" }
        guard let data = Data(base64Encoded: text, options: .ignoreUnknownCharacters),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return nil }
        var checked: Int64 = 0, total: Int64 = 0
        for (k, v) in obj where k.hasPrefix("subinfo") {
            guard let d = v as? [String: Any] else { continue }
            if let n = (d["checkedsize"] as? NSNumber)?.int64Value { checked += n }
            if let n = (d["dlsize"] as? NSNumber)?.int64Value { total += n }
        }
        return total > 0 ? (checked, total) : nil
    }

    /// 迅雷 5.80 下载中不写 `.<infohash>.js`（下完才有），每个文件旁边放一份 `.<文件名>.js`（11-04 实测）：
    /// base64 包的 JSON，`FileSize` 文件大小，`rqWrited` 已写入的 [起点, 长度] 区间（可能是字符串形式）。
    /// 没有这份记录的文件按实际大小算（多文件任务里已下完的）；一份记录都没有时返回 nil
    static func writtenBytes(in folder: URL) -> Int64? {
        let fm = FileManager.default
        let files = (fm.enumerator(at: folder, includingPropertiesForKeys: [.isRegularFileKey, .fileSizeKey])?.allObjects as? [URL] ?? [])
            .filter { !$0.lastPathComponent.hasPrefix(".") && (try? $0.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true }
        var written: Int64 = 0, found = false
        for file in files {
            let record = file.deletingLastPathComponent().appendingPathComponent(".\(file.lastPathComponent).js")
            if let n = writtenRecord(record) {
                written += n; found = true
            } else {
                written += Int64((try? file.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
            }
        }
        return found ? written : nil
    }

    private static func writtenRecord(_ file: URL) -> Int64? {
        guard let raw = FileManager.default.contents(atPath: file.path),
              var text = String(data: raw, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) else { return nil }
        while text.count % 4 != 0 { text += "=" }
        guard let data = Data(base64Encoded: text, options: .ignoreUnknownCharacters),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let size = (obj["FileSize"] as? NSNumber)?.int64Value else { return nil }
        var ranges = obj["rqWrited"]
        if let text = ranges as? String { ranges = try? JSONSerialization.jsonObject(with: Data(text.utf8)) }
        guard let list = ranges as? [[NSNumber]] else { return nil }
        return min(size, list.reduce(0) { $0 + ($1.count == 2 ? $1[1].int64Value : 0) })
    }

    /// 收回前的拦路提示：只提醒一句「先暂停」，不摆一堆说明（11-01 用户反馈字太多）。
    /// 找不到迅雷文件夹时也给一条出路，免得迅雷崩了或换过保存位置就永远收不回来
    static func reclaimAlert(_ state: WriteState) -> NSAlert {
        let alert = NSAlert()
        switch state {
        case .writing:
            alert.messageText = L("迅雷还在下这条")
            alert.informativeText = L("先在迅雷里暂停它，再点收回。两边同时写同一个文件会损坏。")
            alert.addButton(withTitle: L("知道了"))
        case .unknown:
            alert.messageText = L("请先在迅雷中暂停任务")
            alert.informativeText = L("闪电暂时无法确认迅雷是否已暂停。请先到迅雷暂停这条任务，再回来确认，避免两个程序同时写入文件。")
            alert.addButton(withTitle: L("我已暂停，继续收回"))
            alert.addButton(withTitle: L("再等等"))
        case .needsAccessibility:
            alert.messageText = L("需要允许「辅助功能」才能控制迅雷")
            alert.informativeText = L("在系统设置里允许闪电全能下载后，再点收回就能自动暂停迅雷。如果开关已经打开、这里仍说没权限，请把辅助功能列表里的闪电全能下载用「−」移除，再重新添加一次。也可以先在迅雷里手动暂停这条任务。")
            alert.addButton(withTitle: L("我已暂停，继续收回"))
            alert.addButton(withTitle: L("再等等"))
            alert.addButton(withTitle: L("打开系统设置"))
        case .idle, .cancelled:
            break
        }
        return alert
    }

    // MARK: 收回时替用户暂停迅雷

    struct TaskRow: Equatable { var status: String; var name: String }

    /// 暂停、失败、已完成的任务不会再写文件
    nonisolated static func isHalted(_ status: String) -> Bool {
        ["暂停", "失败", "完成"].contains { status.contains($0) }
    }

    nonisolated static func rowMatches(_ row: TaskRow, folderName: String?, name: String?) -> Bool {
        [folderName, name].compactMap { $0 }.contains { !$0.isEmpty && (row.name == $0 || row.name.hasPrefix($0)) }
    }

    private static func thunderElement() -> AXUIElement? {
        guard AXIsProcessTrusted(),
              let pid = NSRunningApplication.runningApplications(withBundleIdentifier: "com.xunlei.Thunder").first?.processIdentifier
        else { return nil }
        return AXUIElementCreateApplication(pid)
    }

    private static func mainWindow() -> AXUIElement? {
        var value: CFTypeRef?
        guard let app = thunderElement(), AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value) == .success,
              let windows = value as? [AXUIElement] else { return nil }
        return windows.first { attr($0, kAXTitleAttribute) == "迅雷" }
    }

    private static func descendants(_ element: AXUIElement, depth: Int = 0) -> [AXUIElement] {
        var value: CFTypeRef?
        guard depth < 12, AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &value) == .success,
              let children = value as? [AXUIElement] else { return [] }
        return children + children.flatMap { descendants($0, depth: depth + 1) }
    }

    /// 任务名紧挨「(共N个文件)」前面。暂停的行是「已暂停、任务名」，在下的行是「速度、剩余时间、任务名」（11-05 实测）
    nonisolated static func taskRow(texts: [String]) -> TaskRow? {
        guard texts.count >= 2, let name = texts.last else { return nil }
        return TaskRow(status: texts[0], name: name)
    }

    /// 迅雷「正在下载」页的任务行（11-04 实测 5.80）
    static func taskRows() -> [TaskRow] {
        guard let window = mainWindow() else { return [] }
        return descendants(window).compactMap { element -> TaskRow? in
            guard attr(element, kAXRoleAttribute) == kAXGroupRole else { return nil }
            var value: CFTypeRef?
            guard AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &value) == .success,
                  let children = value as? [AXUIElement],
                  let files = children.firstIndex(where: { attr($0, kAXTitleAttribute)?.contains("个文件") == true }) else { return nil }
            let texts = children[..<files].filter { attr($0, kAXRoleAttribute) == kAXStaticTextRole }.compactMap { element -> String? in
                var text: CFTypeRef?
                guard AXUIElementCopyAttributeValue(element, kAXValueAttribute as CFString, &text) == .success else { return nil }
                return text as? String
            }
            return taskRow(texts: texts)
        }
    }

    /// 用权限前只做静默检查：`AXIsProcessTrustedWithOptions(prompt:)` 每调一次就弹一次系统框，
    /// 在交任务/暂停这类动作里自动调它，就变成每次操作都被问一遍（11-15 用户反馈「设置过还总弹」）
    private static func requestAccessibility() -> Bool {
        AXIsProcessTrusted()
    }

    /// 只有用户自己点「去授权」时才让系统弹框，且只弹这一次
    @discardableResult
    static func promptAccessibility() -> Bool {
        AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary)
    }

    /// 授权状态（不弹框），设置页和提示条用
    nonisolated static var accessibilityGranted: Bool { AXIsProcessTrusted() }

    /// 窗口关着、或停在已完成等其他页时，切回「正在下载」，任务行和「全部暂停」才看得到
    private static func showThunderWindow(_ app: NSRunningApplication) async {
        if mainWindow() == nil, let url = appURL {
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.activates = false
            _ = try? await NSWorkspace.shared.openApplication(at: url, configuration: configuration)
            for _ in 0..<10 where mainWindow() == nil { try? await Task.sleep(nanoseconds: 300_000_000) }
        }
        if let window = mainWindow(), let tab = findButton(window, title: "正在下载", depth: 0) {
            AXUIElementPerformAction(tab, kAXPressAction as CFString)
            try? await Task.sleep(nanoseconds: 300_000_000)
        }
    }

    private static func pressPauseAll() -> Bool {
        guard let window = mainWindow(), let button = findButton(window, title: "全部暂停", depth: 0) else { return false }
        return AXUIElementPerformAction(button, kAXPressAction as CFString) == .success
    }

    static var isRunning: Bool { !NSRunningApplication.runningApplications(withBundleIdentifier: "com.xunlei.Thunder").isEmpty }

    /// 迅雷刚打开时用：这些任务在迅雷里停着就让它继续。没权限就不动，也不弹授权框
    static func resumeJobsAfterLaunch(_ jobs: [ThunderJob]) async -> Bool {
        guard AXIsProcessTrusted() else { return false }
        for _ in 0..<10 where mainWindow() == nil { try? await Task.sleep(nanoseconds: 500_000_000) }
        let rows = taskRows()
        let paused = jobs.contains { job in
            rows.contains { rowMatches($0, folderName: job.folder.map { URL(fileURLWithPath: $0).lastPathComponent }, name: job.name) && $0.status.contains("暂停") }
        }
        guard paused else { return false }
        return await setAllTasks(paused: false)
    }

    /// 总暂停 / 总继续用。迅雷没开时没什么可停的，算成功
    static func setAllTasks(paused: Bool) async -> Bool {
        guard let running = NSRunningApplication.runningApplications(withBundleIdentifier: "com.xunlei.Thunder").first else { return true }
        guard requestAccessibility() else { return false }
        await showThunderWindow(running)
        guard let window = mainWindow(), let button = findButton(window, title: paused ? "全部暂停" : "全部开始", depth: 0) else { return false }
        return AXUIElementPerformAction(button, kAXPressAction as CFString) == .success
    }

    /// 收回前让迅雷停下：迅雷只开放了「全部暂停」，单条按钮要鼠标悬停才有，所以别的任务在下时先问用户。
    /// 按完再看文件夹还写不写，不靠界面文字判断；做不到（没权限、窗口关了）就退回让用户手动暂停
    static func stopForReclaim(_ job: ThunderJob) async -> WriteState {
        guard let folder = job.folder, FileManager.default.fileExists(atPath: folder) else { return .unknown }
        let folderName = URL(fileURLWithPath: folder).lastPathComponent
        // 迅雷没开就不可能在写
        guard let running = NSRunningApplication.runningApplications(withBundleIdentifier: "com.xunlei.Thunder").first else {
            return await isChanging(folder) ? .writing : .idle
        }
        guard requestAccessibility() else { return .needsAccessibility }
        await showThunderWindow(running)
        let rows = taskRows()
        let row = rows.first { rowMatches($0, folderName: folderName, name: job.name) }
        let writing = await isChanging(folder)
        // 只有迅雷自己写着「已暂停」才算停了：1.5 秒没写不代表没在下，下得慢时迅雷隔一阵才写一次（11-05 用户反馈还要手动停）
        if !writing, row.map({ isHalted($0.status) }) == true { return .idle }
        let others = rows.filter { !rowMatches($0, folderName: folderName, name: job.name) && !isHalted($0.status) }
        if !others.isEmpty {
            let alert = NSAlert()
            alert.messageText = String(localized: "迅雷里还有 \(others.count) 条任务在下载")
            alert.informativeText = L("收回这条需要先暂停迅雷，其他任务也会一起暂停。之后可以到迅雷点「全部开始」继续。")
            alert.addButton(withTitle: L("一起暂停并收回"))
            alert.addButton(withTitle: L("取消"))
            NSApp.activate(ignoringOtherApps: true)
            guard alert.runModal() == .alertFirstButtonReturn else { return .cancelled }
        }
        guard pressPauseAll() else { return writing ? .writing : .unknown }
        for _ in 0..<4 {
            try? await Task.sleep(nanoseconds: 1_500_000_000)
            if await !isChanging(folder) { return .idle }
        }
        return .writing
    }

    /// 迅雷是不是还在往这个文件夹里写。收回前必须先确认：两边同时写同一个文件会损坏数据，
    /// 而迅雷的暂停按钮我们点不到（2026-11-01 实测：辅助功能拿不到它的窗口），只能让用户去停。
    /// 采样 1.5 秒，文件占用和修改时间都没动就算停了
    static func isWriting(_ job: ThunderJob) async -> Bool { await state(of: job) == .writing }

    /// 立刻再找一次任务文件夹（不受 2 秒节流限制）：收回前用，别因为刚好被节流就判成「不知道」
    static func locateNow(_ job: ThunderJob) -> String? {
        lookupAt[job.id] = nil
        downloadsScanAt = .distantPast
        return locate(job)
    }

    enum WriteState: Equatable {
        case writing     // 还在写
        case idle        // 确认没在写（安全收回）
        case unknown     // 没找到迅雷的任务文件夹，判断不了
        case cancelled   // 用户选择不暂停
        case needsAccessibility // 还没给辅助功能权限，按不了迅雷的按钮
    }

    /// 收回到本地前判断迅雷停没停。找不到文件夹时不能当「已停」——那样会在迅雷还在写的时候
    /// 另建一条任务抢同一个文件（11-01 实测：进度只有 0.6 MB，迅雷那边其实已经下完 589 MB）
    static func state(of job: ThunderJob) async -> WriteState {
        guard let folder = job.folder, FileManager.default.fileExists(atPath: folder) else { return .unknown }
        return await isChanging(folder) ? .writing : .unknown
    }

    private static func isChanging(_ folder: String) async -> Bool {
        func snap() -> (bytes: Int64, newest: TimeInterval) {
            var bytes: Int64 = 0, newest: TimeInterval = 0
            let en = FileManager.default.enumerator(at: URL(fileURLWithPath: folder),
                                                    includingPropertiesForKeys: [.totalFileAllocatedSizeKey, .isRegularFileKey, .contentModificationDateKey])
            while let f = en?.nextObject() as? URL {
                if f.lastPathComponent.hasPrefix(".") { continue }
                let v = try? f.resourceValues(forKeys: [.totalFileAllocatedSizeKey, .isRegularFileKey, .contentModificationDateKey])
                if v?.isRegularFile == true {
                    bytes += Int64(v?.totalFileAllocatedSize ?? 0)
                    newest = max(newest, v?.contentModificationDate?.timeIntervalSince1970 ?? 0)
                }
            }
            return (bytes, newest)
        }
        let a = snap()
        try? await Task.sleep(nanoseconds: 1_500_000_000)
        let b = snap()
        return b.bytes != a.bytes || b.newest > a.newest + 0.5
    }

    /// 迅雷自己记的保存位置。交给迅雷之前我们会写这个键，但用户手动改过就不一致，只当补充线索
    static var savedPath: String? { UserDefaults(suiteName: "com.xunlei.Thunder")?.string(forKey: "LastSavePath") }

    /// 找迅雷给这条任务建的文件夹：先认 `.<infohash>.js`（迅雷下完才写），再认 `.magent_*.torrent`
    /// （迅雷 5.80 下载中只有它，算出的 infohash 对得上才算，不靠文件夹名猜）。
    /// 保存位置可能有上百个文件夹，一秒一次的轮询里别反复扫，2 秒才试一次
    private static var lookupAt: [String: Date] = [:]

    /// 读不了的目录（系统隐私授权没给）临时别再碰：不然每次轮询都撞一次权限，弹窗会反复冒出来。
    /// 11-15 用户反馈「老是弹访问下载文件夹」，根因就是无条件的整目录扫描撞上没授权的「下载」
    private static var blockedRoots: [String: Date] = [:]
    /// 「下载」整个目录是兜底线索，全局 60 秒最多翻一次
    private static var downloadsScanAt: Date = .distantPast
    private static let downloadsScanGap: TimeInterval = 60
    private static let blockedGap: TimeInterval = 600

    private static func isBlocked(_ root: URL) -> Bool {
        guard let until = blockedRoots[root.standardizedFileURL.path] else { return false }
        if Date() < until { return true }
        blockedRoots[root.standardizedFileURL.path] = nil
        return false
    }

    private static func noteReadFailure(_ root: URL, _ error: Error) {
        let code = (error as NSError).code
        // NSFileReadNoPermissionError / EPERM / EACCES：没授权，退避十分钟再说
        if code == NSFileReadNoPermissionError || code == Int(EPERM) || code == Int(EACCES) {
            blockedRoots[root.standardizedFileURL.path] = Date().addingTimeInterval(blockedGap)
        }
    }

    private static func locate(_ job: ThunderJob) -> String? {
        if let t = lookupAt[job.id], Date().timeIntervalSince(t) < 2 { return nil }
        lookupAt[job.id] = Date()
        // 没有确定的任务标识时不猜目录，避免多个任务并行时关联错文件。
        guard let h = job.hash?.uppercased() else { return nil }
        // 先只看我们确实把任务放过去的地方（交接目录、迅雷记的上次保存位置）
        for root in searchRoots(job) where !isBlocked(root) {
            if let hit = match(root, hash: h, depth: 1) { return hit }
            if let hit = matchTorrent(root, hash: h, depth: 1) { return hit }
        }
        // 兜底才翻整个「下载」（迅雷可能自己换了位置）。这一步要过系统隐私授权，
        // 所以只在别处都找不到时做，而且全局一分钟最多一次
        guard Date().timeIntervalSince(downloadsScanAt) >= downloadsScanGap else { return nil }
        let fallback = Folders.downloads
        guard !isBlocked(fallback) else { return nil }
        downloadsScanAt = Date()
        if let hit = match(fallback, hash: h, depth: 2) { return hit }
        return matchTorrent(fallback, hash: h, depth: 2)
    }

    /// 重复转交时回到原保存目录，避免不断叠加“迅雷下载/任务编号”。
    static func handoffBase(_ directory: URL) -> URL {
        var base = directory
        while base.lastPathComponent.hasPrefix("thunder-"),
              base.deletingLastPathComponent().lastPathComponent == "迅雷下载" {
            base = base.deletingLastPathComponent().deletingLastPathComponent()
        }
        return base
    }

    /// 任务文件夹在中转目录里时，算出它该回到哪：`保存目录/迅雷下载/thunder-…/片名` → `保存目录/片名`
    static func homeFolder(of folder: URL) -> URL? {
        let holder = folder.deletingLastPathComponent()
        guard holder.lastPathComponent.hasPrefix("thunder-"),
              holder.deletingLastPathComponent().lastPathComponent == "迅雷下载" else { return nil }
        return handoffBase(holder).appendingPathComponent(folder.lastPathComponent)
    }

    /// 迅雷的「正在下载」里还有没有这条能继续的任务。报过错的（比如种子文件丢了）算没有。迅雷没开就先打开
    static func taskExists(folderName: String, name: String?) async -> Bool {
        guard AXIsProcessTrusted(), let app = appURL else { return false }
        if !isRunning {
            NSWorkspace.shared.open(app)
            for _ in 0..<20 where !isRunning || mainWindow() == nil { try? await Task.sleep(nanoseconds: 500_000_000) }
        }
        guard let running = NSRunningApplication.runningApplications(withBundleIdentifier: "com.xunlei.Thunder").first else { return false }
        await showThunderWindow(running)
        return taskRows().contains { rowMatches($0, folderName: folderName, name: name) && !$0.status.contains("失败") }
    }

    /// 文件夹搬走后，里面某个文件的新路径
    nonisolated static func pathAfterMove(_ path: String, from folder: String, to moved: String) -> String {
        guard path.hasPrefix(folder + "/") else { return path }
        return moved + path.dropFirst(folder.count)
    }

    /// 文件夹移走后清掉空的中转目录（重复转交可能套了几层）
    static func removeEmptyHolders(from folder: URL) {
        var holder = folder.deletingLastPathComponent()
        while holder.lastPathComponent.hasPrefix("thunder-") || holder.lastPathComponent == "迅雷下载" {
            HandoffFiles.removeIfEmpty(holder)
            guard !FileManager.default.fileExists(atPath: holder.path) else { return }
            holder = holder.deletingLastPathComponent()
        }
    }

    /// 迅雷可能把任务放在哪：我们指定的位置、迅雷记的上次保存位置（系统「下载」是 locate 里的兜底，
    /// 那里要过隐私授权，不能每次都扫）
    private static func searchRoots(_ job: ThunderJob) -> [URL] {
        var list = [URL(fileURLWithPath: job.dir)]
        // 迅雷可能复用之前交接的同一任务，数据仍留在较早的交接目录中。
        var ancestor = URL(fileURLWithPath: job.dir, isDirectory: true)
        while ancestor.lastPathComponent.hasPrefix("thunder-"),
              ancestor.deletingLastPathComponent().lastPathComponent == "迅雷下载" {
            ancestor = ancestor.deletingLastPathComponent().deletingLastPathComponent()
            list.append(ancestor)
        }
        if let p = savedPath, !p.isEmpty { list.append(URL(fileURLWithPath: p)) }
        var seen = Set<String>()
        return list.filter { seen.insert($0.path).inserted }
    }

    /// 列目录；被系统隐私授权挡住时记一笔，别在轮询里反复撞
    private static func children(of root: URL, keys: [URLResourceKey]?) -> [URL] {
        do {
            return try FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: keys)
        } catch {
            noteReadFailure(root, error)
            return []
        }
    }

    /// 根目录自己 + 下几层子文件夹里找 `.<hash>.js`（「下载」里迅雷常再套一层「迅雷下载」）
    private static func match(_ root: URL, hash: String, depth: Int) -> String? {
        let fm = FileManager.default
        if fm.fileExists(atPath: root.appendingPathComponent(".\(hash).js").path) { return root.path }
        guard depth > 0 else { return nil }
        let kids = children(of: root, keys: [.isDirectoryKey])
        for k in kids {
            if fm.fileExists(atPath: k.appendingPathComponent(".\(hash).js").path) { return k.path }
        }
        guard depth > 1 else { return nil }
        for k in kids {
            let grand = children(of: k, keys: nil)
            if let hit = grand.first(where: { fm.fileExists(atPath: $0.appendingPathComponent(".\(hash).js").path) }) {
                return hit.path
            }
        }
        return nil
    }

    private static func matchTorrent(_ root: URL, hash: String, depth: Int) -> String? {
        if torrentTotal(in: root, hash: hash) != nil { return root.path }
        guard depth > 0 else { return nil }
        let kids = children(of: root, keys: nil)
        for k in kids {
            if let hit = matchTorrent(k, hash: hash, depth: depth - 1) { return hit }
        }
        return nil
    }

    static func torrentTotal(in folder: URL, hash: String?) -> Int64? {
        guard let hash, let names = try? FileManager.default.contentsOfDirectory(atPath: folder.path) else { return nil }
        for name in names where name.hasPrefix(".magent_") && name.hasSuffix(".torrent") {
            guard let data = try? Data(contentsOf: folder.appendingPathComponent(name)) else { continue }
            if let total = TorrentMetadata.totalSize(data: data, expectedHash: hash) { return total }
        }
        return nil
    }

    /// 量一次进度：优先用迅雷 `.js` 里记的真值；文件占用只用来估速度
    static func measure(_ job: inout ThunderJob) {
        let fm = FileManager.default
        if job.folder == nil {
            guard let found = locate(job) else { return }
            job.folder = found
        }
        guard let folder = job.folder else { return }
        let folderURL = URL(fileURLWithPath: folder)
        if job.verifiedTotal == nil { job.verifiedTotal = torrentTotal(in: folderURL, hash: job.hash) }
        let previousChecked = job.checked
        if let s = session(in: folderURL, hash: job.hash) {
            job.checked = s.checked
            job.dlSize = s.total
            job.total = max(job.verifiedTotal ?? 0, s.total)
        } else {
            job.checked = nil; job.dlSize = nil
        }
        job.written = writtenBytes(in: folderURL)
        var bytes: Int64 = 0
        let en = fm.enumerator(at: folderURL, includingPropertiesForKeys: [.totalFileAllocatedSizeKey, .isRegularFileKey])
        while let f = en?.nextObject() as? URL {
            if f.lastPathComponent.hasPrefix(".") {
                continue
            }
            let v = try? f.resourceValues(forKeys: [.totalFileAllocatedSizeKey, .isRegularFileKey])
            if v?.isRegularFile == true { bytes += Int64(v?.totalFileAllocatedSize ?? 0) }
        }
        let now = Date()
        if bytes != job.bytes { job.lastChangeAt = now }
        if let t = job.measuredAt, now.timeIntervalSince(t) > 0.5 {
            let checkedDelta = (job.checked != nil && previousChecked != nil) ? max(0, job.checked! - previousChecked!) : 0
            // 迅雷进度记录可能只在暂停时落盘；文件占用变化只能作为写入速度估算。
            let delta = max(checkedDelta, max(0, bytes - job.bytes))
            let s = Double(delta) / now.timeIntervalSince(t)
            job.speed = Int64(Double(job.speed) * 0.6 + s * 0.4)
        }
        job.bytes = bytes; job.measuredAt = now
        // 只在对应任务的进度记录明确到达总量时判完成；无法读取记录时保持未知。
        if let expected = job.verifiedTotal, expected > 0, let dl = job.dlSize, let ck = job.checked {
            // 迅雷会跳过种子里的小附件（常见 71 字节的说明文件，11-04 三条任务都差这点），所以 dlsize 可能略小于种子总量：
            // 选中的都校验完、没有下载中的 `.<文件名>.js`、差额不超过 1 MB 且不到 1% 才算完成，大文件还没开始时差额大，不会误判
            let gap = expected - dl
            let onlySkippedExtras = dl > 0 && ck >= dl && job.written == nil && gap <= 1 << 20 && gap * 100 <= expected
            job.done = (dl >= expected && ck >= expected) || onlySkippedExtras
            job.completionVerified = job.done
            if job.done { job.speed = 0; job.bytes = expected }
        } else if job.completionVerified != true {
            job.done = false
        }
    }

    /// 任务文件夹里最大的那个文件（截图、播放用）
    static func mainFile(_ job: ThunderJob) -> URL? {
        guard let folder = job.folder else { return nil }
        let fm = FileManager.default
        let files = (fm.enumerator(at: URL(fileURLWithPath: folder), includingPropertiesForKeys: [.fileSizeKey])?.allObjects as? [URL] ?? [])
            .filter { !$0.lastPathComponent.hasPrefix(".") }
        return files.max { ((try? $0.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0) < ((try? $1.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0) }
    }

    // MARK: 迅雷里这条任务还在不在

    /// 迅雷把任务清单存在 `~/Library/Application Support/Thunder/etm3/etm_map.db`（SQLite，带 json1）。
    /// 删掉的任务先留在迅雷的回收站里（`in_rubbish=1`），清空回收站后整行消失。
    /// 11-15 用户反馈：在迅雷里把下载关掉 / 删掉后，闪电这边还一直显示「迅雷下载中 · 0 B/s」，
    /// 因为文件夹和 `.js` 都看不出「任务已经没了」。读它自己的账本最准，读不到就退回看文件夹
    struct LedgerRow: Equatable {
        var rubbish: Bool
        var dir: String        // create_param.download_path：交给迅雷时我们建的 thunder-<id> 目录就在这儿
        var fileName: String   // create_param.file_name：种子里的文件名，和 job.name 对得上
    }

    enum Liveness: Equatable {
        case alive      // 账本里还记着这条（在下 / 暂停 / 排队都算）
        case removed    // 被删了
        /// 账本能读，但里面没有我们交给迅雷时建的那个 `thunder-<id>` 目录，也对不上文件名：
        /// 这条任务根本没建成（用户在新任务窗口里点了取消）
        case neverCreated
        case unknown    // 判断不了：没装迅雷、账本读不到、对不上号
    }

    private static var ledgerPath: String {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/Thunder/etm3/etm_map.db").path
    }

    private static var ledgerCache: (rows: [LedgerRow]?, at: Date)?

    /// 读迅雷的任务账本。老版本没有这个库、库被写锁住、表结构变了，都返回 nil（当作判断不了）
    static func ledger() -> [LedgerRow]? {
        if let c = ledgerCache, Date().timeIntervalSince(c.at) < 3 { return c.rows }
        let rows = readLedger()
        ledgerCache = (rows, Date())
        return rows
    }

    private static func readLedger() -> [LedgerRow]? {
        guard FileManager.default.fileExists(atPath: ledgerPath) else { return nil }
        var db: OpaquePointer?
        guard sqlite3_open_v2(ledgerPath, &db, SQLITE_OPEN_READONLY, nil) == SQLITE_OK, let db else { return nil }
        defer { sqlite3_close(db) }
        sqlite3_busy_timeout(db, 300)
        var st: OpaquePointer?
        guard sqlite3_prepare_v2(db, """
            select in_rubbish,
                   coalesce(json_extract(create_param,'$.download_path'),''),
                   coalesce(json_extract(create_param,'$.file_name'),'')
            from etm_task
            """, -1, &st, nil) == SQLITE_OK else { return nil }
        defer { sqlite3_finalize(st) }
        var rows: [LedgerRow] = []
        while sqlite3_step(st) == SQLITE_ROW {
            func text(_ i: Int32) -> String { sqlite3_column_text(st, i).map { String(cString: $0) } ?? "" }
            rows.append(LedgerRow(rubbish: sqlite3_column_int(st, 0) != 0, dir: text(1), fileName: text(2)))
        }
        return rows.isEmpty ? nil : rows
    }

    /// 纯判断（方便写测试）：
    /// - 正在写文件就一定是活的，先信事实，别被同名旧任务的回收站记录骗了
    /// - 先按我们交给迅雷时建的中转目录认（`迅雷下载/thunder-<id>`，每条任务唯一）；
    ///   认不到再按文件名认。**不能拿用户的保存目录（比如「下载」「movies」）当键**——那里几十条任务的
    ///   记录混在一起，只要有一条活着就会把已经删掉的那条盖过去
    /// - 命中的行全在回收站里才算删了；一行都认不到时，只有「以前找到过迅雷建的文件夹、现在文件夹也没了」
    ///   才敢断定被删（迅雷删任务默认连文件一起删，11-15 实测三条回收站任务的目录都已不在）
    /// - 账本读得到、唯独没有我们这个唯一目录，也认不到文件名 → 任务没建成（用户在新任务窗口里点了取消）
    static func liveness(rows: [LedgerRow]?, dirs: Set<String>, fileName: String?, folderKnown: Bool,
                         folderExists: Bool, wroteRecently: Bool) -> Liveness {
        if folderKnown, folderExists, wroteRecently { return .alive }
        guard let rows else { return folderKnown && !folderExists ? .removed : .unknown }
        let handoff = dirs.filter { $0.contains("/thunder-") }
        var matched = handoff.isEmpty ? [] : rows.filter { handoff.contains($0.dir) }
        if matched.isEmpty, let fileName, !fileName.isEmpty {
            matched = rows.filter { $0.fileName == fileName }
        }
        if !matched.isEmpty { return matched.contains { !$0.rubbish } ? .alive : .removed }
        if folderKnown { return folderExists ? .unknown : .removed }
        // 账本记得着别的任务，就是没有我们这一个 thunder-<id>：迅雷里从没建成这条
        return handoff.isEmpty ? .unknown : .neverCreated
    }

    /// 这条任务现在的状态：文件夹和迅雷账本一起看
    static func liveness(of job: ThunderJob) -> Liveness {
        let dirs = Set([job.dir, job.folder].compactMap { $0.map { URL(fileURLWithPath: $0).standardizedFileURL.path } })
        let known = job.folder != nil
        let exists = job.folder.map { FileManager.default.fileExists(atPath: $0) } ?? false
        let wrote = job.lastChangeAt.map { Date().timeIntervalSince($0) < 120 } ?? false
        return liveness(rows: ledger(), dirs: dirs, fileName: job.name, folderKnown: known,
                        folderExists: exists, wroteRecently: wrote)
    }
}

private extension String {
    func leftPad(_ n: Int) -> String { String(repeating: "0", count: max(0, n - count)) + self }
}
