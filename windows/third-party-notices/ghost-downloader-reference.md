# Ghost Downloader 来源与许可说明

核查日期：2026-11-16。

本说明覆盖 Windows 版镜像竞速与失败回退，以及本仓库 Gopeed 补丁中的自适应多连接改写。当前 Mac 客户端源码未发现对应的镜像选站与回退接入。Mac 构建脚本引用共享内核补丁，但本次未核实其实际构建和发布的内核是否包含这些改动，因此不据此宣称 macOS 已接入。

## 上游来源

- 项目：[XiaoYouChR/Ghost-Downloader-3](https://github.com/XiaoYouChR/Ghost-Downloader-3)。
- 上游项目作者及贡献者：XiaoYouChR 及 Ghost Downloader 项目贡献者。
- 参考版本：[`8aebe3109ad2d9da327a0c2b4eb8de90337eeadc`](https://github.com/XiaoYouChR/Ghost-Downloader-3/tree/8aebe3109ad2d9da327a0c2b4eb8de90337eeadc)，即 2026-11-13 分析时使用的本地上游检出版本。
- 许可证：GNU General Public License version 3。该版本的完整 LICENSE 原文保存在同目录的 `Ghost-Downloader-3-GPL-3.0.txt`，与本项目根目录 `LICENSE` 内容一致。许可证文本中的 Free Software Foundation 版权行属于许可证文本，不是上游业务代码的作者署名。

## 核查结论与改写范围

2026-11-13 的开发记录包含阅读上述版本相关源文件、分析策略和随后编写本项目实现的过程。本项目提交 `bc9fd196c9d5d29fab23d11a517a68d8884ef8e4` 引入了这些功能。逐段对照未发现整份 Python 文件或成段 Python 源码的原样复制；确认存在阅读上游后对部分逻辑的跨语言改写，因此保留来源和 GPL-3.0 许可说明。

| 上游文件与逻辑 | 本项目对应位置 | 对照结果 |
| --- | --- | --- |
| `features/github_pack/config.py` 的 `GITHUB_PROXY_SITES` | `windows/src-tauri/src/ghmirror.rs` 的镜像候选 | 本项目十个初始候选均在上游列表中，开发记录包含对这些站点的实测筛选；属于对候选集合的参考和筛选。 |
| `features/github_pack/pack.py` 的 `isGitHubFileUrl` | `ghmirror.rs::is_github_file` | 域名集合、HTTP/HTTPS 判断、`www.` 处理和四类文件路径判断存在具体对应，改写为 Rust，并排除了上游接收的 API 和 gist 页面域名。 |
| `features/github_pack/probe.py` 的 `toProxyHeaders`、`matchFile`、`probeUrls` | `ghmirror.rs::mirror_headers`、`probe`、`pick` | 参考了凭据过滤、Range/大小合格判断和原站参与竞速；本项目增加了实际吞吐采样、缓存、大小匹配和通过内核 PATCH 换站续传。上游该版本失败时换站并从头下载。 |
| `features/http_pack/task.py` 的 `_autoSpeedUp`、`_splitSlowest` | `engine/shandian-gopeed-v1.9.3.patch` 内新增的 `internal/protocol/http/shandian_boost.go` | 最近五次速度、15% 稳定阈值、增加四个连接、五次采样后比较收益，以及 `speedRatio < 0.8 * workerRatio` 的判断顺序和公式明确对应。改写并适配了 Gopeed 的连接、锁和协程模型；增加了连接上限、最小分片和限流停止条件。 |

改写由本项目于 2026-11-13 引入，后续调整见本项目 Git 历史。本项目相关改写按 GPL-3.0 分发，上游代码的权利属于其原作者和贡献者。

本项目后续新增的 Hugging Face 公开文件直链支持复用了这里的 Rust 测速与回退流程；本次对照未发现移植上游 `features/huggingface_pack` 源码的证据。

## 分发材料

- Windows 打包脚本将本说明与 `Ghost-Downloader-3-GPL-3.0.txt` 放入安装数据的 `ThirdPartyNotices` 目录。macOS 的许可材料由其维护者按实际采用的代码和内核版本补充。
- 本项目对应实现与内核补丁随项目源码提供。发布二进制时仍须按 GPL-3.0 第 6 条提供与该二进制对应的完整源码；本说明和许可证副本不替代对应源码。

感谢 Ghost Downloader 项目对镜像竞速、失败回退和自适应多连接策略的贡献。
