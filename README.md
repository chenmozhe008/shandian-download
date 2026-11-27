<!-- 搜索关键词：下载工具, 全能下载器, Mac 下载工具, macOS 下载器, Windows 下载器, 磁力下载, 种子下载, BT 下载, ed2k, 迅雷替代, Mac 迅雷, IDM 替代, 下载没速度, 磁力 0 速度, 云盘离线, PikPak, 视频下载, 双语字幕, ASS 字幕, 网页音频下载, 网页音视频嗅探, B 站下载, YouTube 下载, 抖音下载, 小红书下载, 百度网盘, 阿里云盘, 夸克网盘, UC 网盘, GitHub 加速, Hugging Face 加速, 飞书文档, yt-dlp 图形界面, MCP 下载, download manager, macos download manager, windows download manager, magnet downloader, torrent client, ed2k client, yt-dlp gui -->
<div align="center">

<img src="docs/images/icon.png" width="96" alt="闪电全能下载图标">

# 闪电全能下载 · Shandian Download Manager

**免费开源、无广告的跨平台下载工具（macOS & Windows）**

「闪电」在于快：多连接分段、镜像测速优选、冷门 BT 自动换路；<br>
「全能」在于全：磁力种子、视频与双语字幕、四大网盘、文档与网页媒体，一个窗口统一管理。

