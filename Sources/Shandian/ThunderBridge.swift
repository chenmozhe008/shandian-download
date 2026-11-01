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
}

@MainActor
enum ThunderBridge {
    static let app = URL(fileURLWithPath: "/Applications/Thunder.app")
    static var installed: Bool { FileManager.default.fileExists(atPath: app.path) }
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
        UserDefaults(suiteName: "com.xunlei.Thunder")?.set(saveDir, forKey: "LastSavePath")
        guard let url = URL(string: link) else { return }
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

    /// 量一次进度：优先用迅雷 `.js` 里记的真值；文件占用只用来估速度
    static func measure(_ job: inout ThunderJob) {
        let fm = FileManager.default
        if job.folder == nil {
            // 按 .<hash>.js 找迅雷建的文件夹；没 hash 的只能按创建时间找最新的
            let dir = URL(fileURLWithPath: job.dir)
            let kids = (try? fm.contentsOfDirectory(at: dir, includingPropertiesForKeys: [.creationDateKey], options: [])) ?? []
            if let h = job.hash {
                // 迅雷里已有同一个任务时它不另存，会沿用原来的位置：在「下载」里（两层内）也找一下
                let downloads = (try? fm.contentsOfDirectory(at: Folders.downloads, includingPropertiesForKeys: nil)) ?? []
                let deeper = downloads.flatMap { (try? fm.contentsOfDirectory(at: $0, includingPropertiesForKeys: nil)) ?? [] }
                job.folder = (kids + downloads + deeper).first { fm.fileExists(atPath: $0.appendingPathComponent(".\(h).js").path) }?.path
            }
            // 不一定有 .<hash>.js（10-28《抓特务》就没有）：按种子名找，再按「交过去之后新建、里面有迅雷的 .magent_ 种子」找
            if job.folder == nil, let n = job.name, !n.isEmpty, fm.fileExists(atPath: dir.appendingPathComponent(n).path) {
                job.folder = dir.appendingPathComponent(n).path
            }
            if job.folder == nil {
                job.folder = kids.first { k in
                    let created = (try? k.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? .distantPast
                    let files = (try? fm.contentsOfDirectory(atPath: k.path)) ?? []
                    return created > job.createdAt.addingTimeInterval(-120) && files.contains { $0.hasPrefix(".magent_") }
                }?.path
            }
            if job.folder == nil, job.hash == nil {
                job.folder = kids.filter { ((try? $0.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? .distantPast) > job.createdAt }
                    .max { a, b in
                        ((try? a.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? .distantPast)
                            < ((try? b.resourceValues(forKeys: [.creationDateKey]).creationDate) ?? .distantPast)
                    }?.path
            }
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
            job.done = true; job.speed = 0; if job.total == 0 { job.total = bytes }
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
