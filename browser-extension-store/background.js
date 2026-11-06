// 商店版：右键发送常见文件直链；网页上开始的下载（种子文件、5 MB 以上的普通文件）交给桌面应用，应用接下了才取消浏览器那份。
// 不读取 Cookie、不扫描网页，只看下载项本身的地址、类型和大小。
const MENU_ID = "shandian-file-link";
const ENDPOINT = "http://127.0.0.1:6800/extension/";
const FILE_EXTENSIONS = new Set(["zip", "7z", "rar", "tar", "gz", "bz2", "xz", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "csv", "dmg", "pkg", "exe", "msi", "deb", "rpm", "iso", "apk"]);
// 与桌面应用的 LinkKind.videoHosts 对齐，避免文件链接被桌面应用改走视频解析。
const BLOCKED_HOSTS = ["youtube.com", "youtu.be", "youtube-nocookie.com", "googlevideo.com", "ytimg.com",
  "bilibili.com", "b23.tv", "douyin.com", "xiaohongshu.com", "xhslink.com", "kuaishou.com",
  "weibo.com", "ixigua.com", "vimeo.com", "x.com", "twitter.com", "xinpianchang.com"];

function supportedFileLink(value) {
  let url;
  try { url = new URL(value); } catch (_) { return false; }
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) return false;
  const host = url.hostname.toLowerCase();
  if (BLOCKED_HOSTS.some(blocked => host === blocked || host.endsWith("." + blocked))) return false;
  const extension = url.pathname.match(/\.([a-z0-9]+)$/i)?.[1]?.toLowerCase();
  return !!extension && FILE_EXTENSIONS.has(extension);
}

async function setResult(status) {
  await chrome.storage.local.set({ lastResult: status });
  await chrome.action.setBadgeText({ text: status === "sent" ? "✓" : "!" });
}

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({ id: MENU_ID, title: "发送到闪电 / Send to Shandian", contexts: ["link"] });
});

async function handleLink(info) {
  if (info.menuItemId !== MENU_ID) return;
  if (!supportedFileLink(info.linkUrl)) return setResult("unsupported");
  try {
    const ping = await fetch(ENDPOINT + "ping", { method: "POST" });
    if (!ping.ok || !(await ping.json()).ready) return setResult("unavailable");
    const response = await fetch(ENDPOINT + "add", {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ url: info.linkUrl })
    });
    await setResult(response.ok ? "sent" : "unavailable");
  } catch (_) { await setResult("unavailable"); }
}

chrome.contextMenus.onClicked.addListener(info => { handleLink(info).catch(() => {}); });

// 交给应用：先确认应用开着且内核就绪，再只发 {url}（网关对商店版只收这一项）
async function sendToApp(url) {
  try {
    const ping = await fetch(ENDPOINT + "ping", { method: "POST" });
    if (!ping.ok || !(await ping.json()).ready) return false;
    const response = await fetch(ENDPOINT + "add", {
      method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ url })
    });
    return response.ok;
  } catch (_) { return false; }
}

// 种子文件只有几十 KB，但点了就该交给应用；其他文件 5 MB 以下、图片、网页、文本留给浏览器（照完整版）
const SMALL = 5 * 1024 * 1024;
function isTorrent(item) {
  const path = (() => { try { return new URL(item.finalUrl || item.url).pathname; } catch (_) { return ""; } })();
  return /\.torrent$/i.test(path) || /\.torrent$/i.test(item.filename || "") || (item.mime || "").toLowerCase() === "application/x-bittorrent";
}
function worthTakingOver(item) {
  if (isTorrent(item)) return true;
  const mime = (item.mime || "").toLowerCase();
  if (/^(image|text)\//.test(mime) || mime === "application/xhtml+xml") return false;
  const size = item.totalBytes > 0 ? item.totalBytes : item.fileSize;
  return !(size > 0 && size < SMALL);
}

// 别的扩展在后台发起的下载不接：接过去会取消它，它一重试又被接（完整版 11-06 出过一次弹出几十个确认框）。
// 同一链接 2 分钟内只接一次，第二次交给浏览器自己下
const recentTakeovers = new Map();
const REPEAT_WINDOW = 2 * 60 * 1000;
function isUserDownload(item, ownId) {
  if (item.byExtensionId && item.byExtensionId !== ownId) return false;
  return !/^(chrome|edge)-extension:/i.test(item.referrer || "");
}
function firstTakeover(url, now) {
  for (const [key, time] of recentTakeovers) if (now - time > REPEAT_WINDOW) recentTakeovers.delete(key);
  if (recentTakeovers.has(url)) return false;
  recentTakeovers.set(url, now);
  return true;
}
function takeoverAllowed(url) {
  let parsed;
  try { parsed = new URL(url); } catch (_) { return false; }
  if (!["http:", "https:"].includes(parsed.protocol) || parsed.username || parsed.password) return false;
  const host = parsed.hostname.toLowerCase();
  return !BLOCKED_HOSTS.some(blocked => host === blocked || host.endsWith("." + blocked));
}
async function enabled() {
  const { takeover = true } = await chrome.storage.local.get("takeover");
  return takeover;
}

async function handleDownload(item) {
  const url = item.finalUrl || item.url;
  if (!takeoverAllowed(url) || !worthTakingOver(item) || !isUserDownload(item, chrome.runtime.id) || !(await enabled())) return;
  if (!firstTakeover(url, Date.now())) return;
  if (!(await sendToApp(url))) return;   // 应用没开、没就绪或拒收：浏览器照常下载
  try { await chrome.downloads.cancel(item.id); } catch (_) {}
  try { await chrome.downloads.erase({ id: item.id }); } catch (_) {}
}

chrome.downloads.onCreated.addListener(item => { handleDownload(item).catch(() => {}); });
