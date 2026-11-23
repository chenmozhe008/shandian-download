import CryptoKit
import Foundation

@MainActor
final class ArchiveDownloads {
    struct Entry: Sendable {
        let url: URL
        let name: String
        let headers: [String: String]
        let expectedBytes: Int64?
    }

    private enum State: String, Codable { case running, paused, packing, failed, done }

    private struct Record: Codable {
        var id = UUID()
        let signature: String
        let title: String
        let directory: URL
        let count: Int
        let expectedBytes: Int64
        var createdAt = Date()
        var state = State.running
        var files: [Int: Int64] = [:]
        var file: URL?
        var archiveBytes: Int64 = 0
        var error: String?
    }

    private struct Sample {
        let index: Int
        var time = Date()
        var bytes: Int64 = 0
        var speed: Int64 = 0
    }

    private let fileManager = FileManager.default
    private let storage: URL
    private var records: [Record]
    private var entries: [UUID: [Entry]] = [:]
    private var tasks: [UUID: Task<Void, Never>] = [:]
    private var transfers: [UUID: ArchiveTransfer] = [:]
    private var samples: [UUID: Sample] = [:]
    var onChange: (() -> Void)?
    var onNotice: ((String) -> Void)?

    init(directory: URL = EngineConfig.supportDir) {
        storage = directory.appendingPathComponent("archive-downloads.json")
        records = (try? JSONDecoder().decode([Record].self, from: Data(contentsOf: storage))) ?? []
        for index in records.indices where records[index].state == .running || records[index].state == .packing {
            records[index].state = .failed
            records[index].error = L("请在浏览器扩展中重新提交这批文件以继续下载")
        }
    }

    var items: [Item] {
        records.map { record in
            let sample = samples[record.id]
            let speed = record.state == .running
                ? sample.flatMap { Date().timeIntervalSince($0.time) < 3 ? $0.speed : nil } ?? 0
                : 0
            let done = record.state == .done
            let status: Item.Status = switch record.state {
            case .running, .packing: .running
            case .paused: .paused
            case .failed: .failed(record.error ?? L("下载失败"))
            case .done: .done
            }
            var item = Item(id: record.id.uuidString, source: .archive(record.id),
                            title: record.file?.lastPathComponent ?? record.title, rawTitle: record.title,
                            origin: "网页打包下载", category: .other, status: status,
                            downloaded: done ? record.archiveBytes : record.files.values.reduce(0, +) + (sample?.bytes ?? 0),
                            total: done ? record.archiveBytes : record.expectedBytes, speed: speed,
                            file: record.file, createdAt: record.createdAt, thumbRemote: nil, linkKey: "")
            item.avgSpeed = speed
            item.archiveProgress = .init(saved: record.files.count, total: record.count, packing: record.state == .packing)
            return item
        }
    }

    func start(name: String, directory: URL, entries incoming: [Entry]) {
        guard !incoming.isEmpty else { return }
        let title = Self.safeName(name) ?? L("音频合集")
        let archive = title.lowercased().hasSuffix(".zip") ? title : title + ".zip"
        // 用摘要找回同一批任务。请求头和源链接只保留在内存中。
        let key = incoming.map { "\($0.url.absoluteString)\n\($0.name)\n\($0.expectedBytes ?? 0)" }.joined(separator: "\n")
        let signature = SHA256.hash(data: Data(key.utf8)).map { String(format: "%02x", $0) }.joined()
        if let record = records.first(where: { $0.signature == signature && $0.title == archive && $0.state != .done }) {
            entries[record.id] = incoming
            if record.state != .packing { resume(record.id) }
            return
        }
        let totalBytes: Int64 = incoming.allSatisfy { $0.expectedBytes != nil }
            ? incoming.compactMap(\.expectedBytes).reduce(0) { sum, size in
                sum > Int64.max - size ? Int64.max : sum + size
            } : 0
        let record = Record(signature: signature, title: archive, directory: directory,
                            count: incoming.count, expectedBytes: totalBytes)
        records.append(record)
        entries[record.id] = incoming
        publish()
        run(record.id)
    }

    func pause(_ id: UUID) {
        guard let index = records.firstIndex(where: { $0.id == id && $0.state == .running }) else { return }
        records[index].state = .paused
        transfers[id]?.setPaused(true)
        publish()
    }

    func resume(_ id: UUID) {
        guard let index = records.firstIndex(where: { $0.id == id && $0.state != .done && $0.state != .packing }) else { return }
        guard entries[id] != nil else {
            onNotice?(L("请在浏览器扩展中重新提交这批文件以继续下载"))
            return
        }
        records[index].state = .running
        records[index].error = nil
        if var sample = samples[id] {
            sample.time = Date()
            sample.speed = 0
            samples[id] = sample
        }
        transfers[id]?.setPaused(false)
        publish()
        run(id)
    }

