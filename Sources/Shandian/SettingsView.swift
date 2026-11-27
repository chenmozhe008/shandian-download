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
    static var autoThunder: Bool { UserDefaults.standard.object(forKey: autoThunderKey) as? Bool ?? true }   // 11-05 用户定：没关联云盘或云盘连不上时，下不动就自动交给迅雷（没装迅雷不会交）
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

    /// 记住上次的保存位置（11-15 用户定）：开着时新建任务直接从上次存的地方开始，不用每次点。
    /// 磁力/种子、视频网站、其他各记一份，免得拿电影的位置去存网页上的小文件
    static let rememberDirKey = "rememberLastDir"
    static var rememberDir: Bool { UserDefaults.standard.object(forKey: rememberDirKey) as? Bool ?? true }
    static func lastDirKey(_ kind: LinkKind?) -> String {
        switch kind {
        case .magnet, .torrentFile, .ed2k: "lastDir.movie"
        case .videoSite: "lastDir.video"
        default: "lastDir.other"
        }
    }
    static func lastDir(_ kind: LinkKind?) -> URL? {
        guard rememberDir, let path = UserDefaults.standard.string(forKey: lastDirKey(kind)),
              FileManager.default.fileExists(atPath: path) else { return nil }
        return URL(fileURLWithPath: path)
    }
    static func setLastDir(_ url: URL, kind: LinkKind?) {
        UserDefaults.standard.set(url.path, forKey: lastDirKey(kind))
    }
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
    @AppStorage(Prefs.rememberDirKey) private var rememberDir = true
    @State private var launchAtLogin = LoginLaunch.isEnabled
    @State private var ownsLinks = DefaultHandler.isDefault
    @State private var axAllowed = ThunderBridge.accessibilityGranted
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
                        do { try LoginLaunch.set(on) }
                        catch { launchAtLogin = LoginLaunch.isEnabled }
                    }
                LabeledContent("磁力链接和种子文件") {
                    if ownsLinks {
                        Text("用闪电全能下载打开").foregroundStyle(.secondary)
                    } else {
                        Button("设为默认") { Task { await DefaultHandler.makeDefault(); ownsLinks = DefaultHandler.isDefault } }
                    }
                }
                .onAppear { ownsLinks = DefaultHandler.isDefault }
                LabeledContent("控制迅雷（辅助功能）") {
                    if axAllowed {
                        Text("已允许").foregroundStyle(.secondary)
                    } else {
                        Button("去允许") {
                            ThunderBridge.openAccessibilitySettings()
                            ThunderBridge.promptAccessibility()
                            axAllowed = ThunderBridge.accessibilityGranted
                        }
                    }
                }
                .help("允许后，交给迅雷的任务能自动点「立即下载」。若系统设置里的开关已经打开、这里仍显示未允许，先把列表里那条「闪电全能下载」用「−」移除，再点「去允许」重新添加一次")
                .onAppear { axAllowed = ThunderBridge.accessibilityGranted }
                .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
                    axAllowed = ThunderBridge.accessibilityGranted
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

            Section("保存位置") {
                Toggle("记住上次的保存位置", isOn: $rememberDir)
                Text("打开后，新建任务直接从上次存的文件夹开始，不用每次去挑。磁力/种子、视频网站、其他文件各记一份。关掉则每次回到设置里的默认位置。")
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

            SubtitleSettingsSection()
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
                Text("PikPak 需要科学上网才能使用。")
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
                    Toggle("下不动时用云盘离线（会先确认）", isOn: $autoCloud)
                        .help("交给 PikPak 服务器下好，再从云盘取回本机。取回走系统代理、会消耗代理流量，所以自动转交前都会先问一句")
                    Text("磁力、种子 1 分钟没速度，或 3 分钟几乎没速度时，先建议交给迅雷；迅雷不可用才问你要不要用云盘。每次用掉一次离线，取回这段算代理流量。").font(.system(size: 12)).foregroundStyle(Theme.sub).fixedSize(horizontal: false, vertical: true)
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
struct SubtitleSettingsSection: View {
    private static let manualTag = "__manual__"
    @State private var config = SubtitlePolishConfig.current
    @State private var keyInput = ""
    @State private var manualModel = false
    @State private var testing = false
    @State private var testResult: (ok: Bool, message: String)?

    private var modelChoice: Binding<String> {
        Binding(
            get: { manualModel ? Self.manualTag : config.model },
            set: { value in
                if value == Self.manualTag {
                    manualModel = true
                } else {
                    manualModel = false
                    config.model = value
                    config.selectedModels[config.provider.rawValue] = value
                    SubtitlePolishConfig.current = config
                }
            }
        )
    }

    var body: some View {
        Section("视频字幕") {
            Toggle("清理自动字幕重叠与断句", isOn: $config.autoCleanStructure)
                .onChange(of: config.autoCleanStructure) { SubtitlePolishConfig.current = config }
            Text("勾选下载字幕时自动建独立文件夹，清理滚动重复行并按标准语言代码命名（如 .en.srt / .zh-en.srt）；原字幕隐藏备份，可随时右键恢复。")
                .font(.system(size: 12)).foregroundStyle(Theme.sub).fixedSize(horizontal: false, vertical: true)

            Toggle("在任务中提供 AI 字幕校对与翻译", isOn: $config.aiEnabled)
                .onChange(of: config.aiEnabled) { SubtitlePolishConfig.current = config }

            if config.aiEnabled {
                Text("每次下载可单独选择，默认只下载原字幕。也可在完成后右键处理。")
                    .font(.caption).foregroundStyle(Theme.sub)
                Picker("服务", selection: $config.provider) {
                    ForEach(SubtitlePolishConfig.Provider.allCases) { p in
                        Text(p.displayName).tag(p)
                    }
                }
                .onChange(of: config.provider) {
                    keyInput = ""
                    testResult = nil
                    if !config.provider.needsBaseURLInput {
                        config.baseURL = config.provider.defaultBaseURL
                    }
                    let list = config.availableModels
                    let saved = config.selectedModels[config.provider.rawValue]
                    if let saved, !saved.isEmpty {
                        config.model = saved
                        manualModel = !list.isEmpty && !list.contains(saved)
                    } else if let first = list.first {
                        config.model = first
                        manualModel = false
                    } else {
                        config.model = ""
                        manualModel = true
                    }
                    SubtitlePolishConfig.current = config
                }

                if config.provider.needsBaseURLInput {
                    TextField("接口地址", text: $config.baseURL, prompt: Text("https://api.example.com/v1"))
                        .onChange(of: config.baseURL) { SubtitlePolishConfig.current = config; testResult = nil }
                }

                if config.provider.needsAPIKey {
                    SecureField("API 密钥", text: $keyInput, prompt: Text(config.hasKeyForCurrentProvider ? L("已保存，留空则不修改") : L("粘贴密钥")))
                        .onSubmit { saveKey() }
                    if config.hasKeyForCurrentProvider, keyInput.isEmpty {
                        Label("密钥已保存在本机，重装或换新版本都不用再填", systemImage: "checkmark.circle")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }

                Picker("模型", selection: modelChoice) {
                    ForEach(config.availableModels, id: \.self) { m in
                        Text(m).tag(m)
                    }
                    Divider()
                    Text("手动填写…").tag(Self.manualTag)
                }

                if manualModel {
                    TextField("模型名称", text: $config.model, prompt: Text("按服务商文档填写"))
                        .onChange(of: config.model) { SubtitlePolishConfig.current = config }
                }

                Toggle("外语视频默认生成中外双语字幕（中文在上，原文在下）", isOn: $config.bilingual)
                    .onChange(of: config.bilingual) { SubtitlePolishConfig.current = config }

                HStack(spacing: 8) {
                    Button(testing ? L("正在测试…") : L("测试连接")) { runTest() }
                        .disabled(testing || (config.provider.needsAPIKey && keyInput.isEmpty && !config.hasKeyForCurrentProvider) || config.effectiveBaseURL.isEmpty)
                    Spacer()
                    if config.provider.needsAPIKey {
                        if !keyInput.isEmpty {
                            Button("保存密钥") { saveKey() }.buttonStyle(.borderedProminent)
                        } else if config.hasKeyForCurrentProvider {
                            Button("删除密钥", role: .destructive) { deleteKey() }
                        }
                    }
                }

                if let testResult {
                    Text(testResult.message)
                        .font(.system(size: 12))
                        .foregroundStyle(testResult.ok ? .green : .red)
                }

                Text("测试连接只读取服务商最新模型列表，不产生费用；连接成功后「模型」下拉框会自动更新为最新可用模型。")
                    .font(.system(size: 12)).foregroundStyle(Theme.sub).fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func saveKey() {
        let trimmed = keyInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        config.apiKeys[config.provider.rawValue] = trimmed
        SubtitlePolishConfig.current = config
        keyInput = ""
        runTest()
    }

    private func deleteKey() {
        config.apiKeys.removeValue(forKey: config.provider.rawValue)
        SubtitlePolishConfig.current = config
        keyInput = ""
        testResult = nil
    }

    private func runTest() {
        testing = true
        testResult = nil
        let snapshot = config
        let requestedProvider = snapshot.provider
        let requestedBase = snapshot.effectiveBaseURL
        let pendingKey = keyInput.trimmingCharacters(in: .whitespacesAndNewlines)
        Task {
            let res = await SubtitlePolisher.testAndFetchModels(config: snapshot, overrideKey: pendingKey.isEmpty ? nil : pendingKey)
            // 异步测试期间若用户已切换服务或接口地址，丢弃过期结果，杜绝密钥串用
            guard config.provider == requestedProvider, config.effectiveBaseURL == requestedBase else {
                return
            }
            testing = false
            if res.ok, !pendingKey.isEmpty {
                config.apiKeys[requestedProvider.rawValue] = pendingKey
                keyInput = ""
            }
            if !res.models.isEmpty {
                config.fetchedModels[requestedProvider.rawValue] = res.models
                if !manualModel, !res.models.contains(config.model) {
                    config.model = res.models[0]
                }
            }
            SubtitlePolishConfig.current = config
            testResult = (res.ok, res.message)
        }
    }
}

struct ThunderSettingsSection: View {
    @AppStorage(Prefs.autoThunderKey) private var autoThunder = true
    @State private var installed = ThunderBridge.installed
    @State private var accessibility = ThunderBridge.accessibilityAllowed

    var body: some View {
        Section("迅雷（可选）") {
            if installed {
                Toggle("下不动时优先交给迅雷", isOn: $autoThunder)
                    .help("连续 3 分钟几乎没速度时，用本机的迅雷接着下，进度仍显示在闪电全能下载里。走本机直连，不消耗代理流量")
                LabeledContent("辅助功能授权") {
                    if accessibility { Text("已允许").foregroundStyle(Theme.sub) }
                    else { Button("去允许") { ThunderBridge.openAccessibilitySettings() } }
                }
                .help("闪电靠这项系统授权操作迅雷：点立即下载、开始和暂停任务")
                .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in accessibility = ThunderBridge.accessibilityAllowed }
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
