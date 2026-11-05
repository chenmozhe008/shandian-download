import AppKit
import SwiftUI
import UniformTypeIdentifiers

struct MainView: View {
    @ObservedObject var store: DownloadStore

    var body: some View {
        // 系统原生分栏：侧栏玻璃、红绿灯位置、工具栏都由系统排，和访达一致
        NavigationSplitView {
            Sidebar(store: store)
                .navigationSplitViewColumnWidth(min: 200, ideal: 224, max: 260)
        } detail: {
            DetailPane(store: store)
                .background {
                    // 左上角一点琥珀暖光（v3 定稿的辨识点，换成原生分栏后补回）
                    ZStack {
                        Theme.window
                        RadialGradient(colors: [Theme.glow, .clear], center: .topLeading, startRadius: 0, endRadius: 620)
                    }
                    .ignoresSafeArea()
                }
                .navigationTitle(L(store.section.title))
                .toolbar { ToolbarItems(store: store) }
                .searchable(text: $store.search, placement: .toolbar, prompt: L("搜索"))
        }
        .tint(Theme.tint)
        .onDrop(of: [.fileURL, .url, .text], isTargeted: nil) { providers in
            for p in providers {
                if p.canLoadObject(ofClass: URL.self) {
                    _ = p.loadObject(ofClass: URL.self) { url, _ in
                        guard let url else { return }
                        let s = url.isFileURL ? url.path : url.absoluteString
                        Task { @MainActor in store.add(link: s) }
                    }
                } else {
                    _ = p.loadObject(ofClass: String.self) { text, _ in  // 拖进来一段文字，里面可能有多个链接
                        guard let text else { return }
                        Task { @MainActor in store.add(text: text) }
                    }
                }
            }
            return true
        }
        .sheet(isPresented: $store.showNewSheet) { NewDownloadSheet(store: store) }
        // 移除一律先确认：一个「移除」按钮 + 「同时将文件移到废纸篓」勾选框（同迅雷、qBittorrent），记住上次的选择
        .onChange(of: store.removalTarget?.id) {
            guard let item = store.removalTarget else { return }
            RemovalConfirm.present(item, store: store)
        }
    }
}

@MainActor
enum RemovalConfirm {
    private static let key = "removeAlsoTrashesFiles"

    static func present(_ item: Item, store: DownloadStore) {
        let hasFiles: Bool = {
            if case .pending = item.source { return false }
            guard !item.fileGone, let f = item.location ?? item.file else { return false }
            return FileManager.default.fileExists(atPath: f.path)
        }()
        let alert = NSAlert()
        alert.messageText = String(localized: "移除「\(item.title)」？")
        alert.informativeText = item.isActive ? L("下载将停止。") : hasFiles ? L("任务移到回收站，可以重新下载。") : L("文件已不在原位置，只移除任务。")
        alert.addButton(withTitle: L("移除"))
        alert.addButton(withTitle: L("取消"))
        if hasFiles {
            alert.showsSuppressionButton = true
            alert.suppressionButton?.title = item.isActive ? L("同时将已下载的部分移到废纸篓") : L("同时将文件移到废纸篓")
            alert.suppressionButton?.state = UserDefaults.standard.bool(forKey: key) ? .on : .off
        }
        let finish: (NSApplication.ModalResponse) -> Void = { r in
            store.removalTarget = nil
            guard r == .alertFirstButtonReturn else { return }
            let trash = hasFiles && alert.suppressionButton?.state == .on
            if hasFiles { UserDefaults.standard.set(trash, forKey: key) }
            store.remove(item, moveFiles: trash)
        }
        if let w = NSApp.keyWindow ?? NSApp.mainWindow { alert.beginSheetModal(for: w, completionHandler: finish) }
        else { finish(alert.runModal()) }
    }
}

// MARK: - 侧栏

private struct Sidebar: View {
    @ObservedObject var store: DownloadStore
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        List(selection: $store.section) {
            row(.active, Image(systemName: "arrow.down.circle"))  // 闪电箭头只用在品牌位（App 图标、侧栏顶部、菜单栏），这里和其它行一样用线条图标
            row(.done, Image(systemName: "checkmark.circle"))
            row(.trash, Image(systemName: "trash"))
            Section("资料库") {
                ForEach(Category.allCases) { c in row(.category(c), Image(systemName: c.symbol)) }
            }
        }
        .listStyle(.sidebar)
        .environment(\.sidebarRowSize, .large)
        .safeAreaInset(edge: .top, spacing: 0) { brand }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(alignment: .leading, spacing: 8) {
                diskStatus
                versionRow
            }
            .padding(.bottom, 12)
        }

    }

    /// 整块都能点：纯文字按钮只有笔画算点中，图标和文字之间、上下边缘都点不到（11-05 用户：要多点一下）
    private func sidebarLabel(_ title: String, systemImage: String) -> some View {
        Label(L(title), systemImage: systemImage).fixedSize()
            .padding(.vertical, 5).padding(.horizontal, 4)
            .contentShape(Rectangle())
    }

    /// 侧栏顶部：App 图标 + 名字，只做标识（「关于」入口在底部，不重复）
    private var brand: some View {
        HStack(spacing: 9) {
            Image(nsImage: NSApp.applicationIconImage).resizable().frame(width: 28, height: 28)
            Text("闪电全能下载").font(.system(size: 17, weight: .semibold)).foregroundStyle(Theme.text)
            Spacer()
        }
        .padding(.horizontal, 18).padding(.top, 4).padding(.bottom, 6)
    }

    /// 侧栏底部一行：设置、关于；有更新或正在更新时显示状态
    private var versionRow: some View {
        HStack(spacing: 16) {
            SettingsLink {
                sidebarLabel("设置", systemImage: "gearshape")
            }
            .simultaneousGesture(TapGesture().onEnded { NSApp.activate(ignoringOtherApps: true) })
            .help("设置（⌘,）")
            Button { AppInfo.showAbout(.intro, openWindow) } label: {
                sidebarLabel("关于", systemImage: "info.circle")
            }
            .help("关于闪电全能下载")
            Spacer(minLength: 0)
            UpdateBadge(checker: store.updates)
        }
        .labelStyle(.titleAndIcon)
        .buttonStyle(.plain)
        .font(.system(size: 12.5)).foregroundStyle(Theme.sub)
        .firstClickActivates()
        .onReceive(NotificationCenter.default.publisher(for: PikPak.relinkRequested)) { _ in
            openWindow(id: "pikpak-login")
            NSApp.activate(ignoringOtherApps: true)
        }
        .padding(.horizontal, 18)
    }

    private func row(_ p: Pane, _ icon: Image) -> some View {
        let n = store.count(p)
        // 图标颜色交给系统：未选中用 App 强调色（Assets 里的 AccentColor），选中时变白
        return Label { Text(L(p.title)) } icon: { icon }
            .badge(n > 0 ? Text("\(n)").monospacedDigit() : nil)
            .tag(p)
    }

    @ViewBuilder private var diskStatus: some View {
        switch store.engineState {
        case .failed(let m):
            Label("内核未启动：\(m)", systemImage: "exclamationmark.triangle")
                .font(.system(size: 12.5)).foregroundStyle(Theme.sub)
                .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 18)
        default:
            EmptyView()
        }
    }
}

