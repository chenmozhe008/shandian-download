# 闪电全能下载（shandian-download，10-29 由 mac-downloader 改名）

> App 名「闪电全能下载」（10-29 用户定，原名「闪电下载」；卖点是常用渠道都能下 + 快；用户嫌「什么都能下」说得太满，对外别这么写）。**对外（README、推广图、文案、文章）不点名迅雷、Downie 等竞品做对比**，只写自己的实测数据（10-29 用户：以免引起商业纠纷），可执行名 `Shandian`，bundle id `io.github.chenmozhe008.shandian`

自用 Mac 下载器，目标是替代迅雷：磁力/BT/ed2k/HTTP + 视频网站 + 网盘直链，一个客户端搞定，浏览器一点就下。
方案与阶段：`docs/PLAN.md`（P0 规则 → P1 引擎 → P2 界面 → P3 浏览器与网盘 → P4 切换）。
实现细节与实测数据：`docs/internals.md`（补丁逐条说明、抖音/PikPak/扩展机制、测速记录）。本文件只放规则和红线。**仓库公开，这里和 docs/ 都别写真实片名等隐私**

## 架构（不自己写下载引擎）

- **内核**：Gopeed headless v1.9.3 **打了闪电全能下载补丁**（`engine/shandian-gopeed-v1.9.3.patch`，`script/build_engine.sh` 编译，产物打进 App、启动时换上），REST API `127.0.0.1:9999`，请求头 `X-Api-Token`；token 由 App 首次运行生成，存 `~/Library/Application Support/ShandianDownload/engine.json`（600 权限，不在仓库里），脚本也读这里
  - 内核程序和任务库在本机 `Application Support/ShandianDownload/engine/`（10-28 用户同意从移动硬盘挪来；硬盘上的旧副本 `/Volumes/Jack Feng/Applications/MacDownloader/` 没删，等用户定）
  - App 启动时没有就拉起；退出 App 只暂停任务、不关内核
  - 补丁 10 项（块记录落盘免重校验、全局限速、边下边播、429/5xx 退避、每段≥20 MB、probe 只探测、失败原因字段、按需校验磁盘已有数据等），逐条见 `docs/internals.md`；补丁测试 `internal/protocol/http/shandian_test.go`
  - **内核默认不看磁盘上已有的文件**（anacrolix 只信自己的块完成记录），所以「接手迅雷没下完的文件」「换内核后找回进度」都要显式让内核校验：创建任务 / PATCH 任务时带 `opts.extra.shandianVerifyExisting = true`（11-01 实测：不带时 4.2 GB 完整文件也会从 0 重下）
  - Go 工具链、Gopeed 源码、编译缓存都在移动硬盘 `Applications/ShandianBuild/`（不装全局）；**必须开 CGO**；`build_engine.sh` 用移动硬盘上 Xcode-26.5 的工具链（本机命令行工具链接器和 SDK 对不上）
  - 推出移动硬盘时内核会被暂停→结束→2 秒后重启（`EngineHost.holdsFiles`），插回自动续
  - 只监听 127.0.0.1，不开 0.0.0.0
- **视频网站**：yt-dlp + ffmpeg，国内站直连、境外站走系统代理。**yt-dlp 用 App 自带的目录版**（`Application Support/ShandianDownload/bin/yt-dlp_macos/`，单文件版启动慢 16 秒；每 7 天查最新 tag 重下），brew 版只做兜底
  - **参数带了 `--print` 必须同时带 `--progress`**，否则不出进度（用户：一直 0 突然下好）
  - 固定 `--concurrent-fragments 4`、`-S res,vcodec:h264,acodec:aac`（不设会挑 AV1）；别设 `--http-chunk-size`。依据见 `docs/internals.md`
