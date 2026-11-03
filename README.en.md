<div align="center">

<img src="docs/images/icon.png" width="128" alt="Shandian icon">

# Shandian Download Manager

**A native macOS download manager**

Manage magnet links, torrents, ed2k, HTTP downloads and cloud-drive links,<br>
plus videos from YouTube, Bilibili, Douyin, Xiaohongshu, WeChat Channels and more — in one window

Browser extension · Stream while downloading · Resume after restart · Cloud offline download

[简体中文](README.md) | English

![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue)
![Platform](https://img.shields.io/badge/macOS-14%2B-black?logo=apple)
![Apple Silicon | Intel](https://img.shields.io/badge/Apple%20Silicon%20%7C%20Intel-arm64%20%7C%20x86__64-555)
![SwiftUI](https://img.shields.io/badge/SwiftUI-native-orange?logo=swift)

<img src="docs/images/screenshot-downloading.png" alt="Shandian: downloads in progress" width="900">

</div>

Shandian Download Manager (闪电全能下载) is an open-source download manager for the Mac. It brings magnet links, torrents, ed2k, HTTP/HTTPS files, cloud-drive direct links and video platforms together in a single window. Copy a link and press ⌘V in the window to start; the link type is detected automatically. With the companion browser extension, downloads started on web pages are handed over to Shandian.

The app is written in SwiftUI. Downloads are handled by a patched build of [Gopeed](https://github.com/GopeedLab/gopeed), and video platforms are supported through [yt-dlp](https://github.com/yt-dlp/yt-dlp).

> **Language note:** the app interface is currently available in Simplified Chinese only.

## Supported sources

| Source | How to add |
| --- | --- |
| Magnet links, torrent files, ed2k | Copy the link and press ⌘V, or drag a torrent file into the window |
| HTTP / HTTPS files | Copy the link and press ⌘V; with the browser extension installed, downloads are taken over automatically |
| YouTube, X, Bilibili, Weibo | Copy the video page URL and press ⌘V |
| Douban movie trailers | Paste the trailer-page URL |
| WeChat Channels | Paste the share link and follow the guide to use the browser extension on a signed-in Tencent Yuanbao page |
| Douyin, Xiaohongshu, Xinpianchang | Open the video page in the browser and send it via the extension |
| Baidu Netdisk and other cloud drives | Obtain a direct link with the [LinkSwift](https://greasyfork.org/scripts/583582-linkswift) userscript (installed separately), then send it to Shandian |

Regular video sites use the highest available quality, preferring H.264 and AAC. Douyin and Xiaohongshu support watermark-free downloads. WeChat Channels uses the video returned by Yuanbao and does not currently offer quality or subtitle selection; it requires a signed-in Tencent Yuanbao session; the share link is submitted to Tencent only when you request parsing. Availability and quality depend on the site and account permissions.

## Performance

- **Fast magnet start**: fresh trackers are merged from public lists on every launch, and a fixed BT listening port makes it easier for peers to connect. In tests with public-domain films, magnet downloads started within 5–18 seconds
- **Multi-connection HTTP**: large files are split into segments of at least 20 MB and downloaded in parallel. Rate limiting (429) and temporary server errors (5xx) trigger automatic back-off; a task is only marked as failed after 3 minutes without progress
- **Concurrent video fragments**: YouTube and similar platforms are downloaded with 4 concurrent fragments; in testing this raised throughput from 1.4–1.6 MiB/s to 3.4–4.1 MiB/s
- **Cloud offline download (optional)**: resources with no active seeders can be fetched by PikPak's servers first, then pulled down from the cloud. Directories with 500 or more entries or a paginated response are not pulled until their complete contents can be confirmed; cloud folders are retained
- **Per-domain proxy routing**: follows the system proxy setting — overseas sites go through the proxy, while mainland China sites and BT traffic connect directly

> Figures above were measured on the author's network. Actual speed depends on network conditions and resource availability.

## Features

<div align="center">
<img src="docs/images/screenshot-library.png" alt="Shandian: movie library" width="900">
</div>

- **Library**: tasks are sorted into Movies, Videos, Cloud Drive and Other Files; finished films are shown as thumbnails and open with a double-click
- **Stream while downloading**: play a film before it finishes; seeking prioritizes the corresponding pieces
- **Background and resume**: downloads continue after the window is closed, with live speed in the menu bar; unfinished tasks resume on the next launch without re-verification
- **Clipboard detection**: copying a download link in another app shows a prompt in the top-right corner — click to start
- **Settings**: separate save locations for torrents, video platforms and other files; adjustable concurrent tasks and a global speed limit
- **Trash**: removed tasks are kept in a list and can be downloaded again
- **Notifications**: a system notification and a sound when a download completes
- **One-click update**: when a new version is out, "发现更新" (update available) appears at the bottom of the sidebar; click it to download the build for your chip, verify and install it inside the app, which then reopens and resumes unfinished tasks

## Requirements

- macOS 14 or later
- Apple silicon (M-series) or Intel Mac — a separate package is provided for each
- ffmpeg for video platforms (used to merge audio and video; see below)

## Installation

### Option 1: one-line install (recommended)

Open Terminal, paste the command below and press Return:

```bash
curl -fsSL https://raw.githubusercontent.com/chenmozhe008/shandian-download/master/script/install.sh | bash
```

The script downloads the latest release, checks its SHA-256 when provided by GitHub, installs it in Applications, opens it, and avoids the "unidentified developer" prompt. If [Homebrew](https://brew.sh) is installed, ffmpeg is installed as well. Run the same command again to update; from 0.18.0 the app can also update itself.

### Option 2: manual install

1. Download the package for your Mac from [Releases](../../releases): `Shandian-<version>-AppleSilicon.zip` for M-series chips, `Shandian-<version>-Intel.zip` for Intel processors (see Apple menu → About This Mac). Unzip it and drag the app into Applications
2. On first launch macOS will report that it cannot verify the developer (the app is not notarized). Click Done, open **System Settings → Privacy & Security**, and click **Open Anyway** near the bottom. Subsequent launches work normally
3. For video platforms, install ffmpeg: `brew install ffmpeg`

## Browser extension

The extension hands web downloads to Shandian automatically and is required for Douyin, Xiaohongshu and Xinpianchang videos. It works in Chrome and Edge, and updates together with the app.

1. In the menu bar choose **闪电全能下载 → 浏览器扩展…** (Browser Extension), then click **Chrome** or **Edge** to open the browser's extensions page
2. Turn on **Developer mode** (top-right in Chrome, left sidebar in Edge)
3. Back in Shandian, click **显示文件夹** (Show Folder) and drag the `BrowserExtension` folder onto the extensions page

Once installed, the window shows **已连接** (Connected).

## License

[GPL-3.0](LICENSE). Download only content you have the right to obtain, and follow each site's terms of service and applicable law. Films shown in the screenshots are public-domain or open-source works; the interface data is for demonstration.
