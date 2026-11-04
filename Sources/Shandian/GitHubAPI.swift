import Foundation

/// 读 GitHub 接口：先按系统设置走（可能经代理）；失败或被限流时直连再试一次。
/// 未登录的接口每个 IP 每小时只有 60 次，代理节点多人共用经常用完（11-04 实测：走代理剩 0 次、偶发 TLS 错误，直连正常）
enum GitHubAPI {
    static func data(for request: URLRequest) async throws -> (Data, URLResponse) {
        var firstError: Error?
        do {
            let result = try await URLSession.shared.data(for: request)
            guard let status = (result.1 as? HTTPURLResponse)?.statusCode, status != 200 else { return result }
        } catch { firstError = error }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.connectionProxyDictionary = [:]
        do { return try await URLSession(configuration: configuration).data(for: request) }
        catch { throw firstError ?? error }
    }
}
