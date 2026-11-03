# 浏览器商店版上架材料草稿

早期内部草稿，已不是现状：Edge 和 Chrome 均已提交审核，实际内容以各商店后台为准。Chrome 现状见 [CHROME_STORE_PREPARATION.md](CHROME_STORE_PREPARATION.md)。

## 名称与简介候选

**名称**：Shandian Download Manager · 闪电全能下载

**简短介绍**：右键常见文件直链，发送到电脑上的闪电全能下载。

**English**: Send supported file links to Shandian Download Manager on your computer.

搜索词候选（最终按商店规则核对）：

- 中文：闪电下载、闪电全能下载、文件下载、下载管理器、Shandian
- English: Shandian, download manager, file downloads, download links

Edge 后台允许最多 7 组搜索词，每组最多 30 个字符；关键词应描述扩展实际功能，不保证搜索排名。

## 详细说明草稿

商店版扩展提供一个右键入口，把常见文档、压缩包和安装包的 HTTP / HTTPS 文件直链发送给本机的闪电全能下载。使用前需要在电脑上安装并打开对应的桌面应用。

在支持的文件链接上点右键，选择「发送到闪电 / Send to Shandian」，然后在桌面应用中查看任务。商店版原型不读取浏览器 Cookie、不扫描网页媒体，也不自动取消浏览器下载；需要登录或特殊请求头的链接可能无法使用。并非所有文件类型、网站和链接都适用。

The extension adds a context-menu action for sending supported document, archive and installer links to the Shandian desktop app. Install and open the desktop app first. The current prototype does not read browser cookies, scan pages for media, or automatically cancel browser downloads. Some links, including those requiring login or special headers, may not work.

## 数据处理事实待复核

商店版当前代码只在用户选择右键菜单时读取该链接，判断是否符合支持的直链规则；尝试向 `127.0.0.1` 的本机应用发送链接，并在扩展本地存储最近一次操作结果状态。链接本身及其查询参数会交给桌面应用，桌面应用随后可能访问用户指定的下载来源。原型代码未使用 `cookies`、`tabs`、`scripting`、`downloads` 或 `<all_urls>` 权限。

以上仅为静态代码初查；公开的隐私政策（`docs/CHROME_EXTENSION_PRIVACY.md`、`docs/EDGE_EXTENSION_PRIVACY.md`） 尚需核对桌面应用保存和传输数据的完整过程、用户操作说明、具体隐私条款和商店的披露要求。正式隐私政策需要准确披露所有实际行为，并提供公开可访问链接。

## Edge 隐私表单准备（未填写、未认证）

- Single purpose: Send a user-selected, supported file link to the Shandian Download Manager desktop application on the same computer.
- `contextMenus`: Adds the right-click action that the user chooses to send a supported link.
- `storage`: Stores the status of the most recent action locally so the popup can show the result.
- `http://127.0.0.1/*`: Connects to the local desktop app to check readiness and send the selected link. The extension does not use this permission to contact a remote server.
- Remote code: The current package contains its JavaScript locally; it does not load remote JavaScript or Wasm.
- Data usage: The selected hyperlink is website content and may include sensitive query parameters. Whether other categories apply must be checked against Microsoft's current definitions and the final desktop/extension behavior. Do not treat a lack of remote analytics as a lack of data processing.
- The form also requests a privacy policy URL and three publisher certifications. A public policy URL is not yet available; the certifications have not been made or checked.

## 尚未完成

- 两家商店的名称可用性、账号和真实扩展 ID；应用网关白名单适配。
- Chrome 与 Edge 的真实浏览器安装、成功与失败体验、更新和安全验证。
- 逐路径审查内容政策、隐私政策、最小权限，以及正式上架说明是否与实际代码一致。
- 真实界面截图、可公开的支持联系方式及最终的中英文商店素材。
- Edge 双语页面各需商店描述和图标；后台建议图标 300×300（最低 128×128），截图尺寸为 1280×800 或 640×400。截图须来自实际可验证的使用流程。
- 商店版的自动接管和失败回退；当前仅有右键发送，不应写成自动接管。
