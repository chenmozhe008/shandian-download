# 实现细节与实测记录

开发备查：内核补丁、各通道的实现机制、实测数据。规则与红线在根目录 `CLAUDE.md`，这里只讲「怎么工作的」和「依据是什么」。

## 内核补丁（`engine/shandian-gopeed-v1.9.3.patch`）

基于 Gopeed headless v1.9.3。补丁测试 `internal/protocol/http/shandian_test.go`。

1. 块完成记录落盘（bolt，`engine/data/piece-completion`）+ 关掉 .part 文件——anacrolix 用 .part 时启动会把没下完文件的块全标未完成，记录等于白存。原版重启内核会整片重新校验，由此解决；App 里「正在校验」（`checkFloor`）显示逻辑保留，换内核那次用得上
2. 全局限速（`config.extra.shandianDownloadLimit`，KB/s，BT 和直链共用；原版没有限速）
3. `PUT /tasks/{id}/stream` 顺序下载、`GET /play/{id}/{key}/{名字}` 边下边送（key = sha256("shandian-play:"+token+":"+id) 前 24 位，播放器带不了请求头）
4. 新建任务前丢掉「解析」留下的同种子副本，否则文件写到默认目录
5. 解析回 403 时带 `Range: bytes=0-` 重试（新片场 xpccdn）
6. 429/5xx 指数退避（2→30 秒），连续 3 分钟没进展才放弃并报错（原版永不放弃、任务永远「下载中」）
7. 每段至少 20 MB（同 aria2 min-split-size），小文件 1 个连接
8. `/resolve?probe=1` 只探测、立即关掉（原版解析会把 HTTP 文件整个预下载到 $TMPDIR 且不清，App 不用 rid 等于下两遍）
9. 任务字段 `shandianError` 存失败原因（App `EngineTask.failureText` 翻成人话）
10. **按需校验磁盘上已有的数据**（`opts.extra.shandianVerifyExisting = true`，创建任务和 PATCH 任务都认）——2026-11-01 实测：anacrolix v1.60 的 `queueInitialPieceCheck` 只要块完成记录报得出状态就不读磁盘，所以把迅雷下了一半的目录交给内核，它会把整份重新下一遍（4.2 GB 完整文件也一样）。带这个标记时 `Fetcher.verifyExisting` 在 `Start()` 里先 `VerifyDataContext` 再放行下载，磁盘上认得的块直接算完成。收回迅雷、换内核后找回进度都走它；8 MB 自造种子实测：完整文件 1 秒 done、内容未改；半成品只算前 4 MB（`/tmp/recheck`）

编译环境：Go 工具链、Gopeed 源码、编译缓存都在作者移动硬盘 `Applications/ShandianBuild/`；必须开 CGO（不开时 anacrolix 用纯 Go uTP，2026-10-28 实测只剩 1 个连接）；本机命令行工具的链接器和 SDK 对不上，`build_engine.sh` 用移动硬盘上 Xcode-26.5 的工具链。

推出移动硬盘：DiskArbitration 推出审批回调里查内核是否开着那块盘上的文件（`EngineHost.holdsFiles`），开着就暂停 → 结束内核 → 批准 → 2 秒后重启，插回自动续。

## 视频网站（yt-dlp）

- 用 App 自带的目录版（`Application Support/ShandianDownload/bin/yt-dlp_macos/`，官方 `yt-dlp_macos.zip`）：2026-10-29 实测单文件版每次启动解包 + 系统扫描白等 16 秒，目录版 0.3 秒。目录版不能 `-U`，每 7 天查 GitHub 最新 tag，不同就重下；brew 版只做兜底（2026-10-27 实测 brew 的 2025-04 版下 B 站 412）
- 速度用最近 3 秒字节自己算，yt-dlp 报的瞬时值开头能到 200 MB/s
- `--concurrent-fragments 4`：2026-10-29 实测 YouTube 默认 1.4–1.6 MiB/s → 3.4–4.1；8、16 不比 4 明显快；`--http-chunk-size` 调大调小都不快
- `-S res,vcodec:h264,acodec:aac`：默认会挑体积最小的 AV1（实测拿到 480P AV1），剪辑软件兼容差
- 实测（2026-10-27，最新 yt-dlp）：B 站不登录 1080P ✅（1080P+/4K 要登录 cookie）、微博 1080P ✅；抖音匿名 ttwid 403 ❌；小红书 yt-dlp 自带测试笔记拿不到视频
- lux（iawia002/lux）最后发版 2024-05，抖音/小红书失效、B 站只有 480P，不用

## 抖音、小红书（浏览器扩展）

