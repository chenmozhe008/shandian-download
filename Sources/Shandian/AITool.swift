import Foundation

/// 各家 AI 工具接入本机 MCP 的方式。OpenCode 的配置格式 V1 和 V2 不同，走「其他工具」
enum AITool: String, CaseIterable, Identifiable {
    case claudeCode, claudeDesktop, codex, chatGPT, cursor, trae, windsurf, vscode, gemini, other
    var id: String { rawValue }

    var name: String {
        switch self {
        case .claudeCode: "Claude Code"
        case .claudeDesktop: "Claude Desktop"
        case .codex: "Codex"
        case .chatGPT: "ChatGPT"
        case .cursor: "Cursor"
        case .trae: "Trae"
        case .windsurf: "Windsurf"
        case .vscode: "VS Code"
        case .gemini: "Gemini CLI"
        case .other: L("其他工具")
        }
    }

    var isCommand: Bool { [.claudeCode, .codex, .gemini].contains(self) }
    var isPath: Bool { self == .chatGPT }

    var hint: String {
        switch self {
        case .claudeCode, .gemini: L("在终端运行这条命令，新开会话生效。")
        case .claudeDesktop: L("设置 → 开发者 → 编辑配置，粘贴到 claude_desktop_config.json，再重开 Claude。")
        case .codex: L("在终端运行这条命令。Codex 命令行和桌面版共用这份配置。")
        case .chatGPT: L("桌面版：设置 → MCP servers → Add server，类型选 STDIO，命令粘贴这个路径，保存后重启。")
        case .cursor: L("粘贴到 ~/.cursor/mcp.json，再重开 Cursor。")
        case .trae: L("设置 → MCP → 手动添加，粘贴后确认。")
        case .windsurf: L("在 Cascade 的 MCP 菜单打开配置文件，粘贴后刷新。")
        case .vscode: L("命令面板运行「MCP: Open User Configuration」，粘贴后启动服务。")
        case .other: L("其他支持 MCP 的工具大多认这份配置。")
        }
    }

    func snippet(path: String) -> String {
        let quoted = "'" + path.replacingOccurrences(of: "'", with: "'\\''") + "'"
        switch self {
        case .chatGPT: return path
        case .claudeCode: return "claude mcp add --scope user shandian -- \(quoted)"
        case .codex: return "codex mcp add shandian -- \(quoted)"
        case .gemini: return "gemini mcp add shandian \(quoted) --scope user"   // 选项放后面：放前面 Gemini 解析会出错
        case .vscode: return Self.json(["servers": ["shandian": ["type": "stdio", "command": path]]])
        default: return Self.json(["mcpServers": ["shandian": ["command": path]]])
        }
    }

    private static func json(_ object: [String: Any]) -> String {
        let data = (try? JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])) ?? Data()
        return String(decoding: data, as: UTF8.self)
    }
}