[**下载 Mac（Apple 芯片）**](https://github.com/chenmozhe008/shandian-download/releases/download/v0.22.8/Shandian-0.22.8-AppleSilicon.zip) · [**下载 Mac（Intel）**](https://github.com/chenmozhe008/shandian-download/releases/download/v0.22.8/Shandian-0.22.8-Intel.zip) · [**下载 Windows（x64）**](https://github.com/cgeek-dev/shandian-download/releases/download/v0.2.23-windows/Shandian-0.2.23-Windows-x64-setup.exe) · [快速安装](#快速安装) · [English](README.en.md)

[![Release](https://img.shields.io/github/v/release/chenmozhe008/shandian-download?color=E0992F&label=Release)](https://github.com/chenmozhe008/shandian-download/releases/latest)
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue)](LICENSE)
![macOS 14+](https://img.shields.io/badge/macOS-14%2B-black?logo=apple)
![Windows 10/11](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D6?logo=windows)
![Chrome & Edge](https://img.shields.io/badge/Extension-Chrome%20%7C%20Edge-4285F4)

<br>

<img src="docs/images/screenshot-downloading.png" alt="闪电全能下载主界面：多来源高速下载与实时速度曲线" width="880">

</div>

---

## 主要功能

- **常用资源全覆盖，连难下的都能下（全能）**：除了磁力、BT 种子、ed2k、HTTP 直链，还支持 **YouTube、B 站、抖音、小红书（无水印）、微信视频号**，以及 **百度 / 阿里 / 夸克 / UC 四大网盘**、**飞书与轻雀文档里的原始音视频**、**网页播放中隐藏的音视频嗅探**——平时最麻烦的下载场景，一个窗口全解决。
- **多连接提速，冷门资源不死等（闪电）**：HTTP 大文件自动分段并行、YouTube 4 路分片并发、**GitHub / Hugging Face** 公开文件自动测速优选镜像并在中断时换站续传；磁力每次启动合并最新公开 tracker，冷门 BT 卡在 0% 时可自动换路本机迅雷或 PikPak 云盘离线，下完取回并清理云端副本。
- **外语视频自动配中英双语字幕**：支持最高 4K 与仅音频，勾选字幕自动建独立文件夹；AI 模式先重排 YouTube 碎片断句并统一全片术语，输出带合理字号比例的 `.ass` 双语字幕，打开直接看，不和原英文轨冲突。
- **浏览器扩展无缝联动**：搭配 Chrome / Edge 完整版扩展，在网页上点一下即可发送到闪电；支持网页音视频实时侦测与波形试听、页面直链批量抓取，以及网盘和飞书 / 轻雀文档的一键批量保存。
- **双端原生体验 + 本机 MCP**：同时支持 **macOS（SwiftUI 原生）** 与 **Windows（Tauri，由 [@cgeek-dev](https://github.com/cgeek-dev) 独立开发与维护）**，均内置自动分类资料库、定时下载、一键更新、多网盘直下与双语 ASS 字幕；还支持边下边播与本机 MCP（可直接接入 Claude Code、Codex、Cursor、ChatGPT 桌面版）。

---

## 快速安装

### 方式一：让 AI 助手帮你装（推荐）

使用 Claude Code、Codex、Cursor、WorkBuddy 等 AI 工具时，直接复制下面这段发给它：

> 请帮我安装「闪电全能下载」（https://github.com/chenmozhe008/shandian-download）：macOS 请在终端运行 `curl -fsSL https://raw.githubusercontent.com/chenmozhe008/shandian-download/master/script/install.sh | bash`；Windows 请从 https://github.com/cgeek-dev/shandian-download/releases/latest 下载最新安装包。装好后，一步步告诉我怎么在浏览器里装它的扩展。

### 方式二：命令行一键安装（macOS）

已安装 Homebrew 时：

```bash
brew install --cask chenmozhe008/shandian/shandian
brew install ffmpeg   # 下载视频网站合并音视频所需
```

没有 Homebrew 时，在终端运行一键脚本（自动按芯片下载、校验 SHA-256、安装并跳过首次拦截）：

```bash
curl -fsSL https://raw.githubusercontent.com/chenmozhe008/shandian-download/master/script/install.sh | bash
```

### 方式三：手动下载安装包（macOS & Windows）

- **macOS（14 及以上）**：Apple 芯片（M 系列）下载 [AppleSilicon.zip](https://github.com/chenmozhe008/shandian-download/releases/download/v0.22.8/Shandian-0.22.8-AppleSilicon.zip)，Intel 处理器下载 [Intel.zip](https://github.com/chenmozhe008/shandian-download/releases/download/v0.22.8/Shandian-0.22.8-Intel.zip)，解压后拖入「应用程序」。首次打开若提示「无法验证开发者」，点「完成」后前往「系统设置 → 隐私与安全性」底部点「仍要打开」；下载视频网站内容需安装 `ffmpeg`（`brew install ffmpeg`）。
- **Windows（10 / 11 x64）**：下载 [Windows 安装器（v0.2.23）](https://github.com/cgeek-dev/shandian-download/releases/download/v0.2.23-windows/Shandian-0.2.23-Windows-x64-setup.exe) 或 [免安装便携版](https://github.com/cgeek-dev/shandian-download/releases/download/v0.2.23-windows/Shandian-0.2.23-Windows-x64-Portable.zip)，更多说明见 [windows/README.md](windows/README.md) 与 [windows/CHANGELOG.md](windows/CHANGELOG.md)。

### Chrome / Edge 浏览器扩展

- **完整版（推荐，支持视频、网盘、文档与嗅探）**：在应用内打开「浏览器扩展」引导页，点「手动安装」，在浏览器扩展页开启「开发者模式」后把 `BrowserExtension` 文件夹拖入即可，随应用自动更新。
- **商店版（一键安装，适合直链与种子接管）**：[Chrome 商店](https://chromewebstore.google.com/detail/achpnnoffkoeomkmofbcjpnmifaeanjg) · [Edge 商店](https://microsoftedge.microsoft.com/addons/detail/onnpjcddpnfmaahjjdcibcofebfgopei)

| 能力 | 完整版（推荐） | 商店版 |
| --- | :---: | :---: |
| 右键发送文件链接 / 自动接管网页下载（5 MB+） | ✓ | ✓ |
| 点击种子文件直接交给应用 | ✓ | ✓ |
| 磁力、ed2k 链接接管 | ✓ | — |
| 视频页悬浮「下载」按钮（YouTube / 抖音 / 小红书 / 新片场 / 视频号） | ✓ | — |
| 阿里云盘 / 夸克网盘 / UC 网盘页面「闪电下载」按钮 | ✓ | — |
| 网页音视频实时侦测、波形试听与批量保存 | ✓ | — |
| 飞书 / 轻雀文档原始音视频扫描与批量下载 | ✓ | — |
| 页面下载链接批量抓取 | ✓ | — |

---

## 界面与演示

### 1. 视频下载与双语字幕

下载外语视频时自动建独立文件夹。选择 **AI 翻译（中外双语）** 后，程序先重排 YouTube 碎片断句、提取全片术语表，再按中文字幕规范输出中文在上、原文在下的 **`.ass` 双语字幕**（中文 52px 纯白细边、英文 34px 浅灰紧贴下方）。目录内只保留最终这一份双语字幕，打开直接看，不和原英文轨冲突；原字幕藏在隐藏备份里，右键随时可恢复。

<div align="center">
<img src="docs/images/subtitle-translation-comparison.png" alt="同一帧 03:31.70 字幕处理前后对比：左侧重叠混乱的原始自动字幕，右侧干净规范的中英双语 ASS" width="880">

<sub>同一帧 03:31.70 实测对比：左侧原始自动字幕重叠且混杂标记，右侧为重排断句后的中英双语 ASS（<a href="https://www.youtube.com/watch?v=X0yezvS3zR4&t=211s">示例视频</a> · <a href="https://raw.githubusercontent.com/chenmozhe008/shandian-download/master/docs/images/workflow-video.mp4">观看 14 秒完整操作视频</a>）</sub>
</div>

- 支持 **本机 Ollama（免费离线）** 及 DeepSeek、Kimi、通义千问、豆包、智谱 GLM、OpenAI、Gemini、Claude 等接口，点「测试连接」可免费拉取最新模型列表。
- 也支持「只下载原字幕」（离线结构清理，不调 AI）或「仅下载音频（M4A）」。

### 2. 冷门 BT 卡在 0% 自动换路

BT 或磁力任务长期没人做种、速度卡在 0% 时，可自动交给本机迅雷或已关联的 PikPak 云盘离线，下完拉回原保存目录并自动清理云端临时副本。

<div align="center">
<img src="docs/images/demo-rescue.gif" alt="冷门 BT 卡在 0% 时自动切换云盘离线并取回本机" width="760">
</div>

### 3. 网页音视频嗅探、文档附件与批量抓链

- **网页音视频嗅探**：在扩展里开启侦测后播放网页内容，可按原名捕获音视频，支持看波形试听并批量保存。
- **飞书 / 轻雀文档音视频**：扫描整篇文档中的原始音视频附件，保留原文件名保存（多选自动放进同一文件夹）。
- **页面链接批量抓取**：提取当前页面的 MP4、MP3、ZIP、磁力、ed2k 链接，勾选后一次加入下载队列。

<div align="center">
<img src="docs/images/workflow-batch.gif" alt="浏览器扩展批量抓取网页链接并加入下载队列" width="760">
</div>

### 4. 自动分类资料库、边下边播与双端界面

已完成的任务按电影、视频、网盘、其他文件自动归类，支持**边下边播**、⌘/Ctrl 与 Shift 批量管理、合集文件抽屉；关掉窗口后仍在后台下载，托盘 / 菜单栏实时显示速度。

<div align="center">
<img src="docs/images/screenshot-library.png" alt="闪电全能下载：自动分类电影资料库" width="880">
</div>

| Windows 版 · 双语字幕样式预览 | Windows 版 · 合集与字幕状态 |
| :---: | :---: |
| <img src="docs/images/windows-subtitle-style.png" alt="Windows 版双语 ASS 样式预览" width="340"> | <img src="docs/images/windows-collection.png" alt="Windows 版合集管理与每集字幕状态" width="500"> |

<div align="center">
<img src="docs/images/windows-queue.png" alt="Windows 版下载队列：视频、合集、网盘与其他文件统一管理" width="840">
</div>

---

## 支持的下载来源

| 分类 | 支持来源 | 使用方式 |
| --- | --- | --- |
| **视频与字幕** | YouTube、B 站、X、微博、豆瓣预告片、西瓜视频、快手、Vimeo | 复制链接粘贴或通过扩展发送；默认最高画质（优先 H.264 / AAC） |
| | 抖音、小红书、新片场 | 在浏览器打开视频页，点完整版扩展「下载这个视频」（支持无水印下载） |
| | 微信视频号 | 粘贴分享链接，按提示在已登录的腾讯元宝页面用完整版扩展下载 |
| **网盘直下** | 百度网盘分享 | 粘贴分享链接与提取码，首次授权官方 [bdpan](https://github.com/baidu-netdisk/bdpan-storage) 后直接保存并自动清理云端转存 |
| | 阿里云盘、夸克网盘、UC 网盘 | 在网页版个人网盘勾选文件，点完整版扩展注入的「闪电下载」按钮直下 |
| | 其他网盘直链 | 直接粘贴直链，或配合 [LinkSwift](https://github.com/hmjz100/LinkSwift) 发送 |
| **协议与开源** | 磁力链接、BT 种子、ed2k、HTTP / HTTPS 直链 | 复制粘贴、拖入 `.torrent` 文件，或由扩展自动接管网页下载 |
| | GitHub / Hugging Face 公开文件 | 粘贴链接自动两轮测速选最快镜像，中途断开自动换站续传 |
| **文档与网页** | 飞书文档 / 知识库音视频 | 打开文档点完整版扩展，或粘贴文档链接批量下载（首次需授权官方 [lark-cli](https://github.com/larksuite/cli)） |
| | 轻雀文档音视频 | 打开文档后在完整版扩展点「扫描当前页面」，勾选原始附件保存 |
| | 网页音视频与批量直链 | 扩展开启「侦测本页面音视频」或「批量抓取页面下载链接」 |

---

## 下载性能

- **磁力起速快**：每次启动合并最新公开 tracker 并固定 BT 监听端口，实测公开片源 5–18 秒开始下载。
- **HTTP 多连接并行**：大文件按 ≥ 20 MB 自动分段并行；遇到限流（429）或临时错误（5xx）自动退避重试。
- **视频 4 路分片并发**：YouTube 等分片流采用 4 路并发，实测速度从 1.4–1.6 MiB/s 提升至 3.4–4.1 MiB/s。
- **按域名自动分流代理**：读取系统代理设置，境外站点走代理，国内站点和 BT 流量直连，不用手动切换。
- **PikPak 离线加速（可选）**：在「设置 → 下载加速」登录免费 PikPak 账号（每天约 3 次 / 20 GB 额度），冷门 BT 1–3 分钟没速度时自动转交云端，取回本地后自动清理云端副本。

---

## 接入 AI 工具（MCP）

内置本机 MCP 服务（仅在本机运行，支持提交任务、查进度、暂停和继续，**不能删除任务或文件**）。在「设置 → AI 工具」复制配置即可接入：

| AI 工具 | 接入方式 |
| --- | --- |
| **Claude Code / Codex / Gemini CLI** | 在终端执行复制的安装命令 |
| **ChatGPT 桌面版** | 设置 → MCP servers → Add server（STDIO），粘贴命令路径 |
| **Claude Desktop / Cursor / VS Code / Trae / Windsurf** | 在各自 MCP 配置文件中粘贴生成的 `mcpServers` JSON |

---

## 常见问题

- **macOS 打开提示「无法验证开发者」**：用上方一键脚本安装可自动处理；手动下载请到「系统设置 → 隐私与安全性」底部点「仍要打开」。
- **视频没有声音或提示合并失败**：未安装 `ffmpeg`，macOS 终端运行 `brew install ffmpeg`。
- **YouTube / 检查更新 / PikPak 连不上**：这些服务在境外，请先开启系统代理（闪电会自动跟随）。
- **控制不了本机迅雷**：在「系统设置 → 隐私与安全性 → 辅助功能」里勾选闪电全能下载（已勾选仍无效时关掉重开一次）。
- **修改默认保存路径**：在「设置 → 通用」中可按磁力/种子、视频平台、其他文件分别指定保存目录。

---

## 从源码构建

- **macOS**：
  ```bash
  swift build -c release          # 编译 SwiftUI 应用
  ./script/build_engine.sh        # 编译定制版 Gopeed v1.9.3 内核（需 Go 环境）
  ./script/build_app.sh package   # 打包为 dist/闪电全能下载.app
  ```
  下载内核补丁位于 `engine/shandian-gopeed-v1.9.3.patch`（含重启免重校验、全局限速、边下边播、限流退避重试等）。构建脚本中的路径请按本机环境调整。
- **Windows**：详见 [windows/README.md](windows/README.md)。

---

## 致谢与声明

- **致谢**：[@cgeek-dev](https://github.com/cgeek-dev)（Windows 版开发与维护）· [Gopeed](https://github.com/GopeedLab/gopeed)（下载内核）· [yt-dlp](https://github.com/yt-dlp/yt-dlp)（视频解析）· [FFmpeg](https://ffmpeg.org)（音视频封装）
- **声明**：本软件仅供个人下载有权获取的内容，请遵守各平台服务条款与当地法律法规；实际下载权限、画质、字幕轨与速度取决于目标站点及网络条件；使用在线 AI 字幕服务时，字幕文本会发送至你自行配置的服务商。README 截图与演示动图使用示例数据。
- **许可证**：[GPL-3.0](LICENSE)
