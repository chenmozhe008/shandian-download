import AppKit
import SwiftUI

/// 自带浏览器扩展（仓库 `browser-extension/`）。打包进 App，启动时拷到 Application Support：
/// 浏览器记的是「已解压扩展」所在路径，放在 App 包里每次重新打包都会变。
/// Chrome / Edge 不允许第三方程序静默安装扩展，只能用户在扩展页打开开发者模式、把文件夹拖进去，一次就好。
enum BrowserExtension {
    static let folder = EngineConfig.supportDir.appendingPathComponent("BrowserExtension")

    /// App 带的扩展版本；扩展和应用的版本号分别管理。
    static var bundledVersion: String {
        guard let u = Bundle.main.resourceURL?.appendingPathComponent("BrowserExtension/manifest.json"),
              let d = try? Data(contentsOf: u), let o = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else { return "" }
        return o["version"] as? String ?? ""
    }

    static func install() {
        guard let src = Bundle.main.resourceURL?.appendingPathComponent("BrowserExtension") else { return }
        let fm = FileManager.default
        let old = try? Data(contentsOf: folder.appendingPathComponent("manifest.json"))
        let new = try? Data(contentsOf: src.appendingPathComponent("manifest.json"))
        let oldJS = try? Data(contentsOf: folder.appendingPathComponent("background.js"))
        let newJS = try? Data(contentsOf: src.appendingPathComponent("background.js"))
        guard new != nil, old != new || oldJS != newJS else { return }
        // 逐个文件覆盖，不删整个目录：浏览器正开着这个扩展
        for f in (try? fm.subpathsOfDirectory(atPath: src.path)) ?? [] {
            let s = src.appendingPathComponent(f), d = folder.appendingPathComponent(f)
            var isDir: ObjCBool = false
            fm.fileExists(atPath: s.path, isDirectory: &isDir)
            if isDir.boolValue { try? fm.createDirectory(at: d, withIntermediateDirectories: true); continue }
            try? fm.createDirectory(at: d.deletingLastPathComponent(), withIntermediateDirectories: true)
            try? fm.removeItem(at: d)
            try? fm.copyItem(at: s, to: d)
        }
    }

    /// 扩展连上 App 的记录（`Chrome.full` 这样的键 → 时间），窗口据此显示状态。不读浏览器配置文件：新版系统上会弹隐私权限框
    @MainActor static func markConnected(_ browser: String, store: Bool) {
        var seen = UserDefaults.standard.dictionary(forKey: seenKey) as? [String: Date] ?? [:]
        seen["\(browser).\(store ? "store" : "full")"] = Date()
        UserDefaults.standard.set(seen, forKey: seenKey)
    }
    static let seenKey = "extensionLastSeen"
}

