import AppKit
import SwiftUI

extension Color {
    static func adaptive(light: NSColor, dark: NSColor) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
        })
    }
}

private func rgb(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, _ a: CGFloat = 1) -> NSColor {
    NSColor(srgbRed: r / 255, green: g / 255, blue: b / 255, alpha: a)
}

/// 效果图 v3（2026-10-27 定稿）：整体黑白灰，唯一的颜色是琥珀色（呼应「闪电」），只用在侧栏图标、进度条、速度曲线
enum Theme {
    static let tint = Color.adaptive(light: rgb(232, 154, 0), dark: rgb(255, 181, 38))
    static let window = Color.adaptive(light: rgb(251, 250, 248), dark: rgb(27, 27, 29))
    static let text = Color.adaptive(light: rgb(29, 29, 31), dark: rgb(245, 245, 247))
    static let sub = Color.adaptive(light: rgb(138, 138, 142), dark: rgb(152, 152, 157))
    static let line = Color.adaptive(light: rgb(0, 0, 0, 0.07), dark: rgb(255, 255, 255, 0.07))
    static let faint = Color.adaptive(light: rgb(0, 0, 0, 0.045), dark: rgb(255, 255, 255, 0.06))
    static let track = Color.adaptive(light: rgb(0, 0, 0, 0.07), dark: rgb(255, 255, 255, 0.10))
    static let glow = Color.adaptive(light: rgb(232, 154, 0, 0.13), dark: rgb(255, 181, 38, 0.14))
    static let raised = Color.adaptive(light: rgb(255, 255, 255), dark: rgb(44, 44, 47))   // 卡片
    static let failed = Color.adaptive(light: rgb(215, 58, 46), dark: rgb(255, 105, 97))
}

extension View {
    /// macOS 26 液态玻璃；更早的系统用磨砂材质
    @ViewBuilder
    func glassPanel(radius: CGFloat) -> some View {
        let shape = RoundedRectangle(cornerRadius: radius, style: .continuous)
        if #available(macOS 26.0, *) {
            self.glassEffect(.regular, in: shape)
        } else {
            self.background(.regularMaterial, in: shape)
                .overlay(shape.strokeBorder(Theme.line, lineWidth: 0.5))
        }
    }
}

/// 鼠标悬停状态放在单独的小对象里，只让这一行重画
final class HoverBox: ObservableObject { @Published var on = false }

/// GitHub 标记（官方 SVG，模板着色，随前景色变化）。SF Symbols 里没有，只能内置
struct GitHubMark: View {
    var size: CGFloat = 14

    private static let image: NSImage? = {
        let svg = ##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="16" height="16"><path fill="#000" d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/></svg>"##
        let image = NSImage(data: Data(svg.utf8))
        image?.isTemplate = true
        return image
    }()

    var body: some View {
        if let image = Self.image {
            Image(nsImage: image).resizable().interpolation(.high).frame(width: size, height: size)
        } else {
            Image(systemName: "chevron.left.forwardslash.chevron.right").font(.system(size: size * 0.8, weight: .semibold))
        }
    }
}

/// 按系统语言取界面文字；表里没有就用中文原文。键见 Resources/en.lproj/Localizable.strings。
/// 只用于显示，别把结果拿去做判断
func L(_ text: String) -> String { Bundle.main.localizedString(forKey: text, value: text, table: nil) }

/// 悬浮提示。全 App 只用这一种：系统同款的浅色小标签，出现在按钮下方。
/// 不用系统 .help：列表每秒刷新，系统提示等不到出现就被打断（11-05 用户：悬停没反应）；
/// 放在独立小窗里：不会盖住行内的状态文字，也不会被列表边缘裁掉
@MainActor
enum HoverTip {
    private static var panel: NSPanel?
    private static var pending: DispatchWorkItem?

    static func show(_ text: String, below view: NSView?) {
        hide()
        let work = DispatchWorkItem { present(text, below: view) }
        pending = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25, execute: work)
    }

    static func hide() {
        pending?.cancel(); pending = nil
        panel?.orderOut(nil)
    }

    private static func present(_ text: String, below view: NSView?) {
        guard let view, let window = view.window else { return }
        let host = NSHostingView(rootView: Text(L(text)).font(.system(size: 12)).foregroundStyle(.primary)
            .padding(.horizontal, 9).padding(.vertical, 5)
            .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(Color(nsColor: .windowBackgroundColor)))
            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(Color.primary.opacity(0.15), lineWidth: 0.5))
            .fixedSize())
        let size = host.fittingSize
        host.frame = NSRect(origin: .zero, size: size)
        let tip = panel ?? {
            let new = NSPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
            new.isOpaque = false; new.backgroundColor = .clear; new.hasShadow = true
            new.ignoresMouseEvents = true; new.level = .popUpMenu; new.isReleasedWhenClosed = false
            panel = new
            return new
        }()
        tip.contentView = host
        let anchor = window.convertToScreen(view.convert(view.bounds, to: nil))
        let screen = window.screen?.visibleFrame ?? anchor
        var origin = CGPoint(x: anchor.midX - size.width / 2, y: anchor.minY - size.height - 5)
        origin.x = min(max(origin.x, screen.minX + 6), screen.maxX - size.width - 6)
        if origin.y < screen.minY { origin.y = anchor.maxY + 5 }
        tip.setFrame(NSRect(origin: origin, size: size), display: true)
        tip.orderFront(nil)
    }
}

/// 记下按钮对应的 NSView，提示才知道显示在哪
final class TipAnchorBox: ObservableObject { weak var view: NSView? }

private struct TipAnchor: NSViewRepresentable {
    let box: TipAnchorBox
    func makeNSView(context: Context) -> NSView { let v = NSView(); box.view = v; return v }
    func updateNSView(_ v: NSView, context: Context) { box.view = v }
}

/// 小圆形图标按钮（暂停、移除）
struct CircleIconButton: View {
    let symbol: String
    let help: String
    let action: () -> Void
    @StateObject private var anchor = TipAnchorBox()
    var body: some View {
        Button {
            HoverTip.hide()
            action()
        } label: {
            Image(systemName: symbol)
                .font(.system(size: 13, weight: .bold))
                .foregroundStyle(Theme.text.opacity(0.8))
                .frame(width: 34, height: 34)
                .background(Circle().fill(Color.primary.opacity(0.09)))  // Theme.faint 在暖色背景上太淡
        }
        .buttonStyle(.plain)
        .accessibilityLabel(help)
        .background(TipAnchor(box: anchor))
        .onHover { inside in
            if inside { HoverTip.show(help, below: anchor.view) } else { HoverTip.hide() }
        }
        .onDisappear { HoverTip.hide() }
    }
}
