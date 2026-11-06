# 闪电全能下载 Chrome 商店扩展隐私政策 / Privacy Policy

更新日期 / Last updated: 2026-11-06（0.2.0）

本政策适用于通过 Chrome Web Store 提供的独立商店版扩展，不适用于另行安装的完整版扩展或桌面应用的全部功能。

## 中文

当用户在网页文件链接上选择扩展的右键菜单时，扩展读取该链接并检查是否支持。如果支持，扩展将链接发送给同一台电脑上的闪电桌面应用，地址为 `127.0.0.1:6800`。链接可能包含查询参数、访问令牌或其他个人信息。扩展会检查本机应用是否准备就绪，并在浏览器本地保存最近一次操作的结果状态，以显示提示；此状态不包含链接。

扩展使用这些信息完成用户主动发起的发送操作。扩展不读取浏览器 Cookie，不扫描网页媒体，不使用远程代码，也不向开发者的远程服务器发送链接。桌面应用收到链接后，会根据用户操作处理下载，连接用户指定的下载来源，并可能在本机保存链接、任务记录和下载文件。来源网站及用户选择使用的其他服务按各自的政策处理请求。

自 0.2.0 起，扩展使用浏览器的「下载」权限接管网页上开始的下载：浏览器新建下载时，扩展读取该下载项的地址、文件名、文件类型和大小，只把种子文件和 5 MB 以上的普通文件的地址发送给本机应用；图片、网页、文本、小文件、其他扩展发起的下载不处理。只有本机应用确认接收后，扩展才取消浏览器中的这次下载并从下载列表移除；应用未运行或未接收时，浏览器照常下载。扩展不读取下载内容、Cookie 或下载历史，不保存下载地址；为避免重复交接，同一地址会在扩展内存中保留 2 分钟，随后丢弃。用户可在扩展弹窗中关闭「接管浏览器下载」，该开关仅保存在浏览器本地。

只有用户选择扩展右键菜单，或在开启接管时于网页上开始下载，扩展才会发送链接。用户可以在 Chrome 扩展管理页面停用或移除扩展；移除扩展不会自动删除桌面应用的任务记录或已下载文件。问题可通过 <https://github.com/chenmozhe008/shandian-download/issues> 联系维护者。

## English

When the user chooses the extension's context-menu action on a file link, the extension reads and checks that link. If supported, the link is sent to the Shandian desktop app on the same computer at `127.0.0.1:6800`. Links may contain query parameters, access tokens, or other personal information. The extension checks whether the local app is ready and stores the most recent operation's status locally in the browser to display feedback; this status does not include the link.

The extension uses this information to perform the action requested by the user. The extension does not read browser cookies, scan pages for media, use remote code, or send links to a developer-operated remote server. The desktop app processes downloads at the user's request, connects to the chosen download source, and may save links, task records, and downloaded files locally. Source websites and other services selected by the user handle requests under their own policies.

From 0.2.0, the extension uses the browser's downloads permission to take over downloads started on web pages. When the browser creates a download, the extension reads that download's URL, file name, file type and size, and sends only the URL of torrent files and ordinary files over 5 MB to the local app. Images, web pages, text, small files and downloads started by other extensions are left alone. The extension cancels the browser's download and removes it from the download list only after the local app confirms it has accepted the URL; if the app is not running or does not accept it, the browser downloads as usual. The extension does not read download contents, cookies or download history, and does not store download URLs; to avoid handing the same download over twice, a URL is kept in the extension's memory for 2 minutes and then discarded. Users can turn off "Take over browser downloads" in the extension popup; this setting is stored only in the browser.

A link is sent only when the user selects the extension's context-menu action, or starts a download on a web page while takeover is on. Users can disable or remove the extension in Chrome's extension settings. Removal does not automatically delete desktop-app task records or downloaded files. Contact the maintainers at <https://github.com/chenmozhe008/shandian-download/issues>.
