import Foundation

/// 读 GitHub 接口：先按系统设置走（可能经代理）；失败或被限流时直连再试一次。
/// 遇到 403/429 限流时记录退避截止时间，避免限流窗口内重复请求。
enum GitHubAPI {
    private actor RateLimitGate {
        private var retryAfterDate: Date = .distantPast

        func isLimited(now: Date = Date()) -> Bool {
            now < retryAfterDate
        }

        func record(response: HTTPURLResponse, now: Date = Date()) {
            guard response.statusCode == 403 || response.statusCode == 429 else { return }
            let remaining = response.value(forHTTPHeaderField: "X-RateLimit-Remaining")
            let isLimit = response.statusCode == 429 || remaining == "0"
            guard isLimit else { return }
            let delay: TimeInterval
            if let ra = response.value(forHTTPHeaderField: "Retry-After").flatMap(TimeInterval.init), ra > 0 {
                delay = min(ra, 3600)
            } else if let reset = response.value(forHTTPHeaderField: "X-RateLimit-Reset").flatMap(TimeInterval.init) {
                delay = min(max(reset - now.timeIntervalSince1970, 60), 3600)
            } else {
                delay = 300
            }
            let target = now.addingTimeInterval(delay)
            if target > retryAfterDate { retryAfterDate = target }
        }
    }

    private static let gate = RateLimitGate()

    struct RateLimitedError: Error {}

    static func data(for request: URLRequest) async throws -> (Data, URLResponse) {
        let isAPI = request.url?.host?.lowercased() == "api.github.com"
        if isAPI, await gate.isLimited() { throw RateLimitedError() }
        var firstError: Error?
        do {
            let result = try await URLSession.shared.data(for: request)
            if let http = result.1 as? HTTPURLResponse {
                if http.statusCode == 200 { return result }
            } else {
                return result
            }
        } catch { firstError = error }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.connectionProxyDictionary = [:]
        let direct = URLSession(configuration: configuration)
        defer { direct.finishTasksAndInvalidate() }
        do {
            let result = try await direct.data(for: request)
            // 仅当直连也返回非 200（限流）时才记录全局退避，避免代理节点限流误伤可用直连
            if isAPI, let http = result.1 as? HTTPURLResponse, http.statusCode != 200 {
                await gate.record(response: http)
            }
            return result
        } catch { throw firstError ?? error }
    }
}
