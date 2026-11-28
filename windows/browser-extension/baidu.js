// 百度网盘直接下载（试点）：用户在百度官方页面授权，扩展用官方开放接口列文件、取下载地址，交给 App。
// 不碰账号密码和 Cookie。AccessToken 只存在本机扩展的存储里，不进任何日志。
// AppKey 由用户在扩展设置里填自己申请的（免费工具不共用一个开发者账号的额度）。AppSecret 用不到，不要填。
const BAIDU_OAUTH = "https://openapi.baidu.com/oauth/2.0/authorize";
const BAIDU_SUCCESS = "https://openapi.baidu.com/oauth/2.0/login_success";
// 百度要求下载时用这个 User-Agent，否则大文件会 403
const BAIDU_UA = "pan.baidu.com";
const BAIDU_META_BATCH = 100;

async function baiduToken() {
  const { baiduAuth } = await chrome.storage.local.get("baiduAuth");
  return baiduAuth && baiduAuth.expires > Date.now() ? baiduAuth.token : null;
}

async function baiduStartAuth() {
  const { baiduAppKey = "" } = await chrome.storage.local.get("baiduAppKey");
  if (!baiduAppKey) {
    const e = new BaiduError("需要百度 AppKey");
    e.noKey = true;
    throw e;
  }
  const q = new URLSearchParams({ response_type: "token", client_id: baiduAppKey, redirect_uri: "oob", scope: "basic,netdisk", display: "page" });
  return chrome.tabs.create({ url: `${BAIDU_OAUTH}?${q}` });
}

// 用户在百度页面点同意后，会跳到 login_success#access_token=...：取下令牌，关掉这个页面
chrome.tabs.onUpdated.addListener(async (tabId, change) => {
  if (!change.url?.startsWith(BAIDU_SUCCESS)) return;
  const hash = new URLSearchParams(change.url.split("#")[1] || "");
  const token = hash.get("access_token");
  if (!token) return;
  const seconds = Number(hash.get("expires_in")) || 30 * 24 * 3600;
  await chrome.storage.local.set({ baiduAuth: { token, expires: Date.now() + seconds * 1000 } });
  chrome.tabs.remove(tabId).catch(() => {});
});

const BAIDU_DESKTOP_AUTH = "http://127.0.0.1:6800/extension/baidu-auth/";
const baiduAuthKey = tabId => "baiduDesktopAuth:" + tabId;
const baiduAuthToken = value => typeof value === "string" && /^[a-f0-9]{32}$/i.test(value);
const baiduAuthWorkingTabs = new Set();
const baiduAuthSubmittingTabs = new Set();

function baiduDesktopAuthRequest(endpoint, body) {
  return fetch(BAIDU_DESKTOP_AUTH + endpoint, {
    method: "POST", cache: "no-store",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  }).then(response => response.json());
}

function baiduAuthorizationPage(page) {
  return page.origin === "https://openapi.baidu.com" &&
    ["/oauth/2.0/authorize", "/oauth/2.0/login_success", "/oauth/2.0/oob"].includes(page.pathname);
}

function baiduResultCode(page, title) {
  if (!baiduAuthorizationPage(page)) return "";
  const params = new URLSearchParams(page.hash.slice(1));
  if (page.searchParams.has("error") || params.has("error")) return "";
  const code = page.searchParams.get("code") || params.get("code");
  if (baiduAuthToken(code)) return code;
  // The official OOB flow also returns the code in the response page's title.
  const text = String(title || "").trim();
  if (baiduAuthToken(text)) return text;
  if (!/(?:授权码|authorization\s+code)/i.test(text)) return "";
  const codes = text.match(/\b[a-f0-9]{32}\b/gi);
  return codes?.length === 1 ? codes[0] : "";
}

