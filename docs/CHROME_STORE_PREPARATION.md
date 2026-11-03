# Chrome 商店版上架准备

更新：2026-11-03。Chrome 0.1.1 已提交审核，尚未上架。Windows 暂不处理。

## 当前状态

- **最新状态（2026-11-03）：0.1.1 已提交审核，后台显示 Pending review，尚未上架。** 已勾选审核通过后自动发布，官方提示审核可能需要数周。发布者显示名 `chenmozhe008`，联系邮箱已验证，发布者声明为 non-trader。商店截图 1 张，为隔离 Chrome 配置中的真实右键菜单；未拍发送后的桌面应用界面。商店签名包的安装、发送和更新仍未实测；通过后要补测并更新应用和 README 的商店链接。
- 现有 `browser-extension-store/` 为 Manifest V3 独立商店版，保留另行安装的完整版。只支持用户右键发送部分 HTTP/HTTPS 文件直链，不读取 Cookie、不扫描视频、不自动接管下载。
- 已重新运行 `node Tests/store_extension_test.js`，权限、链接过滤、发送与失败处理测试通过。这是自动化测试，不等同于 Chrome 真机安装验证。
- Google 分配的 ID：`achpnnoffkoeomkmofbcjpnmifaeanjg`。网关自 0.18.24 起精确允许此 ID；2026-11-03 对已安装的 0.18.24 实测：此 ID、Edge 商店 ID、完整版 ID 的 ping 返回 200，未知扩展和普通网页返回 403。
- 2026-11-03 在 Chrome 临时加载固定相同 ID 的测试副本，与 0.18.24 候选配合：右键发送本机 ZIP 成功，下载内容与来源一致，不支持的链接提示正确。商店签名包尚未验证。
- Chrome 隐私政策已公开：<https://github.com/chenmozhe008/shandian-download/blob/master/docs/CHROME_EXTENSION_PRIVACY.md>。行为变化时须同步更新此政策和后台数据声明。

## 审核通过后

1. 从商店安装扩展，与 0.18.24 或更新版本配合测试：正常发送、应用未运行、不支持的链接、停用及更新。
2. 确认能搜索和安装后，更新应用和 README 的商店链接。
3. 如被拒，按拒绝邮件修改后重新提交；不要为通过审核放宽网关来源。

## 后台操作方法

- 后台内容以 Google 后台为准，本文不抄表单内容。各页路径：`<发布者>/<ID>/edit/status|package|listing|privacy|distribution`、`<发布者>/<ID>/testcredentials`、`<发布者>/settings`。
- Chrome 扩展不能操作 `chrome.google.com/webstore`。可用 AppleScript 在已登录的 Edge 标签里执行 JavaScript，需用户先打开「查看 → 开发人员 → 允许 Apple 事件中的 JavaScript」，用完提醒用户关闭。
- 截图上传：把 `File` 经 `DataTransfer` 写入 `input[type=file]`，再触发 `change`；用容器上的 `data-image-key` 确认已上传。全局截图容器带 `data-image-upload-type="4"` 和 `data-is-global="true"`。
- 真实右键截图：用独立的 `--user-data-dir` 启动 Chrome，加 `--remote-debugging-pipe --enable-unsafe-extension-debugging`，经 CDP `Extensions.loadUnpacked` 加载扩展，再用 `Input.dispatchMouseEvent` 右键并用 `screencapture -R` 截图。原生菜单不在窗口截图内，要按区域截；商店要求 1280×800 或 640×400，JPEG 或无透明通道的 PNG。
- 官方发布流程：<https://developer.chrome.com/docs/webstore/publish>
