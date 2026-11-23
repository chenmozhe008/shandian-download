import CryptoKit
import Foundation

/// Uses Baidu's authorized CLI, independently of the HTTP download engine.
@MainActor
final class BaiduDownloads {
    private struct Record: Codable {
        var id = UUID()
        let key: String
        let directory: URL
        var createdAt = Date()
        var file: URL?
        var bytes: Int64 = 0
        var failure: Failure?
        var cleanCloudCopy: Bool?
    }

    private enum Failure: String, Codable {
        case expiredShare, unavailableShare, ownShare, downloadFailed

        var message: String {
            switch self {
            case .expiredShare: L("百度网盘分享已过期，请向分享者索取新链接。")
            case .unavailableShare: L("百度网盘报告分享链接异常，请打开分享页确认。")
            case .ownShare: L("百度网盘不允许转存自己分享的文件。请从网盘中的原文件下载。")
            case .downloadFailed: L("百度网盘下载未完成。请检查 bdpan 授权、转存空间和分享有效期。")
            }
        }
    }

    private struct CLIResponse: Decodable {
        let code: Int
        let error: String?

        var failure: Failure? {
            guard code != 0 else { return nil }
            if error?.contains("errno=13002,") == true { return .expiredShare }
            if error?.contains("errno=13001,") == true { return .unavailableShare }
            if error?.contains("errno=13045,") == true { return .ownShare }
            return .downloadFailed
        }
    }

    private struct Progress {
        var time: TimeInterval
        var lastWrite: TimeInterval
        var bytes: Int64 = 0
        var speed: Int64 = 0
    }

    private struct ShareListing: Decodable {
        struct Entry: Decodable {
            let name: String
            let size: Int64
            let isDirectory: Bool

            enum CodingKeys: String, CodingKey {
                case name, size
                case isDirectory = "is_dir"
            }
        }

        let hasMore: Bool
        let items: [Entry]

        enum CodingKeys: String, CodingKey {
            case items
            case hasMore = "has_more"
        }

        var singleName: String? { !hasMore && items.count == 1 ? items.first?.name : nil }

        var totalBytes: Int64? {
            guard !hasMore, !items.isEmpty else { return nil }
            var total: Int64 = 0
            for item in items {
                guard !item.isDirectory, item.size >= 0, item.size <= Int64.max - total else { return nil }
                total += item.size
            }
            return total
        }
    }

    static let setupURL = URL(string: "https://github.com/baidu-netdisk/bdpan-storage")!

    static var executable: URL? {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return [
            home.appendingPathComponent(".local/bin/bdpan"),
            URL(fileURLWithPath: "/opt/homebrew/bin/bdpan"),
            URL(fileURLWithPath: "/usr/local/bin/bdpan")
        ].first { FileManager.default.isExecutableFile(atPath: $0.path) }
    }

    private let storage: URL
    private var records: [Record]
    private var processes: [UUID: Process] = [:]
    private var progress: [UUID: Progress] = [:]
    private var listingProcesses: [UUID: Process] = [:]
    private var listings: [UUID: ShareListing] = [:]
    private var shares: [UUID: URL] = [:]
    var onChange: (() -> Void)?
    var onNotice: ((String) -> Void)?

    init(storage: URL = EngineConfig.supportDir.appendingPathComponent("baidu-downloads.json")) {
        self.storage = storage
        records = (try? JSONDecoder().decode([Record].self, from: Data(contentsOf: storage))) ?? []
    }

    var items: [Item] {
        records.map { record in
            let listing = listings[record.id]
            let title = record.file?.lastPathComponent ?? listing?.singleName ?? L("百度网盘分享文件")
            let status: Item.Status
            if record.file != nil { status = .done }
            else if processes[record.id] != nil { status = .running }
            else { status = .failed((record.failure ?? .downloadFailed).message) }
            let sample = progress[record.id]
            let downloaded = sample?.bytes ?? record.bytes
            let total = sample == nil ? record.bytes : (listing?.totalBytes ?? 0)
            return Item(id: record.id.uuidString, source: .baidu(record.id), title: title, rawTitle: title,
                        origin: "百度网盘", category: .cloud, status: status,
                        downloaded: downloaded, total: total >= downloaded ? total : 0,
                        speed: sample?.speed ?? 0, file: record.file,
                        createdAt: record.createdAt, thumbRemote: nil, linkKey: record.key, location: record.file,
                        avgSpeed: sample?.speed ?? 0)
        }
    }

