# 闪电全能下载 · Windows 版

本目录包含 Windows 普通版的主程序、安装器和专用浏览器扩展。Windows 的扩展源码位于 `windows/browser-extension/`，内核补丁位于 `windows/engine/`。 更新摘要见 [CHANGELOG](CHANGELOG.md)。

在仓库根目录运行 Windows 回归测试。

```powershell
$tests = (Get-ChildItem windows/Tests -Recurse -File | Where-Object { $_.Name -match '\.test\.c?js$' }).FullName
node --test @tests
```


跟 macOS 版同一套内核、同一份补丁，换了个外壳。v0.1 的目标是给同事装来测试。

## 结构

```
windows/
├── src-tauri/            Rust 外壳（Tauri v2）
│   ├── src/main.rs       入口、全局状态、命令注册
│   ├── src/engine.rs     起停 gopeed.exe、健康检查、投任务、暂停/继续/删除
│   ├── src/api.rs        跟内核 REST 说话
│   ├── src/links.rs      链接识别（移植自 Sources/Shandian/Links.swift）
│   ├── src/settings.rs   偏好设置、目录策略、写进内核配置
│   └── src/commands.rs   设置页、选目录、打开目录
└── ui/                   界面，纯 HTML/CSS/JS，不引构建链
    ├── index.html        结构 + 图标 sprite（手写的 SF Symbols 替代品）
    ├── style.css         两套配色（data-theme=light/dark）
    ├── app.js            轮询、渲染、事件
    └── assets/           logo.svg（取自 Mac 版 icon.html）、done.wav
```

三层各管什么：

| 层 | 谁干 | 在哪 |
|---|---|---|
| 下载引擎 | 打过补丁的 Gopeed | `gopeed.exe`，独立进程，REST 127.0.0.1:9999 |
| 外壳 | Tauri + Rust | 起子进程、调接口、系统集成（托盘、深链、通知） |
| 界面 | HTML/CSS/JS | WebView2 里跑，配色取自 Mac 版 `Theme.swift` |

**不写下载逻辑**。磁力、BT、ed2k、HTTP 分块、限速、边下边播全在内核里，壳只负责调度和显示。

## 界面与图标（尽量和 Mac 版一致）

界面按 Mac 版的效果图（`docs/images/screenshot-*.png`）重做：侧栏分「正在下载／已完成／回收站 + 资料库」，主区顶部是带速度曲线的大速度卡，任务行左缩略图、右速度与剩余时间，底部一条粘贴框。

图标资源的取舍（Mac 版 `Resources/` 里能复用的都复用了）：

| Mac 资源 | Windows 怎么用 |
|---|---|
| `AppIcon-source.png`（1024） | `tools/make_icon.py` 出 `src-tauri/icons/icon.ico`（16–256 多尺寸）+ 32/256 PNG。`AppIcon.icns` 是 macOS 专有容器，用不上 |
| `icon/icon.html`（1024 SVG） | 原样搬到 `ui/assets/logo.svg`，侧栏品牌位用它；以后托盘、安装包也从这儿取 |
| `BrandGlyph.swift`（闪电箭头路径） | 复刻成 SVG symbol `#i-bolt`（同一套坐标），空状态和磁力任务缩略图用它；托盘 32×32 也能直接渲染 |
| `Theme.swift` + `AccentColor` | 颜色值搬进 `style.css` 变量：琥珀 `rgb(232,154,0)`（深色 `rgb(255,181,38)`）、暖白底、灰阶，连明暗两套都照抄 |
| `done.wav` | 复制到 `ui/assets/done.wav`，任务下完响一声（和 Mac 版同一个文件） |
| `Image(systemName:)`（16 处，6 个符号） | **用不了**。SF Symbols 是苹果系统字体里的字形，不是文件，也不能随包分发；这里按同样造型手写成 24×24 线条 SVG（`index.html` 顶部的 sprite） |

主题跟系统走：`app.js` 用 `matchMedia('(prefers-color-scheme: dark)')` 给 `<html>` 打 `data-theme`，两套颜色都在 `style.css` 里，没有重复的 `@media` 块（以后加「手动选主题」只要改这一个属性）。

还没做：任务缩略图目前是橙底占位块（Mac 版抓豆瓣海报），Windows 侧要单独做海报源。

## 设置与目录策略

三类文件分开存，跟 Mac 版 `Folders.directory(for:)` 一致：