// MARK: - 右侧内容

private struct DetailPane: View {
    @ObservedObject var store: DownloadStore
    @AppStorage("completedRowLayout") private var completedRowLayout = true

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            switch store.section {
            case .active: ActivePage(store: store)
            case .done: DonePage(store: store, items: store.doneItems, rowLayout: completedRowLayout).padding(.top, 8)
            case .trash: TrashPage(store: store)
            case .category(let c): CategoryPage(store: store, category: c)
            }
        }
        .padding(.horizontal, 24).padding(.top, 8)
        // 提示条浮在内容上，不占位：占位的话出现和消失都会把下面整块顶上顶下（11-05 用户反馈）
        .overlay(alignment: .top) { Banners(store: store).padding(.horizontal, 24).padding(.top, 8) }
        .focusable().focusEffectDisabled()
        .onKeyPress(keys: [.upArrow, .downArrow, .space, .return, .delete, .deleteForward]) { press in
            store.handleKey(press.key) ? .handled : .ignored
        }
        .onChange(of: store.section) {
            // 切页时清掉选中；但重复添加会主动切页并选中那条，别把这时的选中也清掉
            if let id = store.selectedID, !store.visibleItems.contains(where: { $0.id == id }) {
                store.selectedID = nil
            }
        }
    }
}

private extension View {
    /// 窗口不在前台时，第一下点击也算数，不只是把窗口叫到前台（macOS 15 起才有这个接口）
    @ViewBuilder func firstClickActivates() -> some View {
        if #available(macOS 15.0, *) { allowsWindowActivationEvents() } else { self }
    }
}

/// 侧栏仅在有更新或更新进行中显示状态。
private struct UpdateBadge: View {
    @ObservedObject var checker: UpdateChecker

    var body: some View {
        switch checker.phase {
        case .checking:
            HStack(spacing: 6) {
                ProgressView().controlSize(.mini)
                Text("正在检查").fixedSize()
            }
        case .downloading(let fraction):
            HStack(spacing: 6) {
                ProgressView(value: fraction).frame(width: 54).tint(Theme.tint)
                Text(fraction > 0 ? "\(Int(fraction * 100))%" : L("连接中")).monospacedDigit().fixedSize()
            }
            .help("正在下载新版本")
        case .installing:
            HStack(spacing: 6) {
                ProgressView().controlSize(.mini)
                Text("正在安装").fixedSize()
            }
        case .idle:
            if checker.available != nil {
                Button { checker.startUpdate() } label: {
                    Label("立即升级", systemImage: "arrow.down.circle.fill")
                        .lineLimit(1).fixedSize()
                        .foregroundStyle(Theme.tint).font(.system(size: 12.5, weight: .semibold))
                        .padding(.horizontal, 7).padding(.vertical, 2)
                        .background(Capsule().fill(Theme.tint.opacity(0.14)))
                }
                .help("升级到 \(checker.available?.version ?? "新版本")：自动下载安装并重新打开，下载中的任务会先暂停，重新打开后继续")
            } else {
                EmptyView()
            }
        }
    }
}

private struct ToolbarItems: ToolbarContent {
    @ObservedObject var store: DownloadStore
    @AppStorage("completedRowLayout") private var completedRowLayout = true
    var body: some ToolbarContent {
        ToolbarItemGroup(placement: .primaryAction) {
            if store.section != .active {
                Button { completedRowLayout.toggle() } label: {
                    Label(completedRowLayout ? L("切换为卡片排列") : L("切换为逐行排列"),
                          systemImage: completedRowLayout ? "square.grid.2x2" : "list.bullet")
                }
                .help(completedRowLayout ? L("切换为卡片排列") : L("切换为逐行排列"))
            }
            Button { store.toggleAll() } label: {
                // 带文字：只有图标时看不出是总开关（11-05 用户反馈）
                Label(store.allPaused ? L("全部继续") : L("全部暂停"), systemImage: store.allPaused ? "play.fill" : "pause.fill")
                    .labelStyle(.titleAndIcon).font(.system(size: 13, weight: .semibold))
                    .padding(.horizontal, 4)
            }
            .buttonStyle(.bordered)
            .help(store.allPaused ? L("全部继续（包括迅雷里的任务）") : L("全部暂停（包括迅雷里的任务）"))
            .disabled(store.items.filter(\.isActive).isEmpty)
            Button { store.showNewSheet = true } label: {
                Label("新建下载", systemImage: "plus").labelStyle(.titleAndIcon).font(.system(size: 13, weight: .semibold))
                    .padding(.horizontal, 4)
            }
            .buttonStyle(.borderedProminent)
            .help("新建下载（⌘N）")
        }
    }
}

/// 剪贴板发现链接、重复提醒、失败原因：一行，可关闭。
/// 提示要「看得见」：字号、行高、底色都比正文重一点，带图标和按钮（11-01 用户反馈太小、一闪就没看到）
private struct Banners: View {
    @ObservedObject var store: DownloadStore
    var body: some View {
        VStack(spacing: 8) {
            if let links = store.clipboardLinks {
                bar(icon: "doc.on.clipboard", text: links.count > 1 ? String(localized: "剪贴板中有 \(links.count) 个链接") : LinkKind.isWeChatChannelShare(links[0]) ? L("检测到视频号分享链接") : L("剪贴板中有链接：") + DownloadStore.displayName(links[0]), tint: Theme.tint) {
                    Button(links.count == 1 && LinkKind.isWeChatChannelShare(links[0]) ? L("查看方法") : L("下载")) { store.acceptClipboard() }.buttonStyle(.borderedProminent).tint(Theme.tint).controlSize(.small)
                    Button("忽略") { store.clipboardLinks = nil }.buttonStyle(.plain).foregroundStyle(Theme.sub).font(.system(size: 12))
                }
            }
            if let n = store.notice {
                bar(icon: DownloadStore.noticeNeedsAttention(n) ? "exclamationmark.circle.fill" : "info.circle.fill", text: n,
                    tint: DownloadStore.noticeNeedsAttention(n) ? Theme.failed : Theme.tint) {
                    if let action = store.noticeAction, action.forText == n {
                        Button(L(action.title)) { action.run() }
                            .buttonStyle(.borderedProminent).tint(Theme.tint).controlSize(.small)
                    }
                    Button {
                        store.notice = nil; store.noticeAction = nil
                    } label: {
                        Image(systemName: "xmark").font(.system(size: 10, weight: .bold))
                    }
                    .buttonStyle(.plain).foregroundStyle(Theme.sub)
                }
            }
        }
    }

