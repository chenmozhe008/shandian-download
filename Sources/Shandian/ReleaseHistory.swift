import Foundation

struct HistoryEntry: Identifiable, Equatable {
    let version: String
    let date: String
    let notes: [String]
    let page: URL?
    let isDevelopment: Bool
    var id: String { version + (isDevelopment ? "-dev" : "-release") }
}

enum ReleaseHistory {
    static func local(_ text: String, currentVersion: String, isDevelopment: Bool) -> [HistoryEntry] {
        var rows: [(String, [String])] = []
        for line in text.components(separatedBy: .newlines) {
            if line.hasPrefix("## ") { rows.append((String(line.dropFirst(3)), [])) }
            else if line.hasPrefix("- "), !rows.isEmpty { rows[rows.count - 1].1.append(String(line.dropFirst(2))) }
        }
        return rows.map { title, notes in
            let parts = title.components(separatedBy: " · ")
            let version = parts[0].trimmingCharacters(in: .whitespaces)
            let date = parts.dropFirst().first?.replacingOccurrences(of: "（开发版）", with: "") ?? ""
            return HistoryEntry(version: version, date: date, notes: notes, page: nil,
                                isDevelopment: title.contains("开发版") || (version == currentVersion && isDevelopment))
        }
    }

    static func online(_ data: Data) -> [HistoryEntry]? {
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]] else { return nil }
        return json.compactMap { item in
            guard item["draft"] as? Bool == false, item["prerelease"] as? Bool == false,
                  let tag = item["tag_name"] as? String, tag.hasPrefix("v"),
                  tag.dropFirst().split(separator: ".").allSatisfy({ Int($0) != nil }),
                  let address = item["html_url"] as? String, let page = URL(string: address) else { return nil }
            let body = item["body"] as? String ?? ""
            let allNotes = body.components(separatedBy: .newlines).filter { $0.hasPrefix("- ") }.map { String($0.dropFirst(2)).replacingOccurrences(of: "**", with: "") }
            let sectionNotes = UpdateChecker.notes(body)
            return HistoryEntry(version: String(tag.dropFirst()), date: String((item["published_at"] as? String ?? "").prefix(10)), notes: sectionNotes.isEmpty ? allNotes : sectionNotes, page: page, isDevelopment: false)
        }
    }

    static func merge(local: [HistoryEntry], online: [HistoryEntry]) -> [HistoryEntry] {
        var result = online
        for row in local {
            // 正式版以 GitHub 发布说明为准：旧安装包带的本机记录可能不全，网上的才是汇总后的
            if row.isDevelopment { result.append(row) }
            else if let index = result.firstIndex(where: { $0.version == row.version }) {
                let published = result[index]
                result[index] = HistoryEntry(version: row.version, date: published.date.isEmpty ? row.date : published.date,
                                             notes: published.notes.isEmpty ? row.notes : published.notes, page: published.page, isDevelopment: false)
            } else { result.append(row) }
        }
        // 英文界面不显示还没有英文说明的旧版本，免得中英混排（11-05 用户定）
        if AppLanguage.isEnglish { result.removeAll { $0.notes.isEmpty || $0.notes.joined().range(of: "\\p{Han}", options: .regularExpression) != nil } }
        return result.sorted {
            if $0.version == $1.version { return $0.isDevelopment && !$1.isDevelopment }
            return UpdateChecker.isNewer($0.version, than: $1.version)
        }
    }
}
