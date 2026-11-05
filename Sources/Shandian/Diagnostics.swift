import AppKit
import UniformTypeIdentifiers

/// 导出诊断信息：用户反馈问题时附上。只写运行状态，路径、片名、链接、令牌一律去掉，发出去不泄露隐私
enum Diagnostics {
    static let exportRequested = Notification.Name("ShandianExportDiagnostics")

    /// 日志里可能带本机路径（含片名）、下载链接和令牌
    static func sanitize(_ text: String, secrets: [String] = []) -> String {
        var result = text
        for secret in secrets where !secret.isEmpty {
            result = result.replacingOccurrences(of: secret, with: "<令牌>")
        }
        let rules: [(String, String)] = [
            (#"magnet:\?[^\s"']+"#, "<磁力链接>"),
            (#"ed2k://[^\s"']+"#, "<ed2k 链接>"),
            (#"data:[a-zA-Z/+.\-]+;base64,[^\s"']+"#, "<种子数据>"),
            (#"(https?://[^/\s"']+)[^\s"']*"#, "$1/…"),
            // 路径可能带空格（Jack Feng、片名），一直删到引号或行尾；宁可多删，不能漏片名
            (#"(?:file://|(?<![\w.])/(?:Users|Volumes|private|tmp)/)[^"'\n]*"#, "<路径>"),
            (#"(?i)(token|authorization|cookie|password|secret)(["']?\s*[:=]\s*["']?)[^\s"',}]+"#, "$1$2<已隐藏>"),
            (#"\b[0-9a-fA-F]{40}\b"#, "<infohash>"),
        ]
        for (pattern, template) in rules {
            result = result.replacingOccurrences(of: pattern, with: template, options: .regularExpression)
        }
        return result
    }

    static func lastLines(_ text: String, count: Int) -> String {
        text.split(separator: "\n", omittingEmptySubsequences: false).suffix(count).joined(separator: "\n")
    }

    static func save(report: String) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "闪电全能下载-诊断信息.txt"
        panel.allowedContentTypes = [.plainText]
        NSApp.activate(ignoringOtherApps: true)
        guard panel.runModal() == .OK, let url = panel.url else { return }
        try? report.write(to: url, atomically: true, encoding: .utf8)
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }
}
