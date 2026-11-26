# LinkSwift 来源与许可

2026-11-17 新增的 `extra-cloud.js` 参考并改写了 LinkSwift 的网盘选择状态读取、取链接口、参数及移动/天翼签名流程。网页入口和本机交接复用本项目的扩展流程，没有嵌入或执行完整用户脚本。

- 上游：[hmjz100/LinkSwift](https://github.com/hmjz100/LinkSwift)。
- 参考版本：[e63b95000d9f78d3aecf2b257f227ae2f8a1e458](https://github.com/hmjz100/LinkSwift/tree/e63b95000d9f78d3aecf2b257f227ae2f8a1e458)，`（改）网盘直链下载助手.user.js`，v1.1.4。
- 上游作者：Hmjz100、油小猴及贡献者。
- 上游脚本声明：`AGPL-3.0-or-later`。本项目新增的改写文件 `extra-cloud.js` 按相同许可提供；完整上游许可文本见同目录 `LinkSwift-AGPL-3.0.txt`。
- 改写范围：123 云盘、天翼、移动、迅雷、光鸭的所选文件读取和取链。移除上游的激活、美化、下载器菜单和内置下载实现；跨域请求改由扩展后台执行，凭据仅临时用于对应服务商请求，不传给本机下载应用。迅雷使用网页当前有效的凭据和验证令牌，不移植上游令牌刷新和验证码签名实现。
- 本说明和许可随 `browser-extension` 目录分发，扩展源代码在本项目仓库中提供。