    private func bar<A: View>(icon: String, text: String, tint: Color, @ViewBuilder actions: () -> A) -> some View {
        HStack(spacing: 11) {
            Image(systemName: icon).font(.system(size: 15, weight: .semibold)).foregroundStyle(tint)
                .frame(width: 26, height: 26)
                .background(Circle().fill(tint.opacity(0.16)))
            Text(L(text)).font(.system(size: 13.5, weight: .medium)).foregroundStyle(Theme.text)
                .lineLimit(2).fixedSize(horizontal: false, vertical: true).help(text)
            Spacer(minLength: 8)
            actions()
        }
        .padding(.horizontal, 14).padding(.vertical, 10).frame(minHeight: 48)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(tint.opacity(0.10)))
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Theme.window))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(tint.opacity(0.35), lineWidth: 1))
        .shadow(color: .black.opacity(0.10), radius: 8, y: 3)
        .transition(.move(edge: .top).combined(with: .opacity))
    }
}

// MARK: - 正在下载

private struct ActivePage: View {
    @ObservedObject var store: DownloadStore
    var body: some View {
        let items = store.activeItems
        VStack(alignment: .leading, spacing: 0) {
            // 没有任务时不显示速度区，只留空状态
            if items.isEmpty {
                EmptyState(store: store)
            } else {
                SpeedHero(store: store).frame(height: 156).padding(.top, 8).padding(.bottom, 20)
                Text(items.allSatisfy(\.isFailed) ? L("未完成") : L("进行中")).font(.system(size: 15, weight: .semibold)).padding(.bottom, 6)
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(spacing: 0) {
                            ForEach(items) { TaskRow(store: store, item: $0).id($0.id) }
                        }
                    }
                    .scrollIndicators(.automatic)
                    .modifier(ScrollToTask(store: store, ids: items.map(\.id), proxy: proxy))
                }
            }
        }
    }
}

/// 列表滚到指定任务：重复添加链接时把已有那条送到眼前，而不是只弹一条看不着的提示
private struct ScrollToTask: ViewModifier {
    @ObservedObject var store: DownloadStore
    let ids: [String]
    let proxy: ScrollViewProxy

    func body(content: Content) -> some View {
        content
            .onChange(of: store.scrollTarget) { scroll() }
            .onChange(of: store.section) { scroll() }
            .task {
                // 刚切页时列表还没排好，等一下再滚
                try? await Task.sleep(nanoseconds: 80_000_000)
                scroll()
            }
    }

    private func scroll() {
        guard let id = store.scrollTarget, ids.contains(id) else { return }
        withAnimation(.easeInOut(duration: 0.35)) { proxy.scrollTo(id, anchor: .center) }
        // 滚到位就清掉目标：不然列表每次重画都可能再被拉回这一行，用户手动滚走也拉回来
        Task {
            try? await Task.sleep(nanoseconds: 500_000_000)
            if store.scrollTarget == id { store.scrollTarget = nil }
        }
    }
}

/// 速度横幅：速度曲线铺满整幅当背景，大号速度压在上面
private struct SpeedHero: View {
    @ObservedObject var store: DownloadStore
    var body: some View {
        let speed = store.totalSpeed
        let moving = store.activeItems.contains { $0.status == .running || $0.status == .waiting || $0.isResolving }
        let parts = moving ? speed.speedText.split(separator: " ").map { String($0) } : [L("已暂停")]
        let shape = RoundedRectangle(cornerRadius: 18, style: .continuous)
        ZStack(alignment: .topLeading) {
            shape.fill(LinearGradient(colors: [Theme.tint.opacity(0.10), Theme.tint.opacity(0.03)], startPoint: .topLeading, endPoint: .bottomTrailing))
            SpeedChart(values: store.speedHistory)
                .padding(.top, 34)
                .opacity(store.speedHistory.contains { $0 > 0 } ? 1 : 0.0)
                .clipShape(shape)
            VStack(alignment: .leading, spacing: 0) {
                Text("本机下载速度").font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.sub)
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(parts.first ?? "0").font(.system(size: 52, weight: .bold, design: .rounded)).monospacedDigit()
                        .contentTransition(.numericText())
                    Text(parts.dropFirst().joined(separator: " ")).font(.system(size: 19, weight: .semibold, design: .rounded)).foregroundStyle(Theme.sub)
                }
                HStack(spacing: 14) {
                    let failed = store.activeItems.filter(\.isFailed).count
                    let n = store.activeItems.count - failed
                    if n > 0 { Text("\(n) 项进行中") }
                    if failed > 0 { Text("\(failed) 项失败") }
                    let reclaiming = store.activeItems.filter { $0.checking && $0.fromThunder }.count
                    let checking = store.activeItems.filter(\.checking).count - reclaiming
                    if reclaiming > 0 { Text("\(reclaiming) 项正在取回迅雷素材") }
                    if checking > 0 { Text("\(checking) 项正在校验已下载的部分") }
                    let eta = remainingText(store.etaAll).replacingOccurrences(of: "剩余 ", with: "").replacingOccurrences(of: " left", with: "")
                    if moving, !eta.isEmpty { Text("预计 \(eta)后全部完成") }
                }
                .font(.system(size: 13)).foregroundStyle(Theme.sub).padding(.top, 4)
            }
            .padding(.horizontal, 24).padding(.vertical, 18)
        }
        .overlay(shape.strokeBorder(Theme.tint.opacity(0.18), lineWidth: 1))
    }
}

