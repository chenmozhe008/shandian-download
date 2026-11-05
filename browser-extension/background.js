var T = globalThis.T || ((text, values) => text.replace(/\{(\w+)\}/g, (_, key) => values?.[key]));  // 测试环境没有 i18n.js
importScripts("i18n.js", "network-capture.js");

// 闪电全能下载浏览器扩展：把浏览器里的下载交给本机的闪电全能下载（127.0.0.1:6800）。
// App 没开时什么都不拦，浏览器照常下载；App 只认本扩展的来源（manifest 里的 key 固定了扩展 ID），不需要填密钥。

const APP = "http://127.0.0.1:6800/extension/add";
const PING = "http://127.0.0.1:6800/extension/ping";
const STATUS = "http://127.0.0.1:6800/extension/status";

// 告诉 App 扩展已装好（设置页显示「已连接」）。扩展版本不匹配时不能反复
// 调用 reload：浏览器可能加载的是另一份目录，重载也不会改变其版本。
async function ping() {
  try {
    await fetch(PING, { method: "POST" });
  } catch (_) {}
}
chrome.alarms.create("shandian-ping", { periodInMinutes: 10 });
chrome.alarms.onAlarm.addListener((a) => { if (a.name === "shandian-ping") ping(); });

async function enabled() {
  const { takeover = true } = await chrome.storage.local.get("takeover");
  return takeover;
}

async function refreshBadge() {
  const on = await enabled();
  await chrome.action.setBadgeText({ text: on ? "" : T("关") });
  await chrome.action.setBadgeBackgroundColor({ color: "#8E8E93" });
  await chrome.action.setTitle({ title: on ? T("闪电全能下载") : T("闪电全能下载：接管下载已关闭") });
}

// 网盘、登录后才能下的文件要带上浏览器的 Cookie、来源页和 UA，内核才能以同样身份下载
async function headersFor(url, referrer) {
  const headers = { "User-Agent": navigator.userAgent };
  if (referrer) headers["Referer"] = referrer;
  try {
    const cookies = await chrome.cookies.getAll({ url });
    if (cookies.length) headers["Cookie"] = cookies.map((c) => `${c.name}=${c.value}`).join("; ");
  } catch (_) {}
  return headers;
}

// 抖音、小红书要登录才能拿到视频：把浏览器里这两家的登录信息一起交给 App（只在用户右键这两家的页面时）
const LOGIN_SITES = ["douyin.com", "xiaohongshu.com", "xinpianchang.com"];

async function loginCookies(url) {
  const host = new URL(url).hostname;
  const site = LOGIN_SITES.find((d) => host === d || host.endsWith("." + d));
  if (!site) return undefined;
  const cookies = await chrome.cookies.getAll({ domain: site });
  return cookies.map((c) => ({ domain: c.domain, path: c.path, secure: c.secure, expires: c.expirationDate, name: c.name, value: c.value }));
}

// 页面里正在播的视频：浏览器已经拿到了真实地址，直接下它，不用再让下载工具去解析页面
// （10-28 抖音网页加了签名校验，yt-dlp 拿不到；「精选」这类信息流页面地址里也没有视频编号）
function pickPlayingVideo() {
  const area = (v) => {
    const r = v.getBoundingClientRect();
    const w = Math.max(0, Math.min(r.right, innerWidth) - Math.max(r.left, 0));
    const h = Math.max(0, Math.min(r.bottom, innerHeight) - Math.max(r.top, 0));
    return w * h;
  };
  // 抖音：douyin-hook.js 记下的页面接口返回里有无水印直链（播放器是 blob，下面的 <video> 找不到地址）
  if (/(^|\.)douyin\.com$/.test(location.hostname) && window.__shandianAweme) {
    const store = window.__shandianAweme;
    const q = new URLSearchParams(location.search);
    let id = location.pathname.match(/\/video\/(\d+)/)?.[1] || q.get("modal_id") || q.get("vid");
    if (!id || !store[id]) {
      // 信息流（精选、推荐）：地址里没编号，找正在播的视频所在卡片上的编号
      const playing = [...document.querySelectorAll("video")].find((v) => !v.paused && v.getBoundingClientRect().height > 0);
      id = playing?.closest("[data-e2e-vid]")?.getAttribute("data-e2e-vid") || id;
    }
    const hit = store[id];
    if (hit) return { src: hit.url, title: hit.title || document.title };
  }
  const vids = [...document.querySelectorAll("video")].sort((a, b) => (b.paused ? 0 : 1e9) + area(b) - ((a.paused ? 0 : 1e9) + area(a)));
  for (const v of vids) {
    if (area(v) === 0) continue;
    const srcs = [v.currentSrc, v.src, ...[...v.querySelectorAll("source")].map((s) => s.src)];
    const src = srcs.find((x) => x && /^https?:/.test(x));
    if (src) return { src, title: document.title };
  }
  // 小红书：播放器是 blob，但页面数据里有视频直链（不用再等下载工具读 19 秒网页）
  try {
    const st = window.__INITIAL_STATE__;
    const map = st?.note?.noteDetailMap;
    const id = location.pathname.split("/").pop();
    const note = (map && (map[id] || Object.values(map)[0]))?.note;
    const streams = note?.video?.media?.stream;
    const list = [...(streams?.h264 || []), ...(streams?.h265 || [])];
    const best = list.sort((a, b) => (b.height || 0) - (a.height || 0))[0];
    const url = best?.masterUrl || best?.backupUrls?.[0];
    if (url) return { src: url.replace(/^http:/, "https:"), title: note.title || document.title };
  } catch (_) {}
  return null;
}

