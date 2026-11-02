import AppKit

/// 只有用户提交视频号链接时才出现；不会自动向第三方提交链接。
@MainActor
enum WeChatGuide {
    static func present(share: String) {
        let alert = NSAlert()
        alert.messageText = "下载微信视频号"
        alert.informativeText = "1. 打开并登录腾讯元宝。\n2. 点击浏览器工具栏里的「闪电全能下载」扩展，选择「下载视频号」。\n3. 粘贴分享链接，点击「解析并下载视频号」。\n\n点击下方按钮会复制链接并打开元宝。链接只在你点击解析后交给腾讯元宝。"
        alert.addButton(withTitle: "复制链接并打开元宝")
        alert.addButton(withTitle: "取消")
        NSApp.activate(ignoringOtherApps: true)
        let finish: (NSApplication.ModalResponse) -> Void = { response in
            guard response == .alertFirstButtonReturn else { return }
            var parts = URLComponents(string: share)
            parts?.scheme = "https"
            let board = NSPasteboard.general
            board.clearContents()
            board.setString(parts?.string ?? share, forType: .string)
            let target = URL(string: "https://yuanbao.tencent.com/")!
            if let edge = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.microsoft.edgemac") {
                NSWorkspace.shared.open([target], withApplicationAt: edge, configuration: NSWorkspace.OpenConfiguration())
            } else { NSWorkspace.shared.open(target) }
        }
        if let window = NSApp.keyWindow ?? NSApp.mainWindow { alert.beginSheetModal(for: window, completionHandler: finish) }
        else { finish(alert.runModal()) }
    }
}
