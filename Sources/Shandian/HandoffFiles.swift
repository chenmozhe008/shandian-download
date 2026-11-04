import Foundation

/// 迅雷、PikPak 下载时各用独立的中转目录（`迅雷下载/thunder-…`、`PikPak 下载/cloud-…`，防止和原任务抢同一个文件）。
/// 下完后把片子移回正常的保存目录、清掉中转目录和原任务留下的半截文件
enum HandoffFiles {
    static func cloudBase(of directory: URL) -> URL? {
        guard directory.lastPathComponent.hasPrefix("cloud-"),
              directory.deletingLastPathComponent().lastPathComponent == "PikPak 下载" else { return nil }
        return directory.deletingLastPathComponent().deletingLastPathComponent()
    }

    static func allocatedSize(of url: URL) -> Int64 {
        let fm = FileManager.default
        var isDirectory: ObjCBool = false
        guard fm.fileExists(atPath: url.path, isDirectory: &isDirectory) else { return 0 }
        guard isDirectory.boolValue else { return (try? url.resourceValues(forKeys: [.totalFileAllocatedSizeKey]).totalFileAllocatedSize).map(Int64.init) ?? 0 }
        var total: Int64 = 0
        let walker = fm.enumerator(at: url, includingPropertiesForKeys: [.totalFileAllocatedSizeKey, .isRegularFileKey])
        while let file = walker?.nextObject() as? URL {
            let values = try? file.resourceValues(forKeys: [.totalFileAllocatedSizeKey, .isRegularFileKey])
            if values?.isRegularFile == true { total += Int64(values?.totalFileAllocatedSize ?? 0) }
        }
        return total
    }

    /// 只有确实是半截才清：在保存目录里、不是保存目录本身、不包含完整文件、比完整文件小
    static func isDisposablePartial(_ partial: URL, base: URL, finished: URL, finishedSize: Int64) -> Bool {
        let path = partial.standardizedFileURL.path, root = base.standardizedFileURL.path, done = finished.standardizedFileURL.path
        guard path.hasPrefix(root + "/"), path != done, !done.hasPrefix(path + "/") else { return false }
        return FileManager.default.fileExists(atPath: path) && allocatedSize(of: partial) < finishedSize
    }

    /// 重名时加序号：x.mp4 → x (2).mp4
    static func unique(_ url: URL) -> URL {
        let fm = FileManager.default
        guard fm.fileExists(atPath: url.path) else { return url }
        let parent = url.deletingLastPathComponent(), ext = url.pathExtension
        let stem = url.deletingPathExtension().lastPathComponent
        for n in 2...999 {
            let candidate = parent.appendingPathComponent(ext.isEmpty ? "\(stem) (\(n))" : "\(stem) (\(n)).\(ext)")
            if !fm.fileExists(atPath: candidate.path) { return candidate }
        }
        return url
    }

    /// 只删空目录；访达留下的 .DS_Store 不算内容
    static func removeIfEmpty(_ directory: URL) {
        let fm = FileManager.default
        guard let names = try? fm.contentsOfDirectory(atPath: directory.path), names.allSatisfy({ $0 == ".DS_Store" }) else { return }
        try? fm.removeItem(at: directory)
    }

    /// 文件夹名里不能有斜杠和冒号
    static func safeName(_ title: String) -> String {
        let cleaned = title.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return cleaned.isEmpty ? "PikPak 下载" : cleaned
    }

    /// 把迅雷的任务文件夹移回保存目录。半截文件先进废纸篓（可恢复）；
    /// keepName：收回给内核接着下时名字必须和种子一致，同名的东西清不掉就不移，返回 nil
    @discardableResult
    static func moveThunderFolder(_ folder: URL, to target: URL, partial: URL?, keepName: Bool,
                                  trash: (URL) throws -> Void = { try FileManager.default.trashItem(at: $0, resultingItemURL: nil) }) throws -> URL? {
        let fm = FileManager.default
        let base = target.deletingLastPathComponent()
        let size = allocatedSize(of: folder)
        if let partial, isDisposablePartial(partial, base: base, finished: folder, finishedSize: size) { try trash(partial) }
        if fm.fileExists(atPath: target.path), keepName { return nil }
        let destination = unique(target)
        try fm.moveItem(at: folder, to: destination)
        return destination
    }
}
