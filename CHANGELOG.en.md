# What's New

## 0.20.6 · 2026-11-15

- Thunder is now the first suggestion when a download stalls: it connects directly, uses no proxy traffic and no cloud quota. PikPak becomes the fallback.
- PikPak offline downloads now ask first: before handing a task over, the app explains that fetching it back goes through the system proxy, roughly how much traffic that takes, and offers "Download via cloud (uses proxy traffic)", "Hand to Thunder" and "Keep waiting". It asks once per task, merges several stalled tasks into one question, and the manual path asks the same way the first time, with a "don't ask again" option.
- Handing a task to Thunder no longer triggers the system permission dialog every time. Accessibility is checked silently; when it is missing, one notice explains it with an "Open System Settings" button, and Settings › General shows the current state with a "Grant access" button.
- The last save location is remembered per category (magnet and torrent, video sites, other files), so a new task starts where you last saved. The location menu and Settings › Downloads let you control it.
- If you delete a task inside Thunder, or cancel it in Thunder's new-task window, the app notices within 25 seconds and shows "Deleted in Thunder" or "Not handed to Thunder" instead of staying on "Check Thunder". Resubmit from the notice, the ↻ button or the context menu, with progress and save location preserved.
- No more two-line notices: nothing pops up after a successful hand-off, and when you do need to click "Download Now" in Thunder the app shows one short line and scrolls to that task.
- The "access files in your Downloads folder" prompt no longer repeats: the app avoids scanning Downloads, looks there only occasionally when it really has to, and stops asking once you decline. The dialog now explains why it is needed.
- The download engine and the video parser are signed with the app's own certificate, so the system treats them as part of the app and keeps the permissions you already granted across updates.

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