| 链接类型 | 存到 | 默认位置 |
|---|---|---|
| 磁力 / 种子 / ed2k | 电影 | `下载\电影` |
| 视频网站 | 视频 | `下载` |
| 直链 | 其他 | `下载` |

默认位置按 `%USERPROFILE%\Downloads` 算；系统把「下载」重定向到别处（OneDrive 常见）时，目录不存在就退到数据目录下的 `downloads`。电影默认落在「下载\电影」而不是移动硬盘——Windows 没这个约定，用户自己改成 `D:\movies` 更合理。

规则写在 `settings.rs::dir_for`，**在 Rust 侧决定，前端只传链接**。让前端传路径的写法早晚会和设置里的位置对不上。

数据分两处放（`migrate.rs`，老版本的数据启动时自动搬过来）：

| 位置 | 放什么 |
|---|---|
| `%APPDATA%\com.shandian.windows` | 用户配置：设置、任务列表、下载历史、更新记录。绿色版的 `BrowserExtension` 也在这儿，浏览器记的是这个路径 |
| `%LOCALAPPDATA%\com.shandian.windows` | 这台机器专用：`bin`（yt-dlp、ffmpeg）、`engine`（内核任务库、token）、`auth`（Cookie、PikPak、NAS 凭据）、`state`（恢复和接力记录）、`cache`（缩略图、处理临时文件）、`logs`，以及原位保留的 WebView 数据 |

分类数据统一通过 `migrate::local_path` 访问。启动时先迁移再加载各模块；旧文件占用时回退旧路径，下次启动重试。分类目录已有文件时保留它，并把旧版本移入同类目录的 `legacy/`。`bin`、`engine`、`EBWebView*` 的位置保持不变。

设置存 `%APPDATA%\com.shandian.windows\settings.json`，保存时同步写进内核：

| 设置 | 落到内核哪 | 说明 |
|---|---|---|
| 同时下载 | `maxRunning` | 默认 5（跟 Mac 版同值） |
| 下载限速 | `extra.shandianDownloadLimit` | KB/s，0 不限；补丁里 BT 和直链共用一个限速器 |
| — | `protocolConfig.http.connections` | 固定 8。内核默认 16，对国内源反而容易被限 |
| — | `protocolConfig.http.userAgent` | `ShandianDownload/<版本> (Windows)`。**别冒充浏览器**：Go 的 TLS 握手配 Chrome UA 会被当成伪装爬虫 403（Mac 版在中科大实测过） |
| — | `protocolConfig.bt.listenPort` | 固定 6881，随机端口下端口映射和防火墙规则没意义 |

`notifyOnFinish` / `preventSleep` / `watchClipboard` / `confirmNew` 四个开关分别对应：完成通知、防睡眠（`SetThreadExecutionState`）、剪贴板监听（每秒读一次，识别到能下的链接就填进输入框）、新建下载确认框。都已在 `shell.rs` 里接上。

设置页的「系统」卡片还多一个**开机启动**——它不写 `settings.json`，状态以 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 为准（`shell.rs::set_autostart`，不需要管理员）。

## 构建

### 1. 编内核（先做这个，它是地基）

```bash
./script/build_engine_windows.sh
```

产物 `C:\ShandianBuild\gopeed-shandian-amd64.exe`（43 MB，静态链接，只依赖系统 DLL）。

必须开 CGO。不开会回落成纯 Go uTP，BT 连接数会掉到个位数。脚本编完会检查二进制里有没有 `libutp`，没有直接报错。

### 2. 编外壳

```bash
cd windows/src-tauri
cargo build            # 开发
cargo build --release  # 发版
```

用 `x86_64-pc-windows-gnu` 目标，复用编内核那套 MinGW，不需要 MSVC Build Tools（省 2–4 GB）。

**target 目录要放到 `%TEMP%`，别放 C 盘工作目录。** 这台机器上 `C://ShandianBuild` 和仓库目录里
每次文件写 / 删要 100–200 ms（`%TEMP%` 只要 1.3 ms），rustc 编完删中间 `.o` 时抢不到句柄，
随机报 `failed to remove ...rcgu.o: 拒绝访问 (os error 5)`，`cargo build` 十几分钟都过不去，
重试也没用。实测：

```bash
CARGO_TARGET_DIR="$TEMP/shandian-target" cargo build   # 42 秒编完
```

`-C save-temps`（让 rustc 不删中间文件）**没用**，它照样会去删。真正管用的只有换目录。

### 绿色包要放三个文件

