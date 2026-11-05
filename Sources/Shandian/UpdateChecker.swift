import AppKit
import CryptoKit

/// 检查更新与一键更新（沿用小清）：每天最多查一次 GitHub 最新正式版（不含预发布，Windows 的 `win-v…` 不算）。
/// 有新版时在 App 里下载本机芯片对应的安装包（显示进度）→ 对 GitHub 给的 SHA-256 → 核对 bundle id、版本号和签名 →
/// 旧版移到废纸篓、新版放回原处（没有写权限时走系统授权框） → 退出（任务先暂停）后重新打开，下载自动续上。
/// App 自己下载的文件不带隔离标记，Gatekeeper 不会拦新版
@MainActor
final class UpdateChecker: ObservableObject {
    struct Release: Equatable, Codable {
        let version: String
        let page: URL
        /// The zip to install; nil when the release has none (then the button opens the release page).
        let asset: URL?
        /// "sha256:…" from GitHub's asset listing.
        let digest: String?
        /// Bullets of the release's "更新内容" section.
        let notes: [String]
    }

    enum Phase: Equatable {
        case idle
        case checking
        case downloading(Double)
        case installing
    }

    @Published private(set) var available: Release?
    @Published private(set) var phase: Phase = .idle
    @Published var automatic = UserDefaults.standard.object(forKey: Keys.automatic) as? Bool ?? true {
        didSet {
            UserDefaults.standard.set(automatic, forKey: Keys.automatic)
            if automatic { checkIfDue() } else { available = nil }
        }
    }

    /// `SHANDIAN_UPDATE_FEED=<url>` stands in for GitHub's answer (a file:// JSON in tests of the install flow).
    private static var latestAPI: URL {
        ProcessInfo.processInfo.environment["SHANDIAN_UPDATE_FEED"].flatMap(URL.init(string:))
            ?? URL(string: "https://api.github.com/repos/chenmozhe008/shandian-download/releases/latest")!
    }

    private enum Keys {
        static let automatic = "updateAutoCheck"
        static let lastCheck = "updateLastCheck"
        static let latest = "updateLatestRelease"
    }

    /// On launch: use the answer from today's check, or ask GitHub if the last one is a day old.
    func checkIfDue() {
        // `SHANDIAN_PRETEND_UPDATE=9.9.9` shows the notice without a release (screenshots).
        if let pretend = ProcessInfo.processInfo.environment["SHANDIAN_PRETEND_UPDATE"] {
            available = Release(version: pretend, page: URL(string: "https://github.com/chenmozhe008/shandian-download/releases/latest")!,
                                asset: nil, digest: nil, notes: [])
            return
        }
        guard automatic, Bundle.main.bundleURL.pathExtension == "app" else { return }
        let defaults = UserDefaults.standard
        if let last = defaults.object(forKey: Keys.lastCheck) as? Date, Date().timeIntervalSince(last) < 86_400,
           ProcessInfo.processInfo.environment["SHANDIAN_UPDATE_FEED"] == nil {
            if let release = Self.usableCachedRelease(defaults.data(forKey: Keys.latest)) {
                show(release)
                return
            }
            // 仅有网页地址的降级结果不能缓存一天：网络恢复后需要重新获取安装包信息。
        }
        Task { await check() }
    }

    nonisolated static func usableCachedRelease(_ data: Data?) -> Release? {
        guard let data, let release = try? JSONDecoder().decode(Release.self, from: data), release.asset != nil else { return nil }
        return release
    }

    /// Asks GitHub now. Returns nil when offline or GitHub did not answer.
    @discardableResult
    func check() async -> Release? {
        var request = URLRequest(url: Self.latestAPI, timeoutInterval: 15)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        let release: Release
        if let (data, response) = try? await GitHubAPI.data(for: request),
           (response as? HTTPURLResponse)?.statusCode ?? 200 == 200,
           let parsed = Self.parse(data) {
            release = parsed
        } else if ProcessInfo.processInfo.environment["SHANDIAN_UPDATE_FEED"] == nil,
                  let fallback = await Self.checkReleasePage() {
            release = fallback
        } else { return nil }
        let defaults = UserDefaults.standard
        defaults.set(Date(), forKey: Keys.lastCheck)
        defaults.set(try? JSONEncoder().encode(release), forKey: Keys.latest)
        show(release)
        // `SHANDIAN_AUTO_INSTALL=1` (tests only, with SHANDIAN_UPDATE_FEED): install without the confirmation dialog.
        if ProcessInfo.processInfo.environment["SHANDIAN_AUTO_INSTALL"] != nil, let available { install(available) }
        return release
    }