/// 最近 60 秒的速度曲线，越往右越新；左侧淡出
private struct SpeedChart: View {
    let values: [Double]
    var body: some View {
        GeometryReader { geo in
            let w = geo.size.width, h = geo.size.height
            let maxV = max(values.max() ?? 0, 1_048_576) * 1.25
            let pts: [CGPoint] = values.enumerated().map { i, v in
                CGPoint(x: w * CGFloat(i) / CGFloat(max(values.count - 1, 1)), y: h - 4 - CGFloat(v / maxV) * (h - 10))
            }
            let line = Path { p in
                guard let first = pts.first else { return }
                p.move(to: first)
                for i in 1..<pts.count {
                    let a = pts[i - 1], b = pts[i], mx = (a.x + b.x) / 2
                    p.addCurve(to: b, control1: CGPoint(x: mx, y: a.y), control2: CGPoint(x: mx, y: b.y))
                }
            }
            ZStack {
                Path { p in p.addPath(line); p.addLine(to: CGPoint(x: w, y: h)); p.addLine(to: CGPoint(x: 0, y: h)); p.closeSubpath() }
                    .fill(LinearGradient(colors: [Theme.tint.opacity(0.28), Theme.tint.opacity(0)], startPoint: .top, endPoint: .bottom))
                line.stroke(Theme.tint, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                    .shadow(color: Theme.tint.opacity(0.5), radius: 6)
                if let last = pts.last { Circle().fill(Theme.tint).frame(width: 8, height: 8).position(x: last.x - 3, y: last.y) }
            }
            .mask(LinearGradient(stops: [.init(color: .clear, location: 0.15), .init(color: .black, location: 0.55)], startPoint: .leading, endPoint: .trailing))
            .animation(.easeOut(duration: 0.8), value: values)
        }
    }
}

/// 空闲页：带暖光的放置区（⌘V 或拖入），下面列支持的来源和累计下载
private struct EmptyState: View {
    @ObservedObject var store: DownloadStore
    private let sources = [L("磁力"), L("种子"), "ed2k", L("直链"), L("网盘"), L("B 站"), "YouTube", "X", L("微博"), L("小红书"), L("新片场"), L("抖音"), L("豆瓣预告"), L("微信视频号")]

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 22, style: .continuous)
        VStack(spacing: 18) {
            VStack(spacing: 14) {
                Image(nsImage: BrandGlyph.image(height: 60)).foregroundStyle(Theme.tint)
                    .shadow(color: Theme.tint.opacity(0.45), radius: 14)
                Text("复制链接后按 ⌘V 开始下载").font(.system(size: 20, weight: .bold))
                Text("也可以拖入链接或种子文件；视频号会显示操作指引").font(.system(size: 13.5)).foregroundStyle(Theme.sub)
                FlowChips(items: sources)
            }
            .frame(maxWidth: .infinity).padding(.vertical, 48)
            .background(
                shape.fill(RadialGradient(colors: [Theme.tint.opacity(0.12), Theme.tint.opacity(0.02)], center: .top, startRadius: 0, endRadius: 420))
            )
            .overlay(shape.strokeBorder(Theme.tint.opacity(0.35), style: StrokeStyle(lineWidth: 1.2, dash: [7, 6])))
        }
        .frame(maxWidth: 760)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(.bottom, 60)
    }
}

// MARK: - 任务行

/// 缩略图统一成三种样子：电影海报（模糊铺底 + 整张海报居中）、截帧或站点封面（铺满）、都没有时的标题卡
private struct Thumb: View {
    @ObservedObject var thumbs: Thumbnails
    let item: Item
    var width: CGFloat = 112
    var body: some View {
        let h = width * 9 / 16
        ZStack {
            if let poster = thumbs.poster(for: item.id) {
                Image(nsImage: poster).resizable().aspectRatio(contentMode: .fill).blur(radius: width / 10).opacity(0.85)
                Color.black.opacity(0.12)
                Image(nsImage: poster).resizable().aspectRatio(contentMode: .fit)
                    .shadow(color: .black.opacity(0.35), radius: 4)
                    .padding(.vertical, h * 0.06)
            } else if let img = thumbs.image(for: item.id) {
                Image(nsImage: img).resizable().aspectRatio(contentMode: .fill)
            } else {
                // 标题卡：琥珀渐变 + 片名，和有图的卡片一样有内容，不是一个空图标
                LinearGradient(colors: [Theme.tint.opacity(0.85), Theme.tint.opacity(0.55)], startPoint: .topLeading, endPoint: .bottomTrailing)
                VStack(alignment: .leading, spacing: 4) {
                    Image(systemName: item.category.symbol).font(.system(size: max(10, width / 14), weight: .semibold))
                    Text(item.title).font(.system(size: max(10, width / 11), weight: .bold)).lineLimit(width > 150 ? 3 : 2)
                        .multilineTextAlignment(.leading)
                }
                .foregroundStyle(.white.opacity(0.95))
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
                .padding(width > 150 ? 14 : 8)
            }
        }
        .frame(width: width, height: h)
        .clipShape(RoundedRectangle(cornerRadius: width > 150 ? 10 : 8, style: .continuous))
        .shadow(color: .black.opacity(0.10), radius: 6, y: 3)
    }
}

struct TaskRow: View {
    @ObservedObject var store: DownloadStore
    let item: Item
    @StateObject private var hover = HoverBox()