    func remove(_ id: UUID) {
        let record = records.first(where: { $0.id == id })
        records.removeAll { $0.id == id }
        entries.removeValue(forKey: id)
        samples.removeValue(forKey: id)
        if let task = tasks[id] { task.cancel() }
        else if let record {
            try? fileManager.removeItem(at: staging(id))
            try? fileManager.removeItem(at: stagingArchive(record))
        }
        publish()
    }

    private func run(_ id: UUID) {
        guard tasks[id] == nil, let source = entries[id] else { return }
        tasks[id] = Task { [self] in
            await download(id, entries: source)
            tasks.removeValue(forKey: id)
            // 继续操作可能发生在上一轮退出前，始终只让一个任务写入同一批文件。
            if records.contains(where: { $0.id == id && $0.state == .running }) { run(id) }
            if !records.contains(where: { $0.id == id }) { try? fileManager.removeItem(at: staging(id)) }
        }
    }

    private func download(_ id: UUID, entries: [Entry]) async {
        guard let record = records.first(where: { $0.id == id }) else { return }
        let stage = staging(id)
        do {
            try fileManager.createDirectory(at: stage, withIntermediateDirectories: true,
                                            attributes: [.posixPermissions: 0o700])
            let names = Self.entryNames(entries)
            for (index, entry) in entries.enumerated() {
                try Task.checkCancellation()
                guard let live = records.first(where: { $0.id == id && $0.state == .running }) else { return }
                let target = stage.appendingPathComponent(names[index])
                if let size = live.files[index], Self.fileSize(target) == size { continue }
                var request = URLRequest(url: entry.url, timeoutInterval: 60)
                for (key, value) in entry.headers { request.setValue(value, forHTTPHeaderField: key) }
                samples[id] = Sample(index: index)
                let transfer = ArchiveTransfer { [weak self] bytes in
                    Task { @MainActor in self?.updateProgress(id, index: index, bytes: bytes) }
                }
                transfers[id] = transfer
                defer { transfers.removeValue(forKey: id) }
                let (temporary, response) = try await transfer.download(request)
                defer { try? fileManager.removeItem(at: temporary) }
                try Task.checkCancellation()
                guard let http = response as? HTTPURLResponse, http.statusCode == 200,
                      let size = Self.fileSize(temporary), size > 0,
                      !["application/json", "text/html"].contains(http.mimeType?.lowercased() ?? ""),
                      entry.expectedBytes.map({ $0 == size }) ?? true else { throw Failure.invalidFile }
                if fileManager.fileExists(atPath: target.path) { try fileManager.removeItem(at: target) }
                try fileManager.moveItem(at: temporary, to: target)
                guard let row = records.firstIndex(where: { $0.id == id }) else { return }
                records[row].files[index] = size
                samples.removeValue(forKey: id)
                publish()
            }
            try Task.checkCancellation()
            guard records.contains(where: { $0.id == id && $0.state == .running }) else { return }
            try await pack(id, stage: stage, record: record)
        } catch {
            samples.removeValue(forKey: id)
            guard !Task.isCancelled, let index = records.firstIndex(where: { $0.id == id }) else { return }
            records[index].state = .failed
            records[index].error = error.localizedDescription
            publish()
            onNotice?(String(localized: "打包下载失败：\(error.localizedDescription)"))
        }
    }

    private func pack(_ id: UUID, stage: URL, record: Record) async throws {
        guard let index = records.firstIndex(where: { $0.id == id }) else { return }
        records[index].state = .packing
        publish()
        try fileManager.createDirectory(at: record.directory, withIntermediateDirectories: true)
        let temporary = stagingArchive(record)
        if fileManager.fileExists(atPath: temporary.path) { try fileManager.removeItem(at: temporary) }
        defer { try? fileManager.removeItem(at: temporary) }
        try await Task.detached(priority: .utility) {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
            process.arguments = ["-c", "-k", "--sequesterRsrc", stage.path, temporary.path]
            try process.run()
            process.waitUntilExit()
            guard process.terminationStatus == 0 else { throw Failure.pack }
        }.value
        try Task.checkCancellation()
        guard let latest = records.firstIndex(where: { $0.id == id }),
              let bytes = Self.fileSize(temporary), bytes > 0 else { throw Failure.pack }
        let stem = (record.title as NSString).deletingPathExtension
        var destination = record.directory.appendingPathComponent(record.title)
        var copy = 2
        while fileManager.fileExists(atPath: destination.path) {
            destination = record.directory.appendingPathComponent("\(stem) (\(copy)).zip")
            copy += 1
        }
        try fileManager.moveItem(at: temporary, to: destination)
        records[latest].state = .done
        records[latest].file = destination
        records[latest].archiveBytes = bytes
        entries.removeValue(forKey: id)
        try? fileManager.removeItem(at: stage)
        publish()
    }

