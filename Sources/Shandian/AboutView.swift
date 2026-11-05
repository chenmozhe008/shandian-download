import AppKit
import SwiftUI

/// 「关于闪电全能下载」：介绍 / 更新记录 / 反馈。结构沿用小清。
/// 介绍里只写功能事实和本机实测；不写竞品对比数字（单次测试不能当结论，10-27 用户指出）
enum AppInfo {
    static let author = "@沉默者之二"
    static let feedbackEmail = "327529709@qq.com"
    static let repository = URL(string: "https://github.com/chenmozhe008/shandian-download")!
    static let issues = URL(string: "https://github.com/chenmozhe008/shandian-download/issues")!
    /// 卡片里显示的仓库地址，不用把整条 URL 塞进提示里
    static let repoLabel = "github.com/chenmozhe008/shandian-download"

    static var version: String { Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? L("开发版") }
    static var build: String { Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "-" }

    /// 本地自己改过、还没提交的构建：打包时 build_app.sh 看到工作区不干净就写这个键，
    /// 界面上按「开发版」标注，免得把没发布的改动当成正式版本
    static var isDev: Bool { (Bundle.main.infoDictionary?["SDDevBuild"] as? String) == "1" }

    /// 标题区的版本行，开发版在后面多一句提示
    static var versionLabel: String {
        let base = "版本 \(version)（\(build)）"
        return isDev ? "\(base) · 开发版" : base
    }

    /// 附在反馈邮件里，便于复现
    static var diagnostics: String {
        "闪电全能下载 \(version)（\(build)）\(isDev ? " 开发版" : "")，macOS \(ProcessInfo.processInfo.operatingSystemVersionString)"
    }

    /// 打开「关于」并停在指定页
    @MainActor static func showAbout(_ tab: AboutTab, _ openWindow: OpenWindowAction) {
        AboutState.shared.tab = tab
        openWindow(id: "about")
        NSApp.activate(ignoringOtherApps: true)
    }

    static func sendFeedback() {
        var c = URLComponents()
        c.scheme = "mailto"
        c.path = feedbackEmail
        c.queryItems = [URLQueryItem(name: "subject", value: "闪电全能下载反馈 v\(version)"),
                        URLQueryItem(name: "body", value: "\n\n（请在此描述遇到的问题，并附上截图）\n\(diagnostics)")]
        if let url = c.url { NSWorkspace.shared.open(url) }
    }

    static func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}

enum AboutTab: String, CaseIterable { case intro = "介绍", changelog = "更新记录", feedback = "反馈" }

/// 窗口开着时从菜单、侧栏切页也要生效，所以用共享对象（@AppStorage 不会响应）
final class AboutState: ObservableObject {
    @MainActor static let shared = AboutState()
    @Published var tab: AboutTab = .intro
}

struct AboutView: View {
    @ObservedObject private var state = AboutState.shared

    var body: some View {
        VStack(spacing: 0) {
            Header()
            Picker("", selection: $state.tab) {
                ForEach(AboutTab.allCases, id: \.self) { Text(L($0.rawValue)).tag($0) }
            }
            .pickerStyle(.segmented).labelsHidden().frame(width: 270).padding(.vertical, 16)
            ScrollView {
                switch state.tab {
                case .intro: IntroPage()
                case .changelog: ChangelogPage()
                case .feedback: FeedbackPage()
                }
            }
        }
        .frame(width: 680, height: 720)
        .background(Theme.window)
        .tint(Theme.tint)
    }
}

/// 琥珀渐变标题区：图标、名字、一句话定位、版本与作者
private struct Header: View {
    var body: some View {
        HStack(spacing: 18) {
            Image(nsImage: NSApp.applicationIconImage).resizable().frame(width: 84, height: 84)
                .shadow(color: .black.opacity(0.18), radius: 10, y: 5)
            VStack(alignment: .leading, spacing: 5) {
                Text("闪电全能下载").font(.system(size: 26, weight: .bold))
                Text("磁力、种子、网盘与视频网站，一个窗口下载").font(.system(size: 14, weight: .medium))
                HStack(spacing: 12) {
                    Text(AppInfo.versionLabel + String(localized: "　作者 \(AppInfo.author)"))
                        .font(.system(size: 12)).opacity(0.75).monospacedDigit()
                    Button { NSWorkspace.shared.open(AppInfo.repository) } label: {
                        HStack(spacing: 6) { GitHubMark(size: 13); Text("GitHub") }
                            .font(.system(size: 11.5, weight: .semibold))
                            .foregroundStyle(Color(red: 0.60, green: 0.35, blue: 0.0))
                            .padding(.horizontal, 11).frame(height: 24)
                            .background(Capsule().fill(.white.opacity(0.92)))
                    }
                    .buttonStyle(.plain)
                    .help("在 GitHub 查看源代码：\(AppInfo.repoLabel)")
                }
                .padding(.top, 2)
            }
            .foregroundStyle(.white)
            Spacer()
        }
        .padding(.horizontal, 28).padding(.top, 44).padding(.bottom, 24)
        .background(
            LinearGradient(colors: [Color(red: 1.0, green: 0.72, blue: 0.20), Color(red: 0.93, green: 0.48, blue: 0.0)],
                           startPoint: .topLeading, endPoint: .bottomTrailing)
                .overlay(RadialGradient(colors: [.white.opacity(0.25), .clear], center: .topTrailing, startRadius: 0, endRadius: 320))
        )
    }
}

