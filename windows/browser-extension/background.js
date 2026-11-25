importScripts("network-capture.js", "site-download.js", "audio-preview.js", "qingque.js");

// 闪电全能下载浏览器扩展：把浏览器里的下载交给本机的闪电全能下载（127.0.0.1:6800）。
// App 没开时什么都不拦，浏览器照常下载；App 只认本扩展的来源（manifest 里的 key 固定了扩展 ID），不需要填密钥。

const APP = "http://127.0.0.1:6800/extension/add";
const PING = "http://127.0.0.1:6800/extension/ping";
const STATUS = "http://127.0.0.1:6800/extension/status";
const BATCH = "http://127.0.0.1:6800/extension/batch";

// 告诉 App 扩展已装好（设置页显示「已连接」）。扩展版本不匹配时不能反复
// 调用 reload：浏览器可能加载的是另一份目录，重载也不会改变其版本。
async function ping() {
  try {
    const manifest=chrome.runtime.getManifest?.() || {};
    const canLink=typeof accountLinkSubmit==="function";
    await fetch(PING, { method: "POST", headers:{"Content-Type":"application/json"}, body:JSON.stringify({
      extensionVersion:manifest.version || "", cloudAccountLink:canLink,
      cloudAccountOfficial:canLink,
      cloudResumeAccount:canLink && typeof startCloudRefreshInTab==="function",
      cloudAccountProviders:canLink && typeof CLOUD_ACCOUNT_LINK_SITES!=="undefined" ? CLOUD_ACCOUNT_LINK_SITES.map(site=>site.id) : [],
    }) });
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
  await chrome.action.setBadgeText({ text: on ? "" : "关" });
  await chrome.action.setBadgeBackgroundColor({ color: "#8E8E93" });
  await chrome.action.setTitle({ title: on ? "闪电全能下载" : "闪电全能下载：接管下载已关闭" });
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
// YouTube 现在对绝大多数出口 IP 都要「确认你不是机器人」，必须带登录 Cookie。
// 浏览器里登录着 YouTube，扩展下载时把 Cookie 一起交给 App，存成 cookies-youtube.txt，
// 之后 App 里的 YouTube 任务自动带上（和抖音、小红书同一套机制）。
const LOGIN_SITES = ["douyin.com", "xiaohongshu.com", "xinpianchang.com", "youtube.com", "youtu.be", "vimeo.com", "ixigua.com", "toutiao.com", "kuaishou.com", "weibo.com", "weibo.cn"];

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
  if (/(^|\.)xiaohongshu\.com$/.test(location.hostname)) {
    const state = window.__INITIAL_STATE__?.note;
    const id = location.pathname.match(/\/(?:explore|discovery\/item|user\/profile\/[^/]+)\/([\da-f]+)\/?$/i)?.[1]
      || state?.currentNoteId;
    const note = id && state?.noteDetailMap?.[id]?.note;
    const streams = note?.video?.media?.stream;
    const variants = [...(streams?.h264 || []), ...(streams?.h265 || [])]
      .filter((v) => {
        const codec = String(v.audioCodec || "").trim();
        return !/^(none|null|unknown|0)$/i.test(codec)
          && (Boolean(codec) || Number(v.audioChannels) > 0 || Number(v.audioBitrate) > 0);
      })
      .sort((a, b) => (Number(b.height) || 0) - (Number(a.height) || 0));
    for (const v of variants) {
      for (const candidate of [v.masterUrl, ...(v.backupUrls || [])]) {
        try {
          const u = new URL(candidate);
          if (!/^https?:$/.test(u.protocol) || /\.(m4s|cmfv|cmfa|ts)$/i.test(u.pathname)) continue;
          u.protocol = "https:";
          return { src: u.href, title: note.title || document.title, area: 1e12, xhsWithAudio: true };
        } catch (_) {}
      }
    }
    // The player's source can be a video-only fragment. Leave note-page fallback to the downloader.
    return null;
  }
  // 公众号自有视频的多画质 MP4 已在文章数据里，播放器尚未开始播放时也能读取。
  // 不把文章地址或 video_iframe 页面交给下载器；只接收页面公开给播放器的媒体地址。
  if (location.hostname === "mp.weixin.qq.com") {
    const infos = window.__videoPageInfos || window.cgiDataNew?.video_page_infos || [];
    const frameId = new URLSearchParams(location.search).get("vid");
    const ids = new Set(frameId ? [frameId] : [...document.querySelectorAll("iframe[data-mpvid], iframe[data-vid]")]
      .map((f) => f.getAttribute("data-mpvid") || f.getAttribute("data-vid")).filter(Boolean));
    const list = (Array.isArray(infos) ? infos : []).filter((v) => !ids.size || ids.has(v.video_id));
    const streams = list.flatMap((v) => v.mp_video_trans_info || []).filter((v) => {
      try {
        const u = new URL(String(v.url || "").replace(/\\x26/g, "&").replace(/&amp;/g, "&"));
        return /^https?:$/.test(u.protocol) && /(^|\.)qpic\.cn$/.test(u.hostname) && /\.mp4$/i.test(u.pathname);
      } catch (_) { return false; }
    }).sort((a, b) => (Number(b.width) * Number(b.height) || 0) - (Number(a.width) * Number(a.height) || 0)
      || (Number(b.filesize) || 0) - (Number(a.filesize) || 0));
    if (streams.length) {
      const seen = new Set();
      const variants = streams.map((v) => ({ width: Number(v.width) || 0, height: Number(v.height) || 0,
        src: v.url.replace(/\\x26/g, "&").replace(/&amp;/g, "&").replace(/^http:/, "https:") }))
        .filter((v) => { const key = `${v.width}x${v.height}`; if (seen.has(key)) return false; seen.add(key); return true; });
      return { src: variants[0].src, variants, title: window.cgiDataNew?.title || document.title, area: 1e12 };
    }
  }
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
    if (hit) return { src: hit.url, title: hit.title || document.title, area: 1e12 };
  }
  const vids = [...document.querySelectorAll("video")].sort((a, b) => (b.paused ? 0 : 1e9) + area(b) - ((a.paused ? 0 : 1e9) + area(a)));
  for (const v of vids) {
    if (area(v) === 0) continue;
    const srcs = [v.currentSrc, v.src, ...[...v.querySelectorAll("source")].map((s) => s.src)];
    const src = srcs.find((x) => x && /^https?:/.test(x));
    // area 带回后台：视频常在 iframe 里，多帧都找到时要挑画面最大的那个，别抓到广告位
    if (src) {
      const title = /(^|\.)pixabay\.com$/.test(location.hostname)
        ? (document.querySelector('meta[property="og:title"]')?.content || document.title).replace(/\.?\s*Free Stock Video.*$/i, "").replace(/\s*[-|]\s*Pixabay.*$/i, "").trim()
        : document.title;
      return { src, title, area: area(v) };
    }
  }
  return null;
}

async function pickInTab(tabId) {
  // 视频经常在 iframe 里（公众号图文的 video_iframe、第三方嵌入播放器），只扫顶层 frame 会什么都找不到。
  // MAIN：要读页面自己的变量（小红书的 __INITIAL_STATE__、抖音 douyin-hook.js 记下的数据）。
  // 个别 frame 拒绝注入时 allFrames 会整体失败，退回只扫顶层。
  const run = (target) => chrome.scripting.executeScript({ target, func: pickPlayingVideo, world: "MAIN" });
  let frames;
  try { frames = await run({ tabId, allFrames: true }); }
  catch (_) {
    try { frames = await run({ tabId }); } catch (_) { return null; }
  }
  return (frames || []).map((f) => f?.result).filter(Boolean).sort((a, b) => (b.area || 0) - (a.area || 0))[0] || null;
}