    var body: some View {
        HStack(spacing: 16) {
            Thumb(thumbs: store.thumbs, item: item)
            VStack(alignment: .leading, spacing: 3) {
                Text(item.title).font(.system(size: 15, weight: .medium)).lineLimit(1).truncationMode(.middle).help(item.rawTitle)
                HStack(spacing: 10) {
                    if case .thunder = item.source {
                        ThunderTag(active: item.isActive, downloaded: item.downloaded,
                                   createdAt: item.createdAt, located: item.thunderLocated)
                    } else { Text(L(item.originLabel ?? item.origin)) }
                    Text(sizeText)
                    if case .failed(let m) = item.status { Text(m).foregroundStyle(Theme.failed) }
                }
                .font(.system(size: 13)).foregroundStyle(Theme.sub).lineLimit(1)
                .frame(height: 18)
                ProgressBar(value: item.cloudProgress.map { Double($0) / 100 } ?? item.progress, dim: item.status == .paused || item.isResolving || item.checking,
                            busy: item.videoStage == .preparing,
                            finishing: item.videoStage == .merging || item.videoStage == .converting)
                    .padding(.top, 7)
            }
            VStack(alignment: .trailing, spacing: 3) {
                Text(L(primary)).font(.system(size: 15, weight: .medium)).monospacedDigit().lineLimit(1).minimumScaleFactor(0.8).frame(height: 20)
                if item.thunderClosed {
                    Button { store.openThunder() } label: {
                        Text("打开迅雷").font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.tint)
                    }
                    .buttonStyle(.plain)
                    .frame(height: 17)
                } else if item.slow, !item.link.isEmpty {
                    // BT 里几乎没人传：给个直接的出路（云盘离线；没登录就去设置）
                    Button {
                        if store.hasCloudRoute { store.sendToCloud(item) } else { store.offerRescue(item) }
                    } label: {
                        Text(store.hasCloudRoute ? L("太慢？云盘离线") : L("下不动？换条路")).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.tint)
                    }
                    .buttonStyle(.plain)
                    .help("这个资源在 BT 网络里几乎没人分享；交给 PikPak 服务器下好，再取回本机")
                    .frame(height: 17)
                } else {
                    // 第二行固定占位：文字有时为空，高度变了第一行会上下跳
                    Text(L(secondary)).font(.system(size: 13)).foregroundStyle(Theme.sub).monospacedDigit().lineLimit(1).frame(height: 17)
                }
            }
            .frame(width: 140, alignment: .trailing)
            HStack(spacing: 6) {
                if item.canStream {
                    CircleIconButton(symbol: store.buffering.contains(item.id) ? "hourglass" : "play.tv",
                                     help: store.buffering.contains(item.id) ? L("正在缓冲片头") : L("边下边播：一边下载一边观看")) { store.playWhileDownloading(item) }
                }
                if case .thunder = item.source, item.isActive {
                    // 迅雷那边的任务：边下边播直接打开迅雷已写好的部分；暂停到迅雷里操作
                    if item.file != nil { CircleIconButton(symbol: "play.tv", help: L("边下边播：一边下载一边观看")) { store.playThunderFile(item) } }
                    CircleIconButton(symbol: "arrow.uturn.backward", help: L("收回到闪电：保留迅雷已下的部分，改由闪电接着下")) { store.reclaimFromThunder(item) }
                    if ThunderBridge.installed {
                        CircleIconButton(symbol: "arrow.up.forward.app", help: L("打开迅雷查看这条任务")) { store.openThunder() }
                    }
                }
                if case .engine = item.source {
                    CircleIconButton(symbol: item.status == .paused || item.isFailed ? "play.fill" : "pause.fill",
                                     help: item.status == .paused ? L("继续下载") : item.isFailed ? L("重新开始") : L("暂停下载")) { store.togglePause(item) }
                } else if item.isFailed {
                    CircleIconButton(symbol: "arrow.clockwise", help: L("重试")) { store.retry(item) }
                }
                CircleIconButton(symbol: "xmark", help: L("移除这条任务")) { store.removalTarget = item }
            }
            .frame(width: 164, alignment: .trailing)
            .opacity(hover.on || selected ? 1 : 0)
        }
        .padding(.vertical, 10).padding(.horizontal, 10)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous)
            .fill(store.flashID == item.id ? Theme.tint.opacity(0.22) : (selected ? Theme.tint.opacity(0.13) : .clear)))
        .overlay {
            // 刚被「定位」过来的那条：描一圈亮边，眼睛能跟上（重复添加链接时）
            if store.flashID == item.id {
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .strokeBorder(Theme.tint.opacity(0.9), lineWidth: 2)
            }
        }
        .animation(.easeInOut(duration: 0.45), value: store.flashID == item.id)
        .overlay(alignment: .top) { Rectangle().fill(Theme.line).frame(height: 0.5).padding(.horizontal, 10).opacity(selected ? 0 : 1) }
        .contentShape(Rectangle())
        .onHover { hover.on = $0 }
        // 单击立即选中（不等双击判定），双击暂停 / 继续
        .onTapGesture(count: 2) { store.primaryAction(item) }
        .simultaneousGesture(TapGesture().onEnded { store.selectedID = item.id })
        .contextMenu { ItemMenu(store: store, item: item) }
    }

    private var selected: Bool { store.selectedID == item.id }

    private var sizeText: String {
        if item.total == 0 { return "" }
        if item.thunderDiskEstimate { return String(localized: "占用约 \(item.downloaded.bytesText) / \(item.total.bytesText)") }
        if item.fromThunder, item.checking { return String(localized: "已取回 \(item.downloaded.bytesText) / \(item.total.bytesText)") }
        if item.fromThunder, item.isActive { return String(localized: "已选文件 \(item.downloaded.bytesText) / \(item.total.bytesText)") }
        return item.downloaded >= item.total ? item.total.bytesText : "\(item.downloaded.bytesText) / \(item.total.bytesText)"
    }
    private var primary: String {
        switch item.status {
        case .running where item.videoStage == .preparing: L("正在解析")
        case .running where item.videoStage == .merging: L("正在合并")
        case .running where item.videoStage == .converting: L("正在转换")
        case .running where item.thunderClosed: L("迅雷没打开")
        case .running where item.isThunder: item.avgSpeed > 0 ? L("迅雷下载中") : L("请查看迅雷")
        case .running where item.cloudProgress == 100: L("云端已下好")
        case .running where item.cloudProgress != nil: L("云端下载中")
        case .running where item.fromThunder && item.checking: L("正在取回迅雷素材")
        case .running: item.verificationFailed ? L("检查失败") : item.checking ? L("正在检查") : item.avgSpeed.speedText
        case .paused: L("已暂停")
        case .waiting: L("排队中")
        case .resolving: L("正在解析")
        case .failed: L("失败")
        case .done: L("已完成")
        }
    }
    private var secondary: String {
        switch item.status {
        case .running where item.cloudProgress == 100: L("正在准备取回本机")
        case .running where item.cloudProgress != nil: String(localized: "\(item.cloudProgress ?? 0)%，完成后取回本机")
        case .running where item.isThunder: item.avgSpeed > 0 ? String(localized: "写入约 \(item.avgSpeed.speedText)") : L("状态以迅雷为准")
        case .running where store.buffering.contains(item.id): L("正在打开播放器")
        case .running where item.verificationFailed: L("请重新检查已下载文件")
        case .running where item.checking: item.fromThunder ? L("取完自动继续下载") : L("正在检查已下载的文件")
        case .running where item.waitingLastPiece: L("漏了最后一小块，正在补")
        case .running where item.videoStage == .preparing: ""   // 标题已写「正在读取视频信息」
        case .running where item.videoStage == .merging: L("合并视频和音频")
        case .running where item.videoStage == .converting: L("转换为 M4A")
        case .running where item.videoStage == .audio && item.avgSpeed == 0: L("正在下载音频")
        case .running:
            item.avgSpeed >= 1024 ? (remainingText(Double(item.total - item.downloaded) / Double(item.avgSpeed)).isEmpty ? L("速度较慢") : remainingText(Double(item.total - item.downloaded) / Double(item.avgSpeed))) : L("正在连接")
        case .resolving(let since):
            Date().timeIntervalSince(since) > 180 ? L("资源较冷门，仍在尝试") : L("正在寻找资源")
        case .paused, .waiting: item.total > 0 ? "\(Int(item.progress * 100))%" : ""
        default: ""
        }
    }
}