```
闪电全能下载.exe       ← target/release/shandian-windows.exe
gopeed.exe             ← C://ShandianBuild//gopeed-shandian-amd64.exe
WebView2Loader.dll     ← target/release/WebView2Loader.dll
```

漏掉 `WebView2Loader.dll` 时程序会一声不响地退出（日志里报 `api-ms-win-core-winrt-l1-1-0.dll`
找不到，实际缺的是这个），本机踩过一次。`gopeed.exe` 放同目录会被 `locate_exe` 的第 3 条规则找到。

### 3. 工具链放哪

都在 `C:\ShandianBuild`，不进 C 盘用户目录：

| 东西 | 路径 |
|---|---|
| Go | `C:\ShandianBuild\go` |
| MinGW (MSYS2) | `C:\ShandianBuild\msys64\mingw64\bin` |
| Rust | `C:\ShandianBuild\rustup` + `C:\ShandianBuild\cargo` |
| 内核产物 | `C:\ShandianBuild\gopeed-shandian-amd64.exe` |

### 4. 打安装包

正式版使用 `installer/bootstrap.nsi` 生成单文件 EXE。签名测试从 `output/windows/<版本>/` 的原始 NSIS 安装包和同版冻结输入生成，先验证发布校验值，再在临时副本上签主程序和内层安装器，封装后签最终 EXE。三次签名均核验签名和时间戳。

签名机器需要 Python 3.11 或更新版本、Windows SDK 的 SignTool，以及公开可信代码签名证书对应的 USB 密钥或云签名客户端。脚本默认从当前 Windows 账户的个人证书库选择证书。`SHANDIAN_SIGN_CERT_SHA1` 用证书指纹，`SHANDIAN_TIMESTAMP_URL` 用签发机构提供的 RFC 3161 地址。

```powershell
$env:SHANDIAN_SIGN_CERT_SHA1 = "<证书指纹>"
$env:SHANDIAN_TIMESTAMP_URL = "<签发机构提供的 RFC 3161 地址>"
python windows/installer/sign_nsis_release.py output/windows/0.1.74 --edition plain --destination output/windows/Shandian-0.1.74-Windows-x64-signed-setup.exe
```

也可以用 `bash windows/installer/build.sh signed`。先设置 `SHANDIAN_OUT` 为正式版的 `plain` 目录，`SHANDIAN_SIGNED_DEST` 为独立输出文件。自定义工具路径可用 `SHANDIAN_SIGNTOOL` 和 `SHANDIAN_MAKENSIS`。

#### 绿色 ZIP

正式包构建完成后，运行 `python windows/installer/make_portable.py output/windows/<版本>` 生成普通版绿色 ZIP。完整解压后直接运行主程序。要清除当前 Windows 用户的闪电设置、任务记录、账号和缓存，从托盘退出闪电，再运行 ZIP 中的「清除闪电数据.exe」确认。绿色版与安装版共用这些数据。默认下载目录保留，程序目录可在清理后手动删除。

#### 历史本地打包入口

```bash
bash windows/installer/build.sh        # 产物：C:\ShandianBuild\out\闪电全能下载_<版本>_x64-setup.exe
```