    private nonisolated static func checkReleasePage() async -> Release? {
        let url = URL(string: "https://github.com/chenmozhe008/shandian-download/releases/latest")!
        var request = URLRequest(url: url, timeoutInterval: 15)
        request.httpMethod = "HEAD"
        guard let (_, response) = try? await URLSession.shared.data(for: request),
              let http = response as? HTTPURLResponse, http.statusCode == 200,
              let finalURL = http.url else { return nil }
        return releaseFromPage(finalURL)
    }

    nonisolated static func releaseFromPage(_ url: URL) -> Release? {
        guard url.scheme == "https", url.host == "github.com" else { return nil }
        let parts = url.pathComponents
        guard parts.count == 6, Array(parts[1...4]) == ["chenmozhe008", "shandian-download", "releases", "tag"] else { return nil }
        let tag = parts[5]
        let version = tag.hasPrefix("v") ? String(tag.dropFirst()) : tag
        guard !version.isEmpty, version.split(separator: ".", omittingEmptySubsequences: false).allSatisfy({ !$0.isEmpty && $0.allSatisfy(\.isNumber) }) else { return nil }
        // API 不通或被限流时只拿得到版本号；附件名固定，地址能直接拼出来，照样走一键更新（发布说明缺省、无 digest）
        let chip = isAppleSilicon ? "AppleSilicon" : "Intel"
        let asset = URL(string: "https://github.com/chenmozhe008/shandian-download/releases/download/\(tag)/Shandian-\(version)-\(chip).zip")
        return Release(version: version, page: url, asset: asset, digest: nil, notes: [])
    }

    nonisolated static func parse(_ data: Data) -> Release? {
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let tag = json["tag_name"] as? String,
              let page = (json["html_url"] as? String).flatMap(URL.init(string:)) else { return nil }
        let assets = json["assets"] as? [[String: Any]] ?? []
        // 按芯片分包：Shandian-<版本>-AppleSilicon.zip / -Intel.zip（Rosetta 下 uname 不准，看 hw.optional.arm64）
        let chip = isAppleSilicon ? "-AppleSilicon.zip" : "-Intel.zip"
        let zip = assets.first { ($0["name"] as? String)?.hasPrefix("Shandian-") == true && ($0["name"] as? String)?.hasSuffix(chip) == true }
        return Release(version: tag.hasPrefix("v") ? String(tag.dropFirst()) : tag, page: page,
                       asset: (zip?["browser_download_url"] as? String).flatMap(URL.init(string:)),
                       digest: zip?["digest"] as? String,
                       notes: notes(json["body"] as? String ?? ""))
    }

    /// The "- " lines under the release's "更新内容" heading.
    nonisolated static func notes(_ body: String) -> [String] {
        var inSection = false
        var lines: [String] = []
        for line in body.components(separatedBy: .newlines) {
            if line.hasPrefix("#") { inSection = line.contains("更新内容"); continue }
            if inSection, line.hasPrefix("- ") { lines.append(String(line.dropFirst(2))) }
        }
        return lines
    }

    /// The menu's 检查更新…: asks now and says what it found.
    func checkFromMenu() {
        Task {
            let release = await check()
            if let release, Self.isNewer(release.version, than: AppInfo.version) {
                offer(release)
            } else {
                let alert = NSAlert()
                alert.messageText = release == nil ? L("无法检查更新") : L("闪电全能下载已是最新版本")
                alert.informativeText = release == nil ? L("无法获取 GitHub 发布信息，可能是网络异常或请求次数受限，请稍后重试。") : String(localized: "当前版本 \(AppInfo.version)。")
                alert.runModal()
            }
        }
    }

    /// 「关于」右边的「立即升级」：点一下直接下载安装，不再弹确认框（11-04 用户定）。
    func startUpdate() {
        guard let available, phase == .idle else { return }
        if available.asset != nil { install(available); return }
        // 发布信息里没有本机芯片的安装包：重新查一次，发布已补齐时继续更新。
        phase = .checking
        Task {
            let refreshed = await check()
            phase = .idle
            if let refreshed, Self.isNewer(refreshed.version, than: AppInfo.version) {
                if refreshed.asset != nil { install(refreshed) } else { offer(refreshed) }
            } else if refreshed == nil {
                showRetryCheckAlert()
            }
        }
    }

    private func showRetryCheckAlert() {
        let alert = NSAlert()
        alert.messageText = L("暂时无法获取更新信息")
        alert.informativeText = L("请检查网络或代理设置，恢复连接后重试。")
        alert.addButton(withTitle: L("重试"))
        alert.addButton(withTitle: L("取消"))
        if alert.runModal() == .alertFirstButtonReturn { checkFromMenu() }
    }