private extension Item {
    var isResolving: Bool { if case .resolving = status { return true }; return false }
    var isFailed: Bool { if case .failed = status { return true }; return false }
    var isThunder: Bool { if case .thunder = source { return true }; return false }
    /// 下载中的 BT 电影可以边下边播
    var canStream: Bool {
        guard case .engine = source, isActive, !isFailed, category == .movie else { return false }
        return origin == "磁力" || origin == "种子文件"
    }
}

/// 交给迅雷的任务：标明「这条在迅雷里下」。只写一个「迅雷」看不出是在下、在等还是下完了（11-01 用户反馈）。
/// 找不到迅雷的任务文件夹时量不到进度，就照实说「已交给迅雷」，不能写成「等待迅雷开始」
/// （11-01 实测：迅雷 2 分钟下完 589 MB，这边却一直显示在等）
private struct ThunderTag: View {
    let active: Bool
    let downloaded: Int64
    let createdAt: Date
    var located = true

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: symbol).font(.system(size: 9.5, weight: .bold))
            Text(L(text)).font(.system(size: 11.5, weight: .semibold))
        }
        .foregroundStyle(color)
        .padding(.horizontal, 7).frame(height: 18)
        .background(Capsule().fill(color.opacity(0.14)))
        .overlay(Capsule().strokeBorder(color.opacity(0.30), lineWidth: 0.8))
        .help(help)
    }

    private var text: String {
        guard active else { return L("迅雷下载完成") }
        if downloaded > 0 { return L("迅雷任务") }
        return L("请在迅雷查看")
    }
    private var symbol: String {
        guard active else { return "checkmark.circle.fill" }
        if downloaded > 0 { return "bolt.fill" }
        return located ? "clock" : "arrow.up.right.circle"
    }
    /// 只在真的在迅雷里跑起来时用主题琥珀色，其余灰色，避免「在等」看着像「在跑」
    private var color: Color {
        guard active, downloaded > 0 else { return Theme.sub }
        return Theme.tint
    }
    private var help: String {
        guard active, downloaded == 0 else { return text }
        return L("尚未读取到可靠进度，请到迅雷中查看任务状态")
    }
}

private struct ProgressBar: View {
    let value: Double
    let dim: Bool
    var busy = false       // 还没有字节进度（解析中）：一段亮条来回走，告诉用户还在干活
    var finishing = false  // 下完在合并：进度条满格、轻轻呼吸；不用来回走的亮条，那样看着像在往回倒（10-29 用户反馈）
    var body: some View {
        GeometryReader { g in
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.track)
                if busy {
                    TimelineView(.animation) { t in
                        let phase = (sin(t.date.timeIntervalSinceReferenceDate * 2.2) + 1) / 2
                        let w = g.size.width * 0.28
                        Capsule().fill(Theme.tint.opacity(0.75)).frame(width: w).offset(x: (g.size.width - w) * phase)
                    }
                } else if finishing {
                    TimelineView(.animation) { t in
                        let phase = (sin(t.date.timeIntervalSinceReferenceDate * 3) + 1) / 2
                        Capsule().fill(Theme.tint).opacity(0.55 + 0.45 * phase)
                    }
                } else {
                Capsule()
                    .fill(dim ? AnyShapeStyle(Theme.sub) : AnyShapeStyle(LinearGradient(colors: [Theme.tint.opacity(0.65), Theme.tint], startPoint: .leading, endPoint: .trailing)))
                    .frame(width: max(0, g.size.width * value))
                    .shadow(color: dim ? .clear : Theme.tint.opacity(0.45), radius: 4)
                    .animation(.easeOut(duration: 0.6), value: value)
                }
            }
        }
        .frame(height: 5)
    }
}

private struct ItemMenu: View {
    @ObservedObject var store: DownloadStore
    let item: Item
    var body: some View {
        if item.fileGone {
            Button("从列表移除") { store.remove(item, moveFiles: false) }
        } else {
            if !item.isActive { Button("打开") { store.open(item) } }
            Button("在访达中显示") { store.reveal(item) }
            if item.isFailed {
                Button("重试") { store.retry(item) }
            } else if case .engine = item.source, item.isActive {
                Button(item.status == .paused ? L("继续") : L("暂停")) { store.togglePause(item) }
            }
            if item.canStream { Button("边下边播") { store.playWhileDownloading(item) } }
            if case .engine = item.source, item.isActive {
                Button("重新检查已下载文件") { store.reverify(item) }
                    .disabled(item.checking)
            }
            if item.isThunder {
                if ThunderBridge.installed { Button("在迅雷中查看") { store.openThunder() } }
                Button("收回到闪电（保留迅雷已下的部分）") { store.reclaimFromThunder(item) }
                if item.thunderHasOriginal { Button("不用迅雷，从闪电原来的进度接着下") { store.restoreOriginalFromThunder(item) } }
                Button("改交云盘离线（PikPak）") { store.sendToCloud(item) }
            }
            if item.isActive, !item.link.isEmpty, item.origin == "磁力" || item.origin == "种子文件" {
                Button("交给云盘离线（PikPak）") { store.sendToCloud(item) }
                Button(ThunderBridge.installed ? L("用迅雷下载") : L("用迅雷下载（需先安装迅雷）…")) { store.handToThunder(item) }
            }
            Divider()
            Button("移除…") { store.removalTarget = item }
        }
    }
}

// MARK: - 已完成 / 分类

private struct DonePage: View {
    @ObservedObject var store: DownloadStore
    let items: [Item]
    var subtitle: String?       // 分类页里「已完成」小标题；页面标题在窗口工具栏
    var rowLayout = true
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let subtitle { Text(subtitle).font(.system(size: 15, weight: .semibold)).padding(.bottom, 10) }
            if items.isEmpty {
                Text("暂无").font(.system(size: 14)).foregroundStyle(Theme.sub)
                    .frame(maxWidth: .infinity, maxHeight: .infinity).padding(.bottom, 80)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        if rowLayout {
                            LazyVStack(spacing: 4) {
                                ForEach(items) { DoneRow(store: store, item: $0).id($0.id) }
                            }
                            .padding(.bottom, 24)
                        } else {
                            LazyVGrid(columns: [GridItem(.adaptive(minimum: 196, maximum: 240), spacing: 18, alignment: .top)], spacing: 22) {
                                ForEach(items) { DoneCard(store: store, item: $0).id($0.id) }
                            }
                            .padding(.bottom, 24)
                        }
                    }
                    .modifier(ScrollToTask(store: store, ids: items.map(\.id), proxy: proxy))
                }
            }
        }
    }
}