- **网盘**：浏览器里的 LinkSwift 油猴脚本拿直链 → 交给内核。**只装 Greasy Fork 上 hmjz100 的版本**；GitHub `Mario82del/LinkSwift` 发安装包，疑似冒名，别碰
- 多语言（11-05）：`Resources/en.lproj/Localizable.strings`，键是中文原文；`Text("字面量")` 自动取表，String 变量显示时包 `L()`，带插值的用 `String(localized:)`。**`L()` 的结果只能显示，判断逻辑（如 `origin == "磁力"`）必须用中文原值**。中文表由 `build_app.sh` 从英文表的键生成，缺了系统会退回英文；设置里可选语言（`AppLanguage`，写 AppleLanguages，重开生效），**界面不许中英混排**：新增文字必须进翻译表；靠文字判断的地方（`noticeNeedsAttention`、`cloudFailureNotice`）两种语言都要认；**origin、迅雷按钮名等内部值绝不许包 `L()`**（`AIToolTests` 有守卫）。扩展文字在 `browser-extension/i18n.js`，跟随浏览器语言；kind 等内部值保持中文，显示时才翻
- 设置页顺序（11-05 用户定）：通用（默认打开，语言在最上）→ 下载 → 保存位置 → 下载加速 → 浏览器扩展 → AI 工具
- 客户端界面（P2）：原生 SwiftUI，`NavigationSplitView` 系统侧栏 + 系统工具栏/搜索框（10-27 用户嫌手搓侧栏和红绿灯对不齐，别再手工画侧栏）；侧栏顶部 App 图标+名字，技术约定沿用小清（`Coding Project/mac-cleanup-manager/CLAUDE.md`）：本机 SwiftUI 宏编译不过，状态放 `ObservableObject @Published`；文案像专业 Mac 软件
  - 重复投同一个链接（已在下载 / 已下过）：**切到列表、滚到那条任务并高亮**，顶部提示带「定位任务」按钮停 15 秒；只弹一条小字、人还留在新建界面上是不行的（11-01 用户反馈）。实现集中在 `DownloadStore.reveal/focus`，列表用 `ScrollToTask` 滚，提示条见 `Banners`
  - 内核启动中（刚打开 App、推出硬盘后重启）收到的链接先排队，读到任务列表后再走查重和确认框（`deferUntilEngineReady`）；只有内核真的启动失败才提示没添加
  - 提示条统一 9 秒（带按钮 15 秒）、48pt 高、带图标和描边，**浮在窗口底部**（11-05：放顶部挡住速度区，推开内容又跳）；别退回一行小灰字
  - 连不上时统一说「需要科学上网，请检查网络设置」（11-05 用户定：不能只报失败；别写「境外」，也别加括号解释）；判定用 `LocalProxy.isUnreachable` / `isOverseas`

## 入口与本机端口

- `127.0.0.1:9999` 内核 REST；`127.0.0.1:6800` App 内置网关（`Gateway.swift`）：`/jsonrpc` 兼容 Aria2 RPC（LinkSwift 推网盘直链），`/api/v1/*` 代理 Gopeed 浏览器扩展（新建任务改走 App 的添加流程：存哪、只选正片、片名整理），密钥都是内核 token
- `shandian://add?url=<编码后的链接>`（可多个 url）：脚本 / skill 交链接给 App，`open -g` 不抢焦点。**测试别再用 System Events 模拟 ⌘V**（窗口没抢到焦点时按键会落到用户正在用的程序里，10-27 踩过）
- 收回会把文件夹搬回保存目录，迅雷旧任务因此找不到种子，一点开始就报「获取种子文件失败」且不可恢复。原位置记在 `thunder-folders.json`，再交给迅雷时先搬回去再让旧任务继续（`taskExists`）
- 迅雷**没有单条开始/暂停的办法**（11-05 用两条测试任务实测）：程序选中行后按右键菜单无效，菜单只认鼠标右键点中的行；只能用「全部开始/全部暂停」。除非愿意移动用户鼠标，否则别再试
- 迅雷拒绝重复任务：收回后旧任务还留在迅雷，同一 hash 再交会弹「下载任务已存在」（回收站里的也算，须永久删除）；App 检测到就撤销转交、原任务继续，并提示用户去迅雷删（11-05 据迅雷日志 `.events/record_*.log`）
- 迅雷通道（`ThunderBridge.swift`，自动转交 11-05 起默认开）：下不动时云盘之后的第二选择；云盘没关联或刚连不上就直接走迅雷，靠辅助功能点按钮、读迅雷的进度文件；收回时按迅雷的「全部暂停」并确认不再写入。**不逆向迅雷协议、不碰会员**（用户多次要求 rev 迅雷，已拒绝：等于白用别人的商业服务）
- 云盘离线（`PikPak.swift`）：接口照开源 PikPakAPI。**不在 App 里输密码**（用户 10-28 定），用网页窗口打开官方登录页取令牌，只存 `pikpak.json`（600）；取回后彻底删除云端副本（回收站也占容量）；已关联时下不动自动转交（1 分钟没速度或 3 分钟几乎没速度）。转交优先云盘、其次迅雷。签名常量会随 PikPak 改版变，细节见 `docs/internals.md`
- 回收站：移除任务写 `removed.json`（最多 500 条），可重新下载
- 磁力默认打开方式已改为闪电全能下载（10-27 用户同意）；`.torrent` 文件关联未改
- 浏览器扩展：**用自带的**（`browser-extension/`，MV3，Chrome / Edge 通用；10-28 用户说别绑篡改猴）。打包进 App、启动时拷到 `Application Support/ShandianDownload/BrowserExtension`（浏览器记路径，别指向 dist）。用户在扩展页开开发者模式、拖文件夹进去装一次——浏览器不允许第三方静默装扩展，Chrome 137+ 也不认 `--load-extension`
  - 扩展 ID 由 manifest 的 `key` 固定，网关 `/extension/*` 只认这个 Origin；**改 key 要同步改 `Gateway.extensionOrigin`**
  - 接管原则：App 接下了才取消浏览器的下载，App 失败就退回浏览器重下；图片/网页/<5 MB 不接。完整规则见 `docs/internals.md`
  - 商店版是另一套：`browser-extension-store/`，只有右键发送文件直链；两版的区别和安装入口都在「浏览器扩展」窗口的对照表（`BrowserSetupView`），改功能时同步表格和 README；网关只精确放行 `Gateway.swift` 里列出的商店 ID，**不许放宽到任意扩展**。上架状态和后台操作方法见 `docs/CHROME_STORE_PREPARATION.md`
  - **抖音别改回包 fetch/XHR**（会和安全 SDK 防篡改死循环、页面卡死），只包 `JSON.parse` / `Response.prototype.json`；**所有视频入口都走 `downloadVideoInTab`**。抖音、小红书已实测，快手和抖音信息流未实测，视频号不接
