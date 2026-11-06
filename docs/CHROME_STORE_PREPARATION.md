# Chrome 商店版上架准备

更新：2026-11-04。Chrome、Edge 商店页均已上线（名称 Shandian Download Manager），README 已写明两个版本的区别和链接；商店签名包尚未实测。Windows 暂不处理。

## 0.2.0 待提交（2026-11-06）

- 新增「接管浏览器下载」：manifest 加 `downloads` 权限（不加 Cookie、不加网站权限）。只接种子文件和 ≥ 5 MB 的普通文件；别的扩展发起的、扩展页面来源的不接；同一链接 2 分钟内只接一次；应用确认接收后才取消浏览器下载。弹窗里有开关，默认开。测试：`node Tests/store_extension_test.js`、`node Tests/store_takeover_test.js`。
- 上传包：`output/store/shandian-store-extension-0.2.0.zip`（只含 `browser-extension-store/` 的内容）。**未提交，未在真实浏览器装过商店签名包。**
- 已知限制：网关对商店版来源只收 `LinkKind == .http`（`Gateway.validStorePayload`），**种子文件地址会被拒（400）**，扩展此时不取消、浏览器照常下载。要让商店版真正接种子，需要网关对商店版放行 http(s) 的 `.torrentFile`，这是网关改动，未做。
- 商店后台要同步改的：
  - Package：上传 0.2.0 zip；
  - Privacy practices：新增 `downloads` 权限理由（中英见下）；数据使用声明里「网站内容 / Web history」类仍不勾——只读下载项的地址、文件名、类型、大小，不保存、只发往本机；单一用途说明补一句「接管下载」；
  - Listing：描述改成「将支持的文件链接和网页上开始的下载交给电脑上的闪电全能下载」；
  - 隐私政策链接不变（`docs/CHROME_EXTENSION_PRIVACY.md` 已更新到 0.2.0）。
- `downloads` 权限理由：
  - 中文：用于在用户于网页上开始下载时，把种子文件和较大文件的下载交给本机的闪电全能下载；应用接收后才取消浏览器中的这次下载，应用未运行时浏览器照常下载。
  - English: Used to hand torrent files and large downloads the user starts on web pages to the Shandian desktop app on the same computer; the browser's download is cancelled only after the app accepts it, and proceeds normally when the app isn't running.

## 当前状态

- **最新状态（2026-11-04）：0.1.1 已通过审核并上架。** 以下是提交时的记录：0.1.1 曾显示 Pending review。 已勾选审核通过后自动发布，官方提示审核可能需要数周。发布者显示名 `chenmozhe008`，联系邮箱已验证，发布者声明为 non-trader。商店截图 1 张，为隔离 Chrome 配置中的真实右键菜单；未拍发送后的桌面应用界面。商店签名包的安装、发送和更新仍未实测；通过后要补测并更新应用和 README 的商店链接。
- 现有 `browser-extension-store/` 为 Manifest V3 独立商店版，保留另行安装的完整版。只支持用户右键发送部分 HTTP/HTTPS 文件直链，不读取 Cookie、不扫描视频、不自动接管下载。
- 已重新运行 `node Tests/store_extension_test.js`，权限、链接过滤、发送与失败处理测试通过。这是自动化测试，不等同于 Chrome 真机安装验证。
- Google 分配的 ID：`achpnnoffkoeomkmofbcjpnmifaeanjg`。网关自 0.18.24 起精确允许此 ID；2026-11-03 对已安装的 0.18.24 实测：此 ID、Edge 商店 ID、完整版 ID 的 ping 返回 200，未知扩展和普通网页返回 403。
- 2026-11-03 在 Chrome 临时加载固定相同 ID 的测试副本，与 0.18.24 候选配合：右键发送本机 ZIP 成功，下载内容与来源一致，不支持的链接提示正确。商店签名包尚未验证。
- Chrome 隐私政策已公开：<https://github.com/chenmozhe008/shandian-download/blob/master/docs/CHROME_EXTENSION_PRIVACY.md>。行为变化时须同步更新此政策和后台数据声明。

## 审核通过后

1. 从商店安装扩展，与 0.18.24 或更新版本配合测试：正常发送、应用未运行、不支持的链接、停用及更新。
2. 应用和 README 的商店链接已于 2026-11-04 加上；从商店实际安装的验证还没做。
3. 如被拒，按拒绝邮件修改后重新提交；不要为通过审核放宽网关来源。

## 后台操作方法

- 后台内容以 Google 后台为准，本文不抄表单内容。各页路径：`<发布者>/<ID>/edit/status|package|listing|privacy|distribution`、`<发布者>/<ID>/testcredentials`、`<发布者>/settings`。
- Chrome 扩展不能操作 `chrome.google.com/webstore`。可用 AppleScript 在已登录的 Edge 标签里执行 JavaScript，需用户先打开「查看 → 开发人员 → 允许 Apple 事件中的 JavaScript」，用完提醒用户关闭。
- 截图上传：把 `File` 经 `DataTransfer` 写入 `input[type=file]`，再触发 `change`；用容器上的 `data-image-key` 确认已上传。全局截图容器带 `data-image-upload-type="4"` 和 `data-is-global="true"`。
- 真实右键截图：用独立的 `--user-data-dir` 启动 Chrome，加 `--remote-debugging-pipe --enable-unsafe-extension-debugging`，经 CDP `Extensions.loadUnpacked` 加载扩展，再用 `Input.dispatchMouseEvent` 右键并用 `screencapture -R` 截图。原生菜单不在窗口截图内，要按区域截；商店要求 1280×800 或 640×400，JPEG 或无透明通道的 PNG。
- 官方发布流程：<https://developer.chrome.com/docs/webstore/publish>
