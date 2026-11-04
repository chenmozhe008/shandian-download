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
            alert.messageText = "迅雷还在下这条"
            alert.informativeText = "先在迅雷里暂停它，再点收回。两边同时写同一个文件会损坏。"
            alert.addButton(withTitle: "知道了")
        case .unknown:
            alert.messageText = "请先在迅雷中暂停任务"
            alert.informativeText = "闪电暂时无法确认迅雷是否已暂停。请先到迅雷暂停这条任务，再回来确认，避免两个程序同时写入文件。"
            alert.addButton(withTitle: "我已暂停，继续收回")
            alert.addButton(withTitle: "再等等")
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

    private static func locate(_ job: ThunderJob) -> String? {
        if let t = lookupAt[job.id], Date().timeIntervalSince(t) < 2 { return nil }
        lookupAt[job.id] = Date()
        let roots = searchRoots(job)
        if let h = job.hash?.uppercased() {
            for root in roots {
                if let hit = match(root, hash: h, depth: root.path == Folders.downloads.path ? 2 : 1) { return hit }
            }
            for root in roots {
                if let hit = matchTorrent(root, hash: h, depth: root.path == Folders.downloads.path ? 2 : 1) { return hit }
            }
        }
        // 没有确定的任务标识时不猜目录，避免多个任务并行时关联错文件。
        return nil
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

    /// 文件夹移走后清掉空的中转目录（重复转交可能套了几层）
    static func removeEmptyHolders(from folder: URL) {
        var holder = folder.deletingLastPathComponent()
        while holder.lastPathComponent.hasPrefix("thunder-") || holder.lastPathComponent == "迅雷下载" {
            HandoffFiles.removeIfEmpty(holder)
            guard !FileManager.default.fileExists(atPath: holder.path) else { return }
            holder = holder.deletingLastPathComponent()
        }
    }

    /// 迅雷可能把任务放在哪：我们指定的位置、迅雷记的上次保存位置、系统「下载」
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

    private static func matchTorrent(_ root: URL, hash: String, depth: Int) -> String? {
        if torrentTotal(in: root, hash: hash) != nil { return root.path }
        guard depth > 0 else { return nil }
        let kids = (try? FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)) ?? []
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
}

private extension String {
    func leftPad(_ n: Int) -> String { String(repeating: "0", count: max(0, n - count)) + self }
}