// Baidu can show the authorization result in the current page without changing its title.
function observeBaiduAuthorizationCode(expectedUrl, ticket, deadline) {
  if (location.href !== expectedUrl || location.origin !== "https://openapi.baidu.com" || self !== top) return;
  if (globalThis.shandianBaiduResultTicket === ticket) return;
  globalThis.shandianBaiduResultTicket = ticket;
  const watchedPage = location.href;
  let timer;
  const observer = new MutationObserver(check);
  function stop() {
    observer.disconnect();
    clearTimeout(timer);
  }
  function check() {
    if (location.href !== watchedPage || Date.now() >= deadline ||
        globalThis.shandianBaiduResultTicket !== ticket) {
      stop();
      return;
    }
    const text = document.body?.innerText || "";
    if (!/(?:授权码|授权成功|authorization\s+code)/i.test(text)) return;
    const label = /(?:授权码|authorization\s+code)[^\r\n]{0,35}?[：:]?\s*([a-f0-9]{32})(?![a-f0-9])/i;
    let code = text.match(label)?.[1];
    if (!code) {
      const standalone = new Set(text.split(/\r?\n/).map(line => line.trim()).filter(line => /^[a-f0-9]{32}$/i.test(line)));
      const readonly = [...document.querySelectorAll('textarea[readonly], input[readonly]:not([type="password"]):not([type="hidden"])')]
        .filter(element => element.getClientRects().length).map(element => element.value.trim());
      readonly.filter(value => /^[a-f0-9]{32}$/i.test(value)).forEach(value => standalone.add(value));
      if (standalone.size === 1) code = standalone.values().next().value;
    }
    if (!code) return;
    stop();
    chrome.runtime.sendMessage({ type: "baiduDesktopAuthorizationCode", ticket, code }).catch(() => {});
  }
  timer = setTimeout(stop, Math.max(0, deadline - Date.now()));
  observer.observe(document.documentElement, { childList: true, characterData: true, subtree: true });
  check();
}

async function finishBaiduDesktopAuthorization(tab, session, code) {
  if (baiduAuthSubmittingTabs.has(tab.id)) return;
  baiduAuthSubmittingTabs.add(tab.id);
  try {
    const reply = await baiduDesktopAuthRequest("finish", { ticket: session.ticket, code });
    await chrome.storage.session.remove(baiduAuthKey(tab.id));
    if (reply?.ok) {
      const current = await chrome.tabs.get(tab.id).catch(() => null);
      if (current?.url === tab.url) await chrome.tabs.remove(tab.id).catch(() => {});
    }
  } finally {
    baiduAuthSubmittingTabs.delete(tab.id);
  }
}

async function handleBaiduDesktopAuthorization(tabId, tab) {
  const page = new URL(tab.url || "about:blank");
  const key = baiduAuthKey(tabId);
  if (page.origin === "http://127.0.0.1:6800" && page.pathname === "/baidu-authorize") {
    const ticket = page.hash.slice(1);
    if (!baiduAuthToken(ticket)) return;
    const response = await baiduDesktopAuthRequest("start", { ticket });
    if (!response?.ok) return;
    const login = new URL(response.url);
    if (login.origin !== "https://openapi.baidu.com" ||
        login.pathname !== "/oauth/2.0/authorize" ||
        login.searchParams.get("response_type") !== "code" ||
        login.searchParams.get("redirect_uri") !== "oob") return;
    if ((await chrome.tabs.get(tabId)).url !== tab.url) return;
    await chrome.storage.session.set({ [key]: { ticket, until: Date.now() + 600000 } });
    await chrome.tabs.update(tabId, { url: login.href });
    return;
  }
  const session = (await chrome.storage.session.get(key))[key];
  if (!baiduAuthorizationPage(page) || !baiduAuthToken(session?.ticket) || session.until < Date.now()) return;
  const code = baiduResultCode(page, tab.title);
  if (code) return finishBaiduDesktopAuthorization(tab, session, code);
  await chrome.scripting.executeScript({
    target: { tabId },
    func: observeBaiduAuthorizationCode,
    args: [tab.url, session.ticket, session.until],
  });
}

chrome.runtime.onMessage.addListener((message, sender, reply) => {
  if (message?.type !== "baiduDesktopAuthorizationCode") return;
  (async () => {
    const tab = sender.tab;
    if (!tab?.id || sender.frameId !== 0 || !sender.url || !baiduAuthToken(message.code)) return;
    const source = new URL(sender.url);
    if (!baiduAuthorizationPage(source) || (await chrome.tabs.get(tab.id)).url !== source.href) return;
    const key = baiduAuthKey(tab.id);
    const session = (await chrome.storage.session.get(key))[key];
    if (!baiduAuthToken(session?.ticket) || session.ticket !== message.ticket || session.until < Date.now()) return;
    await finishBaiduDesktopAuthorization({ id: tab.id, url: source.href }, session, message.code);
  })().then(() => reply({ ok: true }), () => reply({ ok: false }));
  return true;
});

