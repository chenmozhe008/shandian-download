import SwiftUI
import WebKit

/// 用 PikPak 官方网页登录：密码只在 PikPak 自己的网页里输，闪电全能下载不经手。
/// 登录后读网页存的 credentials_<client_id> 和 deviceid，交给 PikPak.adoptWebSession，然后删掉网页里的那份——
/// 每次刷新会作废旧的 refresh token，同一份会话只能有一个刷新者（做法参照开源项目 peach 的 ADR-0093）
struct PikPakLoginView: View {
    var onDone: (String?) -> Void
    @State private var status = L("在下面的网页里登录 PikPak，登录后会自动关联")
    @State private var pageProblem = false
    @State private var reloadCount = 0

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Image(systemName: "lock.shield").foregroundStyle(Theme.tint)
                Text(pageProblem ? L("网页打不开：PikPak 需要科学上网（连接境外网络），请检查网络后点「重新加载」") : status).font(.system(size: 13))
                Spacer()
                if pageProblem {
                    Button("重新加载") { pageProblem = false; reloadCount += 1 }
                }
                Button("取消") { onDone(nil) }
            }
            .padding(.horizontal, 16).frame(height: 44)
            Divider()
            PikPakWebView(reloadCount: reloadCount, onPageProblem: { pageProblem = true }) { cred in
                status = L("已登录，正在关联…")
                pageProblem = false
                Task {
                    do {
                        try await PikPak.shared.adoptWebSession(clientID: cred.clientID, deviceID: cred.deviceID, accessToken: cred.access,
                                                                refreshToken: cred.refresh, userID: cred.sub, name: cred.name)
                        onDone(cred.name)
                    } catch { status = L("关联信息保存失败，请关闭窗口后重试") }
                }
            }
        }
        .frame(width: 900, height: 680)
    }
}

struct WebCredential { var clientID, deviceID, access, refresh, sub, name: String }

