import AppKit
import IOKit.pwr_mgt
import ServiceManagement
import SwiftUI

/// 偏好设置里的开关（UserDefaults 键集中在这里）
enum Prefs {
    static let maxRunningKey = "maxRunning"
    static let preventSleepKey = "preventSleep"
    static let notifyKey = "notifyOnFinish"
    static let downloadLimitKey = "downloadLimit"   // KB/s，0 不限
    static let autoThunderKey = "autoThunder"
    static var autoThunder: Bool { UserDefaults.standard.object(forKey: autoThunderKey) as? Bool ?? false }   // 10-28 用户：不想用迅雷
    static let autoCloudKey = "autoCloud"
    static var autoCloud: Bool { UserDefaults.standard.object(forKey: autoCloudKey) as? Bool ?? true }   // 11-04 用户定：下不动就自动转，提示里写清用了几次
    static let autoCleanCloudKey = "autoCleanCloud"
    static var autoCleanCloud: Bool { UserDefaults.standard.object(forKey: autoCleanCloudKey) as? Bool ?? true }   // 免费空间小，拉回后默认清理才能循环用
    static let scheduleOnKey = "scheduleOn", scheduleFromKey = "scheduleFrom", scheduleToKey = "scheduleTo"
    static var scheduleOn: Bool { UserDefaults.standard.bool(forKey: scheduleOnKey) }
    static var scheduleFrom: Int { UserDefaults.standard.object(forKey: scheduleFromKey) as? Int ?? 1 }
    static var scheduleTo: Int { UserDefaults.standard.object(forKey: scheduleToKey) as? Int ?? 8 }
    static var downloadLimit: Int { UserDefaults.standard.integer(forKey: downloadLimitKey) }

    static var maxRunning: Int { let v = UserDefaults.standard.integer(forKey: maxRunningKey); return v > 0 ? v : 5 }
    static var preventSleep: Bool { UserDefaults.standard.object(forKey: preventSleepKey) as? Bool ?? true }
    static var notify: Bool { UserDefaults.standard.object(forKey: notifyKey) as? Bool ?? true }
    static let doneSoundKey = "doneSound"
    static var doneSound: Bool { UserDefaults.standard.object(forKey: doneSoundKey) as? Bool ?? true }
}

/// 有任务在下时不让 Mac 因闲置睡眠（合盖、手动睡眠照常）；没在下就放开
@MainActor
final class SleepGuard {
    private var assertion: IOPMAssertionID = 0
    private var held = false

    func update(downloading: Bool) {
        let want = downloading && Prefs.preventSleep
        if want, !held {
            held = IOPMAssertionCreateWithName(kIOPMAssertionTypePreventUserIdleSystemSleep as CFString,
                                               IOPMAssertionLevel(kIOPMAssertionLevelOn),
                                               "闪电全能下载正在下载" as CFString, &assertion) == kIOReturnSuccess
        } else if !want, held {
            IOPMAssertionRelease(assertion)
            held = false
        }
    }
}

private enum SettingsTab { case general, download, locations, acceleration, browser, ai }

struct SettingsView: View {
    @ObservedObject var store: DownloadStore
    @AppStorage(Prefs.maxRunningKey) private var maxRunning = 5
    @AppStorage(Prefs.preventSleepKey) private var preventSleep = true
    @AppStorage(Prefs.notifyKey) private var notify = true
    @AppStorage(Prefs.doneSoundKey) private var doneSound = true
    @AppStorage(Prefs.downloadLimitKey) private var limit = 0
    @AppStorage(Prefs.autoCloudKey) private var autoCloud = true
    @State private var offlinesToday: Int?
    @AppStorage(Prefs.autoCleanCloudKey) private var autoCleanCloud = true
    @AppStorage(Prefs.scheduleOnKey) private var scheduleOn = false
    @AppStorage(Prefs.scheduleFromKey) private var scheduleFrom = 1
    @AppStorage(Prefs.scheduleToKey) private var scheduleTo = 8
    @AppStorage(DownloadStore.rescuedKey) private var rescuedCount = 0
    @State private var pkState = PikPak.loggedInUser
    @State private var pkProblem = UserDefaults.standard.bool(forKey: "pikpakSessionProblem")
    @Environment(\.openWindow) private var openWindow
    @AppStorage(ClipboardWatcher.enabledKey) private var watchClipboard = true
    @AppStorage(NewTaskPanels.askKey) private var confirmNew = true
    @State private var launchAtLogin = SMAppService.mainApp.status == .enabled
    @State private var tab = SettingsTab.general
    @State private var language = AppLanguage.current
    @State private var aiTool = AITool.claudeCode
    @State private var aiCopied = false
    @State private var dirsVersion = 0   // 改了位置后刷新显示