private struct DoneRow: View {
    @ObservedObject var store: DownloadStore
    let item: Item
    @State private var hovering = false
    var body: some View {
        HStack(spacing: 14) {
            Thumb(thumbs: store.thumbs, item: item, width: 112)
                .opacity(item.fileGone ? 0.45 : 1)
            VStack(alignment: .leading, spacing: 7) {
                HStack(alignment: .center, spacing: 8) {
                    Text(item.title).font(.system(size: 14.5, weight: .medium))
                        .lineLimit(1).truncationMode(.middle).help(item.rawTitle)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    HStack(spacing: 6) {
                        if !item.fileGone {
                            CircleIconButton(symbol: "folder", help: L("在访达中显示")) { store.reveal(item) }
                        }
                        CircleIconButton(symbol: "trash", help: L("移除")) { store.removalTarget = item }
                    }
                    .frame(width: 66, alignment: .trailing)
                    .opacity(hovering || store.selectedID == item.id ? 1 : 0)
                    .allowsHitTesting(hovering || store.selectedID == item.id)
                    Text(item.fileGone ? L("文件不在原位置") : L("已完成"))
                        .font(.system(size: 12.5)).foregroundStyle(Theme.sub)
                        .fixedSize()
                }
                HStack(spacing: 8) {
                    if !item.origin.isEmpty { Text(L(item.origin)) }
                    if item.total > 0 { Text(item.total.bytesText) }
                    if item.createdAt != .distantPast {
                        Text("添加于 " + item.createdAt.formatted(.dateTime.month().day().hour().minute()))
                    }
                }
                .font(.system(size: 12.5)).foregroundStyle(Theme.sub).lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(8)
        .background(RoundedRectangle(cornerRadius: 10).fill(Theme.tint.opacity(store.selectedID == item.id ? 0.12 : (hovering ? 0.04 : 0))))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture(count: 2) { store.primaryAction(item) }
        .simultaneousGesture(TapGesture().onEnded { store.selectedID = item.id })
        .contextMenu { ItemMenu(store: store, item: item) }
        .help(item.fileGone ? L("文件已不在原位置，可右键从列表移除") : L("双击打开"))
    }

}

private struct DoneCard: View {
    @ObservedObject var store: DownloadStore
    let item: Item
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            GeometryReader { g in Thumb(thumbs: store.thumbs, item: item, width: g.size.width) }
                .aspectRatio(16 / 9, contentMode: .fit)
                .opacity(item.fileGone ? 0.45 : 1)
                .overlay {
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .strokeBorder(Theme.tint,
                                      lineWidth: store.selectedID == item.id ? 3 : (store.flashID == item.id ? 2 : 0))
                }
                .animation(.easeInOut(duration: 0.45), value: store.flashID == item.id)
            Text(item.title).font(.system(size: 14, weight: .medium)).lineLimit(1).truncationMode(.middle).padding(.top, 8).help(item.rawTitle)
            HStack(spacing: 10) {
                if item.fileGone {
                    Text("文件已删除")
                } else {
                    Text(item.total.bytesText)
                    Text(L(item.origin))
                }
            }
            .font(.system(size: 12.5)).foregroundStyle(Theme.sub).padding(.top, 2)
        }
        .contentShape(Rectangle())
        .onTapGesture(count: 2) { store.primaryAction(item) }
        .simultaneousGesture(TapGesture().onEnded { store.selectedID = item.id })
        .contextMenu { ItemMenu(store: store, item: item) }
        .help(item.fileGone ? L("文件已不在原位置，可右键从列表移除") : L("双击打开"))
    }
}

/// 分类页：进行中的用任务行，下完的用画面卡片
private struct CategoryPage: View {
    @ObservedObject var store: DownloadStore
    let category: Category
    @AppStorage("completedRowLayout") private var rowLayout = true
    var body: some View {
        let all = store.items(in: category)
        let active = all.filter(\.isActive)
        VStack(alignment: .leading, spacing: 0) {
            if !active.isEmpty {
                Text("进行中").font(.system(size: 15, weight: .semibold)).padding(.bottom, 6)
                ForEach(active) { TaskRow(store: store, item: $0) }
                Spacer().frame(height: 20)
                DonePage(store: store, items: all.filter { !$0.isActive }, subtitle: L("已完成"), rowLayout: rowLayout)
            } else {
                DonePage(store: store, items: all, rowLayout: rowLayout).padding(.top, 8)
            }
        }
    }
}

// MARK: - 新建下载

private struct NewDownloadSheet: View {
    @ObservedObject var store: DownloadStore
    var body: some View {
        let links = LinkKind.extract(from: store.newLinksText)
        VStack(alignment: .leading, spacing: 14) {
            Text("新建下载").font(.system(size: 15, weight: .semibold))
            TextEditor(text: $store.newLinksText)
                .font(.system(size: 12.5, design: .monospaced))
                .scrollContentBackground(.hidden)
                .padding(8)
                .frame(height: 120)
                .background(RoundedRectangle(cornerRadius: 8).fill(Theme.faint))
                .overlay(alignment: .topLeading) {
                    if store.newLinksText.isEmpty {
                        Text("粘贴磁力、网盘直链或视频网页链接，一行一个").font(.system(size: 12.5)).foregroundStyle(Theme.sub)
                            .padding(.horizontal, 13).padding(.vertical, 8).allowsHitTesting(false)
                    }
                }
            if !links.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(links.prefix(5), id: \.self) { l in
                        let kind = LinkKind.of(l)
                        HStack(spacing: 8) {
                            Text(label(kind)).font(.system(size: 11, weight: .medium)).foregroundStyle(Theme.sub).frame(width: 64, alignment: .leading)
                            Text(DownloadStore.displayName(l)).font(.system(size: 12)).lineLimit(1).truncationMode(.middle)
                            Spacer()
                            Text("存到 " + folderName(kind)).font(.system(size: 11)).foregroundStyle(Theme.sub)
                        }
                    }
                    if links.count > 5 { Text("等 \(links.count) 个链接").font(.system(size: 11)).foregroundStyle(Theme.sub) }
                }
            }
            HStack {
                Spacer()
                Button("取消") { store.showNewSheet = false }.keyboardShortcut(.cancelAction)
                // 开着确认框时下一步是确认框（选位置、种子文件、视频画质）
                Button(NewTaskPanels.ask ? L("下一步") : links.count > 1 ? String(localized: "下载 \(links.count) 项") : L("下载")) { store.submitNewSheet() }
                    .keyboardShortcut(.defaultAction).buttonStyle(.borderedProminent).tint(Theme.tint)
                    .disabled(links.isEmpty)
            }
        }
        .padding(20)
        .frame(width: 520)
        .onAppear {
            if store.newLinksText.isEmpty, let s = NSPasteboard.general.string(forType: .string), !LinkKind.extract(from: s).isEmpty {
                store.newLinksText = s
            }
        }
    }
    private func label(_ k: LinkKind?) -> String {
        switch k {
        case .magnet: L("磁力")
        case .torrentFile: L("种子文件")
        case .ed2k: "ed2k"
        case .videoSite: L("视频网站")
        case .http: L("直链")
        case nil: L("无法识别")
        }
    }
    private func folderName(_ k: LinkKind?) -> String { k.map { Folders.displayName(Folders.directory(for: $0)) } ?? "—" }
}