此入口把 `C:\ShandianBuild\out\shandian-<版本>\` 封进旧版 Rust 自解包外壳，供本地开发使用。

- `installer/app`：界面和安装、卸载逻辑（Tauri）。安装时把自己复制成 `uninstall.exe`。依赖旁边的 `WebView2Loader.dll`
- `installer/stub`：用户拿到的单文件外壳，只用标准库，把界面程序、dll、程序文件解到临时目录再启动，退出后清掉
- **安装方式**：仅为我（`%LOCALAPPDATA%\\Programs`，HKCU）或所有用户（Program Files，公共桌面，HKLM）。所有用户是点了安装才弹 UAC：以管理员再起一份接着装，原来那份藏起来等它结束，因为外壳要等它退出才清临时目录。装完经资源管理器启动主程序，不让它带管理员权限。这种安装主程序不能一键更新，要运行新安装包
- **浏览器扩展**：安装版从 `<安装目录>\\BrowserExtension` 加载，更新时原位替换。绿色版的文件夹可能被挪走，才同步到 `%APPDATA%` 再加载（`gateway::install`）。浏览器不允许程序静默安装，完成页勾选后显示步骤页，由用户加载
- **窗口**：不透明，圆角和细边框交给 DWM。别用 `transparent(true)` 加页面圆角，也别用 `resizable(false)`：后者会留一圈 6px 白边，固定大小要靠最小/最大尺寸
- **清单**：`installer/app.manifest` 必须声明 `asInvoker`，否则文件名带 setup/install 时 Windows 会强行要管理员
- **卸载只删**：`install-manifest.txt` 里的文件、固定的注册表项、用户勾选的数据目录。开机启动和协议关联要指向安装目录才删；数据目录里的 `downloads` 不删
- **测试**：`SHANDIAN_SETUP_SANDBOX=<目录>` 把快捷方式、数据目录、注册表都换到测试位置。`build.sh debug` 出的版本带调试端口 9223，不能发

## 开发时怎么跑

内核 exe 的查找顺序（`engine.rs::locate_exe`）：

1. 环境变量 `SHANDIAN_ENGINE`
2. `%LOCALAPPDATA%\com.shandian.windows\engine\gopeed.exe`
3. 外壳 exe 同目录
4. `C:\ShandianBuild\gopeed-shandian-amd64.exe`（开发期兜底）

所以本地开发不用手动拷 exe，`cargo run` 就能起。

## 几个踩过的坑

**本机代理会拦 127.0.0.1。** 开了全局代理时，环境变量 `HTTP_PROXY` 会让请求走代理，连内核都连不上（实测报 `502 upstream connect failed`）。`api.rs` 里 `Client::builder().no_proxy()` 已经处理。命令行调试同理，`curl` 要加 `--noproxy '*'`。

**内核必须静态链接。** MinGW 默认动态链接，产物会依赖 `libstdc++-6.dll`、`libwinpthread-1.dll`，拷到没装 MinGW 的机器上起不来。必须 `-ldflags "-linkmode external -extldflags -static"`。产物应该只剩 `KERNEL32`、`WS2_32`、`msvcrt`。

**补丁文件是 CRLF。** 源码是 LF，`git apply` 会整片对不上。脚本里先 `tr -d '\r'` 转一遍。

**子进程要 CREATE_NO_WINDOW。** 不然每起一次内核闪一个黑框。

**Go 依赖要走镜像。** `proxy.golang.org` 和 `sum.golang.org` 在国内连不上，Go 会为每个依赖超时一次，几百个依赖能干到十几分钟。用 `GOPROXY=https://goproxy.cn,direct` + `GOSUMDB=off`。**不要**设 `GOPRIVATE=*`，那会让 Go 绕开镜像直连源站，正好搞反。

## 跟上游的关系

内核补丁 `engine/shandian-gopeed-v1.9.3.patch` 是**纯增量**的：435 行新增，只删 3 行（都在 `go.mod`）。它不改上游函数逻辑，只在旁边挂东西，所以跨版本冲突面很小。

上游出新的稳定版时：

```bash
./script/check_upstream.sh              # 自动查最新 tag，试打补丁并编译
./script/check_upstream.sh v2.0.0-beta.3 # 或者指定版本
```

它会依次做三件事：严格 `git apply` → 失败就 `patch --fuzz=3` 模糊应用 → 真编一遍看能不能过。

实测 v2.0.0-beta.3：只有 `go.mod` 的依赖版本冲突（另有 Go 版本要求 1.25.4），其余 19 个 hunk 只是行号漂移。

**以后加功能也保持这个风格**——新建文件优先，别改上游函数体。这是能持续跟版本的唯一前提。

## v0.1 范围

按依赖顺序推进，托盘放最后（用户 10-30 定：先用得起来，再谈托盘观感）：

- [x] 内核、起停、健康检查、投任务、列表轮询
- [x] 链接识别（磁力 / 种子 / ed2k / 视频站 / 直链）
- [x] 保存位置三类分流 + 设置页（同时下载数、限速）
- [x] 视频站：yt-dlp 随安装包带（首次运行拷到 `bin`，之后在那里更新）；ffmpeg 首次自动下载（优先用系统里已有的）
- [x] 深链 `shandian://`、单实例、开机启动
- [ ] 浏览器扩展（原样复用，网关开 127.0.0.1:6800）
- [x] 托盘：左键开关窗口、右键菜单（打开 / 暂停全部 / 继续全部 / 打开目录 / 退出）、tooltip 显示「速度 · 任务数」
- [x] 复制即弹（剪贴板 → 填进输入框）、完成通知、新建确认框、防睡眠、开机启动
- [x] 回收站（内核 `force=false` 删除即进回收站）
- [x] 缩略图：视频站用站点封面（`SDTHUMB`），本地视频用 ffmpeg 截帧，缓存在 `thumbs/<id>.jpg`
- [x] 边下边播（`play_url` 命令 → 内核 `/play/{id}/{key}/{文件名}`，界面里 `video` 直接播，拖进度条可跳）
- [x] 预发布：`.github/workflows/windows-preview.yml` + `docs/windows-ci.md`（**没在 GitHub 上真跑过**）
- [x] 同事测试清单 `docs/windows-test.md`