/// 分区小标题：琥珀色竖条 + 标题
private struct SectionTitle: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View {
        HStack(spacing: 8) {
            Capsule().fill(Theme.tint).frame(width: 4, height: 16)
            Text(text).font(.system(size: 15, weight: .bold))
        }
    }
}

/// 白底卡片（深色模式下是略亮的灰），带细边和轻阴影
private struct Card<Content: View>: View {
    @ViewBuilder var content: Content
    var body: some View {
        content
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(16)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.raised))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.line, lineWidth: 1))
            .shadow(color: .black.opacity(0.05), radius: 8, y: 3)
    }
}

/// 琥珀渐变图标块
private struct IconTile: View {
    let symbol: String
    var body: some View {
        Image(systemName: symbol).font(.system(size: 16, weight: .semibold)).foregroundStyle(.white)
            .frame(width: 36, height: 36)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(LinearGradient(colors: [Color(red: 1.0, green: 0.74, blue: 0.25), Color(red: 0.94, green: 0.52, blue: 0.0)],
                                     startPoint: .top, endPoint: .bottom)))
    }
}

/// 「代码完全开源」入口：开源是下载工具的信任基础，把仓库做成一眼能点的卡片
private struct OpenSourceCard: View {
    var body: some View {
        Button { NSWorkspace.shared.open(AppInfo.repository) } label: {
            HStack(spacing: 12) {
                GitHubMark(size: 20).foregroundStyle(.white)
                    .frame(width: 36, height: 36)
                    .background(RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .fill(Color(red: 0.14, green: 0.16, blue: 0.18)))
                VStack(alignment: .leading, spacing: 3) {
                    Text("代码完全开源").font(.system(size: 14, weight: .bold)).foregroundStyle(Theme.text)
                    Text(AppInfo.repoLabel).font(.system(size: 12)).foregroundStyle(Theme.sub)
                }
                Spacer(minLength: 8)
                Label("在 GitHub 查看", systemImage: "arrow.up.right")
                    .font(.system(size: 13, weight: .semibold)).foregroundStyle(.white)
                    .padding(.horizontal, 14).frame(height: 34)
                    .background(Capsule().fill(Theme.tint))
            }
            .padding(14)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.raised))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.line, lineWidth: 1))
            .shadow(color: .black.opacity(0.05), radius: 8, y: 3)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("在 GitHub 查看源代码")
    }
}