// MARK: - 回收站

private struct TrashPage: View {
    @ObservedObject var store: DownloadStore
    @State private var confirmClear = false
    @State private var selectedID: String?
    @AppStorage("completedRowLayout") private var rowLayout = true
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("移除的任务保留在这里，可以重新下载或找到文件").font(.system(size: 14)).foregroundStyle(Theme.sub)
                Spacer()
                if !store.removedRecords.isEmpty {
                    // 系统按钮的浅灰底在暖色背景上看不出来（10-29 用户反馈）：同「新建下载」的橙底白字，不另起红色系
                    Button { confirmClear = true } label: {
                        Label("清空回收站", systemImage: "trash").labelStyle(.titleAndIcon).font(.system(size: 13, weight: .semibold))
                            .padding(.horizontal, 4)
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
            .padding(.vertical, 12)
            .alert(L("清空回收站？"), isPresented: $confirmClear) {
                Button("清空", role: .destructive) { store.clearRecords() }
                Button("取消", role: .cancel) {}
            } message: {
                Text("\(store.removedRecords.count) 条记录将被删除，之后不能再从这里重新下载。已下载的文件不受影响。")
            }
            if store.removedRecords.isEmpty {
                Text("回收站是空的").font(.system(size: 14)).foregroundStyle(Theme.sub)
                    .frame(maxWidth: .infinity, maxHeight: .infinity).padding(.bottom, 80)
            } else {
                ScrollView {
                    if rowLayout {
                        LazyVStack(spacing: 0) {
                            ForEach(store.removedRecords) { r in TrashRow(store: store, r: r, selectedID: $selectedID) }
                        }
                    } else {
                        LazyVGrid(columns: [GridItem(.adaptive(minimum: 196, maximum: 240), spacing: 18)], spacing: 18) {
                            ForEach(store.removedRecords) { r in TrashCard(store: store, r: r, selectedID: $selectedID) }
                        }
                    }
                }
            }
        }
    }
}

private struct TrashRow: View {
    @ObservedObject var store: DownloadStore
    let r: DownloadStore.RemovedRecord
    @Binding var selectedID: String?
    @State private var hovering = false
    var body: some View {
        let exists = r.path.map { FileManager.default.fileExists(atPath: $0) } ?? false
        HStack(spacing: 14) {
            VStack(alignment: .leading, spacing: 3) {
                Text(r.title).font(.system(size: 15, weight: .medium)).lineLimit(1).truncationMode(.middle)
                HStack(spacing: 12) {
                    Text(L(r.origin))
                    if r.size > 0 { Text(r.size.bytesText) }
                    Text(r.removedAt.formatted(date: .abbreviated, time: .shortened) + L(" 移除"))
                    Text(r.filesTrashed ? L("文件已移到废纸篓") : exists ? L("文件还在原位置") : L("文件已不在"))
                }
                .font(.system(size: 13)).foregroundStyle(Theme.sub).lineLimit(1)
            }
            Spacer()
            // 和下载列表同一套圆形按钮，悬停显示说明
            HStack(spacing: 6) {
                if !r.link.isEmpty { CircleIconButton(symbol: "arrow.clockwise", help: L("重新下载")) { store.redownload(r) } }
                if exists, let p = r.path {
                    CircleIconButton(symbol: "folder", help: L("在访达中显示")) { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: p)]) }
                }
                CircleIconButton(symbol: "xmark", help: L("从回收站删除这条记录")) { store.deleteRecord(r) }
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 12)
        .background(RoundedRectangle(cornerRadius: 10).fill(Theme.tint.opacity(selectedID == r.id ? 0.12 : (hovering ? 0.04 : 0))))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture { selectedID = r.id }
        .contextMenu {
            if !r.link.isEmpty { Button("重新下载") { store.redownload(r) } }
            if exists, let p = r.path { Button("在访达中显示") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: p)]) } }
            Divider()
            Button("从回收站删除") { store.deleteRecord(r) }
        }
    }
}

private struct TrashCard: View {
    @ObservedObject var store: DownloadStore
    let r: DownloadStore.RemovedRecord
    @Binding var selectedID: String?
    @State private var hovering = false
    var body: some View {
        let exists = r.path.map { FileManager.default.fileExists(atPath: $0) } ?? false
        VStack(alignment: .leading, spacing: 8) {
            Image(systemName: "doc.fill").font(.system(size: 26)).foregroundStyle(Theme.sub)
                .frame(maxWidth: .infinity, minHeight: 88)
                .background(RoundedRectangle(cornerRadius: 8).fill(Theme.faint))
            Text(r.title).font(.system(size: 14, weight: .medium)).lineLimit(2).help(r.title)
            Text(r.origin + (r.size > 0 ? " · " + r.size.bytesText : ""))
                .font(.system(size: 12)).foregroundStyle(Theme.sub).lineLimit(1)
            Text(r.filesTrashed ? L("文件已移到废纸篓") : exists ? L("文件还在原位置") : L("文件已不在"))
                .font(.system(size: 12)).foregroundStyle(Theme.sub).lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(8)
        .background(RoundedRectangle(cornerRadius: 10).fill(Theme.tint.opacity(selectedID == r.id ? 0.12 : (hovering ? 0.04 : 0))))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture { selectedID = r.id }
        .contextMenu {
            if !r.link.isEmpty { Button("重新下载") { store.redownload(r) } }
            if exists, let p = r.path { Button("在访达中显示") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: p)]) } }
            Divider()
            Button("从回收站删除") { store.deleteRecord(r) }
        }
    }
}
