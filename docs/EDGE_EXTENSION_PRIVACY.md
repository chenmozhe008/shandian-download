# 闪电全能下载 Edge 扩展隐私政策 / Privacy Policy

更新日期 / Last updated: 2026-11-06（0.2.0）

## 中文

本政策适用于 Microsoft Edge 加载项商店中的「闪电全能下载 · Shandian」扩展，不适用于另行安装的完整版浏览器扩展。商店版的用途是将用户主动选择的文件链接发送到同一台电脑上的闪电全能下载桌面应用。

**处理的信息。** 当你在链接上选择扩展的右键菜单时，扩展读取该链接并检查是否支持；受支持的链接会发送到本机 `127.0.0.1:6800` 的桌面应用。链接可能包含查询参数、访问令牌或其他个人信息。扩展会请求本机应用的状态接口，以确认应用是否准备好接收链接。扩展在浏览器本地存储最近一次操作的结果状态，用于显示提示；该状态不包含链接。

**用途与共享。** 扩展使用以上信息完成你主动发起的发送操作；扩展本身不读取浏览器 Cookie，不扫描网页媒体，不使用远程代码，也不向开发者的远程服务器发送链接。桌面应用收到链接后，会根据你的操作处理下载，连接你指定的下载来源，并可能在本机保存链接、任务记录和下载的文件。来源网站及你选择使用的其他服务按其各自的政策处理请求。

**接管下载（0.2.0 起）。** 扩展使用浏览器的「下载」权限接管网页上开始的下载：浏览器新建下载时，扩展读取该下载项的地址、文件名、文件类型和大小，只把种子文件和 5 MB 以上的普通文件的地址发送给本机应用；图片、网页、文本、小文件、其他扩展发起的下载不处理。只有本机应用确认接收后，扩展才取消浏览器中的这次下载并从下载列表移除；应用未运行或未接收时，浏览器照常下载。扩展不读取下载内容、Cookie 或下载历史，不保存下载地址；为避免重复交接，同一地址会在扩展内存中保留 2 分钟，随后丢弃。你可以在扩展弹窗中关闭「接管浏览器下载」，该开关仅保存在浏览器本地。

**你的选择。** 只有在你选择扩展右键菜单，或在开启接管时于网页上开始下载，才会发送链接；你可以在 Edge 扩展管理页面停用或移除扩展。移除扩展不会自动删除桌面应用的任务记录或已下载文件；可在桌面应用和系统文件管理器中管理这些内容。请只发送你有权使用的链接。

**联系与变更。** 如对本政策或扩展有疑问，可通过 [GitHub Issues](https://github.com/chenmozhe008/shandian-download/issues) 联系项目维护者。我们可能随功能和数据处理方式变化更新本政策，并在此页面标注更新日期。

## English

This policy applies to the **Shandian Download Manager** extension distributed through the Microsoft Edge Add-ons store. It does not cover the separately installed full browser extension. The store extension sends a file link chosen by the user to the Shandian desktop application on the same computer.

**Information processed.** When you choose the extension's context-menu action on a link, the extension reads and checks the link. If supported, it sends the link to the desktop app at `127.0.0.1:6800` on your computer. Links may contain query parameters, access tokens, or other personal information. The extension checks the local app's readiness and stores the latest action's status locally in the browser to display feedback. This status does not include the link.

**Use and sharing.** The extension uses this information to carry out the action you requested. The extension itself does not read browser cookies, scan pages for media, use remote code, or send links to a developer-operated remote server. The separate desktop app processes downloads at your request, connects to the download source you choose, and may save links, task records, and downloaded files locally. Source websites and other services you choose handle requests under their own policies.

**Download takeover (from 0.2.0).** The extension uses the browser's downloads permission to take over downloads started on web pages. When the browser creates a download, the extension reads that download's URL, file name, file type and size, and sends only the URL of torrent files and ordinary files over 5 MB to the local app. Images, web pages, text, small files and downloads started by other extensions are left alone. The extension cancels the browser's download and removes it from the download list only after the local app confirms it has accepted the URL; if the app is not running or does not accept it, the browser downloads as usual. The extension does not read download contents, cookies or download history, and does not store download URLs; to avoid handing the same download over twice, a URL is kept in the extension's memory for 2 minutes and then discarded. You can turn off "Take over browser downloads" in the extension popup; this setting is stored only in the browser.

**Your choices.** A link is sent only when you select the extension's context-menu action, or start a download on a web page while takeover is on. You can disable or remove the extension in Edge's extension settings. Removing it does not automatically delete desktop-app task records or downloaded files; manage those in the desktop app and your file manager. Send only links you are authorized to use.

**Contact and changes.** Contact the maintainers through [GitHub Issues](https://github.com/chenmozhe008/shandian-download/issues). We may update this policy as features or data handling change and will update the date on this page.
