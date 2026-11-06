import AppKit
import UniformTypeIdentifiers

/// 磁力、ed2k 链接和种子文件的默认打开方式。照迅雷：浏览器里一点就弹出下载软件。
/// 只在用户同意后改（首次打开问一次，或设置里点「设为默认」），不悄悄抢别的软件的关联。
@MainActor
enum DefaultHandler {
    static let askedKey = "askedDefaultHandler"
    private static let schemes = ["magnet", "ed2k"]
    private static let torrent = UTType("org.bittorrent.torrent") ?? UTType(filenameExtension: "torrent")

    static var isDefault: Bool {
        let me = Bundle.main.bundleURL.standardizedFileURL
        let ownsSchemes = schemes.allSatisfy { scheme in
            URL(string: "\(scheme)://probe").flatMap { NSWorkspace.shared.urlForApplication(toOpen: $0) }?.standardizedFileURL == me
        }
        let ownsTorrent = torrent.flatMap { NSWorkspace.shared.urlForApplication(toOpen: $0) }?.standardizedFileURL == me
        return ownsSchemes && ownsTorrent
    }

    /// 系统可能会弹框请用户确认，所以结果以之后再查 `isDefault` 为准
    static func makeDefault() async {
        let me = Bundle.main.bundleURL
        for scheme in schemes { try? await NSWorkspace.shared.setDefaultApplication(at: me, toOpenURLsWithScheme: scheme) }
        if let torrent { try? await NSWorkspace.shared.setDefaultApplication(at: me, toOpen: torrent) }
    }

    /// 首次打开问一次；已经是默认或问过就不再问
    static func askOnce() {
        guard !UserDefaults.standard.bool(forKey: askedKey) else { return }
        UserDefaults.standard.set(true, forKey: askedKey)
        guard !isDefault else { return }
        let alert = NSAlert()
        alert.messageText = L("用闪电全能下载打开磁力链接和种子文件？")
        alert.informativeText = L("在浏览器里点磁力链接或种子文件时，直接交给闪电全能下载。之后可在设置的「通用」中更改。")
        alert.addButton(withTitle: L("设为默认"))
        alert.addButton(withTitle: L("暂不"))
        NSApp.activate(ignoringOtherApps: true)
        if alert.runModal() == .alertFirstButtonReturn { Task { await makeDefault() } }
    }
}
