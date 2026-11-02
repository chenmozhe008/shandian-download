// 点扩展图标弹出的小面板：视频网站页面上直接「下载这个视频」；下面是接管下载的开关
const VIDEO_SITES = ["douyin.com", "xiaohongshu.com", "bilibili.com", "youtube.com", "x.com", "twitter.com", "weibo.com", "ixigua.com", "kuaishou.com", "vimeo.com", "xinpianchang.com"];

async function main() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  let url;
  try { url = new URL(tab?.url || ""); } catch (_) {}
  const host = url?.hostname || "";
  const onVideoSite = VIDEO_SITES.some((d) => host === d || host.endsWith("." + d)) ||
    (host === "movie.douban.com" && /^\/trailer\/\d+\/?$/.test(url.pathname));
  const isWebPage = url && ["http:", "https:"].includes(url.protocol);
  const page = document.getElementById("page");
  if (host === "yuanbao.tencent.com") {
    const toggle = document.getElementById("wechat-toggle");
    const panel = document.getElementById("wechat");
    toggle.hidden = false;
    toggle.onclick = () => { panel.hidden = !panel.hidden; toggle.textContent = panel.hidden ? "下载视频号 ›" : "收起视频号下载"; };
    const wechatButton = document.getElementById("wechat-download");
    wechatButton.onclick = async () => {
      const share = document.getElementById("wechat-link").value.trim();
      let parsed; try { parsed = new URL(share); } catch (_) {}
      if (!parsed || parsed.protocol !== "https:" || parsed.hostname !== "weixin.qq.com" || !parsed.pathname.startsWith("/sph/")) {
        page.querySelector("#hint").textContent = "请粘贴完整的视频号分享链接。"; return;
      }
      wechatButton.disabled = true; wechatButton.textContent = "正在解析…";
      try {
        const r = await chrome.runtime.sendMessage({ type: "downloadWeChat", tabId: tab.id, share });
        page.querySelector("#hint").textContent = r?.ok ? "已交给闪电全能下载，请在应用中查看进度。" : r?.login ? "请先登录腾讯元宝，再重新解析。" : "解析或添加失败，请确认元宝已登录且闪电全能下载已打开。";
      } catch (_) { page.querySelector("#hint").textContent = "解析失败，请重试。"; }
      finally { wechatButton.disabled = false; wechatButton.textContent = "解析并下载视频号"; }
    };
  }
  if (host === "mypikpak.com") {
    // PikPak 网页上：把这里登录着的账号关联到闪电全能下载（云盘离线用）
    page.querySelector("#title").textContent = "PikPak 云盘";
    const btn = page.querySelector("#dl");
    btn.textContent = "关联到闪电全能下载";
    page.querySelector("#hint").textContent = "关联后磁力太慢时会自动交给 PikPak 离线下载。这个网页会退出登录（同一份登录不能两边同时用）。";
    btn.onclick = async () => {
      btn.disabled = true; btn.textContent = "正在关联…";
      const r = await chrome.runtime.sendMessage({ type: "linkPikPak", tabId: tab.id });
      btn.textContent = r?.ok ? "已关联" : "没找到登录信息";
      if (!r?.ok) page.querySelector("#hint").textContent = "请先在这个网页里登录 PikPak，再点一次。";
    };
  } else if (!isWebPage) {
    page.querySelector("#title").textContent = "请先打开包含视频的网页";
    page.querySelector("#dl").style.display = "none";
    page.querySelector("#hint").textContent = "浏览器设置等页面无法提取视频。";
  } else {
    page.querySelector("#title").textContent = tab.title || tab.url;
    if (!onVideoSite) page.querySelector("#hint").textContent = "可尝试提取当前页面正在播放的视频。";
    const btn = page.querySelector("#dl");
    btn.onclick = async () => {
      btn.disabled = true; btn.textContent = "正在交给闪电全能下载…";
      const r = await chrome.runtime.sendMessage({ type: "downloadPage", tabId: tab.id, requireVideo: !onVideoSite });
      btn.textContent = r?.ok ? "已交给闪电全能下载" : "未能添加视频";
      page.querySelector("#hint").textContent = r?.ok ? "在闪电全能下载里查看任务状态。" : r?.noVideo ? "没有找到可直接下载的视频，请先播放视频后重试。部分网站不支持此方式。" : "没能交给闪电全能下载，请确认应用已打开后重试。";
      if (!r?.ok) btn.disabled = false;
      if (r?.ok) setTimeout(() => window.close(), 1200);
    };
  }
  const scan = document.getElementById("scan");
  const capture = document.getElementById("capture");
  const results = document.getElementById("results");
  if (isWebPage && host !== "mypikpak.com") {
    let capturing = false;
    const setCaptureLabel = () => { capture.textContent = capturing ? "停止侦测并清除结果" : "开始侦测本页播放资源"; };
    try { capturing = !!(await chrome.runtime.sendMessage({ type: "captureStatus", tabId: tab.id }))?.active; } catch (_) {}
    setCaptureLabel();
    capture.onclick = async () => {
      capture.disabled = true;
      try {
        if (capturing) {
          const result = await chrome.runtime.sendMessage({ type: "stopCapture", tabId: tab.id });
          if (!result?.ok) throw new Error("停止失败");
          capturing = false;
          results.replaceChildren();
          page.querySelector("#hint").textContent = "已停止侦测并清除临时结果。";
        } else {
          // 浏览器只允许在用户点击时申请可选权限；不会在安装扩展时自动授予。
          const granted = await chrome.permissions.request({ permissions: ["webRequest"] });
          if (!granted) { page.querySelector("#hint").textContent = "未授权网络资源侦测，其他下载功能仍可使用。"; return; }
          const result = await chrome.runtime.sendMessage({ type: "startCapture", tabId: tab.id });
          if (!result?.ok) { await chrome.permissions.remove({ permissions: ["webRequest"] }); throw new Error("启动失败"); }
          capturing = true;
          page.querySelector("#hint").textContent = "正在侦测当前标签页，播放视频后点“查找页面中的视频和字幕”。5 分钟后自动停止。";
        }
      } catch (_) {
        if (!capturing) try { await chrome.permissions.remove({ permissions: ["webRequest"] }); } catch (_) {}
        page.querySelector("#hint").textContent = "侦测操作失败，请重新打开扩展后重试。";
      }
      finally { capture.disabled = false; setCaptureLabel(); }
    };
    scan.onclick = async () => {
      scan.disabled = true; scan.textContent = "正在查找…"; results.replaceChildren(); document.body.classList.remove("batch-mode"); document.getElementById("batch-actions").hidden = true;
      try {
        const response = await chrome.runtime.sendMessage({ type: "scanMedia", tabId: tab.id });
        const items = response?.items || [];
        page.querySelector("#hint").textContent = items.length ? `找到 ${items.length} 个媒体项目，点击添加。${items.some(item => ["m3u8", "mpd"].includes(item.ext)) ? "部分分片流需登录或受版权保护，可能无法下载。" : ""}` : "没有找到视频或字幕；请先播放后重新查找。部分站点的媒体无法识别。";
        for (const item of items) {
          const button = document.createElement("button"); button.className = "result";
          button.textContent = `${item.label} · ${item.ext.toUpperCase()}`;
          button.onclick = async () => {
            button.disabled = true;
            try {
              const response = await chrome.runtime.sendMessage({ type: "downloadMedia", tabId: tab.id, item });
              button.textContent = response?.ok ? "已交给闪电全能下载" : "添加失败，点击重试";
              if (!response?.ok) button.disabled = false;
            } catch (_) { button.textContent = "添加失败，点击重试"; button.disabled = false; }
          };
          results.appendChild(button);
        }
      } catch (_) { page.querySelector("#hint").textContent = "读取页面失败，请刷新页面后重试。"; }
      finally { scan.disabled = false; scan.textContent = "重新查找"; }
    };
  } else { scan.style.display = "none"; capture.style.display = "none"; document.getElementById("capture-note").style.display = "none"; }
  const box = document.getElementById("takeover");
  const { takeover = true } = await chrome.storage.local.get("takeover");
  box.checked = takeover;
  box.onchange = async () => {
    await chrome.storage.local.set({ takeover: box.checked });
    chrome.runtime.sendMessage({ type: "refreshBadge" });
  };
}
main();