- 不在 App 里做登录：扩展把页面地址 + 这两家 Cookie 交给网关，存成 `Application Support/ShandianDownload/cookies-<douyin|xhs>.txt`（600）给 yt-dlp `--cookies`（`SiteCookies.swift`）；抖音 `?modal_id=` 地址扩展里换成 `/video/<id>`
- 2026-10-28 实测 yt-dlp 抖音带登录 Cookie 也 403（网页 API 加了签名校验），所以扩展先在页面里找正在播的 `<video>` 真实地址（http 开头）直接交给内核当直链下，带 Referer/UA/Cookie；找不到才退回页面地址给 yt-dlp
- 抖音（2026-10-29 用户实测 ✅，1080P H.264 无水印、不用登录）：`douyin-hook.js`（content_scripts，document_start、world MAIN）包一层 `JSON.parse` / `Response.prototype.json`，记下页面自己请求到的 `aweme_detail` / `aweme_list`。签名 a_bogus 由抖音页面自己算，改算法不受影响（开源 TikTokDownloader / f2 / Evil0ctal 都是自己算签名，f2 已 403）
- 不包 fetch/XHR 的原因：抖音安全 SDK 加载后会换掉它们（4 次 3 次被挤掉），用属性陷阱硬抢最外层会和它的防篡改死循环、页面卡死
- 同一条视频多个接口都有，「相关推荐」里只带低画质地址，只在更好时替换（H.264 > 分辨率 > 码率）。`v26-web.douyinvod.com` 要带抖音 Referer
- 右键视频画面原来按链接处理、blob 就交页面地址给 yt-dlp，用户实测失败（扩展 1.11.2 修：所有入口走 `downloadVideoInTab`）
- 未实测：抖音信息流（精选 / modal_id，按正在播的卡片 `data-e2e-vid` 取）、快手（`<video>` 是普通 mp4）。小红书 2026-10-29 用户实测可下 ✅
- 视频号不接：唯一能用的 wx_channels_download 要装共用私钥的根证书 + 接管系统代理

## 浏览器扩展接管规则

- 扩展 ID 由 manifest 的 `key` 固定为 `pggchniffhkcgigajlapkboikmgdipel`，网关 `/extension/*` 只认这个 Origin，不用密钥
- App 接下了才取消浏览器那边的下载；App 没开浏览器照常下（不拉起 App）。图片/文本/网页和已知 <5 MB 的不接（参照 IDM 默认不接图片）
- 接手后扩展每 5 秒问 `/extension/status` 共 5 分钟，App 那边失败就移除任务、扩展用浏览器重下（不再拦截）
- 用户主动点的（面板按钮、右键）App 没开就打开 `shandian://open`，等 ping 的 `ready`（内核就绪）再交
- 浏览器不允许第三方静默装扩展，Chrome 137+ 也不认 `--load-extension`，所以要用户拖文件夹装一次

## 云盘离线（`PikPak.swift`）

- 接口照开源 PikPakAPI：登录 captcha_init → signin；离线 POST `/drive/v1/files`（UPLOAD_TYPE_URL）；进度 `/drive/v1/tasks`；下载地址要 captcha 的 GET `/drive/v1/files/{id}` → web_content_link
- 登录：App 里的网页窗口打开 PikPak 官方登录页，读 localStorage 的 `credentials_<client_id>` 和 `deviceid` 后删掉网页那份，之后只由 App 刷新（网页端常量来自开源 peach ADR-0093 / AList）。只存令牌 `pikpak.json`（600）
- 网页端签名时间戳会随改版变（2026-10-28 为 1793092691562，盐没变），设备号用 deviceid 最后一段前 32 位；签名被拒时 `refreshWebSign` 自动从网页脚本重取
- 拉回本地后删云端文件（免费 6 GB）。免费约 3 次/天、20 GB/天，所以默认手动
- 2026-10-28 实测：一部国内热门片 BT 为 0，PikPak 7.9 秒云端完成，拉回约 3 MB/s，自动删云端成功

## 迅雷通道（`ThunderBridge.swift`，默认关）

设迅雷 LastSavePath、用迅雷打开链接、辅助功能 API 点「立即下载」；文件夹按 `.<INFOHASH>.js` 认，也按「交过去之后新建、里面有 `.magent_*.torrent`」认。

找文件夹的候选位置有三处：我们指定的保存位置、迅雷自己记的 `LastSavePath`、系统「下载」（「下载」里再允许多套一层）。2026-11-01 用户报「迅雷跑得飞快、这边一直等待迅雷开始」，就是因为只认一个位置：用户在迅雷里改过保存位置、或交给迅雷时它沿用了旧文件夹，就永远找不到。找不到时 2 秒才重试一次（保存位置里常有上百个文件夹，一秒一次太浪费）。任务条目上照实写「已交给迅雷」，不再假装「等待迅雷开始」。