不做：PikPak、迅雷通道、代码签名。

## 视频任务的几条规矩

- **代理**：系统代理不缓存到底（`proxy.rs` 2 秒刷新，端口连得上就算开着）。下载中代理开关了且 4 秒没新字节，自动换线路重连（`videos.rs::reroute`）
- **封面**：缓存在 `%LOCALAPPDATA%\\com.shandian.windows\\thumbs\\`，原封面 `<任务id>.jpg`，截帧替补 `<任务id>.frame.jpg`。截帧只是替补，取到原封面就换掉
- **BT 封面**：种子自带图片（poster、cover 等，下完整才用）> 豆瓣海报 > 从最大的视频截帧
- **进度**：分母永远是所有轨之和（`videos.rs::Progress`），一旦知道过总量就不再回到未知；滚动条只出现在最开头。界面另外保证进度只进不退（`app.js::barHigh`）
- **新建框的字幕选项**：框先开，`probe_video` 后台用 yt-dlp 查（最多 25 秒），清晰度换成视频实际有的（最多 5 档），确认没有字幕才收起；查不出来就保留，批量和不询问模式不查
- **列表上的字幕标签**看实际结果：视频没有想要的语言时记 `no_subs`，不标「中英字幕」，右侧写「这个视频没有字幕」
- **字幕**：主进程不下字幕，只落元数据（`--write-info-json` + 匹配不上的 `--sub-langs`，后者是为了让机翻语种进列表）。`spawn_subs` 读元数据后每个语系挑一条，用 `--load-info-json` 另起进程下。视频不等字幕，限流也影响不到视频；中英两条收尾时合并成一个 `.zh-en.ass` 并嵌进视频
- **`--print` 隐含 quiet**：必须带 `--no-quiet`，否则「已存在」「合并」这些日志行不输出
- **删文件**：只走 `trash::recycle`（系统回收站），内核 `force` 永远传 false。视频任务只认文件名里的「 [视频id]后缀.」或成品文件名（`videos.rs::job_files`），都不知道就一个不碰
- **重复下载**：`find_duplicate` 在投任务前查，认 `links::canonical_url`（同一视频换播放列表入口也算）；用户确认再下一份后按 `_1`、`_2` 命名。认不出 id 的站要等解析完才知道撞车，走 `collided` 再问一次
- **确认框**：别用 `confirm()`/`alert()`，WebView2 里会直接返回 false，用 `askChoice`/`askYes`

## 工具与调试

`windows/tools/` 下四个脚本，都是纯标准库（这台机器装不上 Pillow，也禁了 COM 和 Add-Type）：

| 脚本 | 干什么 |
|---|---|
| `make_icon.py` | 从 `Resources/AppIcon-source.png` 生成 `icons/icon.ico`（7 个尺寸） |
| `screenshot.py` | 截指定标题的窗口（PrintWindow，被遮挡也能抓，黑屏自动退回屏幕抓取） |
| `cdp.py` | 通过 CDP 在页面里执行 JS，用来验证前端逻辑 |
| `close_dialog.py` | 关掉弹出的原生对话框（只认 `#32770`），用来测「选择文件夹」；先 `--list` 看有哪些窗口 |

`cdp.py` 需要页面开了调试端口。临时在 `tauri.conf.json` 的窗口配置里加一行：

```json
"additionalBrowserArgs": "--remote-debugging-port=9222"
```

然后 `python windows/tools/cdp.py "typeof window.__TAURI__"`，或 `--list` 看页面。

**发版前记得删掉这一行**，否则任何本地程序都能操控页面。

### 已验证的行为（2026-10-30）

