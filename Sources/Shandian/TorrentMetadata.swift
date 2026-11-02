import CryptoKit
import Foundation

/// 从 BT 种子的 info 字典核对任务标识与完整大小；不读取文件名等信息。
enum TorrentMetadata {
    private indirect enum Value {
        case integer(Int64), bytes(Data), list([Value]), dictionary([String: Value])
    }
    private struct Parser {
        let bytes: [UInt8]
        var index = 0
        var infoRange: Range<Int>?
        mutating func value(depth: Int = 0) -> Value? {
            guard depth < 64, index < bytes.count else { return nil }
            switch bytes[index] {
            case 105:
                index += 1; let start = index
                while index < bytes.count, bytes[index] != 101 { index += 1 }
                guard index < bytes.count, let n = Int64(String(decoding: bytes[start..<index], as: UTF8.self)) else { return nil }
                index += 1; return .integer(n)
            case 108:
                index += 1; var values: [Value] = []
                while index < bytes.count, bytes[index] != 101 {
                    guard let v = value(depth: depth + 1) else { return nil }; values.append(v)
                }
                guard index < bytes.count else { return nil }; index += 1; return .list(values)
            case 100:
                index += 1; var values: [String: Value] = [:]
                while index < bytes.count, bytes[index] != 101 {
                    guard case .bytes(let keyData)? = value(depth: depth + 1),
                          let key = String(data: keyData, encoding: .utf8) else { return nil }
                    let start = index
                    guard let v = value(depth: depth + 1) else { return nil }
                    if depth == 0 && key == "info" { infoRange = start..<index }
                    values[key] = v
                }
                guard index < bytes.count else { return nil }; index += 1; return .dictionary(values)
            case 48...57:
                let start = index
                while index < bytes.count, bytes[index] >= 48, bytes[index] <= 57 { index += 1 }
                guard index < bytes.count, bytes[index] == 58,
                      let count = Int(String(decoding: bytes[start..<index], as: UTF8.self)),
                      count >= 0, count <= bytes.count - index - 1 else { return nil }
                index += 1; let end = index + count; let data = Data(bytes[index..<end]); index = end
                return .bytes(data)
            default: return nil
            }
        }
    }
    static func totalSize(data: Data, expectedHash: String) -> Int64? {
        guard data.count <= 16 * 1024 * 1024 else { return nil }
        var parser = Parser(bytes: Array(data))
        guard case .dictionary(let root)? = parser.value(), parser.index == data.count,
              case .dictionary(let info)? = root["info"], let range = parser.infoRange else { return nil }
        let hash = Insecure.SHA1.hash(data: data.subdata(in: range)).map { String(format: "%02x", $0) }.joined()
        guard hash.caseInsensitiveCompare(expectedHash) == .orderedSame else { return nil }
        if case .integer(let length)? = info["length"], length > 0 { return length }
        guard case .list(let files)? = info["files"], !files.isEmpty else { return nil }
        var total: Int64 = 0
        for file in files {
            guard case .dictionary(let entry) = file, case .integer(let size)? = entry["length"], size >= 0,
                  total <= Int64.max - size else { return nil }
            total += size
        }
        return total > 0 ? total : nil
    }
}