// 用户主动扫描时只读取页面上已有的媒体标签，不监听浏览历史或后台网络。
function collectPageMedia() {
  const result = [];
  const audioFormats = ["mp3", "m4a", "m4b", "aac", "ogg", "oga", "opus", "wav", "flac", "aif", "aiff", "webm"];
  const audioMime = { "audio/mpeg":"mp3", "audio/mp4":"m4a", "audio/aac":"aac", "audio/ogg":"ogg", "audio/opus":"opus",
    "audio/wav":"wav", "audio/x-wav":"wav", "audio/flac":"flac", "audio/webm":"webm", "audio/aiff":"aiff" };
  const add = (src, kind, label, mime = "") => {
    try {
      const url = new URL(src, location.href);
      if (!src || !["http:", "https:"].includes(url.protocol)) return;
      if (/(^|\.)feishu\.cn$/i.test(url.hostname) &&
          /^\/space\/api\/box\/stream\/download\/preview(?:\/|$)/i.test(url.pathname)) return;
      const fromPath = url.pathname.match(/\.([a-z0-9]{2,5})$/i)?.[1]?.toLowerCase() || "";
      const allowed = kind === "字幕" ? ["vtt", "srt", "ass", "ssa", "ttml"] : ["mp4", "m4v", "webm", "mov", ...audioFormats, "m3u8", "mpd"];
      const ext = allowed.includes(fromPath) ? fromPath : kind === "音频" ? audioMime[mime.split(";")[0].trim().toLowerCase()] || "" : "";
      // 已知是播放器的音频地址时可以交给浏览器响应确定格式，不捏造文件后缀。
      if (!ext && kind !== "音频") return;
      result.push({ url: url.href, referrer: location.href, kind, label, ext, title: document.title });
    } catch (_) {}
  };
  for (const element of document.querySelectorAll("video, audio")) {
    const kind = element.tagName.toLowerCase() === "audio" ? "音频" : "视频";
    const size = element.videoHeight ? `${element.videoWidth}×${element.videoHeight}` : "";
    const label = [kind, size].filter(Boolean).join(" · ");
    const sources = [...element.querySelectorAll("source")];
    const typeFor = src => sources.find(source => source.src === src)?.type || "";
    add(element.currentSrc, kind, label, typeFor(element.currentSrc));
    add(element.src, kind, label, typeFor(element.src));
    for (const source of sources) add(source.src, kind, label, source.type);
    for (const track of element.querySelectorAll("track[src]")) {
      add(track.src, "字幕", ["字幕", track.label || track.srclang || ""].filter(Boolean).join(" · "));
    }
  }
  // 播放器可能通过 JS 动态加载清单并把 blob: 给 video；只在手动扫描时读取本页已有资源。
  // 公众号图文这类嵌入播放器尤其常见：<video> 是 blob:，但真实 mp4 / m3u8 已经在性能条目里。
  for (const entry of performance.getEntriesByType("resource")) {
    try {
      const url = new URL(entry.name, location.href);
      if (entry.initiatorType === "audio" || /\.(mp3|m4a|m4b|aac|ogg|oga|opus|wav|flac|aif|aiff)$/i.test(url.pathname)) {
        add(url.href, "音频", "音频 · 页面资源");
        continue;
      }
      if (!/\.(m3u8|mpd|mp4|m4v|mov|webm)$/i.test(url.pathname)) continue;
      const size = entry.encodedBodySize || entry.transferSize || 0;
      // 太小的多半是广告/预览片段；跨域拿不到体积（0）时不做判断
      if (size > 0 && size < 256 * 1024) continue;
      add(url.href, "视频", "视频 · 页面资源");
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
      const existing = unique.get(item.url);
      if (!existing || (!existing.ext && item.ext)) unique.set(item.url, item);
    }
    return [...unique.values()].slice(0, 100);
  } catch (_) { return []; }
}