- 退出 App：`pauseAll` 后内核留着不关，下载中的任务记进 `resumeOnLaunch`，下次打开续上（补丁后内核重启也不用校验）
- 复制即弹（`ClipboardWatcher`）：App 不在前台时，复制磁力/ed2k/种子/网盘/视频站/常见文件链接 → 右上角浮窗；普通网页不弹
- 代理：启动时读系统代理设置（`LocalProxy.check`，跟浏览器一致，不再写死 7897），没开或连不上就全部直连；开着时国内名单直连、其余 HTTP / 视频站走它（本机 Clash 是规则模式，漏在名单外的国内站也会被 Clash 直连），BT 不走
- 下载完成：打 com.apple.quarantine（`markQuarantine`，同浏览器）、放提示音 `Resources/done.wav`（`script/make_done_sound.py` 合成的上行五度「叮—咚」，不用迅雷的）；系统通知本身静音免得响两次

## 版本与更新记录（沿用小清）

- 版本号在 `script/build_app.sh` 的 `APP_VERSION`，构建号取 git 提交数。修问题升第三位，加功能升第二位
- 每次给本机安装新的开发构建，都要在上次已安装版本基础上递增版本号，并在 `CHANGELOG.md` 新建一节，只记录本次相对上次新增或修复的内容；安装前对照上次版本核实。历史补记须注明当时实际显示的版本，不得冒充当时已安装的新版本号。
- 英文更新说明在 `CHANGELOG.en.md`，发版时写；GitHub 发布说明同时有 `## 更新内容` 和 `## What's New` 两节，英文界面不显示没有英文说明的旧版本
- 每次升版本在 `CHANGELOG.md` 顶部加一节；打包时拷进 App，「关于闪电全能下载 → 更新记录」直接读它，别另写一份。写法按正式更新说明（短句、陈述事实，不写口语）
- **没发布的版本标「开发版」**：标题写成 `## x.y.z · 日期（开发版）`，界面显示开发版标记。**发版时把上个正式版之后的开发版小节汇总成正式版这一节（写用户能看懂的改动，去掉中间过程），并删掉这些开发版小节**，GitHub 发布说明用同一份；有开发版小节时正式打包会被拦下（11-04 用户定）。`build_app.sh` 默认打开发包；正式包须显式指定 `SHANDIAN_RELEASE_BUILD=1`，并确保工作区干净、版本标签指向当前提交。是否发布仍以 GitHub Release 为准。改动太少不单独发版，先留在本机开发版，攒够一批再发（11-04 用户定）。
- 「关于 → 介绍」的优势卡在 `AboutView.swift` 的 `IntroPage`，只写实测过的数据（下面「测速记录」），竞品只写查证过的事实

## 检查更新与一键更新（10-29 加，沿用小清）
- 机制和验证方法见 `docs/internals.md`「检查更新与一键更新」；Release 不含预发布，`win-v…` 不会被当成 Mac 新版