async function pickInTab(tabId) {
  try {
    // MAIN：要读页面自己的变量（小红书的 __INITIAL_STATE__、抖音 douyin-hook.js 记下的数据）
    const [res] = await chrome.scripting.executeScript({ target: { tabId }, func: pickPlayingVideo, world: "MAIN" });
    return res?.result || null;
  } catch (_) {
    return null;
  }
}

// 用户主动扫描时只读取页面上已有的媒体标签，不监听浏览历史或后台网络。
function collectPageMedia() {
  const result = [];
  const add = (src, kind, label, mime = "") => {
    try {
      const url = new URL(src, location.href);
      if (!["http:", "https:"].includes(url.protocol)) return;
      const ext = url.pathname.match(/\.([a-z0-9]{2,5})$/i)?.[1]?.toLowerCase() || "";
      const allowed = kind === "字幕" ? ["vtt", "srt", "ass", "ssa", "ttml"] : ["mp4", "m4v", "webm", "mov", "mp3", "m4a", "aac", "ogg", "wav", "m3u8", "mpd"];
      // 无扩展名的二进制内容无法可靠识别，不猜测格式。
      if (!allowed.includes(ext)) return;
      result.push({ url: url.href, referrer: location.href, kind, label, ext, title: document.title });
    } catch (_) {}
  };
  for (const element of document.querySelectorAll("video, audio")) {
    const kind = element.tagName.toLowerCase() === "audio" ? "音频" : "视频";
    const size = element.videoHeight ? `${element.videoWidth}×${element.videoHeight}` : "";
    const label = [kind, size].filter(Boolean).join(" · ");
    add(element.currentSrc, kind, label);
    add(element.src, kind, label);
    for (const source of element.querySelectorAll("source")) add(source.src, kind, label, source.type);
    for (const track of element.querySelectorAll("track[src]")) {
      add(track.src, "字幕", ["字幕", track.label || track.srclang || ""].filter(Boolean).join(" · "));
    }
  }
  // 播放器可能通过 JS 动态加载清单并把 blob: 给 video；只在手动扫描时读取本页已有资源。
  for (const entry of performance.getEntriesByType("resource")) {
    try {
      const url = new URL(entry.name, location.href);
      if (/\.(m3u8|mpd)$/i.test(url.pathname)) add(url.href, "视频", "视频 · 分片流");
    } catch (_) {}
  }
  return result.slice(0, 100);
}

async function scanPageMedia(tabId) {
  try {
    let frames;
    try {
      frames = await chrome.scripting.executeScript({ target: { tabId, allFrames: true }, func: collectPageMedia });
    } catch (_) {
      // 部分嵌入页面可能禁止扩展访问，至少尝试主页面。
      frames = await chrome.scripting.executeScript({ target: { tabId }, func: collectPageMedia });
    }
    const unique = new Map();
    for (const frame of frames) for (const item of frame.result || []) {
      if (!unique.has(item.url)) unique.set(item.url, item);
    }
    return [...unique.values()].slice(0, 100);
  } catch (_) { return []; }
}

