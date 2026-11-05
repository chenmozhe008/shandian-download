// 界面文字：浏览器是中文就用原文，其他语言用英文。键是中文原文，和 App 的翻译表同一思路
const SHANDIAN_EN = {
  "闪电全能下载": "Shandian",
  "闪电全能下载：接管下载已关闭": "Shandian: download takeover is off",
  "关": "Off",
  "用闪电全能下载": "Download with Shandian",
  "用闪电全能下载这个视频": "Download this video with Shandian",
  "已关联到闪电全能下载": "Linked to Shandian",
  "关联失败": "Link failed",
  "PikPak 已关联到闪电全能下载，可以关掉这个页面了。": "PikPak is linked to Shandian. You can close this page.",
  "没能交给闪电全能下载，请确认闪电全能下载已打开。": "Couldn't reach Shandian. Please make sure it's open.",
  "视频": "Video",
  "音频": "Audio",
  "字幕": "Subtitles",
  "磁力": "Magnet",
  "电驴": "ed2k",
  "种子": "Torrent",
  "直链": "Direct link",
  "分片流": "Segmented stream",
  "本页侦测": "Detected on this page",
  "正在抓取…": "Scanning…",
  "已添加": "Added",
  "下载所选（{n}）": "Download Selected ({n})",
  "已交给闪电处理 {n} 个链接，请在应用中确认任务；取消后可重新选择。": "Sent {n} links to Shandian. Confirm them in the app; if you cancel, you can choose again.",
  "没能交给闪电，请确认应用已打开后重试。": "Couldn't reach Shandian. Make sure the app is open and retry.",
  "找到 {n} 个链接（最多显示 100 个）。": "Found {n} links (showing up to 100). ",
  "未能检查已有任务。": "Existing tasks couldn't be checked. ",
  "需要登录的直链可能无法下载。": "Links that require signing in may not download.",
  "当前页面没有找到磁力、种子或常见文件直链。": "No magnet, torrent or common file links found on this page.",
  "无法读取当前页面，请刷新后重试。": "Can't read this page. Refresh and retry.",
  "重新抓取下载链接": "Scan for Download Links Again",
  "下载视频号 ›": "Download WeChat Channels Video ›",
  "收起视频号下载": "Hide WeChat Channels Download",
  "请粘贴完整的视频号分享链接。": "Please paste the full WeChat Channels share link.",
  "正在解析…": "Detecting…",
  "已交给闪电全能下载，请在应用中查看进度。": "Sent to Shandian. Check progress in the app.",
  "请先登录腾讯元宝，再重新解析。": "Sign in to Tencent Yuanbao first, then detect again.",
  "解析或添加失败，请确认元宝已登录且闪电全能下载已打开。": "Detection or adding failed. Make sure you're signed in to Yuanbao and Shandian is open.",
  "解析失败，请重试。": "Detection failed. Please retry.",
  "解析并下载视频号": "Detect and Download",
  "PikPak 云盘": "PikPak",
  "关联到闪电全能下载": "Link to Shandian",
  "关联后磁力太慢时会自动交给 PikPak 离线下载。这个网页会退出登录（同一份登录不能两边同时用）。": "Once linked, slow magnet tasks go to PikPak automatically. This page will sign out, because one sign-in can't be used in both places.",
  "正在关联…": "Linking…",
  "已关联": "Linked",
  "没找到登录信息": "Sign-in not found",
  "请先在这个网页里登录 PikPak，再点一次。": "Sign in to PikPak on this page, then click again.",
  "请先打开包含视频的网页": "Open a page with a video first",
  "浏览器设置等页面无法提取视频。": "Videos can't be extracted from pages like browser settings.",
  "可尝试提取当前页面正在播放的视频。": "You can try extracting the video playing on this page.",
  "正在交给闪电全能下载…": "Sending to Shandian…",
  "已交给闪电全能下载": "Sent to Shandian",
  "未能添加视频": "Couldn't add the video",
  "在闪电全能下载里查看任务状态。": "Check the task status in Shandian.",
  "没有找到可直接下载的视频，请先播放视频后重试。部分网站不支持此方式。": "No directly downloadable video found. Play the video and retry. Some sites don't support this.",
  "没能交给闪电全能下载，请确认应用已打开后重试。": "Couldn't reach Shandian. Make sure the app is open and retry.",
  "停止侦测并清除结果": "Stop Detecting and Clear Results",
  "开始侦测本页播放资源": "Detect Media Playing on This Page",
  "已停止侦测并清除临时结果。": "Detection stopped and temporary results cleared.",
  "未授权网络资源侦测，其他下载功能仍可使用。": "Network detection wasn't allowed. Other download features still work.",
  "正在侦测当前标签页，播放视频后点“查找页面中的视频和字幕”。5 分钟后自动停止。": "Detecting on this tab. Play the video, then click “Find Videos and Subtitles”. Stops automatically after 5 minutes.",
  "侦测操作失败，请重新打开扩展后重试。": "Detection failed. Reopen the extension and retry.",
  "正在查找…": "Searching…",
  "找到 {n} 个媒体项目，点击添加。": "Found {n} media items. Click to add. ",
  "部分分片流需登录或受版权保护，可能无法下载。": "Some segmented streams require signing in or are copy-protected and may not download.",
  "没有找到视频或字幕；请先播放后重新查找。部分站点的媒体无法识别。": "No videos or subtitles found. Play the media and search again. Media on some sites can't be recognized.",
  "添加失败，点击重试": "Couldn't add. Click to retry",
  "读取页面失败，请刷新页面后重试。": "Couldn't read the page. Refresh and retry.",
  "重新查找": "Search Again",
  "粘贴微信视频号分享链接": "Paste a WeChat Channels share link",
  "链接将交给已登录的腾讯元宝解析，不读取聊天记录。": "The link is detected by your signed-in Tencent Yuanbao. Chat history isn't read.",
  "下载这个视频": "Download This Video",
  "仅在本页侦测媒体类型，5 分钟后自动停止；首次启用需要浏览器授权": "Detects media types on this page only and stops after 5 minutes. The first use needs browser permission",
  "浏览器会为扩展授予网站网络请求访问权限；侦测只处理当前标签页，5 分钟后停止并撤销授权。": "The browser grants the extension access to site network requests. Detection covers only this tab, then stops and revokes access after 5 minutes.",
  "查找页面中的视频和字幕": "Find Videos and Subtitles",
  "批量抓取页面下载链接": "Scan Page for Download Links",
  "全选": "Select All",
  "下载所选": "Download Selected",
  "接管浏览器下载": "Take Over Browser Downloads",
  "网页上点下载时交给闪电全能下载": "Send downloads you start on web pages to Shandian"
};

function T(text, values) {
  const language = globalThis.chrome?.i18n?.getUILanguage?.() || "zh";
  const template = language.toLowerCase().startsWith("zh") ? text : (SHANDIAN_EN[text] ?? text);
  return template.replace(/\{(\w+)\}/g, (_, key) => values?.[key]);
}

// popup.html 里写死的文字和提示
if (globalThis.document?.createTreeWalker) {
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  while (walker.nextNode()) {
    const node = walker.currentNode, text = node.nodeValue.trim();
    if (text) node.nodeValue = node.nodeValue.replace(text, T(text));
  }
  for (const element of document.querySelectorAll("[title], [placeholder]")) {
    if (element.title) element.title = T(element.title);
    if (element.placeholder) element.placeholder = T(element.placeholder);
  }
}