| 项 | 结果 |
|---|---|
| 窗口 + WebView2 | 正常，标题「闪电全能下载」 |
| 启动自动起内核 | 正常，`gopeed.exe` 子进程无黑框 |
| token 持久化 | `%LOCALAPPDATA%\com.shandian.windows\engine\token.txt` |
| 任务列表轮询 | 1 秒一次，进度/速度/状态都正确 |
| 链接识别 | 磁力/视频网站/直链/ed2k/不认识，5 类全对 |
| 界面投任务 | 输入 → `add_task` → 内核 → 列表更新，全通 |
| 内核下载 | HTTP 30 MB/s，BT 6.1 MB/s，文件校验无误 |
| 设置写入内核 | 启动后 `maxRunning`/限速/UA/端口/连接数 都对；改设置后内核立即跟随 |
| 目录策略 | 直链落「下载」、磁力落自定义「电影」目录，实测按类型分流 |
| 选文件夹对话框 | 原生 `#32770` 弹出，标题正确，取消返回 null 不崩 |
| 界面单元测试 | `cargo test` 8 个用例（链接识别、片名整理、选片、路径规整、目录分流） |

## 体验约定（对齐 Mac 0.18.5）

- **提示条**：`toast(msg, { action })`。10 字以内 3 秒、40 字以内 9 秒、更长或带按钮 15 秒；点一下关闭。最多 3 行，放不下用省略号，悬停看全文
- **定位任务**：`revealTask` 清搜索词、切页面、滚到中间、闪两下
- **BT 校验**：`verify_task` 走内核补丁 `opts.extra.shandianVerifyExisting`，只对运行中的 BT 任务开放。只剩不到 0.5% 且 30 秒不动时标「收尾中」，60 秒没速度自动校验，最多 3 次、间隔 90 秒
- **开发版标记**：`build.rs` 打包时看 `windows/` 和 `browser-extension/` 有没有未提交的改动，有就在「关于」标「开发版」。发版前先提交
- **豆瓣海报**：名称和年份都核对才用（`pick_poster`）；核对过的留 `<id>.poster-ok`，没有这个标记的旧海报作废重查

- **剩余时间**：单任务用平滑速度（时间常数 8 秒），显示的速度仍是瞬时值。总时间取「总剩余 ÷ 总速度」和「最慢任务自己的时间」中较长的；有大小未知或没速度的任务时写「至少还要」
- **视频任务排队**：同时最多 3 条 yt-dlp（`MAX_VIDEO_RUNNING`），多的进 `wait`，有任务结束、暂停或删除时 `pump` 接上
- **抓取页面链接**：扩展注入 `grab.js`（所有 frame），搜 `<a>` 和整页源码里的磁力、ed2k、`.torrent`，抓到就直接交给 App，之后继续监听页面变化；一开始没抓到或后来出现新链接，才在页面右上角放面板。不替用户点页面上的按钮（可能是广告或跳转）。`POST /extension/batch` 交给 App 勾选；Mac 版网关还没有这个接口，扩展会提示更新 App

- **合并后改名被占用**：yt-dlp 不重试，App 替它重试 60 秒并用 Restart Manager 查占用程序（写进 `startup.log`）；仍失败时任务报错并写出程序名，点继续直接收尾
- **无进展提示**：磁力 5 分钟拿不到文件信息、其余 10 分钟进度不动时在行内提示，只提示不停止
- **多选**：暂停、继续、重新下载、取消、删除都作用于所有选中项，只动状态适用的那些

- **排序**：顶部按钮只在已完成、分类、下载历史页出现（时间、名称、大小、类型，再点反向），选择存 `localStorage`
- **拖动顺序**：正在下载页用鼠标事件实现（Tauri 接管了系统拖放）。顺序存 `localStorage`，并通过 `set_queue_order` 告诉视频队列和内核（补丁接口 `PUT /api/v1/tasks/order`）。只影响等待中的任务，正在跑的不会被挤掉
- **任务名**：不可用鼠标选中（多选会带上文字），复制走右键「复制名称」；双击整行都是打开文件
- **字幕润色**（`polish.rs`）：先去滚动重复、拼整句、标点归位，再按批交给本机 Ollama（只连 127.0.0.1）。时间轴由程序定，模型条数不对就重试、拆批，单句仍不行保留原文。原字幕留底为 `*.original.bak`，重复润色总从留底开始，可右键恢复。只选中文时没有英文参考，只改通顺、不判断翻译对错
- **字幕翻译**：开了润色且没有人工中文时，其他语言视频只下原文（`translate_source`），翻译结果写成 `*.zh-en.ass`，原文件留着当底稿；英文视频默认润色机翻中文，设置里可改成从英文翻译