// 只在用户点击时读取当前页面的链接，不监听或遍历其他页面。
function collectDownloadLinks() {
  const extensions = new Set(["torrent", "zip", "rar", "7z", "gz", "tgz", "bz2", "xz", "dmg", "pkg", "exe", "msi", "iso", "img", "apk", "ipa", "mp4", "mkv", "avi", "mov", "m4v", "ts", "flv", "wmv", "webm", "mp3", "flac", "wav", "m4a", "aac"]);
  const found = [];
  for (const anchor of document.querySelectorAll("a[href]")) {
    const href = anchor.href;
    let kind;
    if (/^magnet:\?/i.test(href)) kind = "磁力";
    else if (/^ed2k:\/\//i.test(href)) kind = "电驴";
    else {
      try {
        const url = new URL(href, location.href);
        if (!["http:", "https:"].includes(url.protocol)) continue;
        const ext = url.pathname.match(/\.([a-z0-9]+)$/i)?.[1]?.toLowerCase();
        if (!extensions.has(ext)) continue;
        kind = ext === "torrent" ? "种子" : "直链";
      } catch (_) { continue; }
    }
    let title = (anchor.textContent || anchor.getAttribute("title") || "").replace(/\s+/g, " ").trim();
    if (!title && kind === "磁力") {
      try { title = new URL(href).searchParams.get("dn") || ""; } catch (_) {}
    }
    if (!title) {
      try { title = decodeURIComponent(new URL(href).pathname.split("/").pop()) || kind; } catch (_) { title = kind; }
    }
    found.push({ url: href, title: title.slice(0, 180), kind });
    if (found.length >= 500) break;
  }
  return found;
}

async function scanDownloadLinks(tabId) {
  let frames;
  try { frames = await chrome.scripting.executeScript({ target: { tabId, allFrames: true }, func: collectDownloadLinks }); }
  catch (_) { frames = await chrome.scripting.executeScript({ target: { tabId }, func: collectDownloadLinks }); }
  const unique = new Map();
  for (const frame of frames) for (const item of frame.result || []) {
    let key = item.url;
    if (item.kind === "磁力") {
      try { key = new URL(item.url).searchParams.getAll("xt").find(x => /^urn:btih:/i.test(x))?.toLowerCase() || key; } catch (_) {}
    }
    if (!unique.has(key)) unique.set(key, item);
  }
  return [...unique.values()].slice(0, 100);
}

async function extensionBatchRequest(path, data) {
  const response = await fetch("http://127.0.0.1:6800/extension/" + path, {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(data)
  });
  if (!response.ok) throw new Error("交接失败");
  return response.json();
}

// 刷新标签页并等它加载完，再多等一会儿让页面把视频数据请求回来
async function reloadAndWait(tabId, extraMs) {
  await new Promise((resolve) => {
    const done = (id, info) => { if (id === tabId && info.status === "complete") { chrome.tabs.onUpdated.removeListener(done); resolve(); } };
    chrome.tabs.onUpdated.addListener(done);
    chrome.tabs.reload(tabId);
    setTimeout(resolve, 20000);
  });
  await new Promise((r) => setTimeout(r, extraMs));
}

async function downloadVideoInTab(tab, requireVideo = false) {
  let picked = await pickInTab(tab.id);
  // 抖音页面如果是扩展装好 / 更新之前打开的，douyin-hook.js 没挂上、什么都没记：刷新一次再取（10-29 用户：点了没下成）
  if (!picked && /(^|\.)douyin\.com$/.test(new URL(tab.url).hostname)) {
    await reloadAndWait(tab.id, 4000);
    picked = await pickInTab(tab.id);
  }
  if (picked && /\.(m3u8|mpd)$/i.test(new URL(picked.src).pathname)) {
    return sendToApp(picked.src, tab.url, undefined, tab.id);
  }
  if (picked) {
    const title = (picked.title || "视频").replace(/\s*[-|]\s*(抖音|小红书|哔哩哔哩.*|YouTube)\s*$/, "").trim().slice(0, 80) || "视频";
    return sendToApp(picked.src, tab.url, title.replace(/[\\/:*?"<>|]/g, " ") + ".mp4", tab.id);
  }
  if (requireVideo) return null; // 普通网页找不到视频时，不把网页误当文件下载
  return sendToApp(videoPageURL(tab.url), tab.url, undefined, tab.id);   // 已支持的网站交给 yt-dlp
}

// 抖音网页版看视频时地址常是 ?modal_id=…，换成视频页地址下载工具才认
function videoPageURL(url) {
  const u = new URL(url);
  const id = u.searchParams.get("modal_id") || u.searchParams.get("vid");
  if (u.hostname.endsWith("douyin.com") && id) return `https://www.douyin.com/video/${id}`;
  return url;
}

async function appReady() {
  try {
    const r = await fetch(PING, { method: "POST" });
    return (await r.json()).ready !== false;
  } catch (_) {
    return false;
  }
}

// 用户主动点的（面板按钮、右键菜单）App 没开就把它打开：在当前标签页打开 shandian://open，
// 浏览器会问一次「要打开闪电全能下载吗」，勾「始终允许」后不再问；然后等内核就绪（最多 20 秒）
async function launchApp(tabId) {
  if (await appReady()) return true;
  try { await chrome.tabs.update(tabId, { url: "shandian://open" }); } catch (_) { return false; }
  for (let i = 0; i < 40; i++) {
    await new Promise((r) => setTimeout(r, 500));
    if (await appReady()) return true;
  }
  return false;
}

// tabId：传了就表示是用户主动点的，App 没开时自动打开它；浏览器自动接管的下载不传，App 没开就让浏览器自己下
async function sendToApp(url, referrer, name, tabId, mediaType) {
  const headers = url.startsWith("http") ? await headersFor(url, referrer) : {};
  const cookies = url.startsWith("http") ? await loginCookies(referrer || url) : undefined;
  const post = () => fetch(APP, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ url, headers, cookies, name, mediaType }),
  }).then((r) => r.ok);
  try {
    return await post();
  } catch (_) {
    // App 没开
    if (tabId === undefined || !(await launchApp(tabId))) return false;
    return post().catch(() => false);
  }
}

// 图片、网页、文本和已知很小的文件留给浏览器：多连接快不了，交出去反而多一个出错环节（参照 IDM 默认不接图片）
const SMALL = 5 * 1024 * 1024;
function worthTakingOver(item) {
  const mime = (item.mime || "").toLowerCase();
  if (/^(image|text)\//.test(mime) || mime === "application/xhtml+xml") return false;
  const size = item.totalBytes > 0 ? item.totalBytes : item.fileSize;
  return !(size > 0 && size < SMALL);
}

// 失败后交回浏览器重下的链接：浏览器再建下载时不再拦
const handedBack = new Set();

// 交给 App 后盯 5 分钟：App 那边失败了（会自己移除任务）就用浏览器重新下，不让这次下载丢掉
async function watchHandOver(url) {
  for (let i = 0; i < 60; i++) {
    await new Promise((r) => setTimeout(r, 5000));
    await chrome.runtime.getPlatformInfo(); // 调一次扩展接口，后台脚本不会因空闲 30 秒被回收
    let state;
    try {
      const r = await fetch(STATUS, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ url }) });
      state = (await r.json()).state;
    } catch (_) {
      return; // App 关了：不再管
    }
    if (state === "failed") {
      handedBack.add(url);
      chrome.downloads.download({ url });
      return;
    }
    if (state === "done" || state === "none") return;
  }
}

