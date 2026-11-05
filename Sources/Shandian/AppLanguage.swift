import AppKit
import Foundation

/// 界面语言。写进本 App 的 AppleLanguages，下次启动生效：系统菜单、表里的文字和格式都跟着变，界面不会中英混排
enum AppLanguage: String, CaseIterable, Identifiable {
    case system, chinese, english
    var id: String { rawValue }

    var label: String {
        switch self {
        case .system: L("跟随系统")
        case .chinese: "简体中文"
        case .english: "English"
        }
    }

    private static let key = "AppleLanguages"
    private var codes: [String]? {
        switch self {
        case .system: nil
        case .chinese: ["zh-Hans"]
        case .english: ["en"]
        }
    }

    /// 只读本 App 自己存的值；UserDefaults.object 会读到系统全局语言，分不清用户选过没有
    static var current: AppLanguage {
        guard let domain = Bundle.main.bundleIdentifier,
              let saved = UserDefaults.standard.persistentDomain(forName: domain)?[key] as? [String] else { return .system }
        return allCases.first { $0.codes == saved } ?? .system
    }

    func save() {
        if let codes { UserDefaults.standard.set(codes, forKey: Self.key) }
        else { UserDefaults.standard.removeObject(forKey: Self.key) }
    }

    /// 问用户要不要现在重开。退出时下载中的任务会暂停，重开后继续（和升级走同一条路）
    static func askToReopen() {
        let alert = NSAlert()
        alert.messageText = L("需要重新打开才能切换语言")
        alert.informativeText = L("下载中的任务会先暂停，重新打开后继续。")
        alert.addButton(withTitle: L("立即重新打开"))
        alert.addButton(withTitle: L("稍后"))
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; open \"$2\"",
                             "shandian-language", String(getpid()), Bundle.main.bundleURL.path]
        guard (try? process.run()) != nil else { return }
        NSApp.terminate(nil)
    }
}