private struct IntroPage: View {
    private let points: [(String, String, String)] = [
        ("globe", L("海外资源可连接"), L("下载经本机代理连接，可访问海外的 Tracker 和下载服务器。")),
        ("bolt.fill", L("磁力更快找到来源"), L("启动时更新公共 Tracker 列表，使用固定监听端口，连接更多来源。")),
        ("film.stack", L("只下载需要的文件"), L("种子只选正片和字幕，跳过样片、海报；同一影片多种格式只下一个。")),
        ("play.rectangle.fill", L("视频网站最高画质"), L("默认最高分辨率，优先 H.264，可直接导入剪辑软件；可选字幕和仅音频。")),
        ("photo.on.rectangle", L("影片一目了然"), L("自动整理影片名称，显示影片画面或视频封面。")),
        ("checkmark.seal.fill", L("无广告"), L("下载核心为开源项目 Gopeed 和 yt-dlp；部分网站需要登录。")),
    ]
    private let sources = [L("磁力"), L("种子"), "ed2k", L("HTTP 直链"), L("网盘直链"), "YouTube", "X", L("B 站"), L("微博"), L("小红书"), L("新片场"), L("抖音"), L("豆瓣预告"), L("微信视频号")]
    private let notes: [(String, String)] = [
        ("doc.on.clipboard", L("复制链接后在窗口中按 ⌘V 即可下载，也可拖入链接或种子文件；装好浏览器扩展后，网页上点下载会直接交给闪电全能下载。")),
        ("externaldrive", L("磁力、种子和其他下载默认存到「下载」，可在设置中选择保存位置。")),
        ("play.tv", L("下载中的电影可「边下边播」：先下片头，用 IINA 或 VLC 打开。")),
        ("cloud", L("网盘下载需先在浏览器中获取直链，速度受网盘自身限速影响。")),
        ("gauge.with.dots.needle.33percent", L("下载速度取决于资源热度、来源数量和网络环境。")),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 24) {
            VStack(alignment: .leading, spacing: 10) {
                SectionTitle(L("它是什么"))
                Text("闪电全能下载是一款 Mac 下载工具。磁力链接、种子文件、ed2k、HTTP 直链、网盘直链，以及 B 站、YouTube 等视频网站的内容，都在同一个窗口下载。链接类型自动识别，文件按类型保存到不同位置；关闭窗口后下载继续，菜单栏显示当前速度。")
                    .font(.system(size: 13.5)).foregroundStyle(Theme.text).lineSpacing(4)
                    .fixedSize(horizontal: false, vertical: true)
                OpenSourceCard().padding(.top, 4)
                Text("全部代码以 GPL-3.0 许可公开，下载内核为开源项目 Gopeed 的定制版本，视频下载由 yt-dlp 提供。欢迎在 GitHub 提交问题、建议或代码。")
                    .font(.system(size: 12.5)).foregroundStyle(Theme.sub).lineSpacing(3)
                    .fixedSize(horizontal: false, vertical: true)
            }
            VStack(alignment: .leading, spacing: 12) {
                SectionTitle(L("核心优势"))
                LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)], spacing: 12) {
                    ForEach(points, id: \.1) { p in
                        Card {
                            VStack(alignment: .leading, spacing: 10) {
                                IconTile(symbol: p.0)
                                Text(p.1).font(.system(size: 14, weight: .bold))
                                Text(p.2).font(.system(size: 12.5)).foregroundStyle(Theme.text.opacity(0.72)).lineSpacing(2)
                                    .fixedSize(horizontal: false, vertical: true)
                                Spacer(minLength: 0)
                            }
                        }
                        .frame(maxHeight: .infinity)
                    }
                }
            }
            VStack(alignment: .leading, spacing: 12) {
                SectionTitle(L("支持的来源"))
                FlowChips(items: sources)
                Text("小红书、新片场、抖音：在浏览器里打开视频页，点扩展里的「下载这个视频」。视频号：粘贴分享链接后按提示操作，需要登录腾讯元宝。")
                    .font(.system(size: 12.5)).foregroundStyle(Theme.sub)
            }
            VStack(alignment: .leading, spacing: 12) {
                SectionTitle(L("使用说明"))
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(notes, id: \.1) { n in
                        HStack(alignment: .firstTextBaseline, spacing: 10) {
                            Image(systemName: n.0).font(.system(size: 13)).foregroundStyle(Theme.tint).frame(width: 18)
                            Text(n.1).font(.system(size: 13)).foregroundStyle(Theme.text).fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }
        }
        .padding(.horizontal, 28).padding(.bottom, 28)
    }
}

/// 一行排不下自动换行的标签
struct FlowChips: View {
    let items: [String]
    var body: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 84), spacing: 8)], alignment: .leading, spacing: 8) {
            ForEach(items, id: \.self) { s in
                Text(s).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.tint)
                    .frame(maxWidth: .infinity).frame(height: 30)
                    .background(Capsule().fill(Theme.tint.opacity(0.12)))
            }
        }
    }
}

private struct Bullet: View {
    let text: String
    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Circle().fill(Theme.tint).frame(width: 5, height: 5).offset(y: -2)
            Text(text).font(.system(size: 13)).fixedSize(horizontal: false, vertical: true)
        }
    }
}

private struct ChangelogPage: View {
    @State private var online: [HistoryEntry] = []
    @State private var message: String?
    @State private var loading = false
    private let cacheKey = "shandianReleaseHistory"

    private var releases: [HistoryEntry] {
        let text = Bundle.main.url(forResource: "CHANGELOG", withExtension: "md")
            .flatMap { try? String(contentsOf: $0, encoding: .utf8) } ?? ""
        return ReleaseHistory.merge(local: ReleaseHistory.local(text, currentVersion: AppInfo.version, isDevelopment: AppInfo.isDev), online: online)
    }

