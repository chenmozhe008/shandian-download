import Foundation
import ServiceManagement

/// 登录时自动打开。不用 SMAppService：它要求 Apple 开发者身份，发布包在同类接口下会被系统杀掉（小清 2026-11-05 实测）。
/// 写一个普通的用户级 LaunchAgent，由系统下次登录时加载；App 不在了任务自己清除
enum LoginLaunch {
    private static let label = "io.github.chenmozhe008.shandian.login"
    private static var file: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/LaunchAgents/\(label).plist")
    }

    static let script = #"[ -d "$1" ] && exec /usr/bin/open -g "$1"; rm -f "$2""#

    static func plist(app: String, file: String) -> [String: Any] {
        ["Label": "io.github.chenmozhe008.shandian.login", "ProgramArguments": ["/bin/sh", "-c", script, "shandian-login", app, file],
         "RunAtLoad": true, "LimitLoadToSessionType": "Aqua"]
    }

    static var isEnabled: Bool { FileManager.default.fileExists(atPath: file.path) }

    static func set(_ on: Bool) throws {
        guard on else {
            if isEnabled { try FileManager.default.removeItem(at: file) }
            return
        }
        let data = try PropertyListSerialization.data(fromPropertyList: plist(app: Bundle.main.bundleURL.path, file: file.path), format: .xml, options: 0)
        try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: file, options: .atomic)
    }

    /// 启动时调用：接过旧版用 SMAppService 打开的设置；App 换了位置就更新路径
    static func migrate() {
        let old = SMAppService.mainApp
        if old.status == .enabled {
            try? old.unregister()
            try? set(true)
        } else if isEnabled {
            try? set(true)
        }
    }
}
