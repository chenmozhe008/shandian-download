# What's New

## 0.22.0 · 2026-11-23

- **Bulk media downloads from Qingque Docs**. Scan the open document with the full Chrome or Edge extension to list original audio and video attachments available to your browser session, including items you have yet to scroll to. Save files with their original names or select several to download as one ZIP.
- **Bulk downloads from web pages**. Scan the current page for downloadable links and add selected items together. On Feishu Docs and similar pages, use this for accessible attachment links. You can also pack supported audio and video found by scanning or detection into a ZIP. Results depend on which resources the page exposes and what your account can download.
- **ZIP task progress**. The app shows the number of files downloaded, transfer speed, and when it is creating the archive. Resuming or retrying reuses downloaded files that passed validation.
- **Baidu Netdisk share downloads**. Paste the share URL and access code, then use Shandian to save the files locally after installing and authorizing Baidu's official `bdpan` tool. The Baidu Netdisk desktop client is optional. The app shows transferred bytes, speed, and percentage when the total size is available. It also offers the official share page and explains common link errors, including attempted transfers of your own shares. Transfers and speeds are subject to Baidu Netdisk.
- The bundled full browser extension is now `1.11.34`. Reload it once on your browser's extensions page after upgrading.

## 0.21.2 · 2026-11-22

- The browser extension now supports “Detect Media on This Page”: captures common audio and video formats automatically, shows a live badge count on the toolbar icon, and adapts the main action to audio or video.
- Added inline audio preview with a waveform and progress bar: click to preview in the current tab's context (preserving referrer and sign-in state), with live time display and click-to-seek along the waveform.
- Automatic media naming: detects the item's title from the active card on the page, skips opaque hash filenames and UI labels, and saves downloads with readable names.
- Multi-select ZIP download: selecting multiple audio or video items packs them into a single `.zip` archive via the desktop app instead of opening multiple confirmation dialogs; stopping detection with captured items now asks for confirmation with Cancel / Clear.
- Starting a download brings the main Shandian window to the front on the Active downloads view so progress is immediately visible.
- The browser extension has been updated to `1.11.32`. Click Reload for it once on the browser's extensions page.

## 0.20.6 · 2026-11-15

- When a download makes no progress for a while, Thunder is now the first option: it connects directly, uses no proxy traffic and no daily cloud quota, and PikPak becomes the fallback.
- PikPak hand-offs disclose the cost first: before handing a task over, the app states that fetching it back goes through the system proxy, roughly how much traffic that takes, and offers "Download via cloud (uses proxy traffic)", "Hand to Thunder" and "Keep waiting". Each task is asked once; several stalled tasks are merged into a single prompt. Manual hand-offs ask the same way once, with an option to stop asking.
- Handing a task to Thunder no longer raises repeated system permission requests: Accessibility is checked silently, and when it is missing a single notice explains it with an "Open System Settings" button. Settings › General shows the current state with a direct way to grant it.
- New tasks start in the last used save location: magnet and torrent files, video sites and other files are tracked separately. The location menu offers "Remember this location", and Settings › Downloads turns this on or off.
- Task state stays in sync with Thunder: tasks deleted in Thunder, or cancelled in its new-task window, are detected automatically and marked "Deleted in Thunder" or "Not handed to Thunder" instead of staying on "Check Thunder". Hand them over again from the notice, the row button or the context menu, keeping progress and save location.
- Fewer notices: none appear after a successful hand-off, and when confirmation in Thunder is required the app shows a single line and reveals that task.
- Fewer requests for the Downloads folder: it is read only when needed, and declining access is not asked again. The system dialog now explains why access is needed.
- The download engine and the video parser are signed with the app's own certificate, so the system treats them as part of the app and permissions already granted carry over to new versions.

## 0.20.1 · 2026-11-06

- Clicking a torrent file on a web page now hands it straight to Shandian Download Manager.
- On Douyin, Xiaohongshu, Bilibili, YouTube and other video sites, a Download button appears in the top-right corner when you point at a video.
- Settings › General now lets you make Shandian Download Manager the default app for magnet links and torrent files. It asks once on first use.
- The browser extension takes over downloads from web pages: files of 5 MB and above, and torrent files. The extension is also on the Chrome Web Store and Edge Add-ons, and the store version takes over downloads and sends torrent links in the same way.

## 0.19.11 · 2026-11-06

- Fixed a flood of “New Download” windows that could appear when opening the browser. The extension no longer takes over downloads that other extensions start in the background, takes over the same link only once within a short time, and the app no longer opens a second window for a link that is already waiting for confirmation.
- Fixed “Open Shandian at Login” possibly failing to start in release builds. The login item is removed when the app is uninstalled.
- The extension icon in the browser toolbar is larger, matching other extensions.
- The browser extension has been updated. Click Reload for it once on the browser's extensions page.

## 0.19.7 · 2026-11-05

- Fixed tasks taken back from Thunder staying on “Check Thunder” and never being retrieved.
- Fixed “Failed to get torrent file” when handing a task back to Thunder: the folder now returns to where Thunder expects it, and its original task continues.
- If Thunder says the task already exists, the hand-off is undone, Shandian keeps downloading, and it explains that the old task must be permanently deleted in Thunder, including from its trash.
- Stuck tasks now go to Thunder automatically when PikPak isn't linked or can't be reached. This is on by default and can be turned off in Settings.
- A task that is paused in Thunder after hand-off now starts automatically. Other paused tasks in Thunder start too.
- Clear messages when PikPak, GitHub or a site can't be reached and a VPN or proxy may be needed.
- Guidance when Accessibility permission is missing, plus a permission status in Settings.
- Notices moved to the bottom of the window so they no longer cover the download speed. The list no longer flickers during Take Back.
- Release builds now use a fixed signature, so Accessibility permission survives future updates. Updating from an older version requires allowing it once more.

## 0.19.0 · 2026-11-05

- When a task is stuck, Shandian explains the PikPak and Thunder routes. After you sign in to PikPak, the task moves over automatically, and a local counter shows how many tasks were rescued.
- Thunder and PikPak now act only as relays: finished files return to the original location, and relay folders and cloud copies are cleaned up automatically. Retrieval from Thunder shows its progress in stages.
- Pause All and Resume All also control tasks in Thunder. If Thunder isn't open, Shandian says so, and tasks continue once it opens.
- New scheduled downloads: download only during chosen hours, with automatic pause and resume.
- New language setting: Same as System, Simplified Chinese or English. The browser extension also supports English and follows the browser's language.
- New MCP server for AI tools to add downloads, check progress, pause and resume. Works with Claude Code, Codex, ChatGPT, Cursor and more, and can't delete tasks or files.
- Diagnostics can be exported for feedback, without paths, titles, links or tokens.
- Settings reorganized: General comes first and opens by default, all pages are the same size, and Browser Extension and AI Tools have their own pages.
- Notices now float above the content so the list doesn't jump. Tooltips share one style and appear immediately.
- Fixed missing context menu items, play-while-downloading and automatic hand-off after retrieval. Fixed Settings and About needing two clicks when the window wasn't in front.