- **发版 tag 必须是 `v<版本>`、附件名照现在的格式**，否则一键更新找不到包 / 比较不出版本
- **⚠️ 测试副本别用临时签名在本机跑**（10-29 踩坑）：系统把它当新 App，反复弹「想访问文件」框、点了也过不去，而且和正式版同时开会抢端口。要测就先退出正式版、副本用开发证书签名，测完立刻关掉、`lsregister -u` 注销、移到废纸篓
- 手动把 GitHub 正式包装到本机时同理：先核对 SHA-256，再用 `codesign --force --sign "Apple Development" --identifier io.github.chenmozhe008.shandian --timestamp=none` 改回开发证书签名，否则可能反复弹权限框

## 脚本

- `script/engine.sh start|stop|status|configure`：启停内核；`configure` 写下载设置（每次内核重装/任务库重建后跑一次）
- `Sources/ShandianMCP`：给 AI 工具用的 MCP（stdio），打包到 `Contents/MacOS/shandian-mcp`，工具有 add_download / list_downloads / pause_download / resume_download；只连本机，**不提供删除**；各家接入方式在 `AITool.swift`（设置里选工具复制）
- `script/dl.sh "<链接或本地.torrent>"`：统一入口，自动分流
- `script/build_app.sh [run|package]`：编译打包成 `dist/闪电全能下载.app`（会带上已编好的内核）。**不带参数＝run：会杀掉并重开用户正在用的 App**（用户日常用 `/Applications` 里的安装版，装开发版走 `--install-dev`，见 `AGENTS.md`）；只想编译验证用 `package`
- `script/build_engine.sh`：下 Go、拉 Gopeed v1.9.3、打补丁、编译内核
- Homebrew：`chenmozhe008/homebrew-shandian` 的 `Casks/shandian.rb`，**每次发版要改 version 和两个 sha256**。本机测试加 `HOMEBREW_NO_INSTALL_FROM_API=1`（本机 Homebrew 不认 macOS 27）
- `script/install.sh`：给别人用的一键安装（README 里 `curl … | bash`）：查 GitHub 最新 Release → 装进 /Applications → 去 quarantine → 有 brew 就装 ffmpeg。`SHANDIAN_DEST=<目录> SHANDIAN_TEST=1` 在临时目录测，不碰用户正在用的 App

## 下载设置（`configure` 写入，改值先改这里的说明）

| 项 | 值 | 依据 |
|---|---|---|
| 落盘 | 磁力/种子/ed2k → `/Volumes/Jack Feng/movies`；视频站和其余 → `~/Downloads`（10-28 用户定：视频网站不再单建「视频」） | 用户主要下电影；普通文件不混进电影目录 |
| 代理 | 内核全局代理**关**；磁力/种子一律直连；HTTP 直链、视频站按域名判断（`LocalProxy.needed`）：国内站直连，其余单个任务走系统代理 | 全局开着时 BT 全绕 Clash、速度乱跳；用户定别一刀切 |
| HTTP 连接数 | 8 | 镜像站对 16 线程回 403/418 |
| BT 监听端口 | 6881 固定 | 默认随机端口不利于被连入 |
| HTTP User-Agent | `ShandianDownload/<版本>`，不冒充浏览器（`EngineClient.userAgent`） | 冒充 Chrome 被镜像站识别为爬虫 403；要浏览器身份的下载由扩展带真实 UA/Cookie |
| tracker | 每次 configure 从 trackerslist.com + ngosang 合并拉取 | headless 内核没有桌面版的 tracker 订阅，空列表时磁力解析很慢 |

## 测速

- 数据和对比脚本在 `docs/internals.md`「测速记录」。对外只用区间（磁力起速 5–18 秒），不写竞品数字
- 「比迅雷会员快」在一种情况做不到：迅雷服务器缓存命中的冷门资源。不要对用户承诺这一项

## 已知事项（踩坑警示）

- Gopeed API 新建任务参数是 `opts`（不是 `opt`），写错不报错、路径静默失效
- **磁力/种子任务绝不能带 `req.proxy`**：BT 客户端全内核只建一次，代理取第一个 BT 任务的设置，之后改不了（读 v1.9.3 源码确认）
- yt-dlp 抖音带登录 Cookie 也 403，所以抖音靠扩展拿页面里的视频地址当直链下；lux 已失效，不用
- 上传为 0 多半是对方不要数据或路由器没开 UPnP，不是上传被关（`cfg.Seed = true`）
- `download-movie` skill 10-28 改成 `dl_shandian.sh` → `shandian://add?confirm=0&url=…`（不弹确认框）；闪电全能下载不可用时退回迅雷 `dl_fast.sh`