    /// What is new, and one button that downloads, installs and reopens.
    private func offer(_ release: Release) {
        let alert = NSAlert()
        alert.messageText = String(localized: "新版本 \(release.version) 可用")
        var text = release.asset == nil ? String(localized: "当前版本 \(AppInfo.version)。暂时无法获取自动更新所需的安装包信息，请检查网络或代理设置后重试。") : String(localized: "当前版本 \(AppInfo.version)。下载完成后自动更新并重新打开；正在下载的任务会先暂停，重新打开后继续。")
        if !release.notes.isEmpty {
            text += "\n\n" + release.notes.prefix(6).map { "• " + $0 }.joined(separator: "\n")
        }
        alert.informativeText = text
        alert.addButton(withTitle: release.asset == nil ? L("重新检查") : L("更新并重新打开"))
        alert.addButton(withTitle: L("以后"))
        if release.asset == nil { alert.addButton(withTitle: L("前往下载页面")) }
        let response = alert.runModal()
        if response == .alertThirdButtonReturn, release.asset == nil { NSWorkspace.shared.open(release.page); return }
        guard response == .alertFirstButtonReturn else { return }
        if release.asset == nil { checkFromMenu() } else { install(release) }
    }

    // MARK: Installing

    func install(_ release: Release) {
        guard phase == .idle, let asset = release.asset else { return }
        phase = .downloading(0)
        Task {
            do {
                let zip = try await Self.download(asset) { fraction in
                    Task { @MainActor [weak self] in
                        if case .downloading = self?.phase { self?.phase = .downloading(fraction) }
                    }
                }
                phase = .installing
                let app = try await Task.detached { try Self.prepare(zip, release: release) }.value
                installPrepared(app)
            } catch {
                let wasDownloading: Bool
                if case .downloading = phase { wasDownloading = true } else { wasDownloading = false }
                phase = .idle
                let alert = NSAlert()
                alert.messageText = L("未能完成更新")
                let hint = wasDownloading ? L("请检查网络或代理设置后重试。") : L("更新文件无法使用，请重新下载后重试。")
                alert.informativeText = "\(error.localizedDescription)\n\n\(hint)"
                alert.addButton(withTitle: L("重试"))
                alert.addButton(withTitle: L("取消"))
                alert.addButton(withTitle: L("前往下载页面"))
                let response = alert.runModal()
                if response == .alertFirstButtonReturn { install(release) }
                else if response == .alertThirdButtonReturn { NSWorkspace.shared.open(release.page) }
            }
        }
    }

    /// 下载和校验完成后复用已解压的应用，安装失败不再让用户重复下载。
    private func installPrepared(_ app: URL) {
        do {
            phase = .installing
            try Self.swapAndRelaunch(with: app)
            NSApp.terminate(nil)
        } catch {
            phase = .idle
            let alert = NSAlert()
            alert.messageText = L("未能自动安装更新")
            guard FileManager.default.fileExists(atPath: app.path) else {
                alert.informativeText = error.localizedDescription
                alert.addButton(withTitle: L("好"))
                alert.runModal()
                return
            }
            alert.informativeText = String(localized: "\(error.localizedDescription)\n\n更新包已下载并校验。可以重试，或显示已下载的应用并手动替换；系统可能要求管理员授权。")
            alert.addButton(withTitle: L("重试安装"))
            alert.addButton(withTitle: L("显示已下载的应用"))
            alert.addButton(withTitle: L("取消"))
            let response = alert.runModal()
            if response == .alertFirstButtonReturn { installPrepared(app) }
            else if response == .alertSecondButtonReturn { NSWorkspace.shared.activateFileViewerSelecting([app]) }
        }
    }

    struct UpdateError: LocalizedError {
        let errorDescription: String?
        var permissionDenied = false
        init(_ message: String, permissionDenied: Bool = false) {
            errorDescription = message
            self.permissionDenied = permissionDenied
        }
    }

    private nonisolated static func download(_ url: URL, progress: @escaping @Sendable (Double) -> Void) async throws -> URL {
        let delegate = DownloadDelegate(progress: progress)
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 30
        let session = URLSession(configuration: configuration, delegate: delegate, delegateQueue: nil)
        defer { session.finishTasksAndInvalidate() }
        return try await withCheckedThrowingContinuation { continuation in
            delegate.continuation = continuation
            session.downloadTask(with: url).resume()
        }
    }