    func refreshProgress(now: TimeInterval = ProcessInfo.processInfo.systemUptime) {
        let keys: [URLResourceKey] = [.isRegularFileKey, .fileSizeKey, .isSymbolicLinkKey]
        for record in records where processes[record.id] != nil {
            var bytes: Int64 = 0
            if let files = FileManager.default.enumerator(at: stage(record), includingPropertiesForKeys: keys) {
                for case let path as URL in files {
                    guard let values = try? path.resourceValues(forKeys: Set(keys)),
                          values.isRegularFile == true, values.isSymbolicLink != true else { continue }
                    bytes += Int64(values.fileSize ?? 0)
                }
            }
            guard var sample = progress[record.id], now > sample.time else { continue }
            if bytes > sample.bytes {
                let elapsed = now - (sample.speed > 0 ? sample.lastWrite : sample.time)
                sample.speed = Int64(Double(bytes - sample.bytes) / max(elapsed, 0.1))
                sample.lastWrite = now
            } else if bytes < sample.bytes || now - sample.lastWrite >= 5 {
                sample.speed = 0
            }
            sample.time = now
            sample.bytes = bytes
            progress[record.id] = sample
        }
    }

    static func arguments(share: URL, local: URL, id: UUID) -> [String] {
        ["download", share.absoluteString, local.path + "/", "-t", transferPath(id), "--no-check-update", "--json"]
    }

    private nonisolated static func transferPath(_ id: UUID) -> String { "Shandian/\(id.uuidString)" }

    func start(share: URL, directory: URL, executable: URL, cleanCloudCopy: Bool = false) {
        var identity = URLComponents(url: share, resolvingAgainstBaseURL: false)
        identity?.scheme = "https"
        identity?.host = "pan.baidu.com"
        identity?.fragment = nil
        let identityQuery = identity?.queryItems?.filter { ["surl", "uk", "shareid"].contains($0.name) } ?? []
        identity?.queryItems = identityQuery.isEmpty ? nil : identityQuery
        let key = "baidu-share-" + SHA256.hash(data: Data((identity?.string ?? share.absoluteString).utf8)).description
        if let index = records.firstIndex(where: { $0.key == key && $0.file == nil }) {
            let previous = records[index]
            guard processes[previous.id] == nil else {
                onNotice?(L("这条百度网盘分享正在下载"))
                return
            }
            shares[previous.id] = share
            records[index].cleanCloudCopy = cleanCloudCopy
            run(records[index], share: share, executable: executable)
            return
        }
        let record = Record(key: key, directory: directory, cleanCloudCopy: cleanCloudCopy)
        records.append(record)
        shares[record.id] = share
        run(record, share: share, executable: executable)
    }

    func retry(_ id: UUID) {
        guard processes[id] == nil, let record = records.first(where: { $0.id == id && $0.file == nil }) else { return }
        guard let share = shares[id], let executable = Self.executable else {
            onNotice?(L("请完成 bdpan 安装和百度账号授权，再粘贴原分享链接重试。"))
            return
        }
        run(record, share: share, executable: executable)
    }

    func remove(_ id: UUID) {
        if let record = records.first(where: { $0.id == id }), processes[id] == nil {
            try? FileManager.default.removeItem(at: stage(record))
        }
        records.removeAll { $0.id == id }
        progress.removeValue(forKey: id)
        clearListing(id)
        shares.removeValue(forKey: id)
        if let task = processes[id], task.isRunning { task.terminate() }
        publish()
    }

    func shutdown() {
        for process in processes.values where process.isRunning { process.terminate() }
        for process in listingProcesses.values where process.isRunning { process.terminate() }
    }

    private func stage(_ record: Record) -> URL {
        record.directory.appendingPathComponent(".shandian-baidu-\(record.id.uuidString)", isDirectory: true)
    }