    @MainActor private func loadHistory() async {
        guard !loading else { return }
        loading = true
        defer { loading = false }
        if online.isEmpty, let data = UserDefaults.standard.data(forKey: cacheKey), let cached = ReleaseHistory.online(data) { online = cached }
        var request = URLRequest(url: URL(string: "https://api.github.com/repos/chenmozhe008/shandian-download/releases?per_page=100")!, timeoutInterval: 15)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        do {
            let (data, response) = try await GitHubAPI.data(for: request)
            guard (response as? HTTPURLResponse)?.statusCode == 200, let rows = ReleaseHistory.online(data) else { throw URLError(.badServerResponse) }
            online = rows
            UserDefaults.standard.set(data, forKey: cacheKey)
            message = rows.isEmpty ? L("GitHub 暂无正式发布记录，下面显示本机记录。") : nil
        } catch {
            message = L("暂时连不上 GitHub，下面显示本机记录。")
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                Button(loading ? L("正在获取…") : L("刷新发布记录")) { Task { await loadHistory() } }.disabled(loading)
                Spacer()
                Link("GitHub 发布页", destination: URL(string: "https://github.com/chenmozhe008/shandian-download/releases")!)
            }
            if let message { Text(message).font(.callout).foregroundStyle(Theme.sub) }
            ForEach(Array(releases.enumerated()), id: \.element.id) { i, r in
                Card {
                    VStack(alignment: .leading, spacing: 10) {
                        HStack(spacing: 8) {
                            Text("v" + r.version).font(.system(size: 13, weight: .bold)).foregroundStyle(.white)
                                .padding(.horizontal, 10).frame(height: 24)
                                .background(Capsule().fill(Theme.tint))
                            if i == 0 {
                                Text("最新").font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Theme.tint)
                                    .padding(.horizontal, 8).frame(height: 22)
                                    .background(Capsule().strokeBorder(Theme.tint, lineWidth: 1))
                            }
                            if r.isDevelopment {
                                Text("开发版").font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Theme.sub)
                                    .padding(.horizontal, 8).frame(height: 22)
                                    .background(Capsule().fill(Theme.faint))
                                    .help("本机自己改过、还没发布的版本")
                            }
                            Spacer()
                            if let page = r.page { Link("发布页", destination: page).font(.caption) }
                            if !r.date.isEmpty { Text(r.date).font(.system(size: 12)).foregroundStyle(Theme.sub).monospacedDigit() }
                        }
                        VStack(alignment: .leading, spacing: 7) {
                            ForEach(r.notes, id: \.self) { Bullet(text: $0) }
                        }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 28).padding(.bottom, 28)
        .task { await loadHistory() }
    }
}

private struct FeedbackPage: View {
    @State private var copied = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Card {
                VStack(alignment: .leading, spacing: 8) {
                    Text("使用中遇到问题，欢迎反馈").font(.system(size: 17, weight: .bold))
                    Text("请描述问题并附上截图，便于定位。可在 GitHub 提交问题（公开、便于他人查阅），也可直接发邮件。").font(.system(size: 13)).foregroundStyle(Theme.sub)
                        .fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 10) {
                        Button { NSWorkspace.shared.open(AppInfo.issues) } label: {
                            Label("在 GitHub 提交问题", systemImage: "arrow.up.right")
                        }
                        .buttonStyle(.borderedProminent).controlSize(.large)
                        Button { AppInfo.sendFeedback() } label: { Label("发反馈邮件", systemImage: "envelope.fill") }
                            .controlSize(.large)
                        Button { NotificationCenter.default.post(name: Diagnostics.exportRequested, object: nil) } label: { Label("导出诊断信息", systemImage: "doc.text") }
                            .controlSize(.large)
                            .help("排查问题用。只含版本、设置和运行状态，路径、片名、链接和令牌都已去掉")
                        Button { AppInfo.copy(AppInfo.feedbackEmail); copied = AppInfo.feedbackEmail } label: {
                            Label(copied == AppInfo.feedbackEmail ? L("已复制邮箱") : AppInfo.feedbackEmail,
                                  systemImage: copied == AppInfo.feedbackEmail ? "checkmark" : "doc.on.doc")
                        }
                        .controlSize(.large)
                    }
                    .padding(.top, 6)
                }
            }
            VStack(alignment: .leading, spacing: 8) {
                Text("反馈时请尽量包括").font(.system(size: 14, weight: .semibold))
                Bullet(text: L("出现问题的链接，或链接类型（磁力、视频网站等）"))
                Bullet(text: L("任务行显示的状态或失败原因"))
                Bullet(text: L("版本信息（「发反馈邮件」会自动附上，也可在下方复制）"))
            }
            HStack {
                Text(AppInfo.diagnostics).font(.system(size: 12)).foregroundStyle(Theme.sub).textSelection(.enabled)
                Spacer()
                Button(copied == AppInfo.diagnostics ? L("已复制") : L("复制")) { AppInfo.copy(AppInfo.diagnostics); copied = AppInfo.diagnostics }
                    .controlSize(.small)
            }
        }
        .padding(.horizontal, 28).padding(.bottom, 26)
    }
}
