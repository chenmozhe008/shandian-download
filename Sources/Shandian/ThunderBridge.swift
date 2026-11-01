import AppKit
@preconcurrency import ApplicationServices

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
    var done = false
    var bytes: Int64 = 0
    var speed: Int64 = 0
    var measuredAt: Date?
    /// 迅雷写在任务文件夹里 `.<infohash>.js` 的进度：已完成字节 / 任务总字节。
    /// 文件实际占用按 4K 对齐，4 GB 的片子会比真实大小多出几 MB，靠它判「满」永远差一点（11-01 实测），
    /// 完成后迅雷也照旧留着这个文件，所以「.js 没了才算完」也不成立。这两个数是迅雷自己记的真值
    var checked: Int64?
    var dlSize: Int64?
    /// 文件夹里最后一个字节都没变化的时间：判「老版本迅雷下完删了 .js」用，刚建文件夹时不至于立刻算完成
    var lastChangeAt: Date?
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
        alert.messageText = "还没安装迅雷"
        alert.informativeText = """
        「用迅雷下载」要把任务交给本机的迅雷，没装就交不出去。

        不装也能照常下磁力、种子和直链，只是国内冷门资源多在迅雷用户之间传，装上通常更快。到官网下载 Mac 版（免费），装好后回来再点一次。
        """
        alert.addButton(withTitle: "去迅雷官网")
        alert.addButton(withTitle: "继续用闪电全能下载")
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
        if let d = try? enc.encode(jobs) { try? d.write(to: store) }
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
    static func open(link: String, saveDir: String, onClicked: @escaping (Bool) -> Void) {
        guard let app = appURL, let url = URL(string: link) else { onClicked(false); return }
        UserDefaults(suiteName: "com.xunlei.Thunder")?.set(saveDir, forKey: "LastSavePath")
        NSWorkspace.shared.open([url], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration(), completionHandler: nil)
        Task {
            // 第一次会请求「辅助功能」权限；没给就只能让用户自己在迅雷里点
            guard AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary) else {
                onClicked(false); return
            }
            for _ in 0..<40 {
                if pressDownload() { onClicked(true); return }
                try? await Task.sleep(nanoseconds: 500_000_000)
            }
            onClicked(false)
        }
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
    static func session(in folder: URL) -> (checked: Int64, total: Int64)? {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: folder.path)) ?? []
        guard let js = names.first(where: { $0.hasPrefix(".") && $0.hasSuffix(".js") }) else { return nil }
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

    /// 收回前的拦路提示：只提醒一句「先暂停」，不摆一堆说明（11-01 用户反馈字太多）。
    /// 找不到迅雷文件夹时也给一条出路，免得迅雷崩了或换过保存位置就永远收不回来
    static func reclaimAlert(_ state: WriteState) -> NSAlert {
        let alert = NSAlert()
        switch state {
        case .writing:
            alert.messageText = "迅雷还在下这条"
            alert.informativeText = "先在迅雷里暂停它，再点收回。两边同时写同一个文件会损坏。"
            alert.addButton(withTitle: "知道了")
        case .unknown:
            alert.messageText = "还没找到迅雷下的文件"
            alert.informativeText = "迅雷可能刚建任务，或者换过保存位置。确认迅雷里这条已经暂停或下完，再收回。"
            alert.addButton(withTitle: "再等等")
            alert.addButton(withTitle: "仍然收回")
        case .idle:
            break
        }
        return alert
    }

    /// 迅雷是不是还在往这个文件夹里写。收回前必须先确认：两边同时写同一个文件会损坏数据，
    /// 而迅雷的暂停按钮我们点不到（2026-11-01 实测：辅助功能拿不到它的窗口），只能让用户去停。
    /// 采样 1.5 秒，文件占用和修改时间都没动就算停了
    static func isWriting(_ job: ThunderJob) async -> Bool { await state(of: job) == .writing }

    /// 立刻再找一次任务文件夹（不受 2 秒节流限制）：收回前用，别因为刚好被节流就判成「不知道」
    static func locateNow(_ job: ThunderJob) -> String? {
        lookupAt[job.id] = nil
        return locate(job)
    }

    enum WriteState: Equatable {
        case writing     // 还在写
        case idle        // 确认没在写（安全收回）
        case unknown     // 没找到迅雷的任务文件夹，判断不了
    }

    /// 收回到本地前判断迅雷停没停。找不到文件夹时不能当「已停」——那样会在迅雷还在写的时候
    /// 另建一条任务抢同一个文件（11-01 实测：进度只有 0.6 MB，迅雷那边其实已经下完 589 MB）
    static func state(of job: ThunderJob) async -> WriteState {
        guard let folder = job.folder else { return .unknown }
        return await isChanging(folder) ? .writing : .idle
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

    /// 找迅雷给这条任务建的文件夹：先认 `.<infohash>.js`（迅雷自己写进度的文件），再按种子名，
    /// 再按「交给迅雷之后新建、里面有迅雷 .magent_ 种子」认。
    /// 保存位置可能有上百个文件夹，一秒一次的轮询里别反复扫，2 秒才试一次
    private static var lookupAt: [String: Date] = [:]

    private static func locate(_ job: ThunderJob) -> String? {
        if let t = lookupAt[job.id], Date().timeIntervalSince(t) < 2 { return nil }
        lookupAt[job.id] = Date()
        let fm = FileManager.default
        let roots = searchRoots(job)
        if let h = job.hash?.uppercased() {
            for root in roots {
                if let hit = match(root, hash: h, depth: root.path == Folders.downloads.path ? 2 : 1) { return hit }
            }
        }
        // 不一定有 .<hash>.js（10-28《抓特务》就没有）：按种子名找
        if let n = job.name, !n.isEmpty {
            for root in roots where fm.fileExists(atPath: root.appendingPathComponent(n).path) {
                return root.appendingPathComponent(n).path
            }
        }
        // 「交给迅雷之后新建、里面有迅雷 .magent_ 种子」的那个文件夹
        for root in roots {
            let kids = (try? fm.contentsOfDirectory(at: root, includingPropertiesForKeys: [.creationDateKey])) ?? []
            if let hit = kids.first(where: { k in
                let created = (try? k.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? .distantPast
                guard created > job.createdAt.addingTimeInterval(-180) else { return false }
                let files = (try? fm.contentsOfDirectory(atPath: k.path)) ?? []
                return files.contains { $0.hasPrefix(".magent_") }
            }) { return hit.path }
        }
        return nil
    }

    /// 迅雷可能把任务放在哪：我们指定的位置、迅雷记的上次保存位置、系统「下载」
    private static func searchRoots(_ job: ThunderJob) -> [URL] {
        var list = [URL(fileURLWithPath: job.dir)]
        if let p = savedPath, !p.isEmpty { list.append(URL(fileURLWithPath: p)) }
        list.append(Folders.downloads)
        var seen = Set<String>()
        return list.filter { seen.insert($0.path).inserted }
    }

    /// 根目录自己 + 下几层子文件夹里找 `.<hash>.js`（「下载」里迅雷常再套一层「迅雷下载」）
    private static func match(_ root: URL, hash: String, depth: Int) -> String? {
        let fm = FileManager.default
        if fm.fileExists(atPath: root.appendingPathComponent(".\(hash).js").path) { return root.path }
        guard depth > 0 else { return nil }
        let kids = (try? fm.contentsOfDirectory(at: root, includingPropertiesForKeys: [.isDirectoryKey])) ?? []
        for k in kids {
            if fm.fileExists(atPath: k.appendingPathComponent(".\(hash).js").path) { return k.path }
        }
        guard depth > 1 else { return nil }
        for k in kids {
            let grand = (try? fm.contentsOfDirectory(at: k, includingPropertiesForKeys: nil)) ?? []
            if let hit = grand.first(where: { fm.fileExists(atPath: $0.appendingPathComponent(".\(hash).js").path) }) {
                return hit.path
            }
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
        if let s = session(in: folderURL) {
            job.checked = s.checked
            job.dlSize = s.total
            job.total = s.total      // 迅雷记的总大小比种子解析出来的准
        }
        var bytes: Int64 = 0, hiddenProgress = 0
        let en = fm.enumerator(at: folderURL, includingPropertiesForKeys: [.totalFileAllocatedSizeKey, .isRegularFileKey])
        while let f = en?.nextObject() as? URL {
            if f.lastPathComponent.hasPrefix(".") {
                if f.pathExtension == "js" { hiddenProgress += 1 }
                continue
            }
            let v = try? f.resourceValues(forKeys: [.totalFileAllocatedSizeKey, .isRegularFileKey])
            if v?.isRegularFile == true { bytes += Int64(v?.totalFileAllocatedSize ?? 0) }
        }
        let now = Date()
        if bytes != job.bytes { job.lastChangeAt = now }
        if let t = job.measuredAt, now.timeIntervalSince(t) > 0.5 {
            let s = Double(max(0, bytes - job.bytes)) / now.timeIntervalSince(t)
            job.speed = Int64(Double(job.speed) * 0.6 + s * 0.4)
        }
        job.bytes = bytes; job.measuredAt = now
        // 迅雷记的已下字节追上任务总大小 = 下完了（.js 完成后仍留着，不能靠它消失来判；留一点容差，
        // 免得最后一块的尾巴让它永远差一点）；老版本迅雷下完会清掉 .js，那条路也留着
        if let dl = job.dlSize, dl > 0, let ck = job.checked, ck >= dl - min(64 * 1024, dl / 100) {
            job.done = true; job.speed = 0; job.bytes = dl
        } else if hiddenProgress == 0, bytes > 0 {
            // 没有 .js 时别一看到文件就判完成：迅雷刚建好文件夹、还没写进度文件的那几秒也长这样。
            // 大小到齐，或者二十秒没再长，才算它下完了
            let complete = job.total > 0 && bytes >= job.total - min(64 * 1024, job.total / 100)
            let quiet = job.lastChangeAt.map { now.timeIntervalSince($0) > 20 } ?? false
            if complete || quiet {
                job.done = true; job.speed = 0; if job.total == 0 { job.total = bytes }
            }
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
}

private extension String {
    func leftPad(_ n: Int) -> String { String(repeating: "0", count: max(0, n - count)) + self }
}