private struct PikPakWebView: NSViewRepresentable {
    var reloadCount: Int
    var onPageProblem: () -> Void
    var onCredential: (WebCredential) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onCredential: onCredential, onPageProblem: onPageProblem) }

    /// 只在窗口刚打开时清一次旧登录。不能每次加载都清：登录成功后网页会跳转，刚写入的登录信息会被删掉，
    /// 窗口就拿不到结果、页面退回未登录（11-04 复现，用户看到白屏卡住）
    static let clearOnce = """
    if (!sessionStorage.getItem('shandian_cleared')) {
      Object.keys(localStorage).filter(k => k.startsWith('credentials_')).forEach(k => localStorage.removeItem(k));
      sessionStorage.setItem('shandian_cleared', '1');
    }
    """

    func makeNSView(context: Context) -> WKWebView {
        let cfg = WKWebViewConfiguration()
        cfg.preferences.javaScriptCanOpenWindowsAutomatically = true
        // 网页脚本运行前先删掉旧的登录信息：网页总从未登录开始，不会拿旧 token 去刷新、也不会被当成新登录读回来
        cfg.userContentController.addUserScript(WKUserScript(source: Self.clearOnce, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        let v = WKWebView(frame: .zero, configuration: cfg)
        v.uiDelegate = context.coordinator
        v.navigationDelegate = context.coordinator
        v.customUserAgent = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15"
        v.load(URLRequest(url: URL(string: "https://mypikpak.com/drive/login")!))
        context.coordinator.start(v)
        return v
    }
    func updateNSView(_ v: WKWebView, context: Context) {
        guard context.coordinator.reloadCount != reloadCount else { return }
        context.coordinator.reloadCount = reloadCount
        context.coordinator.blankChecks = 0
        if v.url == nil { v.load(URLRequest(url: URL(string: "https://mypikpak.com/drive/login")!)) } else { v.reload() }
    }

    @MainActor final class Coordinator: NSObject, WKUIDelegate, WKNavigationDelegate {
        let onCredential: (WebCredential) -> Void
        let onPageProblem: () -> Void
        var reloadCount = 0
        var blankChecks = 0
        private var timer: Timer?
        private var taken = false
        private var popups: [ObjectIdentifier: NSWindow] = [:]
        init(onCredential: @escaping (WebCredential) -> Void, onPageProblem: @escaping () -> Void) {
            self.onCredential = onCredential
            self.onPageProblem = onPageProblem
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            guard (error as NSError).code != NSURLErrorCancelled else { return }
            onPageProblem()
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            guard (error as NSError).code != NSURLErrorCancelled else { return }
            onPageProblem()
        }

        /// 页面连续 15 秒一个字都没有：多半是脚本经代理没加载全，提示重新加载
        private func checkBlank(_ v: WKWebView) {
            v.evaluateJavaScript("document.body ? document.body.innerText.trim().length : 0") { [weak self] result, _ in
                MainActor.assumeIsolated {
                    guard let self else { return }
                    if (result as? Int ?? 0) > 0 { self.blankChecks = 0; return }
                    self.blankChecks += 1
                    if self.blankChecks == 15 { self.onPageProblem() }
                }
            }
        }

        /// Google、Apple 登录开的是独立小窗，登录结果靠小窗传回打开它的页面。
        /// 必须用系统给的配置建真正的新窗口；在原窗口里打开会把 PikPak 页面换掉，结果传不回来，登录完就白屏（11-04）
        func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                     for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
            let size = NSSize(width: windowFeatures.width?.doubleValue ?? 520, height: windowFeatures.height?.doubleValue ?? 720)
            let popup = WKWebView(frame: NSRect(origin: .zero, size: size), configuration: configuration)
            popup.uiDelegate = self
            popup.customUserAgent = webView.customUserAgent
            let window = NSWindow(contentRect: popup.frame, styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
            window.title = L("登录 PikPak")
            window.isReleasedWhenClosed = false
            window.contentView = popup
            window.center()
            window.makeKeyAndOrderFront(nil)
            popups[ObjectIdentifier(popup)] = window
            return popup
        }

        func webViewDidClose(_ webView: WKWebView) {
            popups.removeValue(forKey: ObjectIdentifier(webView))?.close()
        }

        func start(_ v: WKWebView) {
            timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self, weak v] _ in
                MainActor.assumeIsolated { if let v { self?.check(v) } }
            }
        }

        private func check(_ v: WKWebView) {
            guard !taken else { return }
            if popups.isEmpty { checkBlank(v) } else { blankChecks = 0 }
            let js = """
            (() => { const k = Object.keys(localStorage).find(k => k.startsWith('credentials_'));
              if (!k) return null; const c = JSON.parse(localStorage.getItem(k) || '{}');
              if (!c.access_token || !c.refresh_token) return null;
              const r = JSON.stringify({ key: k, c, device: localStorage.getItem('deviceid') || '' });
              localStorage.removeItem(k); return r; })()
            """
            v.evaluateJavaScript(js) { [weak self] result, _ in
                MainActor.assumeIsolated {
                    guard let self, !self.taken, let str = result as? String, let d = str.data(using: .utf8),
                          let o = try? JSONSerialization.jsonObject(with: d) as? [String: Any],
                          let key = o["key"] as? String, let c = o["c"] as? [String: Any] else { return }
                    self.taken = true; self.timer?.invalidate()
                    self.popups.values.forEach { $0.close() }
                    self.popups.removeAll()
                    // 键名形如 credentials_<client_id>（多账号时后面还有 @<sub>）
                    let client = String(key.dropFirst("credentials_".count).split(separator: "@").first ?? "")
                    var device = o["device"] as? String ?? ""
                    if device.hasPrefix("\"") { device = device.trimmingCharacters(in: CharacterSet(charactersIn: "\"")) }
                    let sub = c["sub"] as? String ?? (c["user_id"] as? String ?? "")
                    let name = (c["username"] as? String) ?? (c["email"] as? String) ?? L("PikPak 账号")
                    self.onCredential(WebCredential(clientID: client.isEmpty ? PikPak.web.clientID : client, deviceID: device,
                                                    access: c["access_token"] as? String ?? "", refresh: c["refresh_token"] as? String ?? "",
                                                    sub: sub, name: name))
                }
            }
        }
    }
}

/// 独立窗口（不用表单弹窗：表单里的网页常常点不动）；登录成功或取消后自己关掉
struct PikPakLoginWindow: View {
    @Environment(\.dismissWindow) private var dismissWindow
    var body: some View {
        PikPakLoginView { name in
            if name != nil { NotificationCenter.default.post(name: PikPak.loginChanged, object: nil) }
            dismissWindow(id: "pikpak-login")
        }
    }
}