chrome.downloads.onCreated.addListener(async (item) => {
  const url = item.finalUrl || item.url;
  if (handedBack.delete(url) || handedBack.delete(item.url)) return;
  if (!/^https?:/i.test(url) || !worthTakingOver(item) || !(await enabled())) return;
  if (await sendToApp(url, item.referrer)) {
    // App 接下了才取消浏览器这边的，避免两边都没下
    try { await chrome.downloads.cancel(item.id); } catch (_) {}
    try { await chrome.downloads.erase({ id: item.id }); } catch (_) {}
    watchHandOver(url);
  }
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId === "shandian-page") {
    await downloadVideoInTab(tab);
    return;
  }
  // 在视频画面上右键：抖音、小红书的视频是 blob: 地址，抖音的 <video> 即使有地址也不是最高画质。
  // 这几家和所有 blob 视频都走「取页面里的视频」，不能交页面地址（yt-dlp 下不了抖音；10-29 用户右键视频没下成）
  const url = info.linkUrl || info.srcUrl;
  const pageSite = /(^|\.)(douyin|xiaohongshu|xinpianchang)\.com$/.test(new URL(info.pageUrl || tab.url).hostname);
  if (!url || url.startsWith("blob:") || (info.mediaType === "video" && pageSite)) {
    await downloadVideoInTab(tab);
    return;
  }
  await sendToApp(url, info.pageUrl, undefined, tab?.id);
});