    /// Unpacks the download and makes sure it is the app it claims to be, ready to put in place.
    private nonisolated static func prepare(_ zip: URL, release: Release) throws -> URL {
        if let digest = release.digest, digest.hasPrefix("sha256:") {
            let data = try Data(contentsOf: zip, options: .mappedIfSafe)
            let hash = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
            guard hash == digest.dropFirst("sha256:".count).lowercased() else { throw UpdateError(L("下载的文件不完整或已损坏。")) }
        }
        let folder = FileManager.default.temporaryDirectory.appending(path: "shandian-update-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        guard run("/usr/bin/ditto", ["-x", "-k", zip.path, folder.path]) else { throw UpdateError(L("无法解压下载的文件。")) }
        let apps = (try? FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)) ?? []
        guard let app = apps.first(where: { $0.pathExtension == "app" }), let bundle = Bundle(url: app),
              bundle.bundleIdentifier == Bundle.main.bundleIdentifier,
              bundle.infoDictionary?["CFBundleShortVersionString"] as? String == release.version else {
            throw UpdateError(String(localized: "下载的文件不是闪电全能下载 \(release.version)。"))
        }
        guard run("/usr/bin/codesign", ["--verify", "--deep", "--strict", app.path]) else { throw UpdateError(L("新版本的签名校验未通过。")) }
        _ = run("/usr/bin/xattr", ["-dr", "com.apple.quarantine", app.path])
        return app
    }

    /// Moves the running app to the Trash (macOS allows this; the process keeps running), puts the new one in
    /// its place, and leaves a small script to open it once this process has quit. A failed move puts the old app back.
    /// 没有写权限（标准账号、App 属于别的用户）时弹系统授权框，由管理员密码完成替换，不让用户手动替换。
    private static func swapAndRelaunch(with app: URL) throws {
        let target = installTarget(for: Bundle.main.bundleURL)
        let relaunch = {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/bin/sh")
            process.arguments = ["-c", "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; open \"$2\"",
                                 "shandian-update", String(getpid()), target.path]
            try process.run()
        }
        let backupOld: () throws -> URL? = {
            guard FileManager.default.fileExists(atPath: target.path) else { return nil }
            var trashed: NSURL?
            try FileManager.default.trashItem(at: target, resultingItemURL: &trashed)
            guard let backup = trashed as URL? else {
                throw UpdateError(L("旧版已移入废纸篓，但无法确定其位置；请从废纸篓恢复旧版。"))
            }
            return backup
        }
        do {
            guard FileManager.default.isWritableFile(atPath: target.deletingLastPathComponent().path) else {
                throw UpdateError(String(localized: "没有权限写入「\(target.deletingLastPathComponent().path)」。"), permissionDenied: true)
            }
            try replaceApp(at: target, with: app, backup: backupOld, scheduleRelaunch: relaunch)
        } catch where isPermissionDenied(error) {
            try replaceAsAdministrator(target: target, with: app)
            try relaunch()
        }
    }

    /// 从「下载」等位置直接运行时系统把 App 放在只读的临时路径（App Translocation），替换不了也留不住，改装进「应用程序」。
    nonisolated static func installTarget(for bundle: URL) -> URL {
        guard bundle.path.contains("/AppTranslocation/") else { return bundle }
        return URL(fileURLWithPath: "/Applications", isDirectory: true).appendingPathComponent(bundle.lastPathComponent)
    }

    nonisolated static func isPermissionDenied(_ error: Error) -> Bool {
        if let error = error as? UpdateError { return error.permissionDenied }
        let ns = error as NSError
        if ns.domain == NSCocoaErrorDomain { return [NSFileWriteNoPermissionError, NSFileReadNoPermissionError].contains(ns.code) }
        return ns.domain == NSPOSIXErrorDomain && [Int(EPERM), Int(EACCES)].contains(ns.code)
    }

    /// 旧版移进废纸篓再放新版，放不进去就把旧版放回；路径单独加引号，可含空格、中文和单引号。
    nonisolated static func replaceCommand(target: URL, app: URL, backup: URL) -> String {
        let quote = { (path: String) in "'" + path.replacingOccurrences(of: "'", with: "'\\''") + "'" }
        let (t, a, b) = (quote(target.path), quote(app.path), quote(backup.path))
        return "if [ -e \(t) ]; then mv \(t) \(b) || exit 1; fi; mv \(a) \(t) || { [ -e \(b) ] && mv \(b) \(t); exit 1; }"
    }