    var body: some View {
        TabView(selection: $tab) {
            Form {
            Section {
                Picker("语言", selection: $language) {
                    ForEach(AppLanguage.allCases) { Text(verbatim: $0.label).tag($0) }
                }
                .onChange(of: language) { _, new in new.save(); AppLanguage.askToReopen() }
            }
            Section("系统") {
                Toggle("下载时不让 Mac 进入睡眠", isOn: $preventSleep)
                    .help("只阻止闲置睡眠；合上盖子或手动睡眠照常")
                Toggle("下载完成时通知", isOn: $notify)
                Toggle("下载完成时播放提示音", isOn: $doneSound)
                Toggle("登录时自动打开闪电全能下载", isOn: $launchAtLogin)
                    .onChange(of: launchAtLogin) { _, on in
                        do { if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() } }
                        catch { launchAtLogin = SMAppService.mainApp.status == .enabled }
                    }
                UpdateToggle(checker: store.updates)
            }
            }
            .formStyle(.grouped)
            .tabItem { Label("通用", systemImage: "gearshape") }
            .tag(SettingsTab.general)

            Form {
            Section("下载") {
                Picker("同时下载", selection: $maxRunning) {
                    ForEach([1, 2, 3, 5, 8, 10], id: \.self) { Text("\($0) 个任务").tag($0) }
                }
                .onChange(of: maxRunning) { store.reapplyEngineSettings() }
                Picker("下载限速", selection: $limit) {
                    Text("不限速").tag(0)
                    ForEach([512, 1024, 2048, 5120, 10240], id: \.self) { kb in
                        Text(kb < 1024 ? "\(kb) KB/s" : "\(kb / 1024) MB/s").tag(kb)
                    }
                }
                .onChange(of: limit) { store.reapplyEngineSettings() }
                Toggle("新建任务时显示确认框", isOn: $confirmNew)
                Toggle("复制链接后弹出下载提示", isOn: $watchClipboard)
                    .onChange(of: watchClipboard) { if !watchClipboard { store.clipboardLinks = nil } }
                Text("仅提示磁力、视频页面和常见文件链接；不会自动开始下载。关闭后仍可手动粘贴。")
                    .font(.caption).foregroundStyle(.secondary)
            }

            Section("定时下载") {
                Toggle("只在指定时段下载", isOn: $scheduleOn)
                    .onChange(of: scheduleOn) { store.applyDownloadSchedule() }
                if scheduleOn {
                    Picker("开始时间", selection: $scheduleFrom) {
                        ForEach(0..<24, id: \.self) { Text(String(format: "%02d:00", $0)).tag($0) }
                    }
                    .onChange(of: scheduleFrom) { store.applyDownloadSchedule() }
                    Picker("结束时间", selection: $scheduleTo) {
                        ForEach(0..<24, id: \.self) { Text(String(format: "%02d:00", $0)).tag($0) }
                    }
                    .onChange(of: scheduleTo) { store.applyDownloadSchedule() }
                }
                Text("时段外自动暂停，到点自动继续；时段外手动继续的任务不会被再次暂停。闪电全能下载需要开着。迅雷和视频网站的任务不受影响。")
                    .font(.system(size: 12)).foregroundStyle(Theme.sub).fixedSize(horizontal: false, vertical: true)
            }
            }
            .formStyle(.grouped)
            .tabItem { Label("下载", systemImage: "arrow.down.circle") }
            .tag(SettingsTab.download)

            Form {            Section("保存位置") {
                dirRow(L("磁力、种子"), key: "dir.movie", current: Folders.movies)
                dirRow(L("视频网站"), key: "dir.video", current: Folders.videos)
                dirRow(L("其他文件"), key: "dir.other", current: Folders.others)
            }
            .id(dirsVersion)

            }
            .formStyle(.grouped)
            .tabItem { Label("保存位置", systemImage: "folder") }
            .tag(SettingsTab.locations)


            Form {
            if rescuedCount > 0 {
                Section {
                    LabeledContent("已换路救回", value: String(localized: "\(rescuedCount) 条任务"))
                }
            }
            ThunderSettingsSection()

            Section("云盘离线（PikPak）") {
                Text("PikPak 需要科学上网（连接境外网络）才能使用。")
                    .font(.system(size: 12)).foregroundStyle(Theme.sub)
                Toggle("拉回后自动清理云端副本", isOn: $autoCleanCloud)
                Text("PikPak 只作中转：取回本机并核对后彻底删除云端副本，不进回收站。云盘空间不够时，也会清理回收站里离线任务留下的文件。")
                    .font(.system(size: 12)).foregroundStyle(Theme.sub)
                Button(store.cloudCleanupCount > 0 ? String(localized: "清理已下载的云端副本（\(store.cloudCleanupCount)）") : L("打开 PikPak 云盘管理空间")) {
                    store.manageCloudCopies()
                }
                if let u = pkState {
                    LabeledContent(pkProblem ? L("关联需检查") : L("已关联")) {
                        HStack { Text(L(u)).foregroundStyle(Theme.sub); Button("退出") { Task { await PikPak.shared.logout(); pkState = nil } } }
                    }
                    Text(pkProblem ? L("会话刷新失败，请检查网络或重新关联账号。") : L("已保存账号关联，实际登录状态会在使用时验证。"))
                        .font(.system(size: 12)).foregroundStyle(Theme.sub)
                    HStack {
                        Button("在窗口里登录…") { openWindow(id: "pikpak-login"); NSApp.activate(ignoringOtherApps: true) }
                        Button("从浏览器重新关联") { linkPikPakFromBrowser() }
                    }
                    Text("浏览器已登录 PikPak 时可直接重新关联，需安装闪电全能下载扩展。关联后浏览器中的 PikPak 可能退出登录。")
                        .font(.system(size: 12)).foregroundStyle(Theme.sub).fixedSize(horizontal: false, vertical: true)
                    Toggle("下不动时自动交给云盘离线", isOn: $autoCloud)
                        .help("BT 网络里几乎没人传的资源，交给 PikPak 服务器下好，再从云盘满速拉回")
                    Text("磁力、种子 1 分钟没速度，或 3 分钟几乎没速度时转交，每次用掉一次离线。").font(.system(size: 12)).foregroundStyle(Theme.sub)
                    LabeledContent("今天已用") {
                        Text(offlinesToday.map { String(localized: "\($0) 次（免费约 \(PikPak.freeDailyOfflines) 次）") } ?? L("读取中")).foregroundStyle(Theme.sub)
                    }
                    .task { offlinesToday = try? await PikPak.shared.offlinesUsedToday() }
                } else {
                    LabeledContent("PikPak 账号") {
                        HStack {
                            Button("在窗口里登录…") { openWindow(id: "pikpak-login"); NSApp.activate(ignoringOtherApps: true) }
                                .buttonStyle(.borderedProminent)
                            Button("从浏览器关联") { linkPikPakFromBrowser() }
                        }
                    }
                    Text("浏览器里已登录 PikPak 时点「从浏览器关联」即可（需装好闪电全能下载扩展）；关联后浏览器里的 PikPak 网页会退出登录，因为同一份登录不能两边同时用。")
                        .font(.system(size: 12)).foregroundStyle(Theme.sub).fixedSize(horizontal: false, vertical: true)
                }
            }

            }
            .formStyle(.grouped)
            .tabItem { Label("下载加速", systemImage: "bolt") }
            .tag(SettingsTab.acceleration)

            ScrollView { BrowserSetupView(token: store.engineToken) }
                .tabItem { Label("浏览器扩展", systemImage: "puzzlepiece.extension") }
                .tag(SettingsTab.browser)

            Form {
            Section("给 AI 工具用（MCP）") {
                Text("让 AI 工具提交下载、查看进度、暂停和继续。只在本机运行，不能删除任务或文件。")
                    .font(.system(size: 12)).foregroundStyle(Theme.sub).fixedSize(horizontal: false, vertical: true)
                Picker("工具", selection: $aiTool) {
                    ForEach(AITool.allCases) { Text(verbatim: $0.name).tag($0) }
                }
                .onChange(of: aiTool) { _, _ in aiCopied = false }
                Text(verbatim: aiTool.hint)
                    .font(.system(size: 12)).foregroundStyle(Theme.sub).fixedSize(horizontal: false, vertical: true)
                if !aiTool.isCommand && !aiTool.isPath {
                    Text("已有其他配置时，把 shandian 这一段并进去，不要整份覆盖。")
                        .font(.system(size: 12)).foregroundStyle(Theme.sub).fixedSize(horizontal: false, vertical: true)
                }
                Button(aiCopied ? L("已复制") : aiTool.isCommand ? L("复制命令") : aiTool.isPath ? L("复制路径") : L("复制配置")) {
                    AppInfo.copy(aiTool.snippet(path: Self.mcpPath)); aiCopied = true
                }
            }
            }
            .formStyle(.grouped)
            .tabItem { Label("AI 工具", systemImage: "sparkles") }
            .tag(SettingsTab.ai)
        }
        .onReceive(NotificationCenter.default.publisher(for: PikPak.loginChanged)) { _ in pkState = PikPak.loggedInUser; pkProblem = UserDefaults.standard.bool(forKey: "pikpakSessionProblem") }
        .onAppear { tab = .general; ThunderBridge.recheck() }   // 每次打开都从「通用」开始（11-05 用户定）
        .frame(width: 708, height: 640)   // 所有页同样大，切换时窗口不跳（11-05 用户定）；以内容最多的浏览器扩展页为准
        .tint(Theme.tint)
    }

    private static var mcpPath: String { Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/shandian-mcp").path }

    private func linkPikPakFromBrowser() {
        let url = URL(string: "https://mypikpak.com/drive/#shandian-link")!
        for app in ["/Applications/Google Chrome.app", "/Applications/Microsoft Edge.app"] where FileManager.default.fileExists(atPath: app) {
            NSWorkspace.shared.open([url], withApplicationAt: URL(fileURLWithPath: app), configuration: NSWorkspace.OpenConfiguration(), completionHandler: nil)
        }
    }

    private func dirRow(_ title: String, key: String, current: URL) -> some View {        LabeledContent(title) {
            HStack(spacing: 8) {
                Text(Folders.displayName(current)).foregroundStyle(Theme.sub).lineLimit(1).truncationMode(.middle)
                    .help(current.path)
                Button("更改…") {
                    let p = NSOpenPanel()
                    p.canChooseDirectories = true; p.canChooseFiles = false; p.canCreateDirectories = true
                    p.directoryURL = current; p.prompt = L("选择")
                    if p.runModal() == .OK, let u = p.url { UserDefaults.standard.set(u.path, forKey: key); dirsVersion += 1 }
                }
                if UserDefaults.standard.string(forKey: key) != nil {
                    Button("恢复默认") { UserDefaults.standard.removeObject(forKey: key); dirsVersion += 1 }
                }
            }
        }
    }
}