// 只在用户点击时读取当前页面的链接，不监听或遍历其他页面。
function collectDownloadLinks() {
  const extensions = new Set(["torrent", "zip", "rar", "7z", "gz", "tgz", "bz2", "xz", "dmg", "pkg", "exe", "msi", "iso", "img", "apk", "ipa", "mp4", "mkv", "avi", "mov", "m4v", "ts", "flv", "wmv", "webm", "mp3", "flac", "wav", "m4a", "aac", "tgz", "txz", "tbz", "tbz2", "zst", "deb", "rpm", "appimage", "snap", "flatpak", "flatpakref", "flatpakrepo", "run", "bin", "msix"]);
  const found = [];
  for (const anchor of document.querySelectorAll("a[href]")) {
    let href = anchor.href;
    let kind;
    if (/^magnet:\?/i.test(href)) kind = "磁力";
    else if (/^ed2k:\/\//i.test(href)) kind = "电驴";
    else {
      try {
        const url = new URL(href, location.href);
        if(url.hostname==="github.com" && /^\/[^/]+\/[^/]+\/blob\//.test(url.pathname)){url.pathname=url.pathname.replace("/blob/","/raw/");href=url.href;}
        if (!["http:", "https:"].includes(url.protocol)) continue;
        const ext = url.pathname.match(/\.([a-z0-9]+)$/i)?.[1]?.toLowerCase();
        const asset = (url.hostname==="github.com" && /^\/[^/]+\/[^/]+\/(releases\/(?:latest\/)?download\/|raw\/|archive\/)/.test(url.pathname)) || url.hostname==="raw.githubusercontent.com";
        if (!extensions.has(ext) && !asset && !anchor.hasAttribute("download")) continue;
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

async function submitCapturedMedia(tabId, urls) {
  const tab = await chrome.tabs.get(tabId);
  const captured = await capturedForTab(tabId);
  const page = new URL(tab.url);
  page.username = page.password = page.search = page.hash = "";
  const selected = new Set(Array.isArray(urls) ? urls : []);
  const items = captured.items.filter(item => !item.stream);
  const chosen = items.filter(item => selected.has(item.url));
  if (!chosen.length || chosen.length !== selected.size || tab.pendingUrl || chosen[0].referrer !== page.href) {
    return { ok: false, error: "侦测结果已失效，请重新播放要下载的素材。" };
  }
  const names = new Set();
  const links = await Promise.all(chosen.map(async item => {
    const title = item.trackTitle || item.filename || (item.kind + " " + (items.indexOf(item) + 1));
    const stem = title.replace(/[\\/:*?"<>|]/g, " ").trim().replace(/[. ]+$/, "").slice(0, 80) || item.kind;
    let name = stem + "." + item.ext;
    for (let number = 2; names.has(name.toLowerCase()); number++) name = stem + " (" + number + ")." + item.ext;
    names.add(name.toLowerCase());
    return { url: item.url, name, headers: await headersFor(item.url, item.referrer) };
  }));
  return extensionBatchRequest("batch", { links, referrer: chosen[0].referrer });
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

// 只有这两家要从页面里取视频地址（App 解析不了，见 CLAUDE.md）。其余视频站交页面地址，
// 由 App 用 yt-dlp 解析：<video> 上的地址只是 CDN 上的一段，拿去下载不完整，也认不出站点
// 豆瓣预告片：页面就是播放器，交网页地址只会下到网页本身；页面里 <source> 是 mp4 直链。
// 新片场要先过浏览器安全校验，下载工具抓网页会 403；微博信息流的页面地址不是视频。这两家也从页面里取
const PICK_FROM_PAGE = /(^|\.)(douyin\.com|xiaohongshu\.com|xinpianchang\.com|weibo\.com|weibo\.cn)$|^movie\.douban\.com$/;
// 只有这些站点的「页面地址」才是 yt-dlp 能吃的东西（和 App 的 LinkKind.videoHosts 对齐）。
// 以前是「不在 PICK_FROM_PAGE 就交页面地址」，结果公众号图文、随便一个带 <video> 的博客
// 都会被当成文件下载：内核下回来一个 HTML，文件名就是文章 id，用户不知道那是什么（11-14 用户实测）。
const VIDEO_PAGE_HOSTS = /(^|\.)(bilibili\.com|b23\.tv|youtube\.com|youtu\.be|douyin\.com|xiaohongshu\.com|xhslink\.com|kuaishou\.com|weibo\.com|weibo\.cn|ixigua\.com|vimeo\.com|x\.com|twitter\.com|xinpianchang\.com)$|^movie\.douban\.com$/;

async function downloadVideoInTab(tab, requireVideo = false, quality = "") {
  // 视频号预览页直接在后台解析，无需用户复制分享链接或切换页面。
  const sph = wechatShareOf(tab.url);
  if (sph) {
    const result = await downloadChannel(tab);
    if (!result.ok) throw new Error(result.error || "视频号下载失败，请重试。");
    return true;
  }
  const pageHost = new URL(tab.url).hostname;
  const knownSite = VIDEO_PAGE_HOSTS.test(pageHost);
  // requireVideo：用户明确要页面里的视频，不交页面地址。已知视频站也没必要先去页面里翻 <video>（那只是 CDN 的一段）
  if (!requireVideo && knownSite && !PICK_FROM_PAGE.test(pageHost)) {
    return sendToApp(videoPageURL(tab.url), tab.url, undefined, tab.id);
  }
  let picked = await pickInTab(tab.id);
  if (/(^|\.)xiaohongshu\.com$/.test(pageHost) && !picked?.xhsWithAudio) {
    const note = new URL(tab.url);
    const id = note.pathname.match(/\/(?:explore|discovery\/item|user\/profile\/[^/]+)\/([\da-f]+)\/?$/i)?.[1];
    if (!id) throw new Error("请打开这条小红书笔记，再下载带声音的视频。");
    note.hostname = "www.xiaohongshu.com";
    if (note.pathname.startsWith("/user/profile/")) note.pathname = `/explore/${id}`;
    return sendToApp(note.href, tab.url, undefined, tab.id);
  }
  if (!picked && pageHost === "mp.weixin.qq.com") {
    const channels = await articleChannelLinks(tab.id);
    if (channels.links.length === 1) {
      const result = await downloadChannel({ ...tab, url: channels.links[0] });
      if (!result.ok) throw new Error(result.error || "视频号解析失败，请重试。");
      return true;
    }
    if (channels.links.length > 1) throw new Error("文章包含多个视频号，请打开需要下载的视频号卡片后再点击闪电下载。");
    if (channels.embedded) throw new Error("检测到内嵌视频号，但文章未提供公开分享链接。请点击视频号卡片进入预览页，再点击闪电下载。");
  }

  if (quality && pageHost === "mp.weixin.qq.com") {
    const variant = picked?.variants?.find((v) => `${v.width}x${v.height}` === quality);
    if (!variant) throw new Error("所选分辨率已不可用，请重新打开扩展选择画质。");
    picked = { ...picked, src: variant.src };
  }
  // 抖音页面如果是扩展装好 / 更新之前打开的，douyin-hook.js 没挂上、什么都没记：刷新一次再取（10-29 用户：点了没下成）
  if (!picked && /(^|\.)douyin\.com$/.test(new URL(tab.url).hostname)) {
    await reloadAndWait(tab.id, 4000);
    picked = await pickInTab(tab.id);
  }
  if (picked && /\.(m3u8|mpd)$/i.test(new URL(picked.src).pathname)) {
    return sendToApp(picked.src, tab.url, undefined, tab.id, "stream");
  }
  if (picked) {
    const title = (picked.title || "视频").replace(/\s*[-|]\s*(抖音|小红书|哔哩哔哩.*|YouTube)\s*$/, "").trim().slice(0, 80) || "视频";
    return sendToApp(picked.src, tab.url, title.replace(/[\\/:*?"<>|]/g, " ") + ".mp4", tab.id);
  }
  // 页面变量和 <video> 都没给出地址时，再翻一遍所有 frame 里已有的媒体标签 / 分片清单。
  const clean = (s) => (s || "视频").replace(/[\\/:*?"<>|]/g, " ").trim().slice(0, 80) || "视频";
  const pageMedia = await scanPageMedia(tab.id);
  const best = pageMedia.find((x) => x.kind === "视频" && /^https?:/i.test(x.url));
  if (best) {
    const stream = /\.(m3u8|mpd)$/i.test(new URL(best.url).pathname);
    return sendToApp(best.url, best.referrer || tab.url, stream ? undefined : clean(best.title) + ".mp4", tab.id, stream ? "stream" : undefined);
  }
  // 用户已经为这个标签页授权过「开始侦测本页播放资源」并且播放过了：用它抓到的真实地址。
  const captured = await capturedForTab(tab.id);
  const cap = captured.items?.find((x) => x.kind === "视频" && /^https?:/i.test(x.url));
  if (cap) {
    const stream = cap.stream === true || ["m3u8", "mpd"].includes(cap.ext);
    return sendToApp(cap.url, cap.referrer || tab.url, stream ? undefined : clean(cap.title) + "." + cap.ext, tab.id, stream ? "stream" : undefined);
  }
  // 普通网页找不到真实媒体，就明确告诉用户没找到，绝不把网页地址当文件交出去。
  if (requireVideo || !knownSite) return null;
  return sendToApp(videoPageURL(tab.url), tab.url, undefined, tab.id);   // 已支持的网站交给 yt-dlp
}

function wechatShareOf(url) {
  try {
    const u = new URL(url);
    const id = u.searchParams.get("id");
    if (u.hostname === "channels.weixin.qq.com" && u.pathname.startsWith("/finder-preview/pages/sph") && id && /^[\w-]{4,64}$/.test(id)) return `https://weixin.qq.com/sph/${id}`;
  } catch (_) {}
  return null;
}

// 抖音网页版看视频时地址常是 ?modal_id=…，换成视频页地址下载工具才认
function videoPageURL(url) {
  const u = new URL(url);
  const id = u.searchParams.get("modal_id") || u.searchParams.get("vid");
  if (u.hostname.endsWith("douyin.com") && id) return `https://www.douyin.com/video/${id}`;
  return url;
}

async function videoCollectionAction(tab) {
  const url = new URL(tab.url);
  const host = url.hostname;
  const path = url.pathname.replace(/\/$/, "");
  const action = (target, { explicit = false, count = 0 } = {}) => ({
    url: target,
    explicit,
    count,
    label: count ? `下载合集（${count}）` : "下载合集",
  });
  const readPage = async (func, args = []) => {
    if (!Number.isInteger(tab.id)) return null;
    const [frame] = await chrome.scripting.executeScript({
      target: { tabId: tab.id }, world: "MAIN", func, args,
    }).catch(() => []);
    return frame?.result ?? null;
  };
  if (/(^|\.)youtube\.com$/.test(host) || host === "youtu.be") {
    const list = url.searchParams.get("list");
    const explicit = path === "/playlist" && host !== "youtu.be";
    if (!list || !/^[\w-]+$/.test(list) || (!explicit && (list.startsWith("RD") || !(path === "/watch" || host === "youtu.be")))) {
      return null;
    }
    const count = await readPage((playlistId, onPlaylistPage) => {
      if (new URL(location.href).searchParams.get("list") !== playlistId) return 0;
      const initial = window.ytInitialData;
      const panel = document.querySelector("ytd-playlist-panel-renderer");
      const visible = panel?.data;
      const watcher = initial?.contents?.twoColumnWatchNextResults?.playlist?.playlist;
      for (const playlist of [visible, watcher]) {
        if (playlist?.playlistId === playlistId) {
          const total = Number(playlist.totalVideos);
          if (Number.isSafeInteger(total) && total > 0) return total;
        }
      }
      // A scrolling playlist can load only part of its rows. Read the site's displayed total.
      if (visible?.playlistId === playlistId) {
        const index = panel.querySelector("#publisher-container .index-message, .index-message")?.textContent;
        const total = Number(index?.match(/\/\s*([0-9][0-9,. \u00a0\u202f]*)\s*$/)?.[1]?.replace(/[^\d]/g, ""));
        if (Number.isSafeInteger(total) && total > 0) return total;
      }
      if (onPlaylistPage) {
        const browse = document.querySelector('ytd-browse[page-subtype="playlist"]:not([hidden])')?.data;
        for (const data of [browse, initial]) {
          const header = data?.header?.playlistHeaderRenderer;
          if (header?.playlistId !== playlistId) continue;
          const label = header.numVideosText;
          const text = label?.simpleText || label?.runs?.map(run => run.text).join("") || "";
          const total = Number(text.match(/[0-9][0-9,. \u00a0\u202f]*/)?.[0]?.replace(/[^\d]/g, ""));
          if (Number.isSafeInteger(total) && total > 0) return total;
        }
      }
      return 0;
    }, [list, explicit]);
    return action(explicit ? tab.url : `https://www.youtube.com/playlist?list=${list}`, { explicit, count: count || 0 });
  }
  if (host === "space.bilibili.com" && /^\/\d+\/(?:lists\/\d+|channel\/(?:collectiondetail|seriesdetail))$/.test(path)) {
    return action(tab.url, { explicit: true });
  }
  if (!["www.bilibili.com", "bilibili.com", "m.bilibili.com"].includes(host)) return null;
  if (/^\/(?:list|medialist\/play)\/\w+$/.test(path)) {
    return action(tab.url, { explicit: !url.searchParams.get("bvid") });
  }
  if (!/^\/video\/(?:BV[\da-z]+|av\d+)$/i.test(path)) return null;
  const collection = await readPage(() => {
    const video = window.__INITIAL_STATE__?.videoData;
    const current = location.pathname.split("/").filter(Boolean).pop();
    if (!video || current !== video.bvid && current !== "av" + video.aid) return null;
    const season = video.ugc_season;
    const mid = video.owner?.mid;
    if (/^\d+$/.test(season?.id) && /^\d+$/.test(mid)) {
      const total = Number(season.ep_count);
      const episodes = season.sections?.flatMap(section => section.episodes || []).length || 0;
      const count = Number.isSafeInteger(total) && total > 0 ? total : episodes;
      return { url: `https://space.bilibili.com/${mid}/lists/${season.id}?type=season`, count };
    }
    const count = video.pages?.length || 0;
    return count > 1 ? { url: location.href, count } : null;
  });
  return collection ? action(collection.url, collection) : null;
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
async function sendToApp(url, referrer, name, tabId, mediaType, collection) {
  const baiduShare = mediaType === "baiduShare" || mediaType === "baiduShareLocked";
  const headers = !baiduShare && url.startsWith("http") ? await headersFor(url, referrer) : {};
  // 先看链接自己的站点：在别的网页上右键 Vimeo 链接时，来源页不是 Vimeo，也要带上它的登录信息
  let cookies;
  if (!baiduShare && url.startsWith("http")) {
    cookies = await loginCookies(url);
    if (!cookies && referrer) cookies = await loginCookies(referrer);
  }
  const post = () => fetch(APP, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ url, headers, cookies, name, page: referrer, mediaType, collection }),
  }).then(async (r) => r.ok && (await r.json().catch(() => null))?.ok !== false);
  try {
    return await post();
  } catch (_) {
    // App 没开
    if (tabId === undefined || !(await launchApp(tabId))) return false;
    return post().catch(() => false);
  }
}

// A fresh click replaces any old status watcher for that URL.
const handoverWatches = new Map();

// 交给 App 后盯 5 分钟：App 那边失败了（会自己移除任务）就用浏览器重新下，不让这次下载丢掉
async function watchHandOver(url) {
  const watch = Symbol();
  handoverWatches.set(url, watch);
  try {
    for (let i = 0; i < 60; i++) {
      await new Promise((r) => setTimeout(r, 5000));
      if (handoverWatches.get(url) !== watch) return;
      await chrome.runtime.getPlatformInfo(); // 调一次扩展接口，后台脚本不会因空闲 30 秒被回收
      const r = await fetch(STATUS, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ url }) });
      const { state } = await r.json();
      if (handoverWatches.get(url) !== watch) return;
      if (state === "failed") {
        await chrome.downloads.download({ url });
        return;
      }
      if (state === "done" || state === "none") return;
    }
  } catch (_) {
    // The app is unavailable or the browser rejected the fallback.
  } finally {
    if (handoverWatches.get(url) === watch) handoverWatches.delete(url);
  }
}

// Explicit per-site handoffs use the site's own authorized download flow, including small audio.
function isUserDownload(item){return !item.byExtensionId && !/^(chrome|edge)-extension:/i.test(item.referrer||"");}
async function restoreDownloadPage(intent,url){
  if(!intent)return;
  try{const tab=await chrome.tabs.get(intent.tabId);if(tab.url===url || tab.pendingUrl===url)await chrome.tabs.update(intent.tabId,{url:intent.page});}catch(_){}
}
const siteDownloadIntents = new Map();
// The content script owns the queue; only the background can hand off captured URLs.
const siteBatchRuns = new Map();
const siteBatchSubmissions = new Map();
const siteBatchStorageKey = tabId => 'pixabayBatch:'+tabId;
function siteBatchOwner(tabId){
  return [...siteBatchRuns].find(([,run])=>run.flow?.tabId===tabId)?.[0] ?? tabId;
}
function collectedSiteBatchLinks(tabId,run){
  const links=run.links ||= new Map();
  for(const intent of siteDownloadIntents.values()){
    if(intent.tabId===tabId && intent.batchId===run.id && intent.collected && !intent.cancelled &&
        run.keys.includes(intent.itemHref) && isSiteDownloadURL(intent.capturedURL)){
      links.set(intent.itemHref,{url:intent.capturedURL,name:siteDownloadFileName(intent.capturedURL,intent.name)});
    }
  }
  return links;
}
function saveSiteBatch(tabId,run){
  const snapshot={id:run.id,page:run.page,links:[...run.links],sent:[...run.sent]};
  run.saved=(run.saved || Promise.resolve()).catch(()=>{}).then(()=>{
    if(siteBatchRuns.get(tabId)===run)return chrome.storage.session.set({[siteBatchStorageKey(tabId)]:snapshot});
  });
  return run.saved;
}
function submitSiteBatchSelection(tabId,batchId,keys,automatic=false){
  if(!automatic && (!Array.isArray(keys) || !keys.length || keys.length>100 || new Set(keys).size!==keys.length)){
    return Promise.resolve({ok:false,error:'请选择已解析的项目'});
  }
  const previous=siteBatchSubmissions.get(tabId);
  if(previous && !automatic)return Promise.resolve({ok:false,error:'正在交给闪电，请稍候'});
  const operation=(previous || Promise.resolve()).catch(()=>{}).then(async()=>{
    const tab=await chrome.tabs.get(tabId);
    const run=siteBatchRuns.get(tabId);
    const storageKey=siteBatchStorageKey(tabId);
    const stored=run?null:(await chrome.storage.session.get(storageKey))[storageKey];
    const batch=run || stored;
    if(!batch || batch.id!==batchId || batch.page!==tab.url)return {ok:false,error:'这批地址已过期，请重新解析'};
    if(run?.browserDownload)return {ok:false,error:'这批已停止，请重新解析'};
    const available=run?collectedSiteBatchLinks(tabId,run):new Map(stored.links);
    const sent=run?run.sent:new Set(stored.sent);
    if(automatic){
      if(run?.stage!=='original' || !run.keys.every(key=>available.has(key)))return {ok:false,error:'有项目未取得地址，请选择已解析的项目'};
      keys=run.keys.filter(key=>!sent.has(key) && !run.unchecked?.has(key));
      if(!keys.length)return {ok:true,keys};
    }
    if(keys.some(key=>!available.has(key) || sent.has(key)))return {ok:false,error:'部分项目尚未解析或已经交给闪电'};
    const links=await Promise.all(keys.map(async key=>{
      const link=available.get(key);
      return {...link,headers:await headersFor(link.url,tab.url)};
    }));
    if(run && (siteBatchRuns.get(tabId)!==run || run.browserDownload))return {ok:false,error:'这批已停止，请重新解析'};
    if(!(await extensionBatchRequest('batch',{links,referrer:tab.url}))?.ok)return {ok:false,error:'未能交给闪电，请确认应用已打开'};
    for(const key of keys)sent.add(key);
    if(run?.stage==='review')await saveSiteBatch(tabId,run).catch(()=>{});
    if(stored)await chrome.storage.session.set({[storageKey]:{...stored,sent:[...sent]}}).catch(()=>{});
    await chrome.tabs.sendMessage(tabId,{type:'siteBatchDelivered',batchId,keys},{frameId:0}).catch(()=>{});
    return {ok:true,keys};
  });
  siteBatchSubmissions.set(tabId,operation);
  return operation.finally(()=>{
    if(siteBatchSubmissions.get(tabId)===operation)siteBatchSubmissions.delete(tabId);
  });
}
let siteFlowObserverAttached=false;
const siteFlowPage=value=>{try{const u=new URL(value);return u.origin+u.pathname.replace(/\/$/,'')}catch(_){return ''}};
function observeSiteFlow(details) {
  if(details.frameId<0)return;
  for(const [key,intent] of siteDownloadIntents){
    if(!intent.batchOnly || intent.collected || intent.cancelled || intent.until<Date.now() || intent.flowTabId!==details.tabId || !intent.flowUrl)continue;
    if(!intent.flowRequests)continue;
    const ownedDocument=intent.flowDocumentId && (details.documentId===intent.flowDocumentId || details.parentDocumentId===intent.flowDocumentId);
    if(!details.redirectUrl && details.requestId && (ownedDocument || (details.type==='main_frame' && siteFlowPage(details.url)===siteFlowPage(intent.flowUrl)) ||
      (details.type==='main_frame' && !details.documentId && intent.nativeStarted && details.initiator===new URL(intent.flowUrl).origin && details.timeStamp>=intent.nativeStartedAt))) {
      intent.flowRequests.add(details.requestId);
    }
    if(!ownedDocument && !intent.flowRequests.has(details.requestId))continue;
    const url=details.redirectUrl || details.url;
    if(!isSiteDownloadURL(url))continue;
    intent.collected=true;intent.capturedURL=url;
    console.info('Pixabay original URL captured before browser download');
    chrome.tabs.sendMessage(intent.tabId,{type:'siteDownloadResult',requestId:intent.requestId,batchId:intent.batchId,itemHref:intent.itemHref,ok:true,collected:true,url,name:siteDownloadFileName(url,intent.name)},{frameId:intent.ownerFrameId}).catch(()=>{});
    closeSiteFlow(intent).catch(()=>{});
    break;
  }
}
async function closeSiteFlow(intent) {
  const tabId=intent.flowTabId,ruleId=intent.flowRuleId;
  delete intent.flowTabId;delete intent.flowRuleId;
  if(intent.sharedFlow)return;
  if(tabId!=null)try{await chrome.tabs.remove(tabId)}catch(_){}
  if(ruleId){await chrome.declarativeNetRequest.updateSessionRules({removeRuleIds:[ruleId]}).catch(()=>{});await chrome.alarms.clear('shandian-site-flow:'+tabId+':'+ruleId);}
}
async function openSiteFlow(intent) {
  const run=siteBatchRuns.get(intent.tabId);
  if(!run || run.id!==intent.batchId || run.stage!=='original')throw Error('请先选择继续原站取链');
  if(run.flowClosed)throw Error('原站取链页已关闭');
  let flow=run.flow;
  try{
    if(!flow){
      const source=await chrome.tabs.get(intent.tabId);
      const tab=await chrome.tabs.create({windowId:source.windowId,url:'about:blank',active:false});
      flow=run.flow={tabId:tab.id};
      if(siteBatchRuns.get(intent.tabId)!==run || intent.cancelled)throw Error('取链已取消');
      const rules=await chrome.declarativeNetRequest.getSessionRules();
      const used=new Set(rules.map(r=>r.id));let id;
      do{id=1800000000+Math.floor(Math.random()*200000000)}while(used.has(id));
      if(run.flow!==flow || intent.cancelled)throw Error('取链已取消');
      // Add a stricter download policy; never remove the site's CSP or X-Frame-Options.
      await chrome.declarativeNetRequest.updateSessionRules({addRules:[{id,priority:10000,condition:{tabIds:[tab.id],regexFilter:'^https://(www\\.)?pixabay\\.com/(music|sound-effects|videos)/',resourceTypes:['main_frame']},action:{type:'modifyHeaders',responseHeaders:[{header:'Content-Security-Policy',operation:'append',value:'sandbox allow-scripts allow-same-origin allow-forms'}]}}]});
      if(run.flow!==flow){await chrome.declarativeNetRequest.updateSessionRules({removeRuleIds:[id]});throw Error('取链已取消');}
      flow.ruleId=id;
    }else{
      const [helper,source]=await Promise.all([chrome.tabs.get(flow.tabId),chrome.tabs.get(intent.tabId)]);
      if(helper.active && helper.windowId===source.windowId)await chrome.tabs.update(source.id,{active:true});
    }
    if(siteBatchRuns.get(intent.tabId)!==run || intent.cancelled)throw Error('取链已取消');
    intent.flowTabId=flow.tabId;intent.sharedFlow=true;intent.flowRequests=new Set();
    await chrome.alarms.create('shandian-site-flow:'+flow.tabId+':'+flow.ruleId,{delayInMinutes:2});
    // The host is sandboxed too, so even an immediate HTTP redirect to a file
    // starts from a document which is not permitted to create downloads.
    await chrome.tabs.update(flow.tabId,{url:chrome.runtime.getURL('pixabay-flow-host.html')+'#'+encodeURIComponent(intent.flowUrl)});
  }catch(error){await closeSiteBatchFlow(run);throw error;}
}
async function closeSiteBatchFlow(run) {
  const flow=run?.flow;
  if(!flow)return;
  delete run.flow;
  await closeSiteFlow({flowTabId:flow.tabId,flowRuleId:flow.ruleId});
}
chrome.alarms.onAlarm.addListener(alarm=>{
  if(!alarm.name.startsWith('shandian-site-flow:'))return;
  const [,tabText,ruleText]=alarm.name.split(':'),tabId=Number(tabText),ruleId=Number(ruleText);
  if(Number.isInteger(tabId)&&ruleId>=1800000000&&ruleId<2000000000)closeSiteFlow({flowTabId:tabId,flowRuleId:ruleId}).catch(()=>{});
});
function attachSiteFlowObserver() {
  if(siteFlowObserverAttached)return true;
  if(!chrome.webRequest?.onBeforeRequest || !chrome.webRequest?.onBeforeRedirect)return false;
  const filter={urls:['https://pixabay.com/*','https://www.pixabay.com/*','https://cdn.pixabay.com/*']};
  try{
    chrome.webRequest.onBeforeRequest.addListener(observeSiteFlow,filter);
    chrome.webRequest.onBeforeRedirect.addListener(observeSiteFlow,filter);
    siteFlowObserverAttached=true;return true;
  }catch(_){chrome.webRequest.onBeforeRequest.removeListener(observeSiteFlow);chrome.webRequest.onBeforeRedirect.removeListener(observeSiteFlow);return false;}
}
async function finishSiteBatchNetwork(tabId,review=false) {
  const run=siteBatchRuns.get(tabId);
  if(review && run){
    collectedSiteBatchLinks(tabId,run);
    run.stage='review';
    if(run.links.size)await saveSiteBatch(tabId,run).catch(()=>{});
  }else siteBatchRuns.delete(tabId);
  await closeSiteBatchFlow(run);
  for(const [key,intent] of siteDownloadIntents)if(intent.tabId===tabId && intent.batchOnly){await closeSiteFlow(intent);if(!review || !intent.collected)siteDownloadIntents.delete(key);}
  const lease='site-batch:'+tabId;
  if(![...networkPermissionLeases].some(key=>key.startsWith('site-batch:') && key!==lease) && siteFlowObserverAttached){
    chrome.webRequest?.onBeforeRequest?.removeListener(observeSiteFlow);
    chrome.webRequest?.onBeforeRedirect?.removeListener(observeSiteFlow);
    siteFlowObserverAttached=false;
  }
  if(networkPermissionLeases.has(lease))await releaseNetworkPermission(lease);
}
chrome.tabs.onRemoved.addListener(tabId=>{
  chrome.storage?.session?.remove(siteBatchStorageKey(tabId)).catch(()=>{});
  for(const run of siteBatchRuns.values())if(run.flow?.tabId===tabId){run.flowClosed=true;closeSiteBatchFlow(run).catch(()=>{});}
  for(const [key,intent] of siteDownloadIntents){
    if(intent.tabId===tabId){closeSiteFlow(intent).catch(()=>{});siteDownloadIntents.delete(key);}
    else if(intent.flowTabId===tabId){closeSiteFlow(intent).catch(()=>{});if(!intent.collected){intent.cancelled=true;chrome.tabs.sendMessage(intent.tabId,{type:'siteDownloadResult',requestId:intent.requestId,ok:false,error:'原站验证页已关闭'},{frameId:intent.ownerFrameId}).catch(()=>{});}}
  }
  if(siteBatchRuns.has(tabId) || networkPermissionLeases.has('site-batch:'+tabId))finishSiteBatchNetwork(tabId).catch(()=>{});
});
chrome.tabs.onUpdated?.addListener((tabId, change)=>{
  if(change.status==='loading')chrome.storage?.session?.remove(siteBatchStorageKey(tabId)).catch(()=>{});
  if(change.status==='loading' && siteBatchRuns.has(tabId))finishSiteBatchNetwork(tabId).catch(()=>{});
});

chrome.runtime.onMessage.addListener((msg, sender, reply) => {
  if(msg.type==='siteFlowDocumentReady'){
    const canonical=value=>{try{const u=new URL(value);return u.origin+u.pathname.replace(/\/$/,'')}catch(_){return ''}};
    const intent=[...siteDownloadIntents.values()].find(i=>i.flowTabId===sender.tab?.id && !i.nativeStarted && !i.cancelled && !i.collected && canonical(i.flowUrl)===canonical(sender.url || sender.tab?.url));
    if(!intent || sender.frameId!==0){reply({ok:false});return;}
    intent.nativeStarted=true;intent.flowDocumentId=sender.documentId;intent.nativeStartedAt=Date.now();
    chrome.scripting.executeScript({target:{tabId:intent.flowTabId},world:'MAIN',func:siteFlowNavigation}).then(()=>chrome.tabs.sendMessage(intent.flowTabId,{type:'runSiteFlowNative',itemHref:intent.flowUrl},{frameId:0})).then(result=>{
      if(!result?.ok && !intent.collected)chrome.tabs.sendMessage(intent.tabId,{type:'siteDownloadResult',requestId:intent.requestId,ok:false,error:result?.error || '未找到原站下载按钮'},{frameId:intent.ownerFrameId}).catch(()=>{});
      reply({ok:true});
    }).catch(error=>{chrome.tabs.sendMessage(intent.tabId,{type:'siteDownloadResult',requestId:intent.requestId,ok:false,error:'原站下载初始化失败：'+String(error?.message || '未知错误').replace(/https?:\/\/\S+/g,'[URL]')},{frameId:intent.ownerFrameId}).catch(()=>{});reply({ok:false});});return true;
  }
  if(msg.type==="siteBatchFinished") {
    const tabId=sender.tab?.id, run=siteBatchRuns.get(tabId);
    if(!run || run.id!==msg.batchId || sender.frameId!==0){reply({ok:false});return;}
    run.finished ||= (async()=>{
      let result;
      if(msg.autoSubmit===true && run.stage==='original'){
        result=await submitSiteBatchSelection(tabId,run.id,null,true).catch(()=>({ok:false,error:'未能自动交给闪电，请重试'}));
      }
      if(siteBatchRuns.get(tabId)!==run)return {ok:false};
      await finishSiteBatchNetwork(tabId,msg.review===true);
      return {ok:true,keys:result?.keys,autoError:result?.ok===false?result.error:undefined};
    })();
    run.finished.then(reply,()=>reply({ok:false}));return true;
  }
  if(msg.type==="siteBatchProgress") {
    if(sender.tab?.id!=null)chrome.runtime.sendMessage({...msg,type:"siteBatchProgressUI",tabId:sender.tab.id}).catch(()=>{});
    reply({ok:true});return;
  }
  if(msg.type==="releaseSiteIntent") {const intent=siteDownloadIntents.get(msg.requestId);if(intent?.tabId===sender.tab?.id){if(intent.batchOnly){intent.cancelled=true;closeSiteFlow(intent).catch(()=>{});}else intent.collectOnly=false;}reply({ok:true});return;}
  if (msg.type === "siteDownloadKeepAlive") {
    const intent=siteDownloadIntents.get(msg.requestId);
    reply({ok:!!intent && intent.tabId===sender.tab?.id && intent.until>Date.now()});return;
  }
  if (msg.type === "siteDownloadDirect") {
    const intent=siteDownloadIntents.get(msg.requestId);
    if(!intent || intent.tabId!==sender.tab?.id || !["pixabay.com","aigei.com"].includes(intent.host) || intent.until<Date.now() || !isSiteDownloadURL(msg.url)){reply({ok:false});return;}
    if(intent.cancelled){reply({ok:false,error:"批量已停止"});return;}
    const name=siteDownloadFileName(msg.url,intent.name);
    if(intent.collectOnly){intent.collected=true;intent.capturedURL=msg.url;reply({ok:true,collected:true,requestId:intent.requestId,batchId:intent.batchId,itemHref:intent.itemHref,url:msg.url,name});return;}
    siteDownloadIntents.delete(msg.requestId);
    sendToApp(msg.url,intent.page,name,intent.tabId).then(ok=>{
      if(!ok)chrome.downloads.download({url:msg.url,filename:name}).catch(()=>{});
      reply({ok});
    }).catch(()=>{chrome.downloads.download({url:msg.url,filename:name}).catch(()=>{});reply({ok:false});});
    return true;
  }
  if (msg.type !== "siteDownload") return;
  let page;
  try { page = new URL(sender.tab?.url || ""); } catch (_) { reply({ok:false}); return; }
  if (!/^(?:www\.)?(aigei|pixabay)\.com$/.test(page.hostname) || sender.tab?.id == null) { reply({ok:false}); return; }
  if (msg.collectOnly && /^(www\.)?aigei\.com$/.test(page.hostname)) {reply({ok:false,error:'爱给暂不支持批量下载，请逐项下载'});return;}
  const batch=siteBatchRuns.get(sender.tab.id);
  if(msg.collectOnly && (!batch || batch.id!==msg.batchId || !['direct','original'].includes(batch.stage) || !batch.keys.includes(msg.itemHref) || (batch.stage==='original' && !batch.deferred.has(msg.itemHref)))){reply({ok:false,error:'批量任务已过期，请重新选择下载'});return;}
  const now = Date.now();
  for (const [key, intent] of siteDownloadIntents) {
    const expired=intent.until<now && !(intent.collected && intent.batchId===siteBatchRuns.get(intent.tabId)?.id);
    const sameType=intent.batchOnly===!!msg.collectOnly;
    const replaces=intent.tabId===sender.tab.id && !intent.collected && sameType &&
      (!intent.batchOnly || intent.batchId!==batch.id || batch.stage==='original' || intent.itemHref===msg.itemHref || intent.cancelled);
    if(expired || replaces){closeSiteFlow(intent).catch(()=>{});siteDownloadIntents.delete(key);}
  }
  siteDownloadIntents.set(msg.requestId, {requestId:msg.requestId,batchId:msg.collectOnly?batch.id:null,itemHref:msg.itemHref,tabId:sender.tab.id,ownerFrameId:sender.frameId || 0,page:page.href,origin:page.origin,host:page.hostname.replace(/^www\./,""),name:String(msg.name||"").slice(0,180),collectOnly:!!msg.collectOnly,batchOnly:!!msg.collectOnly,until:now+120000});
  if(["pixabay.com","aigei.com"].includes(page.hostname.replace(/^www\./,""))) {
    prepareSiteDownload(sender.tab.id,msg.requestId,!!msg.collectOnly,msg.itemHref,batch?.stage==='original').then(async result=>{
      if(result?.needsOriginal && msg.collectOnly && batch?.stage==='direct')batch.deferred.add(msg.itemHref);
      if(result?.flowUrl){
        if(!(await chrome.permissions.contains({permissions:['webRequest']})) || !attachSiteFlowObserver())throw Error('需要临时下载地址观察权限，请从扩展弹窗发起批量下载');
        const intent=siteDownloadIntents.get(msg.requestId);
        if(!intent)throw Error('取链已取消');
        intent.flowUrl=result.flowUrl;
        await openSiteFlow(intent);
      }
      reply({ok:true,...result});
    }).catch(error=>{
      siteDownloadIntents.delete(msg.requestId);
      const detail=String(error?.message || error).replace(/(?:https?|chrome-extension):\/\/\S+/g,'[URL]').slice(0,240);
      console.warn('Site capture preparation failed:',detail);
      reply({ok:false,error:/[\u4e00-\u9fff]/.test(detail)?detail:'获取下载地址失败，请回素材页重试'});
    });
    return true;
  }
  reply({ok:true});
});
function takeSiteDownloadIntent(item) {
  const now=Date.now(); let ref,asset;
  try { ref=new URL(item.referrer); } catch (_) {}
  try { asset=new URL(item.finalUrl || item.url); } catch (_) {}
  for (const [key, intent] of siteDownloadIntents) {
    if(intent.batchOnly && !intent.sharedFlow)continue;
    if(intent.until < now){siteDownloadIntents.delete(key);continue;}
    const related = ref?.hostname.replace(/^www\./,"") === intent.host
      || (!ref && asset && (asset.hostname === intent.host || asset.hostname.endsWith("."+intent.host)));
    const media=/^(audio|video)\//i.test(item.mime||"") || /\.(mp3|wav|flac|m4a|aac|ogg|opus|mp4|webm|mov|mkv)(?:$|[?#])/i.test(asset?.pathname||item.filename||"");
    if(related && media && (!intent.collected || intent.capturedURL===asset?.href)){siteDownloadIntents.delete(key);return intent;}
  }
  return null;
}
function feishuAttachmentDownload(url) {
  try {
    const link = new URL(url);
    return link.protocol === "https:" && (link.hostname === "feishu.cn" || link.hostname.endsWith(".feishu.cn")) &&
      link.pathname.startsWith("/space/api/box/stream/download/");
  } catch (_) { return false; }
}

async function feishuDownloadName(item, url) {
  const tail = new URL(url).pathname.split("/").pop();
  for (let attempt = 0; attempt < 5; attempt++) {
    const name = (item.filename || "").split(/[\\/]/).pop();
    const stem = name?.replace(/\.[a-z0-9]{2,5}$/i, "");
    if (name && name !== tail && stem !== tail && !/\.crdownload$/i.test(name)) return name;
    if (attempt === 4) break;
    await new Promise(resolve => setTimeout(resolve, 120));
    const [current] = await chrome.downloads.search({ id: item.id }).catch(() => []);
    if (current) item = current;
  }
  return undefined;
}

chrome.downloads.onCreated.addListener(async (item) => {
  const url = item.finalUrl || item.url;
  if(isSiteDownloadURL(url))console.warn('Pixabay browser download item created');
  // A browser fallback carries its initiator on the item itself. The same URL
  // clicked again by the user is a separate request and can be taken over.
  if (item.byExtensionId && item.byExtensionId === chrome.runtime.id) return;
  if (!/^https?:/i.test(url)) return;
  const intent = takeSiteDownloadIntent(item);
  if (!intent && (!isUserDownload(item) || !(await enabled()))) return;
  const originalName=item.filename?.split(/[\\/]/).pop();
  const name = intent || isSiteDownloadURL(url) ? siteDownloadFileName(url,intent?.name,originalName) : undefined;
  if(intent?.collectOnly){
    // A browser download is never an acquisition path for an explicit batch.
    const run=siteBatchRuns.get(intent.tabId);
    if(run && run.id===intent.batchId)run.browserDownload=true;
    try{await chrome.downloads.cancel(item.id)}catch(_){}
    try{await chrome.downloads.erase({id:item.id})}catch(_){}
    chrome.tabs.sendMessage(intent.tabId,{type:'siteBatchBrowserTask'},{frameId:intent.ownerFrameId || 0}).catch(()=>{});
    chrome.tabs.sendMessage(intent.tabId,{type:"siteDownloadResult",requestId:intent.requestId,ok:false,error:"网页尝试启动浏览器下载，已停止本次批量"}).catch(()=>{});
    return;
  }
  handoverWatches.delete(url);
  let ok=false,paused=false;
  try {
    const resolvedName = name || (feishuAttachmentDownload(url) ? await feishuDownloadName(item, url) : undefined);
    if (!item.paused) {
      try { await chrome.downloads.pause(item.id); paused = true; }
      catch (_) {
        const [current] = await chrome.downloads.search({ id: item.id }).catch(() => []);
        if (current && current.state !== "in_progress") return;
      }
    }
    ok=await sendToApp(url, item.referrer || intent?.page, resolvedName, intent?.tabId);
  } catch (_) {} finally {
    if(paused && !ok)try{await chrome.downloads.resume(item.id);}catch(_){}
  }
  if (intent) chrome.tabs.sendMessage(intent.tabId, {type:"siteDownloadResult",requestId:intent.requestId,ok}).catch(()=>{});
  if (ok) {
    // Keep the browser download unless the App actually accepted it.
    try { await chrome.downloads.cancel(item.id); } catch (_) {}
    try { await chrome.downloads.erase({id:item.id}); } catch (_) {}
    if(intent){restoreDownloadPage(intent,url);setTimeout(()=>restoreDownloadPage(intent,url),150);}
    watchHandOver(url);
  }
});

function grabPage(tabId) {
  return chrome.scripting.executeScript({ target: { tabId, allFrames: true }, files: ["feedback.js", "grab.js"] });
}

// 抓到的链接交给 App。老版本的 App 没有这个接口，会回 404
async function sendGrabbed(links, tabId) {
  const post = () => fetch(BATCH, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ links, urls: links.map((l) => l.url) }), // urls 给还不认 links 的 App
  });
  let r;
  try {
    r = await post();
  } catch (_) {
    if (!(await launchApp(tabId))) return { ok: false };
    r = await post().catch(() => null);
  }
  return { ok: !!r?.ok, outdated: r?.status === 404 };
}

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId === "shandian-page") {
    await downloadVideoInTab(tab);
    return;
  }
  if (info.menuItemId === "shandian-grab") {
    await grabPage(tab.id);
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

// 网页上的悬停按钮。视频统一走 downloadVideoInTab，和右键入口一致
chrome.runtime.onMessage.addListener((msg, sender, reply) => {
  if (msg.type !== "hoverDownload" || !sender.tab) return;
  (async () => {
    if (typeof msg.qingqueName === "string" && msg.qingqueName &&
        new URL(sender.tab.url).hostname === "docs.qingque.cn") {
      const resolved = await readQingqueFiles(sender.tab, msg.qingqueName);
      if (resolved.error) return { ok: false, error: resolved.error };
      const file = resolved.links[0];
      return { ok: await sendToApp(file.url, sender.tab.url, file.name, sender.tab.id) };
    }
    const result = msg.video
      ? await downloadVideoInTab(sender.tab)
      : await sendToApp(msg.url, sender.url, undefined, sender.tab.id);
    // null = 没找到真实视频。以前一律回 ok，按钮显示「已交给闪电全能下载」但其实什么都没下
    return { ok: result === true, noVideo: result === null };
  })().then(reply, (error) => reply({ ok: false, error: /[\u4e00-\u9fff]/.test(error?.message || "") ? error.message : "未能添加下载，请稍后重试。" }));
  return true;
});

// 弹出面板发来的请求
chrome.runtime.onMessage.addListener((msg, _sender, reply) => {
  if (msg.type === "autoChannel") {
    // Only our loopback landing page may use this bridge; the App nonce remains single-use.
    if (!_sender.tab || _sender.id !== chrome.runtime.id || _sender.url?.split("#")[0] !== "http://127.0.0.1:6800/channel-open") {
      reply({ok:false,error:"自动解析入口无效"}); return;
    }
    if (msg.action === "extensions") {
      if (!/^[a-f0-9]{32}$/.test(msg.autoToken || "")) { reply({ok:false,error:"请求无效"}); return; }
      extensionBatchRequest("wechat-auto", {token:msg.autoToken,share:"shandian:extension-manager"}).then(async claim => {
        if (!claim?.ok) return {ok:false,error:"请求已过期，请重新点击打开扩展管理页"};
        const page = navigator.userAgent.includes("Edg/") ? "edge://extensions/" : "chrome://extensions/";
        const tabs = await chrome.tabs.query({windowId:_sender.tab.windowId});
        const existing = tabs.find(tab => tab.id !== _sender.tab.id && tab.url?.startsWith(page));
        if (existing) {
          await chrome.tabs.update(existing.id, {active:true});
          await chrome.tabs.remove(_sender.tab.id);
        } else {
          await chrome.tabs.update(_sender.tab.id, {url:page,active:true});
        }
        await chrome.windows.update(_sender.tab.windowId, {focused:true});
        return {ok:true};
      }).then(reply).catch(()=>reply({ok:false,error:"未能打开扩展管理页，请在地址栏打开"}));
      return true;
    }
    let share;
    try {
      const u = new URL(msg.share);
      const id = u.protocol === "https:" && u.hostname === "weixin.qq.com" && u.pathname.match(/^\/sph\/([\w-]{4,64})\/?$/)?.[1];
      if (!id || !/^[a-f0-9]{32}$/.test(msg.autoToken || "")) throw Error();
      share = `https://weixin.qq.com/sph/${id}`;
      const preview = `https://channels.weixin.qq.com/finder-preview/pages/sph?id=${id}`;
      extensionBatchRequest("wechat-auto", {token:msg.autoToken,share}).then(claim => claim?.ok
        ? downloadChannel({..._sender.tab,url:preview})
        : {ok:false,error:"自动解析请求已过期，请重新复制视频号链接"}).then(result => {
          reply(result);
          if (result?.ok) chrome.tabs.remove(_sender.tab.id).catch(()=>{});
        })
        .catch(()=>reply({ok:false,error:"自动解析失败，请重试"}));
    } catch (_) { reply({ok:false,error:"视频号分享链接无效"}); }
    return true;
  }
  if (msg.type === "downloadWeChat") {
    Promise.resolve().then(()=>{ const u=new URL(msg.share); const id=u.pathname.match(/^\/sph\/([\w-]{4,64})$/)?.[1]; if(u.hostname!=="weixin.qq.com" || !id) throw Error("invalid share"); return downloadChannel({id:msg.tabId,url:`https://channels.weixin.qq.com/finder-preview/pages/sph?id=${id}`},true); }).then(reply).catch(() => reply({ ok: false, error: "解析失败，请刷新腾讯元宝后重试。" }));
    return true;
  }
  if (msg.type === "scanFeishuMedia" || msg.type === "downloadFeishuMedia") {
    if (_sender.tab || _sender.url !== chrome.runtime.getURL("popup.html")) { reply({ok:false}); return; }
    (async () => {
      const tab = await chrome.tabs.get(msg.tabId);
      const url = new URL(tab.url);
      if (url.protocol !== "https:" || url.username || url.password || url.port ||
          !(url.hostname === "feishu.cn" || url.hostname.endsWith(".feishu.cn")) ||
          !/^\/(wiki|docx)\/[a-z0-9]+\/?$/i.test(url.pathname)) return {ok:false};
      url.search = ""; url.hash = "";
      if (msg.type === "scanFeishuMedia") return extensionBatchRequest("feishu-media", {url:url.href});
      return extensionBatchRequest("feishu-media/download", {url:url.href,ticket:msg.ticket,selected:msg.selected});
    })().then(reply, () => reply({ok:false,unavailable:true}));
    return true;
  }
  if (msg.type === "downloadFeishuDocument") {
    (async () => {
      const tab = await chrome.tabs.get(msg.tabId);
      const url = new URL(tab.url);
      if (url.protocol !== "https:" || url.username || url.password || url.port ||
          !(url.hostname === "feishu.cn" || url.hostname.endsWith(".feishu.cn")) ||
          !/^\/(wiki|docx)\/[a-z0-9]+\/?$/i.test(url.pathname)) return { ok: false };
      url.search = "";
      url.hash = "";
      return { ok: await sendToApp(url.href, "", "", tab.id, "feishuDocument") };
    })().then(reply, () => reply({ ok: false }));
    return true;
  }

  if (msg.type === "downloadPage" && msg.autoChannel && _sender.tab) {
    extensionBatchRequest("wechat-auto",{token:msg.autoToken,share:wechatShareOf(_sender.tab.url)}).then(claim=>claim?.ok ? downloadChannel(_sender.tab) : {ok:false,error:"自动解析请求已过期，请点击闪电下载"}).then(reply).catch(()=>reply({ok:false,error:"自动解析失败，请点击扩展重试"}));
    return true;
  }
  if (msg.type === "downloadPage") {
    chrome.tabs.get(msg.tabId).then(async (tab) => {
      if (wechatShareOf(tab.url)) return downloadChannel(tab, !!msg.forceWake);
      const ok = await downloadVideoInTab(tab, msg.requireVideo, msg.quality || "");
      return { ok: ok === true, noVideo: ok === null };
    }).then(reply).catch((error) => reply({ ok: false, error: /[\u4e00-\u9fff]/.test(error?.message || "") ? error.message : "未能添加视频，请刷新页面后重试。" }));
    return true; // 异步回复
  }
  if (msg.type === "videoCollectionAction" || msg.type === "downloadCollection") {
    if (_sender.tab && _sender.frameId !== 0) { reply(null); return; }
    chrome.tabs.get(_sender.tab?.id ?? msg.tabId).then(async (tab) => {
      if (msg.page && tab.url !== msg.page) return null;
      const action = await videoCollectionAction(tab);
      if (msg.type === "videoCollectionAction") return action && { ...action, currentLabel: "下载当前视频" };
      const ok = !!action && await sendToApp(action.url, tab.url, undefined, tab.id, undefined, true);
      return { ok };
    }).then(reply, () => reply(null));
    return true;
  }
  if (msg.type === "wechatFormats") {
    chrome.tabs.get(msg.tabId).then(async (tab) => {
      if (new URL(tab.url).hostname !== "mp.weixin.qq.com") return [];
      const picked = await pickInTab(tab.id);
      return (picked?.variants || []).filter((v) => v.width > 0 && v.height > 0)
        .map((v) => ({ value: `${v.width}x${v.height}`, label: `${v.height}p（${v.width}×${v.height}）` }));
    }).then((formats) => reply({ formats })).catch(() => reply({ formats: [] }));
    return true;
  }
  if (msg.type === "grabPage") {
    grabPage(msg.tabId).then(() => reply({ ok: true }), () => reply({ ok: false }));
    return true;
  }
  if (msg.type === "grabFrame" && _sender.tab) {
    chrome.tabs.sendMessage(_sender.tab.id, { type: "grabMerge", links: msg.links }, { frameId: 0 }).catch(() => {});
  }
  if (msg.type === "grabSend") {
    sendGrabbed(msg.links, _sender.tab?.id).then(reply);
    return true;
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
  if (msg.type === "aigeiPlayback") {
    handleAigeiPlayback(msg, _sender).then(() => reply({ ok: true }), () => reply({ ok: false }));
    return true;
  }
  if (msg.type === "capturedAudioDuration") {
    if (_sender.tab || _sender.url !== chrome.runtime.getURL("popup.html")) { reply({ duration: 0 }); return; }
    capturedAudioDuration(msg.tabId, msg.url).then(duration => reply({ duration }), () => reply({ duration: 0 }));
    return true;
  }
  if (msg.type === "downloadCapturedMedia") {
    if (_sender.tab || _sender.url !== chrome.runtime.getURL("popup.html")) { reply({ ok: false }); return; }
    submitCapturedMedia(msg.tabId, msg.urls).then(reply).catch(() => reply({ ok: false }));
    return true;
  }
  if (msg.type === "scanLinks") {
    chrome.tabs.get(msg.tabId).then(async tab=>{
      const host=new URL(tab.url).hostname;
      if(/^(www\.)?aigei\.com$/.test(host)){reply({items:[],known:[],checked:true,disabledReason:'爱给暂不支持批量下载，请逐项下载'});return;}
      if (host === "docs.qingque.cn") {
        reply(await readQingqueFiles(tab));
        return;
      }
      let items,batch=null;
      if(/^(www\.)?(aigei|pixabay)\.com$/.test(host)) {
        const result=await chrome.tabs.sendMessage(tab.id,{type:"collectSiteCatalog"},{frameId:0});items=result?.items || [];batch=result?.batch || null;
      } else items=await scanDownloadLinks(msg.tabId);
      let known=[],checked=false;
      const files=items.filter(item=>!item.resolve);
      if(files.length)try{const result=await extensionBatchRequest("known",{urls:files.map(x=>x.url)});known=result.known||[];checked=true;}catch(_){}
      reply({items,known,checked,batch});
    }).catch(()=>reply({error:true}));return true;
  }
  if(['getSiteBatchStatus','focusSiteFlow','cancelSiteBatch','setSiteBatchSelection'].includes(msg.type)){
    if(_sender.tab || _sender.url!==chrome.runtime.getURL('popup.html')){reply({ok:false});return;}
    const owner=siteBatchOwner(msg.tabId);
    if(msg.type==='focusSiteFlow'){
      const intent=[...siteDownloadIntents.values()].find(i=>i.tabId===owner && i.batchId===msg.batchId && !i.collected && !i.cancelled && i.flowTabId!=null);
      if(!intent){reply({ok:false,error:'当前没有待查看的原站页面'});return;}
      chrome.tabs.update(intent.flowTabId,{active:true}).then(()=>reply({ok:true}),()=>reply({ok:false}));return true;
    }
    if(msg.type==='cancelSiteBatch' && siteBatchRuns.get(owner)?.id!==msg.batchId){reply({ok:false});return;}
    if(msg.type==='setSiteBatchSelection'){
      const run=siteBatchRuns.get(owner);
      if(run && run.id===msg.batchId){
        const unchecked=new Set(Array.isArray(msg.unchecked)?msg.unchecked:[]);
        run.unchecked=new Set(run.keys.filter(key=>unchecked.has(key)));
      }
    }
    chrome.tabs.sendMessage(owner,{type:msg.type,batchId:msg.batchId,unchecked:msg.unchecked},{frameId:0})
      .then(result=>reply({...result,tabId:owner}),()=>reply({ok:false}));return true;
  }
  if (msg.type === "downloadLinks") {
    chrome.tabs.get(msg.tabId).then(async tab=>{
      if(/^(www\.)?aigei\.com$/.test(new URL(tab.url).hostname))return {ok:false,error:'爱给暂不支持批量下载，请逐项下载'};
      if(!(await appReady()))return {ok:false};
      const items=msg.items || (msg.urls||[]).map(url=>({url}));
      if (new URL(tab.url).hostname === "docs.qingque.cn") {
        if (_sender.tab || _sender.url !== chrome.runtime.getURL("popup.html")) return { ok: false };
        const result = await readQingqueFiles(tab, items.map(item => item.url));
        if (result.error) return { ok: false, error: result.error };
        const links = await Promise.all(result.links.map(async file => ({
          ...file, headers: await headersFor(file.url, tab.url),
        })));
        const title = String(tab.title || "").replace(/\s*[-–—|]\s*轻雀文档\s*$/, "").trim();
        const folderName = title && title !== "轻雀文档" ? title : "轻雀文档附件";
        return extensionBatchRequest("batch", { links, referrer: tab.url, folderName });
      }
      if(items.some(item=>item.resolve)){
        if(_sender.tab || _sender.url!==chrome.runtime.getURL('popup.html'))return {ok:false};
        if(!(await chrome.permissions.contains({permissions:['webRequest']})))return {ok:false,error:'未授权获取 Pixabay 原站下载地址'};
        const existing=siteBatchRuns.get(tab.id);
        if(siteBatchSubmissions.has(tab.id) || (existing && existing.stage!=='review'))return {ok:false,error:'当前页正在批量处理'};
        if(existing)await finishSiteBatchNetwork(tab.id);
        try{
          const selected=[...new Map(items.filter(item=>item.resolve).map(item=>[item.url,{url:item.url,title:item.title,kind:item.kind}])).values()].slice(0,100);
          const keys=selected.map(item=>item.url);
          const run={id:crypto.randomUUID(),keys,page:tab.url,stage:'original',deferred:new Set(keys),sent:new Set()};
          siteBatchRuns.set(tab.id,run);
          retainNetworkPermission('site-batch:'+tab.id);
          await chrome.storage.session.remove(siteBatchStorageKey(tab.id));
          const result=await chrome.tabs.sendMessage(tab.id,{type:"runSiteBatch",batchId:run.id,items:selected,format:msg.format},{frameId:0});
          if(!result?.ok)await finishSiteBatchNetwork(tab.id);
          return result;
        }catch(error){await finishSiteBatchNetwork(tab.id);throw error;}
      }
      return extensionBatchRequest("batch",{links:items.map(item=>({url:item.url,name:item.title||""})),referrer:tab.url});
    }).then(reply).catch(()=>reply({ok:false}));return true;
  }
  if(msg.type==='downloadCollectedSiteBatch'){
    if(_sender.tab || _sender.url!==chrome.runtime.getURL('popup.html')){reply({ok:false});return;}
    submitSiteBatchSelection(msg.tabId,msg.batchId,msg.keys).then(reply,()=>reply({ok:false,error:'未能交给闪电，请重试'}));return true;
  }
  if (msg.type === "scanMedia") {
    Promise.all([scanPageMedia(msg.tabId), capturedForTab(msg.tabId)]).then(([items, captured]) => {
      const merged = new Map(items.map(item => [item.url, item]));
      for (const item of captured.items) if (!merged.get(item.url)?.ext) merged.set(item.url, item);
      reply({ items: [...merged.values()], captureActive: captured.active });
    }).catch(() => reply({ items: [] }));
    return true;
  }
  if (msg.type === "downloadMedia") {
    const item = msg.item;
    if (!item || !/^https?:/i.test(item.url) || !["视频", "音频", "字幕"].includes(item.kind) ||
      !(typeof item.ext === "string" && (/^[a-z0-9]{2,5}$/.test(item.ext) || (item.kind === "音频" && !item.ext)))) { reply({ ok: false }); return; }
    const clean = text => text.replace(/[\\/:*?"<>|]/g, " ").trim();
    const stem = clean(item.title || item.kind).slice(0, 70) || item.kind;
    const label = clean(item.label || item.kind).slice(0, 30);
    let hash = 2166136261;
    for (const char of item.url) hash = Math.imul(hash ^ char.charCodeAt(0), 16777619);
    const suffix = " - " + label + " - " + (hash >>> 0).toString(16).padStart(8, "0");
    const stream = item.stream === true || ["m3u8", "mpd"].includes(item.ext);
    const name = stream || !item.ext ? undefined : stem + suffix + "." + item.ext;
    sendToApp(item.url, item.referrer, name, msg.tabId, stream ? "stream" : undefined).then(ok => reply({ ok })).catch(() => reply({ ok: false }));
    return true;
  }
  if (msg.type === "refreshBadge") refreshBadge();
  if (msg.type === "linkPikPak") {
    const tabId = msg.tabId ?? _sender.tab?.id;
    linkPikPak(tabId).then((ok) => {
      if (ok) chrome.storage.session.remove("pikpakLink" + tabId);
      reply({ ok });
    });
    return true;
  }
});

chrome.runtime.onInstalled.addListener(async () => {
  await chrome.contextMenus.removeAll();   // 更新扩展时旧菜单还在，先清掉再建
  chrome.contextMenus.create({ id: "shandian", title: "用闪电全能下载", contexts: ["link", "video", "audio"] });
  chrome.contextMenus.create({ id: "shandian-grab", title: "抓取磁力、种子和电驴", contexts: ["page", "frame", "selection"] });
  // 抖音、小红书页面上任何地方右键都有，不用找视频元素
  chrome.contextMenus.create({
    id: "shandian-page", title: "用闪电全能下载这个视频", contexts: ["page", "frame", "image"],
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
// 后台脚本闲置会被回收，所以标签页记在 storage.session。登录常要一两分钟，Google 弹窗回来页面也不一定刷新，
// 所以在页面里等登录信息出现，再通知这边取走
function watchPikPakLogin() {
  if (window.__shandianLink) return;
  window.__shandianLink = true;
  const timer = setInterval(() => {
    const k = Object.keys(localStorage).find((k) => k.startsWith("credentials_"));
    let c = {};
    try { c = JSON.parse(localStorage.getItem(k) || "{}"); } catch (_) {}
    if (!c.access_token || !c.refresh_token) return;
    clearInterval(timer);
    chrome.runtime.sendMessage({ type: "linkPikPak" }).catch(() => {});
  }, 1000);
}

chrome.tabs.onUpdated.addListener(async (tabId, change, tab) => {
  const key = "pikpakLink" + tabId;
  if (typeof accountLinkBlocksLegacyPikPak==="function" && await accountLinkBlocksLegacyPikPak({...tab,id:tabId,url:change.url || tab.url})) return;
  if ((change.url || tab.url || "").includes("#shandian-link")) await chrome.storage.session.set({ [key]: true });
  if (change.status !== "complete" || !tab.url || !tab.url.startsWith("https://mypikpak.com/")) return;
  if (!(await chrome.storage.session.get(key))[key]) return;
  chrome.scripting.executeScript({ target: { tabId }, func: watchPikPakLogin }).catch(() => {});
});

async function linkPikPak(tabId) {
  const tab=await chrome.tabs.get(tabId).catch(()=>null);
  if (!tab?.url?.startsWith("https://mypikpak.com/"))return false;
  if (typeof accountLinkBlocksLegacyPikPak==="function" && await accountLinkBlocksLegacyPikPak(tab))return false;
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
      await chrome.scripting.executeScript({ target: { tabId }, func: (ok) => {
        document.title = ok ? "已关联到闪电全能下载" : "关联失败";
        alert(ok ? "PikPak 已关联到闪电全能下载，可以关掉这个页面了。" : "没能交给闪电全能下载，请确认闪电全能下载已打开。");
      }, args: [ok] });
      return ok;
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
  return false;   // 没登录：留在页面上让用户登录，登录后再点一次关联
}

// 百度网盘直接下载（用到上面的 APP、launchApp）
importScripts("baidu.js");
// 夸克 / UC 网盘直接下载
importScripts("quark.js");
importScripts("ali.js");
importScripts("cloud-sites.js", "extra-cloud.js");
importScripts("cloud-account-link.js");
// Report account-link support immediately after an unpacked-extension reload.
ping();
importScripts("linkswift-settings.js");
importScripts("wechat.js");

importScripts("cloud-refresh.js");

// Cold startup may restore the first tab before declarative scripts are registered.
// Reuse the same bridge script once the worker is ready; its isolated-world guard prevents duplicates.
function reconnectChannelPage(tab) {
  try {
    const page = new URL(tab.url || "");
    if (page.origin !== "http://127.0.0.1:6800" || page.pathname !== "/channel-open" || !page.hash.includes("wechat-auto=")) return;
    chrome.scripting.executeScript({target:{tabId:tab.id},files:["channel-open.js"]}).catch(()=>{});
  } catch (_) {}
}
async function reconnectChannelPages() {
  const tabs = await chrome.tabs.query({url:"http://127.0.0.1/*"});
  tabs.forEach(reconnectChannelPage);
}
chrome.tabs.onUpdated.addListener((id, info, tab) => {
  if (info.url || info.status === "complete") reconnectChannelPage(tab);
});
chrome.runtime.onStartup.addListener(() => reconnectChannelPages().catch(()=>{}));
reconnectChannelPages().catch(()=>{});