    private static func replaceAsAdministrator(target: URL, with app: URL) throws {
        let backup = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".Trash", isDirectory: true)
            .appendingPathComponent("闪电全能下载-旧版-\(AppInfo.version)-\(UUID().uuidString.prefix(8)).app")
        let escape = { (text: String) in text.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") }
        let command = replaceCommand(target: target, app: app, backup: backup)
        let source = "do shell script \"\(escape(command))\" with prompt \"闪电全能下载需要授权才能安装新版本。\" with administrator privileges"
        var failure: NSDictionary?
        NSAppleScript(source: source)?.executeAndReturnError(&failure)
        guard let failure else { return }
        if failure[NSAppleScript.errorNumber] as? Int == -128 { throw UpdateError(L("已取消授权，闪电全能下载没有更改。")) }
        let reason = failure[NSAppleScript.errorMessage] as? String ?? L("未知错误")
        throw UpdateError(String(localized: "授权安装失败：\(reason)"))
    }

    /// 把备份和启动操作作为参数，以便在独立临时目录验证失败回滚。
    nonisolated static func replaceApp(at target: URL, with app: URL, backup: () throws -> URL?,
                                       scheduleRelaunch: () throws -> Void) throws {
        let backup = try backup()
        do {
            try FileManager.default.moveItem(at: app, to: target)
        } catch {
            let failure = error.localizedDescription
            let denied = isPermissionDenied(error)
            if let backup {
                do { try FileManager.default.moveItem(at: backup, to: target) }
                catch { throw UpdateError(L("无法替换应用，且自动恢复旧版失败。请从废纸篓恢复旧版。")) }
            }
            throw UpdateError(String(localized: "无法替换闪电全能下载，旧版已恢复：\(failure)"), permissionDenied: denied)
        }
        do {
            try scheduleRelaunch()
        } catch {
            // 重新启动助手未能启动时，恢复旧版；别报告失败却留下已替换的应用。
            do {
                try FileManager.default.moveItem(at: target, to: app)
                if let backup { try FileManager.default.moveItem(at: backup, to: target) }
            } catch {
                throw UpdateError(L("无法启动更新后的应用，且恢复旧版失败；请检查废纸篓中的旧版和更新文件。"))
            }
            throw UpdateError(L("无法启动更新后的应用，旧版已恢复。"))
        }
    }

    private nonisolated static func run(_ tool: String, _ arguments: [String]) -> Bool {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: tool)
        process.arguments = arguments
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        guard (try? process.run()) != nil else { return false }
        process.waitUntilExit()
        return process.terminationStatus == 0
    }

    nonisolated static var isAppleSilicon: Bool {
        var value: Int32 = 0
        var size = MemoryLayout<Int32>.size
        return sysctlbyname("hw.optional.arm64", &value, &size, nil, 0) == 0 && value == 1
    }

    private func show(_ release: Release) {
        available = automatic && Self.isNewer(release.version, than: AppInfo.version) ? release : nil
    }

    /// Numeric, part by part: 1.10.0 is newer than 1.9.2. A development build ("开发版") never asks.
    nonisolated static func isNewer(_ candidate: String, than current: String) -> Bool {
        let parse = { (value: String) in value.split(separator: ".").map { Int($0) } }
        let new = parse(candidate), old = parse(current)
        guard !new.contains(nil), !old.contains(nil), !old.isEmpty else { return false }
        for index in 0..<max(new.count, old.count) {
            let a = index < new.count ? new[index]! : 0
            let b = index < old.count ? old[index]! : 0
            if a != b { return a > b }
        }
        return false
    }
}

/// Reports download progress and hands back the finished file (moved out of URLSession's temporary spot,
/// which is deleted as soon as the callback returns).
private final class DownloadDelegate: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
    let progress: @Sendable (Double) -> Void
    var continuation: CheckedContinuation<URL, Error>?

    init(progress: @escaping @Sendable (Double) -> Void) { self.progress = progress }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64,
                    totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        if totalBytesExpectedToWrite > 0 { progress(Double(totalBytesWritten) / Double(totalBytesExpectedToWrite)) }
    }

    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
        if let status = (downloadTask.response as? HTTPURLResponse)?.statusCode, status != 200 {
            continuation?.resume(throwing: UpdateChecker.UpdateError(String(localized: "下载失败（HTTP \(status)）。")))
            continuation = nil
            return
        }
        let kept = FileManager.default.temporaryDirectory.appending(path: "shandian-update-\(UUID().uuidString).zip")
        do {
            try FileManager.default.moveItem(at: location, to: kept)
            continuation?.resume(returning: kept)
        } catch {
            continuation?.resume(throwing: error)
        }
        continuation = nil
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        guard let error else { return }
        continuation?.resume(throwing: UpdateChecker.UpdateError(String(localized: "下载中断：\(error.localizedDescription)")))
        continuation = nil
    }
}