    private func run(_ record: Record, share: URL, executable: URL) {
        let staging = stage(record)
        let process = Process()
        process.executableURL = executable
        let transferID = UUID()
        process.arguments = Self.arguments(share: share, local: staging, id: transferID)
        process.standardInput = FileHandle.nullDevice
        let output = Pipe()
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do {
            if FileManager.default.fileExists(atPath: staging.path) {
                try FileManager.default.removeItem(at: staging)
            }
            try FileManager.default.createDirectory(at: staging, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            try process.run()
            processes[record.id] = process
            let now = ProcessInfo.processInfo.systemUptime
            progress[record.id] = Progress(time: now, lastWrite: now)
            Task.detached { [weak self] in
                let response = Self.readOutput(from: output.fileHandleForReading, process: process)
                    .flatMap { try? JSONDecoder().decode(CLIResponse.self, from: $0) }
                let failure = response == nil ? Failure.downloadFailed : response?.failure
                await self?.finish(record, failure: failure, transferID: transferID, executable: executable)
            }
            fetchListing(for: record.id, share: share, executable: executable)
        } catch {
            onNotice?(L("无法启动百度网盘下载，请检查 bdpan 和本地保存位置。"))
        }
        publish()
    }

    private func fetchListing(for id: UUID, share: URL, executable: URL) {
        let process = Process()
        let output = Pipe()
        process.executableURL = executable
        process.arguments = ["transfer", "list", share.absoluteString, "--json", "--no-check-update"]
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do { try process.run() }
        catch { return }
        listingProcesses[id] = process
        Task.detached { [weak self] in
            let listing = Self.readOutput(from: output.fileHandleForReading, process: process)
                .flatMap { try? JSONDecoder().decode(ShareListing.self, from: $0) }
            await self?.receiveListing(listing, for: id, process: process)
        }
    }

    private func receiveListing(_ listing: ShareListing?, for id: UUID, process: Process) {
        guard listingProcesses[id] === process else { return }
        listingProcesses.removeValue(forKey: id)
        if let listing {
            listings[id] = listing
            onChange?()
        }
    }

    private func clearListing(_ id: UUID) {
        listings.removeValue(forKey: id)
        if let process = listingProcesses.removeValue(forKey: id), process.isRunning { process.terminate() }
    }

    private nonisolated static func readOutput(from output: FileHandle, process: Process) -> Data? {
        // Drain the pipe while bdpan runs. Never persist raw output from the CLI.
        let limit = 1024 * 1024
        var data = Data()
        while let chunk = try? output.read(upToCount: 16 * 1024), !chunk.isEmpty {
            if data.count <= limit { data.append(chunk) }
        }
        process.waitUntilExit()
        guard process.terminationReason == .exit, process.terminationStatus == 0, data.count <= limit else { return nil }
        return data
    }

    private func finish(_ record: Record, failure: Failure?, transferID: UUID, executable: URL) {
        processes.removeValue(forKey: record.id)
        progress.removeValue(forKey: record.id)
        let expectedBytes = listings[record.id]?.totalBytes
        clearListing(record.id)
        let staging = stage(record)
        guard let index = records.firstIndex(where: { $0.id == record.id }) else {
            try? FileManager.default.removeItem(at: staging)
            return
        }
        if failure == nil, let output = try? Self.commit(staging: staging, to: record.directory, expectedBytes: expectedBytes) {
            records[index].file = output.file
            records[index].bytes = output.bytes
            records[index].failure = nil
            shares.removeValue(forKey: record.id)
            if record.cleanCloudCopy == true {
                Task.detached { [weak self] in
                    guard !Self.cleanTransfer(transferID, executable: executable) else { return }
                    await self?.reportCleanupFailure(transferID)
                }
            }
        } else {
            let reason = failure ?? .downloadFailed
            records[index].failure = reason
            try? FileManager.default.removeItem(at: staging)
            onNotice?(reason.message)
        }
        publish()
    }

    private func reportCleanupFailure(_ id: UUID) {
        onNotice?(String(format: L("网盘转存目录可在「我的应用数据 → bdpan → %@」手动清理。"), Self.transferPath(id)))
    }

    private nonisolated static func cleanTransfer(_ id: UUID, executable: URL) -> Bool {
        let process = Process()
        let output = Pipe()
        process.executableURL = executable
        process.arguments = ["rm", transferPath(id), "--force", "--json", "--no-check-update"]
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do { try process.run() }
        catch { return false }
        let timeout = DispatchWorkItem { if process.isRunning { process.terminate() } }
        DispatchQueue.global().asyncAfter(deadline: .now() + 30, execute: timeout)
        defer { timeout.cancel() }
        guard let data = readOutput(from: output.fileHandleForReading, process: process),
              let result = try? JSONDecoder().decode(CLIResponse.self, from: data) else { return false }
        return result.code == 0
    }

    nonisolated static func commit(staging: URL, to directory: URL, expectedBytes: Int64? = nil) throws -> (file: URL, bytes: Int64) {
        let fm = FileManager.default
        let keys: Set<URLResourceKey> = [.isRegularFileKey, .fileSizeKey, .isSymbolicLinkKey]
        let children = try fm.contentsOfDirectory(at: staging, includingPropertiesForKeys: Array(keys))
        let files = fm.enumerator(at: staging, includingPropertiesForKeys: Array(keys))
        var count = 0
        var bytes: Int64 = 0
        while let entry = files?.nextObject() as? URL {
            let metadata = try entry.resourceValues(forKeys: keys)
            guard metadata.isSymbolicLink != true else { throw CocoaError(.fileReadInvalidFileName) }
            if metadata.isRegularFile == true {
                count += 1
                bytes += Int64(metadata.fileSize ?? 0)
            }
        }
        guard count > 0, !children.isEmpty, expectedBytes.map({ $0 == bytes }) ?? true else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let single = children.count == 1
        let source = single ? children[0] : staging
        let name = single ? source.lastPathComponent : L("百度网盘分享文件")
        let ext = single ? (name as NSString).pathExtension : ""
        let stem = ext.isEmpty ? name : (name as NSString).deletingPathExtension
        var target = directory.appendingPathComponent(name)
        var sequence = 2
        while true {
            do {
                try fm.moveItem(at: source, to: target)
                if single { try? fm.removeItem(at: staging) }
                return (target, bytes)
            } catch let error as NSError where error.domain == NSCocoaErrorDomain && error.code == NSFileWriteFileExistsError {
                let suffix = ext.isEmpty ? "" : "." + ext
                target = directory.appendingPathComponent("\(stem) (\(sequence))\(suffix)")
                sequence += 1
            }
        }
    }

    private func publish() {
        do { try PrivateFile.write(JSONEncoder().encode(records), to: storage) }
        catch { onNotice?(L("无法保存百度网盘下载记录")) }
        onChange?()
    }
}
