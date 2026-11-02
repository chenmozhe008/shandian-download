import Foundation
import Isaac64

/// ISAAC-64 参考实现来自 Bob Jenkins 的公共领域代码，处理视频号文件前 128 KiB。
struct WeChatCipher {
    private let seed: UInt64
    init(seed: UInt64) { self.seed = seed }
    mutating func apply(to data: inout Data) {
        data.withUnsafeMutableBytes { buffer in
            guard let base = buffer.bindMemory(to: UInt8.self).baseAddress else { return }
            shandian_wechat_xor(seed, base, buffer.count)
        }
    }
}