// 弹出面板发来的请求
chrome.runtime.onMessage.addListener((msg, _sender, reply) => {
  if (msg.type === "downloadWeChat") {
    (async () => {
      const tab = await chrome.tabs.get(msg.tabId);
      if (new URL(tab.url).origin !== "https://yuanbao.tencent.com") return { ok: false };
      const result = await chrome.scripting.executeScript({ target: { tabId: tab.id }, world: "MAIN", args: [msg.share], func: async (share) => {
        const response = await fetch("/api/weixin/get_parse_result", { method: "POST", credentials: "include", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ type: "video_channel_url", url: share, scene: 1 }) });
        if (response.status === 401) return { login: true };
        if (!response.ok) return {};
        const result = await response.json();
        return { playable: result?.data?.playable_url, title: result?.data?.desc || "微信视频号" };
      }});
      const parsed = result?.[0]?.result;
      if (!parsed?.playable) return { ok: false, login: !!parsed?.login };
      return await extensionBatchRequest("wechat", { share: msg.share, playable: parsed.playable, title: parsed.title });
    })().then(reply).catch(() => reply({ ok: false }));
    return true;
  }
  if (msg.type === "downloadPage") {
    chrome.tabs.get(msg.tabId).then((tab) => downloadVideoInTab(tab, msg.requireVideo)).then((ok) => reply({ ok: ok === true, noVideo: ok === null })).catch(() => reply({ ok: false }));
    return true; // 异步回复
  }
  if (msg.type === "startCapture") {
    chrome.tabs.get(msg.tabId).then(tab => startNetworkCapture(tab.id, tab.url, tab.title)).then(ok => reply({ ok })).catch(() => reply({ ok: false }));
    return true;
  }
  if (msg.type === "stopCapture") {
    stopNetworkCapture().then(() => reply({ ok: true })).catch(() => reply({ ok: false }));
    return true;
  }
  if (msg.type === "captureStatus") {
    capturedForTab(msg.tabId).then(reply).catch(() => reply({ active: false, items: [] }));
    return true;
  }
  if (msg.type === "scanLinks") {
    scanDownloadLinks(msg.tabId).then(async items => {
      let known = [], checked = false;
      if (items.length) try { const result = await extensionBatchRequest("known", { urls: items.map(x => x.url) }); known = result.known || []; checked = true; } catch (_) {}
      reply({ items, known, checked });
    }).catch(() => reply({ error: true }));
    return true;
  }
  if (msg.type === "downloadLinks") {
    chrome.tabs.get(msg.tabId).then(async tab => {
      // 不用当前标签页打开自定义协议，避免覆盖用户正在查看的网页。
      if (!(await appReady())) return { ok: false };
      return extensionBatchRequest("batch", { urls: msg.urls, referrer: tab.url });
    }).then(reply).catch(() => reply({ ok: false }));
    return true;
  }
  if (msg.type === "scanMedia") {
    Promise.all([scanPageMedia(msg.tabId), capturedForTab(msg.tabId)]).then(([items, captured]) => {
      const seen = new Set(items.map(x => x.url));
      reply({ items: [...items, ...captured.items.filter(x => !seen.has(x.url))], captureActive: captured.active });
    }).catch(() => reply({ items: [] }));
    return true;
  }
  if (msg.type === "downloadMedia") {
    const item = msg.item;
    if (!item || !/^https?:/i.test(item.url) || !["视频", "音频", "字幕"].includes(item.kind) || !/^[a-z0-9]{2,5}$/.test(item.ext)) { reply({ ok: false }); return; }
    const clean = text => text.replace(/[\\/:*?"<>|]/g, " ").trim();
    const stem = clean(item.title || item.kind).slice(0, 70) || item.kind;
    const label = clean(item.label || item.kind).slice(0, 30);
    let hash = 2166136261;
    for (const char of item.url) hash = Math.imul(hash ^ char.charCodeAt(0), 16777619);
    const suffix = " - " + label + " - " + (hash >>> 0).toString(16).padStart(8, "0");
    const stream = item.stream === true || ["m3u8", "mpd"].includes(item.ext);
    const name = stream ? undefined : stem + suffix + "." + item.ext;
    sendToApp(item.url, item.referrer, name, msg.tabId, stream ? "stream" : undefined).then(ok => reply({ ok })).catch(() => reply({ ok: false }));
    return true;
  }
  if (msg.type === "refreshBadge") refreshBadge();
  if (msg.type === "linkPikPak") {
    linkPikPak(msg.tabId).then((ok) => reply({ ok }));
    return true;
  }
});

