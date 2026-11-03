// 商店版仅通过右键发送常见文件直链；不读取 Cookie、扫描网页或取消浏览器下载。
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