进度读任务文件夹里的 `.<INFOHASH>.js`（base64 包的 JSON，`subinfo0/1…` 里 `checkedsize` 已完成、`dlsize` 总大小，多文件逐个加），`checkedsize` 追上 `dlsize` 算下完。2026-11-01 实测：下完这个文件也不删，`.js` 还在，所以不能靠它消失判完成；文件占用按 4K 对齐、比真实大小多出几 MB，靠它判「满」永远差一点——两条都踩过，才有现在这条规则。老版本迅雷下完会清掉 `.js`，那条路留着兜底：没有 `.js` 时要「大小到齐」或「20 秒没再长」才算完成，免得迅雷刚建好文件夹、还没写进度文件的那几秒就被误判成下完。

收回（不用迅雷了改由本应用下）：拿文件夹里的 `.magent_*.torrent` 建任务、目录指回原处，内核会校验已有数据只补没下的（2026-11-01 用 8 MB 自造种子实测：文件完整时 1 秒内直接完成、内容和 mtime 都没被改；半成品保留已下的一半。用 256 MB 半成品再测：内核 0.5 秒就报出已有的 128 MB，不会先掉到 0）。

收回前必须确认迅雷没在写：迅雷的暂停按钮点不到（辅助功能拿不到它的窗口），所以采样 1.5 秒——文件夹里文件占用与修改时间都没变才算停了；还在写就只弹一句「先在迅雷里暂停它」，不接受收回。判不出（连文件夹都没找到）时不能当「已停」：那样会在迅雷还在写的时候另建一条任务抢同一个文件（11-01 实测：本地这条只有 0.6 MB，迅雷那边其实已下完 589 MB），这时弹「再等等 / 仍然收回」让用户决定。2026-11-01 第一版是「建成暂停任务、让用户去停迅雷再点继续」，用户看到的是「已暂停 + 进度 0」，像从头重下，已弃用。迅雷那边的任务不一定要删，它目录里那份 `.js` 会留着；重新交给迅雷时才需要清掉。

迅雷是第三方软件、不是依赖：别处电脑多半没装。是否安装用 `NSWorkspace.urlForApplication(withBundleIdentifier: "com.xunlei.Thunder")` 查（LaunchServices 登记的位置，含装在 `~/Applications` 或改过名的副本），`/Applications/Thunder.app` 只作兜底，结果缓存 3 秒、`recheck()` 可立即刷新。没装时用户主动点「用迅雷下载」弹说明 + 官网（`https://mac.xunlei.com/`，Intel 与 Apple 芯片同一个包），自动接管那条路只提示、不弹窗，任务继续在本地下。设置里另有一行状态：没装也不藏，给官网与「重新检测」。

## 网络与上传

- 全局代理开着时（2026-10-28 实测）BT 连接全绕 Clash、上传为 0、速度几 KB～4 MB/s 乱跳，所以 BT 一律直连
- BT 客户端全内核只建一次（`internal/protocol/bt/fetcher.go initClient`），`cfg.HTTPProxy` 取第一个 BT 任务的代理设置，之后改全局代理不生效、要重启内核（2026-10-28 读 v1.9.3 源码确认）
- 上传：内核 `cfg.Seed = true`。某片上传一直 0 是对方不向我们要数据（迅雷用户为主），同时下的另一部传了 49 MB。作者路由器 SSDP 发现不到 UPnP 网关，IPv4 下别人连不进来，要在路由器里开 UPnP
- HTTP 连接数 8：清华/中科大/华为云镜像对 16 线程回 403/418（2026-10-27 实测被封）
- User-Agent 如实写 `ShandianDownload/<版本>`：Go 的 TLS 握手配 Chrome UA 被中科大等镜像识别为伪装爬虫 403，Chrome 116/141 都拦（2026-10-27 实测）
- 维基百科原图（upload.wikimedia.org）对代理机房 IP 按频率随机放行（2026-10-28 实测隔 3 秒约一半 429），靠补丁 6、7（单连接 + 退避）

## 测速记录

- 2026-10-27 公开片源对比（迅雷 vs Gopeed，轮流测、每边 2 分钟）：磁力起速 45–64 秒 vs 5–18 秒；海外种子 0 vs 5 MB/s；迅雷会员只在命中缓存时短暂爆发
- 新内核（tracker 全 + 固定端口）：同一海外种子 8–10 MB/s（桌面版 Gopeed 约 5）；HTTP 腾讯 CDN 峰值 41.6 MB/s（≈ 带宽上限）
- 对比脚本 `script/compare.sh`（迅雷部分复用 `download-movie` skill 的 `dl_fast.sh`）