    private func updateProgress(_ id: UUID, index: Int, bytes: Int64) {
        guard records.contains(where: { $0.id == id && $0.state == .running }),
              var sample = samples[id], sample.index == index, bytes >= sample.bytes else { return }
        let now = Date()
        let seconds = now.timeIntervalSince(sample.time)
        guard seconds >= 0.4 else { return }
        let measured = Int64(Double(bytes - sample.bytes) / seconds)
        sample.speed = sample.speed == 0 ? measured : (sample.speed + measured) / 2
        sample.bytes = bytes
        sample.time = now
        samples[id] = sample
        onChange?()
    }

    private func publish() {
        do {
            let data = try JSONEncoder().encode(records)
            try PrivateFile.write(data, to: storage)
        } catch {
            onNotice?(L("无法保存打包下载记录"))
        }
        onChange?()
    }

    private func staging(_ id: UUID) -> URL {
        storage.deletingLastPathComponent().appendingPathComponent("archive-parts-\(id.uuidString)", isDirectory: true)
    }

    private func stagingArchive(_ record: Record) -> URL {
        record.directory.appendingPathComponent(".shandian-\(record.id.uuidString).partial.zip")
    }

    private static func fileSize(_ url: URL) -> Int64? {
        (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize).map(Int64.init)
    }

    private static func safeName(_ value: String) -> String? {
        let clean = value.replacingOccurrences(of: #"[\\/:*?"<>|\p{Cc}]"#, with: " ", options: .regularExpression)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return clean.isEmpty || clean == "." || clean == ".." ? nil : clean
    }

    private static func entryNames(_ entries: [Entry]) -> [String] {
        var names = Set<String>()
        return entries.enumerated().map { index, entry in
            let raw = safeName(entry.name) ?? safeName(entry.url.lastPathComponent) ?? "file-\(index + 1)"
            let ext = (raw as NSString).pathExtension
            let stem = ext.isEmpty ? raw : (raw as NSString).deletingPathExtension
            var candidate = raw
            var copy = 2
            while !names.insert(candidate.lowercased()).inserted {
                candidate = ext.isEmpty ? "\(stem) (\(copy))" : "\(stem) (\(copy)).\(ext)"
                copy += 1
            }
            return candidate
        }
    }

    private enum Failure: LocalizedError {
        case invalidFile, pack
        var errorDescription: String? {
            switch self {
            case .invalidFile: L("原文件读取失败或大小不符")
            case .pack: L("无法生成压缩包")
            }
        }
    }
}

// URLSession's async download overload uses its own completion handler. A regular
// download task lets the same transfer report byte progress and be cancelled.
private final class ArchiveTransfer: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
    private let onProgress: @Sendable (Int64) -> Void
    private let lock = NSLock()
    private var task: URLSessionDownloadTask?
    private var completion: CheckedContinuation<(URL, URLResponse), Error>?
    private var cancelled = false
    private var paused = false
    // These are assigned only on the session's serial delegate queue.
    private var file: URL?
    private var fileError: Error?

    init(onProgress: @escaping @Sendable (Int64) -> Void) { self.onProgress = onProgress }

    func download(_ request: URLRequest) async throws -> (URL, URLResponse) {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                let session = URLSession(configuration: .ephemeral, delegate: self, delegateQueue: nil)
                let transfer = session.downloadTask(with: request)
                lock.withLock {
                    completion = continuation
                    task = transfer
                    if cancelled { transfer.cancel() }
                    else if !paused { transfer.resume() }
                }
            }
        } onCancel: {
            lock.withLock {
                cancelled = true
                task?.cancel()
            }
        }
    }

    func setPaused(_ value: Bool) {
        lock.withLock {
            guard !cancelled, paused != value else { return }
            paused = value
            if value { task?.suspend() }
            else { task?.resume() }
        }
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64,
                    totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        onProgress(totalBytesWritten)
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest: URLRequest, completionHandler: @escaping @Sendable (URLRequest?) -> Void) {
        var request = newRequest
        let from = response.url
        let to = request.url
        if from?.scheme != to?.scheme || from?.host != to?.host || from?.port != to?.port {
            request.setValue(nil, forHTTPHeaderField: "Cookie")
            request.setValue(nil, forHTTPHeaderField: "Authorization")
        }
        completionHandler(request)
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
        let retainedFile = FileManager.default.temporaryDirectory.appendingPathComponent("shandian-archive-\(UUID().uuidString)")
        do {
            try FileManager.default.moveItem(at: location, to: retainedFile)
            file = retainedFile
        } catch { fileError = error }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        let continuation = lock.withLock {
            let pending = completion
            completion = nil
            self.task = nil
            return pending
        }
        session.finishTasksAndInvalidate()
        if let error = error ?? fileError {
            if let file { try? FileManager.default.removeItem(at: file) }
            continuation?.resume(throwing: error)
        } else if let file, let response = task.response {
            continuation?.resume(returning: (file, response))
        } else {
            if let file { try? FileManager.default.removeItem(at: file) }
            continuation?.resume(throwing: URLError(.badServerResponse))
        }
    }
}