- **PikPak 和迅雷都只是中转**（11-04 用户定）：下完文件回保存目录、清中转目录和半截文件、云端副本彻底删（回收站也占容量），都不让用户动手；细节见 `docs/internals.md`
- **自动化测试别挪用户的鼠标**：11-05 为测悬停把光标移到「移除」按钮上，弹出了移除确认框。悬停效果让用户看
- **测试别碰用户迅雷的任务状态**：11-04 按「全部开始」把两条已暂停任务变成了报错。控制迅雷的测试先确认列表里没有用户任务
- 留多少包看 `tools/mac-app-workflow/README.md`：`--install-dev` 只在 `tmp/previous-install` 留上一版；发版收尾时把 `output/release` 的旧版本移到废纸篓

## 仓库

GitHub **公开**库 `chenmozhe008/shandian-download`（10-28 建，10-29 用户同意公开，分支 master，GPL-3.0——内核基于 Gopeed GPL-3.0，只能用 GPL）。推送仍按全局红线先问用户
- 发布：**M 芯片和 Intel 分开出包**（10-29 用户定：通用包内核翻倍，且用户习惯按芯片选）。`script/build_engine.sh` 一次编出 `gopeed-shandian-arm64` / `-amd64`；`ARCH=arm64|x86_64 script/build_app.sh package` 按芯片打包（本机以外的芯片在 `dist/<芯片>/`）。`output/release/<版本>/<芯片>/` 里拷 App（`ditto`，Release 说明写在 `output/release/<版本>/notes.md`；旧版本目录发版后移到废纸篓），**`script/release_sign.sh <App>` 改用发布专用的自签名证书再打包**（本机 Apple Development 证书带用户邮箱不能外发；ad-hoc 每次更新都会让辅助功能授权失效。私钥在 `~/Library/Application Support/ShandianRelease/`，不进仓库，丢了可重建但用户要再授权一次），`ditto -c -k --keepParent` 打 zip，`gh release create v<版本>`。**附件名 `Shandian-<版本>-AppleSilicon.zip` / `Shandian-<版本>-Intel.zip`**（英文，中文名会被 GitHub 改成 default.zip；以后 Windows 照此加 `-Windows-x64`）。v0.16.0 首个公开版，v0.17.0 起分芯片
- **install.sh 按附件名后缀 `-AppleSilicon.zip` / `-Intel.zip` 选包**（芯片用 `hw.optional.arm64` 判断，Rosetta 终端下 `uname -m` 不准）：发版改附件名要同步改脚本，否则一行命令安装失效
- Intel 版是尽力支持：发版前用 Rosetta 跑 Intel 内核实测（另开端口 9998、BT 6882，下一个公开文件 + Sintel 磁力）。本机升到 macOS 27 后没有 Rosetta，就只能编译不能实测，届时跟用户商量是否停掉 Intel 版
- 公开页面别放下载记录截图（有片名）；README 的安装步骤要和 App 里「浏览器扩展」窗口的按钮文字一致；对外数字用测速记录里的区间（磁力起速 5–18 秒），别挑最有利的单值，不带竞品对比
- 图标 10-29 定稿 J1（`Resources/icon/icon.html`：深橙底 + 原闪电箭头，和闪电说的黄橙区分）；菜单栏 18 点。别再换图形，改前先出方案给用户挑
- 推广图和文案在 `output/闪电全能下载-推广/`（源文件 `output/promo/`，Chrome 无头渲染 1080×1350 @2x）
- 定位（11-05 用户定）：**任务卡在 0%，自动换路下完**；免费开源、原生 Mac。README 标题区和 About 用这一句，别退回功能清单，也别主打创作者。演示动图 `docs/images/demo-rescue.gif` 源文件 `output/readme-shots/demo.html`（`?page=demo&t=秒`，Chrome 无头逐帧 + ffmpeg），数字用实测量级
- README 截图（`docs/images/screenshot-*.png`）是仿真图：`output/readme-shots/shot.html?page=active|library`，照 v0.16.0 界面手画、片名只用公版/开源片，Chrome 无头 1000×700 @2x 渲染。界面改版后要同步改这个 HTML 再重渲；README 措辞用产品说明的书面语、数字带测试条件（10-29 用户：「能下什么」这类话不像 App 该说的）。README 标题区（logo 下三行）和 GitHub About 简介 / topics（`gh repo edit`）是同一套说法，加减功能时两处一起改

## 目录

`script/` 脚本 · `docs/` 方案 · `tmp/` 临时（用完清）· `output/` 效果图等产物 · `logs/` 内核日志（gitignore）