/// 「浏览器扩展」设置页：一张对照表讲清两个版本能干什么，安装按钮放在对应列下面，不用切换就能选
struct BrowserSetupView: View {
    private enum Cell { case yes, no, note(String), text(String) }
    private static let abilities: [(String, Cell, Cell)] = [
        (L("右键下载压缩包、安装包、文档"), .yes, .yes),
        (L("网页上点下载，自动交给软件"), .yes, .no),
        (L("右键下载磁力、电驴链接"), .yes, .no),
        (L("点种子文件，直接交给软件"), .yes, .no),
        (L("视频上出现「下载」按钮"), .yes, .no),
        (L("抖音、小红书、视频号等视频"), .yes, .no),
        (L("需要登录才能下的文件"), .yes, .note(L("可能失败"))),
    ]
    private static let setup: [(String, Cell, Cell)] = [
        (L("需要的权限"), .text(L("较多，要读取网页和登录状态")), .text(L("很少，看不到网页内容"))),
        (L("以后更新"), .text(L("在扩展页点「重新加载」")), .text(L("自动更新"))),
        (L("怎么装"), .text(L("点下面的按钮，打开「开发者模式」（Chrome 在右上角，Edge 在左侧栏），再把弹出的文件夹拖进页面")),
                  .text(L("点下面的按钮到商店页，再点页面上的添加"))),
    ]
    private static let labelWidth: CGFloat = 200, columnWidth: CGFloat = 196, gap: CGFloat = 12, tablePadding: CGFloat = 16
    let token: String
    @State private var seen: [String: Date] = [:]
    @State private var copied = ""
    @State private var showPan = false

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            VStack(alignment: .leading, spacing: 8) {
                Text("浏览器扩展").font(.system(size: 20, weight: .bold))
                Text("下电影、视频、网盘文件，选完整版；只偶尔下安装包、文档，选商店版。")
                    .font(.system(size: 13.5)).fixedSize(horizontal: false, vertical: true)
                Text("两个都不装也能用：复制链接后在软件里按 ⌘V。")
                    .font(.system(size: 12)).foregroundStyle(Theme.sub)
            }
            VStack(spacing: 0) {
                tableRow(alignment: .top) { Color.clear.frame(height: 1) } full: {
                    columnTitle(L("完整版"), hint: L("功能齐全"), recommended: true)
                } store: {
                    columnTitle(L("商店版"), hint: L("安装简单"), recommended: false)
                }
                .padding(.bottom, 12)
                ForEach(Self.abilities, id: \.0) { contentRow($0) }
                sectionGap
                ForEach(Self.setup, id: \.0) { contentRow($0) }
                sectionGap
                browserRow("Chrome", bundle: "com.google.Chrome", page: "chrome://extensions",
                           store: "https://chromewebstore.google.com/detail/achpnnoffkoeomkmofbcjpnmifaeanjg")
                browserRow("Edge", bundle: "com.microsoft.edgemac", page: "edge://extensions",
                           store: "https://microsoftedge.microsoft.com/addons/detail/onnpjcddpnfmaahjjdcibcofebfgopei")
            }
            .padding(Self.tablePadding)
            // 推荐的一列整列铺浅色，一眼看出该选哪个
            .background(alignment: .leading) {
                RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Theme.tint.opacity(0.08))
                    .frame(width: Self.columnWidth + 12)
                    .padding(.vertical, 8)
                    .padding(.leading, Self.tablePadding + Self.labelWidth + Self.gap - 6)
            }
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.raised))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.line))
            DisclosureGroup(isExpanded: $showPan) {
                VStack(alignment: .leading, spacing: 10) {
                    Text("百度网盘等需要在网页里先取得直链。可另装篡改猴和 LinkSwift 脚本，把直链推送给闪电全能下载。")
                        .font(.system(size: 12.5)).foregroundStyle(Theme.sub).fixedSize(horizontal: false, vertical: true)
                    HStack {
                        Button("篡改猴（Chrome）") { open("https://chromewebstore.google.com/detail/tampermonkey/dhdgffkkebhmkfjojejmpbldmpobfkfo", app: "Google Chrome") }
                        Button("篡改猴（Edge）") { open("https://microsoftedge.microsoft.com/addons/detail/iikmkjmpaadaobahmlepeloendndfphd", app: "Microsoft Edge") }
                        Button("LinkSwift 脚本") { NSWorkspace.shared.open(URL(string: "https://greasyfork.org/scripts/583582-linkswift")!) }
                    }
                    .controlSize(.small)
                    Text(verbatim: String(localized: "LinkSwift 设置 → Aria2 RPC：地址 http://127.0.0.1，端口 \(String(Gateway.port))，路径 /jsonrpc，密钥填下面的 Token。"))
                        .font(.system(size: 12.5)).foregroundStyle(Theme.text.opacity(0.8)).fixedSize(horizontal: false, vertical: true)
                    field(L("密钥 / Token"), token)
                }
                .padding(.top, 8)
            } label: {
                Text("网盘直链（可选）").font(.system(size: 14, weight: .semibold))
            }
        }
        .padding(26)
        .frame(width: 708)
        .background(Theme.window)
        .tint(Theme.tint)
        .onAppear(perform: reload)
        .onReceive(NotificationCenter.default.publisher(for: UserDefaults.didChangeNotification)) { _ in reload() }
    }

    private func tableRow<L: View, F: View, S: View>(alignment: VerticalAlignment = .firstTextBaseline, @ViewBuilder label: () -> L, @ViewBuilder full: () -> F, @ViewBuilder store: () -> S) -> some View {
        HStack(alignment: alignment, spacing: Self.gap) {
            label().frame(width: Self.labelWidth, alignment: .leading)
            full().frame(width: Self.columnWidth, alignment: .leading)
            store().frame(width: Self.columnWidth, alignment: .leading)
        }
    }

    private func contentRow(_ row: (String, Cell, Cell)) -> some View {
        tableRow {
            Text(L(row.0)).font(.system(size: 13))
        } full: {
            cell(row.1)
        } store: {
            cell(row.2)
        }
        .padding(.vertical, 6)
    }

    private var sectionGap: some View {
        Rectangle().fill(Theme.line).frame(height: 1).padding(.vertical, 8)
    }

    private func columnTitle(_ title: String, hint: String, recommended: Bool) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(L(title)).font(.system(size: 15, weight: .bold))
                if recommended {
                    Text("推荐").font(.system(size: 10.5, weight: .semibold)).foregroundStyle(.white)
                        .padding(.horizontal, 6).padding(.vertical, 2)
                        .background(Capsule().fill(Theme.tint))
                }
            }
            .frame(height: 20)
            Text(L(hint)).font(.system(size: 12)).foregroundStyle(Theme.sub)
        }
    }

    @ViewBuilder private func cell(_ value: Cell) -> some View {
        switch value {
        case .yes: Image(systemName: "checkmark").font(.system(size: 12, weight: .bold)).foregroundStyle(Theme.tint)
        case .no: Text("不支持").font(.system(size: 12.5)).foregroundStyle(Theme.sub)
        case .note(let text): Text(L(text)).font(.system(size: 12.5)).foregroundStyle(Theme.sub)
        case .text(let text): Text(L(text)).font(.system(size: 12.5)).foregroundStyle(Theme.text.opacity(0.85)).fixedSize(horizontal: false, vertical: true)
        }
    }

    private func reload() { seen = UserDefaults.standard.dictionary(forKey: BrowserExtension.seenKey) as? [String: Date] ?? [:] }

    /// 完整版每 10 分钟连一次 App，近 20 分钟连过才算已连接；商店版只在发送链接时连接，所以只能说「用过」
    private func status(_ browser: String, store: Bool) -> (text: String, connected: Bool) {
        let at = seen["\(browser).\(store ? "store" : "full")"]
        if store { return at == nil ? ("", false) : (L("已用过"), true) }
        guard let at, Date().timeIntervalSince(at) < 1200 else { return ("", false) }
        return (L("已连接"), true)
    }

    private func browserRow(_ name: String, bundle: String, page: String, store: String) -> some View {
        let app = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundle)
        return HStack(spacing: Self.gap) {
            HStack(spacing: 9) {
                if let app {
                    Image(nsImage: NSWorkspace.shared.icon(forFile: app.path)).resizable().frame(width: 24, height: 24)
                } else {
                    Image(systemName: "globe").font(.system(size: 18)).foregroundStyle(Theme.sub).frame(width: 24, height: 24)
                }
                Text(app == nil ? String(localized: "\(name)（未安装）") : name).font(.system(size: 13.5, weight: .semibold))
            }
            .frame(width: Self.labelWidth, alignment: .leading)
            installCell(status(name, store: false), title: L("手动安装"), prominent: true, enabled: app != nil) {
                guard let app else { return }
                BrowserExtension.install()
                NSWorkspace.shared.activateFileViewerSelecting([BrowserExtension.folder])
                openPage(page, app: app.deletingPathExtension().lastPathComponent)
            }
            .frame(width: Self.columnWidth, alignment: .leading)
            installCell(status(name, store: true), title: L("去商店添加"), prominent: false, enabled: app != nil) {
                guard let app else { return }
                NSWorkspace.shared.open([URL(string: store)!], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
            }
            .frame(width: Self.columnWidth, alignment: .leading)
        }
        .padding(.vertical, 5)
    }

    private func installCell(_ status: (text: String, connected: Bool), title: String, prominent: Bool, enabled: Bool, action: @escaping () -> Void) -> some View {
        HStack(spacing: 8) {
            if prominent {
                Button(L(title), action: action).buttonStyle(.borderedProminent).disabled(!enabled)
            } else {
                Button(L(title), action: action).buttonStyle(.bordered).disabled(!enabled)
            }
            if status.connected {
                Label(L(status.text), systemImage: "checkmark.circle.fill").font(.system(size: 11.5)).foregroundStyle(Theme.sub).fixedSize()
            }
        }
    }

    private func field(_ label: String, _ value: String) -> some View {
        HStack(spacing: 10) {
            Text(label).font(.system(size: 12.5)).foregroundStyle(Theme.sub).frame(width: 84, alignment: .leading)
            Text(String(repeating: "•", count: 16)).font(.system(size: 12.5, design: .monospaced)).lineLimit(1)
            Spacer()
            Button(copied == value ? L("已复制") : L("复制")) { AppInfo.copy(value); copied = value }.controlSize(.small)
        }
        .padding(.horizontal, 12).frame(height: 36)
        .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(Theme.faint))
    }

    /// chrome:// / edge:// 页面不能当普通网址打开，用 AppleScript 让浏览器自己开新标签
    private func openPage(_ url: String, app: String) {
        let src = "tell application \"\(app)\" to open location \"\(url)\"\ntell application \"\(app)\" to activate"
        var err: NSDictionary?
        NSAppleScript(source: src)?.executeAndReturnError(&err)
    }

    private func open(_ url: String, app: String) {
        let appURL = URL(fileURLWithPath: "/Applications/\(app).app")
        NSWorkspace.shared.open([URL(string: url)!], withApplicationAt: appURL, configuration: NSWorkspace.OpenConfiguration())
    }
}