chrome.runtime.onInstalled.addListener(async () => {
  await chrome.contextMenus.removeAll();   // 更新扩展时旧菜单还在，先清掉再建
  chrome.contextMenus.create({ id: "shandian", title: T("用闪电全能下载"), contexts: ["link", "video", "audio"] });
  // 抖音、小红书页面上任何地方右键都有，不用找视频元素
  chrome.contextMenus.create({
    id: "shandian-page", title: T("用闪电全能下载这个视频"), contexts: ["page", "frame", "image"],
    documentUrlPatterns: ["*://*.douyin.com/*", "*://*.xiaohongshu.com/*", "*://*.xinpianchang.com/*"],
  });
  refreshBadge();
  ping();
});
chrome.runtime.onStartup.addListener(() => { refreshBadge(); ping(); });

// 把浏览器里已经登录着的 PikPak 关联到闪电全能下载（App 用 mypikpak.com/drive/#shandian-link 打开这个页面）。
// 取走网页存的登录信息后删掉网页那份：同一份登录两边都刷新会互相作废，所以之后网页端会变成未登录
function grabPikPakSession() {
  const k = Object.keys(localStorage).find((k) => k.startsWith("credentials_"));
  if (!k) return null;
  const c = JSON.parse(localStorage.getItem(k) || "{}");
  if (!c.access_token || !c.refresh_token) return null;
  return { key: k, c, device: localStorage.getItem("deviceid") || "" };
}

// PikPak 网页一打开会自己跳到 /drive/all，地址里的 #shandian-link 会丢：记住是哪个标签页
const linkTabs = new Set();

chrome.tabs.onUpdated.addListener(async (tabId, change, tab) => {
  if ((change.url || tab.url || "").includes("#shandian-link")) linkTabs.add(tabId);
  if (change.status !== "complete" || !linkTabs.has(tabId) || !tab.url || !tab.url.startsWith("https://mypikpak.com/")) return;
  if (await linkPikPak(tabId)) linkTabs.delete(tabId);
});

async function linkPikPak(tabId) {
  for (let i = 0; i < 10; i++) {   // 网页登录信息可能稍后才写好
    const [res] = await chrome.scripting.executeScript({ target: { tabId }, func: grabPikPakSession }).catch(() => [null]);
    const s = res?.result;
    if (s) {
      const ok = await fetch("http://127.0.0.1:6800/extension/pikpak", {
        method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(s),
      }).then(async (r) => r.ok && (await r.json()).ok === true).catch(() => false);
      if (ok) {
        await chrome.scripting.executeScript({ target: { tabId }, func: (key, access) => {
          const value = localStorage.getItem(key);
          try { if (value && JSON.parse(value).access_token === access) localStorage.removeItem(key); } catch (_) {}
        }, args: [s.key, s.c.access_token] }).catch(() => {});
      }
      await chrome.scripting.executeScript({ target: { tabId }, func: (title, message) => {   // 这段在网页里跑，用不了 T，文字由参数带进去
        document.title = title;
        alert(message);
      }, args: ok ? [T("已关联到闪电全能下载"), T("PikPak 已关联到闪电全能下载，可以关掉这个页面了。")] : [T("关联失败"), T("没能交给闪电全能下载，请确认闪电全能下载已打开。")] });
      return ok;
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
  return false;   // 没登录：留在页面上让用户登录，登录后再点一次关联
}
