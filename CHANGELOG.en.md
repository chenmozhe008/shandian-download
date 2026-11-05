# What's New

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