/// 「自动检查更新」：单独观察 UpdateChecker，关掉时侧栏的新版本提示立即消失
private struct UpdateToggle: View {
    @ObservedObject var checker: UpdateChecker
    var body: some View {
        Toggle("自动检查更新", isOn: $checker.automatic)
            .help("每天最多访问一次 GitHub 查询最新版本，不上传任何数据")
    }
}

/// 迅雷是第三方软件、可选：装了能把任务交给它下，没装也不用藏开关——
/// 说清「不装也能下」，给出官网，装完点重新检测就能用（11-01 用户反馈：别人电脑上没装迅雷怎么办）
struct ThunderSettingsSection: View {
    @AppStorage(Prefs.autoThunderKey) private var autoThunder = false
    @State private var installed = ThunderBridge.installed

    var body: some View {
        Section("迅雷（可选）") {
            if installed {
                Toggle("磁力、种子太慢时交给迅雷（没登录云盘时）", isOn: $autoThunder)
                    .help("连续 3 分钟几乎没速度时，用本机的迅雷接着下，进度仍显示在闪电全能下载里")
                Text("已检测到本机的迅雷。单个任务也可以在右键菜单里选「用迅雷下载」。")
                    .font(.system(size: 12)).foregroundStyle(Theme.sub).fixedSize(horizontal: false, vertical: true)
            } else {
                LabeledContent("未检测到迅雷") {
                    HStack(spacing: 8) {
                        Button("去官网下载") { ThunderBridge.openOfficialSite() }
                        Button("重新检测") { ThunderBridge.recheck(); installed = ThunderBridge.installed }
                    }
                }
                // 迅雷被卸载/移走后，之前开着的自动接管还留在偏好里；这里留个开关，别让人没法关
                if autoThunder {
                    Toggle("装上迅雷后自动接管太慢的任务（当前已开启）", isOn: $autoThunder)
                        .help("没装迅雷时不会真的交出去，任务仍由闪电全能下载下")
                }
                Text("迅雷是第三方下载工具，不装也能正常下磁力、种子和直链。装上后可以把任务交给它下，国内冷门资源通常更快；装好点「重新检测」即可。")
                    .font(.system(size: 12)).foregroundStyle(Theme.sub).fixedSize(horizontal: false, vertical: true)
            }
        }
        .onAppear { ThunderBridge.recheck(); installed = ThunderBridge.installed }
    }
}