chrome.tabs.onUpdated.addListener((tabId, change, tab) => {
  if (change.status !== "complete" && !change.title) return;
  if (!/^https:\/\/openapi\.baidu\.com\/oauth\/2\.0\/|^http:\/\/127\.0\.0\.1:6800\/baidu-authorize#/.test(tab.url || "")) return;
  if (baiduAuthWorkingTabs.has(tabId)) return;
  baiduAuthWorkingTabs.add(tabId);
  handleBaiduDesktopAuthorization(tabId, tab)
    .catch(() => {})
    .finally(() => baiduAuthWorkingTabs.delete(tabId));
});
chrome.tabs.onRemoved.addListener(async tabId => {
  await chrome.storage.session.remove(baiduShareCodeKey(tabId));
  const key = baiduAuthKey(tabId);
  const session = (await chrome.storage.session.get(key))[key];
  if (!baiduAuthToken(session?.ticket)) return;
  await chrome.storage.session.remove(key);
  await baiduDesktopAuthRequest("close", { ticket: session.ticket }).catch(() => {});
});

class BaiduError extends Error {
  constructor(message, needAuth) {
    super(message);
    this.needAuth = !!needAuth;
  }
}

async function baiduApi(path, params, step) {
  const token = await baiduToken();
  if (!token) throw new BaiduError("需要授权", true);
  const q = new URLSearchParams({ ...params, access_token: token });
  const r = await fetch(`https://pan.baidu.com/rest/2.0/${path}&${q}`, { headers: { "User-Agent": BAIDU_UA } });
  const j = await r.json();
  if (j.errno === -6 || j.errno === 111) {
    await chrome.storage.local.remove("baiduAuth");
    throw new BaiduError("授权已过期，请重新授权", true);
  }
  if (j.errno) throw new BaiduError(`${step}时百度网盘返回错误 ${j.errno}`);
  return j;
}

async function baiduList(dir) {
  const all = [];
  for (let start = 0; ; start += 1000) {
    const j = await baiduApi("xpan/file?method=list", { dir, web: 1, limit: 1000, start, order: "name" }, "列文件");
    const list = j.list || [];
    all.push(...list.map((f) => ({ id: f.fs_id, name: f.server_filename, path: f.path, isdir: !!f.isdir, size: f.size || 0 })));
    if (list.length < 1000) return all;
  }
}

// 文件夹展开成里面的所有文件，记下相对位置（选中项所在目录算起），下载时按网盘里的结构存放。
// 逐层用 list：递归接口 listall 实测返回 20020
async function baiduExpand(items) {
  const files = new Map();
  const queue = items.map((it) => ({ ...it, base: it.path.slice(0, it.path.lastIndexOf("/") + 1) }));
  while (queue.length) {
    const it = queue.shift();
    if (it.isdir) queue.push(...(await baiduList(it.path)).map((f) => ({ ...f, base: it.base })));
    else files.set(String(it.id), it.path.slice(it.base.length));
  }
  return files;
}

async function baiduSend(files, referrer, tabId, requireAll = false) {
  const ids = [...files.keys()];
  const resolved = new Map();
  const token = await baiduToken();
  let sent = 0;
  for (let i = 0; i < ids.length; i += BAIDU_META_BATCH) {
    const part = ids.slice(i, i + BAIDU_META_BATCH);
    if (!part.every(baiduFileId)) throw new BaiduError("无法识别文件编号");
    const j = await baiduApi("xpan/multimedia?method=filemetas", { fsids: "[" + part.join(",") + "]", dlink: 1 }, "取下载地址");
    for (const f of j.list || []) {
      const id = baiduFileId(f.fs_id);
      if (!f.dlink || !part.includes(id)) continue;
      const url = new URL(f.dlink);
      if (url.protocol !== "https:" || !/(^|\.)(baidu\.com|baidupcs\.com)$/.test(url.hostname) ||
          url.username || url.password || url.port || url.hash || url.searchParams.has("access_token")) continue;
      const download = url.href + (url.search ? "&" : "?") + "access_token=" + encodeURIComponent(token);
      resolved.set(id, { filename: f.filename, url: download });
    }
  }
  if (requireAll && resolved.size !== ids.length) throw new BaiduError("百度网盘没有返回所有文件的下载地址");
  for (const [id, f] of resolved) {
    const body = JSON.stringify({ url: f.url, headers: { "User-Agent": BAIDU_UA }, name: ids.length === 1 ? f.filename || files.get(id) : files.get(id) || f.filename, page: referrer, group: true });
    const post = () => fetch(APP, { method: "POST", headers: { "Content-Type": "application/json" }, body }).then((r) => r.ok);
    let ok = await post().catch(() => null);
    if (ok === null && tabId !== undefined && (await launchApp(tabId))) ok = await post().catch(() => false);
    if (!ok) throw new BaiduError("没能交给闪电全能下载，请确认应用已打开");
    sent++;
  }
  return sent;
}

// 读网盘页面里选中的文件。页面把选择存在 Vue 组件上，要在页面自己的环境里读
async function baiduPageSelection(tabId) {
  const [r] = await chrome.scripting.executeScript({
    target: { tabId },
    world: "MAIN",
    func: () => {
      const core = document.querySelector(".wp-s-core-pan")?.__vue__?.selectedList;
      const old = document.querySelector(".file-list")?.__vue__?.allFileList?.filter((x) => x.selected);
      return (core?.length ? core : old || []).map((f) => ({ id: f.fs_id, path: f.path, isdir: !!Number(f.isdir), name: f.server_filename }));
    },
  });
  return r?.result || [];
}

function baiduShareLink(value) {
  try {
    const url = new URL(value);
    if (!["https:", "http:"].includes(url.protocol) || url.hostname !== "pan.baidu.com" || url.username || url.password || url.port) return "";
    if (!/^\/(s\/[-_a-z0-9]+\/?|share\/(init|link)\/?)$/i.test(url.pathname)) return "";
    url.hash = "";
    for (const key of [...url.searchParams.keys()]) {
      if (!["pwd", "surl", "shareid", "uk"].includes(key)) url.searchParams.delete(key);
    }
    return url.href;
  } catch (_) { return ""; }
}

const baiduShareCodeKey = tabId => "baiduShareCode:" + tabId;
const isBaiduShareCode = value => typeof value === "string" && /^[a-z0-9]{4}$/i.test(value);

function baiduShareIdentity(link) {
  if (!link) return "";
  const url = new URL(link);
  const path = url.pathname.replace(/\/$/, "");
  if (path.startsWith("/s/")) return "short:" + path.slice(3);
  if (path === "/share/init") {
    const surl = url.searchParams.get("surl");
    return /^[-_a-z0-9]+$/i.test(surl || "") ? "short:1" + surl : "";
  }
  const shareid = url.searchParams.get("shareid");
  const uk = url.searchParams.get("uk");
  return /^\d+$/.test(shareid || "") && /^\d+$/.test(uk || "") ? "long:" + shareid + "/" + uk : "";
}

async function baiduShareWithRememberedCode(tab, link) {
  const url = new URL(link);
  if (isBaiduShareCode(url.searchParams.get("pwd"))) return url.href;
  const identity = baiduShareIdentity(link);
  if (!identity) return url.href;
  const key = baiduShareCodeKey(tab.id);
  const record = (await chrome.storage.session.get(key))[key];
  if (record?.identity === identity && isBaiduShareCode(record.code)) {
    url.searchParams.set("pwd", record.code);
  }
  return url.href;
}

// Read only the selection and filenames already visible to the user on the share page.
async function baiduShareSelection(tab) {
  const [result] = await chrome.scripting.executeScript({
    target: { tabId: tab.id },
    world: "MAIN",
    args: [tab.url],
    func: expected => {
      if (location.href !== expected) return null;
      const visible = el => el.getClientRects().length > 0;
      const locked = [...document.querySelectorAll('input#accessCode, input[placeholder*="提取码"], input[aria-label*="提取码"]')].some(visible);
      if (locked) return { locked: true };
      const owner = [...document.querySelectorAll('a, button, [role="button"]')]
        .some(el => visible(el) && (el.innerText || el.textContent || "").trim() === "取消分享");
      const core = document.querySelector(".wp-s-core-pan")?.__vue__?.selectedList;
      const old = document.querySelector(".file-list")?.__vue__?.allFileList?.filter(item => item.selected);
      const selected = core?.length ? core : old || [];
      const activeList = [...document.querySelectorAll(".JS-list-view-item, .module-list-view")].some(visible);
      const grid = !activeList && (
        /(?:^|[&?#])vmode=grid(?:&|$)/i.test(location.hash)
        || [...document.querySelectorAll(".JS-grid-view-item, .module-grid-view, .share-list-file-list [role='grid']")].some(visible)
      );
      const rows = [...document.querySelectorAll("[data-file-key]")].filter(visible);
      const modern = rows.filter(row => {
        if (row.matches('[aria-selected="true"], .wp-selected')) return true;
        const mark = row.querySelector('.wp-checkbox--checked, [role="checkbox"][aria-checked="true"], input[type="checkbox"]:checked');
        return mark?.closest("[data-file-key]") === row;
      });
      const legacy = [...document.querySelectorAll(
        '.module-grid-view [node-type="list"] > .item-active[data-id], .module-list-view [node-type="list"] > .item-active[data-id]'
      )].filter(visible);
      const checked = [...new Set([...modern, ...legacy])];
      const idOf = id => typeof id === "string" ? id : Number.isSafeInteger(id) ? String(id) : "";
      const rowId = row => idOf(row.getAttribute("data-file-key") || row.getAttribute("data-id"));
      const ids = [...new Set([...checked.map(rowId), ...selected.map(item => idOf(item.fs_id))])];
      const advertisedCount = [...document.querySelectorAll(".module-share-top-bar,.module-share-bottom-bar,.share-list-toolbar")]
        .reduce((count, el) => Math.max(count, Number((el.innerText || "").match(/已选(?:中|择)?\s*(\d+)\s*(?:个|项)/)?.[1] || 0)), 0);

      let listed = [];
      try {
        const list = window.locals?.dump?.()?.file_list?.value;
        if (Array.isArray(list)) listed = list;
      } catch {}
      if (owner && !ids.length && listed.length === 1) ids.push(idOf(listed[0].fs_id));
      const selectionNames = new Map([...listed, ...selected].map(item =>
        [idOf(item.fs_id), item.server_filename || item.name || ""]));
      const files = ids.map(id => {
        const label = checked.find(row => rowId(row) === id)
          ?.querySelector('.file-name, .filename, .file-name-text, .wp-s-file-name, [node-type="name"]');
        return { id, name: selectionNames.get(id) || label?.title || label?.textContent?.trim() || "" };
      });

      // Prefer the actual root filenames over the browser tab title or share ID.
      const inFolder = /(?:^|[&#/])path=/.test(location.hash);
      const names = inFolder ? [] :
        listed.map(item => item.server_filename).filter(name => typeof name === "string" && name.trim());
      return { owner, ids, files, advertisedCount, grid, inFolder, firstName: names[0] || "", multiple: names.length > 1 };
    },
  });
  return result?.result;
}

function baiduFileId(value) {
  const id = typeof value === "string" ? value : Number.isSafeInteger(value) ? String(value) : "";
  return /^[1-9][0-9]{0,19}$/.test(id) ? id : "";
}

async function sendBaiduSelection(tab, share, files) {
  const submit = async () => {
    // Old desktop clients reject this request because it does not contain the generic "url" field.
    const response = await fetch(new URL("/extension/baidu-selection", APP), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ share, files }),
    });
    const result = await response.json();
    return response.ok && result.selection === true
      ? { ok: true, pending: true }
      : { ok: false, error: "请更新闪电全能下载后再提交所选文件。" };
  };
  try { return await submit(); }
  catch (_) {
    if (await launchApp(tab.id)) return submit().catch(() => ({ ok: false, error: "提交所选文件失败，请重试。" }));
    return { ok: false, error: "请打开闪电全能下载后重试。" };
  }
}

async function baiduDownloadShare(tab, share) {
  const selection = await baiduShareSelection(tab);
  if (!selection) return { ok: false, error: "分享页已切换，请重试。" };
  const ids = selection.ids || [];
  const selectionHint = message => ({
    ok: false,
    selectionHint: true,
    error: selection.grid ? "请切到列表视图，再点「闪电下载」。" : message,
  });
  if ((selection.advertisedCount > 0 && selection.advertisedCount !== ids.length) || !ids.every(baiduFileId)) {
    return selectionHint("未能读取全部勾选项，请重新勾选。");
  }
  if (!selection.owner && ids.length) {
    if (!selection.files.every(item => item.name.trim())) {
      return selectionHint("未能读取所选文件名，请重新勾选。");
    }
    return sendBaiduSelection(tab, await baiduShareWithRememberedCode(tab, share), selection.files);
  }
  if (!selection.locked && !ids.length && (selection.owner || selection.inFolder || selection.grid)) {
    return selectionHint("请先勾选要下载的文件。");
  }
  if (selection.owner) {
    if (!(await baiduToken())) {
      await baiduStartAuth();
      return { ok: false, needAuth: true };
    }
    const originals = new Map();
    for (let i = 0; i < ids.length; i += BAIDU_META_BATCH) {
      const part = ids.slice(i, i + BAIDU_META_BATCH);
      const response = await baiduApi("xpan/multimedia?method=filemetas", { fsids: "[" + part.join(",") + "]", dlink: 0 }, "读取网盘原文件");
      for (const item of response.list || []) {
        const id = baiduFileId(item.fs_id);
        if (part.includes(id) && typeof item.path === "string" && item.path.startsWith("/")) {
          originals.set(id, { id, path: item.path, isdir: !!Number(item.isdir) });
        }
      }
    }
    if (originals.size !== ids.length) {
      return { ok: false, error: "当前授权的百度网盘账号找不到所选原文件，请核对授权账号。" };
    }
    const files = await baiduExpand(ids.map(id => originals.get(id)));
    if (!files.size) return { ok: false, error: "所选文件夹里没有可下载的文件。" };
    return { ok: true, sent: await baiduSend(files, share, tab.id, true) };
  }

  const link = await baiduShareWithRememberedCode(tab, share);
  const needCode = !!selection.locked && !isBaiduShareCode(new URL(link).searchParams.get("pwd"));
  const name = selection.firstName ? selection.firstName + (selection.multiple ? " 等" : "") : "";
  const ok = await sendToApp(link, "", name, tab.id, needCode ? "baiduShareLocked" : "baiduShare");
  return { ok, pending: ok, needCode: ok && needCode, error: ok ? undefined : "请打开闪电全能下载后重试。" };
}

chrome.runtime.onMessage.addListener((msg, sender, reply) => {
  if (!String(msg.type).startsWith("baidu")) return;
  if (msg.type === "baiduDesktopAuthorizationCode") return;
  (async () => {
    if (msg.type === "baiduShareCodeEntered") {
      const identity = baiduShareIdentity(baiduShareLink(sender.url || ""));
      if (!identity || sender.frameId !== 0 || !sender.tab?.id || !isBaiduShareCode(msg.code)) return { ok: false };
      await chrome.storage.session.set({ [baiduShareCodeKey(sender.tab.id)]: { identity, code: msg.code } });
      return { ok: true };
    }
    if (msg.type === "baiduSelected" || msg.type === "baiduShareDownload") {
      const tab = sender.tab || (Number.isInteger(msg.tabId) ? await chrome.tabs.get(msg.tabId) : null);
      const share = baiduShareLink(tab?.url || "");
      if (share) return baiduDownloadShare(tab, share);
      if (msg.type === "baiduShareDownload" || !tab || !tab.url?.startsWith("https://pan.baidu.com/")) {
        return { ok: false, error: "请先打开百度网盘分享页。" };
      }
      if (sender.tab && typeof baiduResumeInTab === "function") {
        const result = await baiduResumeInTab(tab);
        if (result) return result;
      }
      if (!(await baiduToken())) {
        await baiduStartAuth();
        return { ok: false, needAuth: true };
      }
      const items = await baiduPageSelection(tab.id);
      if (!items.length) return { ok: false, error: "先选中文件" };
      const ids = await baiduExpand(items);
      return { ok: true, sent: await baiduSend(ids, tab.url, tab.id) };
    }
    if (msg.type === "baiduAuth") return { ok: true, authed: !!(await baiduToken()) };
    if (msg.type === "baiduStartAuth") {
      await baiduStartAuth();
      return { ok: true };
    }
    if (msg.type === "baiduList") return { ok: true, list: await baiduList(msg.dir) };
    if (msg.type === "baiduDownload") {
      const ids = await baiduExpand(msg.items);
      return { ok: true, sent: await baiduSend(ids, msg.referrer, msg.tabId) };
    }
    return { ok: false };
  })().then(reply, (e) => reply({ ok: false, error: e.message, needAuth: !!e.needAuth, noKey: !!e.noKey }));
  return true;
});
