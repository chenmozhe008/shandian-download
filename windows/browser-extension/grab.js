// 抓取当前页面上的下载链接（磁力、ed2k、.torrent），由扩展图标或右键菜单注入到每个 frame。
// 抓到就直接交给 App 勾选，不多点一下。之后继续监听：有的页面要点一下才出链接，
// 一开始没抓到、或者后来又出现新的，才在页面右上角放小面板让用户交出去。
(() => {
  if (window.__sdGrab) {
    window.__sdGrab();
    return;
  }

  const isTop = window === window.top;
  const found = new Map(); // 去重键 -> { url, name, size }
  let active = true;
  const sent = new Set(); // 已经交给 App 的去重键

  const MAGNET = /magnet:\?[^\s"'<>\\，。；！？、）】》」』]+/gi;
  const ED2K = /ed2k:\/\/\|file\|[^\s"'<>\\，。；！？、）】》」』]+/gi;

  function keyOf(url) {
    const bt = /xt=urn:btih:([a-z0-9]+)/i.exec(url);
    if (bt) return "bt:" + bt[1].toLowerCase();
    const ed = /^ed2k:\/\/\|file\|[^|]*\|\d+\|([0-9a-f]+)/i.exec(url);
    if (ed) return "ed2k:" + ed[1].toLowerCase();
    return url;
  }

  function decode(s) {
    try { return decodeURIComponent(s.replace(/\+/g, " ")); } catch (_) { return s; }
  }

  function humanSize(n) {
    if (!(n > 0)) return "";
    const u = ["B", "KB", "MB", "GB", "TB"];
    let i = 0;
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return `${n >= 100 ? n.toFixed(0) : n.toFixed(1)} ${u[i]}`;
  }

  /// 链接自己带的名字和大小：磁力的 dn / xl，ed2k 的文件名和字节数，种子的文件名
  function ownInfo(url) {
    if (url.startsWith("magnet:")) {
      const dn = /[?&]dn=([^&]+)/i.exec(url);
      const xl = /[?&]xl=(\d+)/i.exec(url);
      return { name: dn ? decode(dn[1]) : "", size: xl ? humanSize(Number(xl[1])) : "" };
    }
    if (url.startsWith("ed2k:")) {
      const p = url.split("|");
      return { name: decode(p[2] || ""), size: humanSize(Number(p[3])) };
    }
    const file = url.split(/[?#]/)[0].split("/").pop() || "";
    return { name: decode(file), size: "" };
  }

  /// 链接没带名字时看它所在的那一行：这一行只有它一个链接才可信，否则会把别的版本的名字安到它头上
  function rowInfo(a) {
    const text = (a.textContent || a.title || "").replace(/\s+/g, " ").trim();
    const row = a.closest("tr, li, [class*=item], [class*=row]") || a.parentElement;
    const own = !!row && row.querySelectorAll('a[href^="magnet:"], a[href^="ed2k:"], a[href$=".torrent"]').length <= 1;
    const rowText = own ? (row.textContent || "").replace(/\s+/g, " ").trim() : "";
    const size = /(\d+(?:\.\d+)?)\s*(TB|GB|MB)\b/i.exec(rowText);
    // 「磁力链接」「下载」这种按钮文字不算名字
    // 整行文字里去掉大小和按钮文字，剩下的才是片名
    const name = text.length >= 6 ? text : rowText.replace(size?.[0] || "", " ").replace(text, " ").replace(/\s+/g, " ").trim();
    return { name: name.slice(0, 120), size: size ? `${size[1]} ${size[2].toUpperCase()}` : "" };
  }

  function add(url, a) {
    const key = keyOf(url);
    if (found.has(key)) return false;
    const own = ownInfo(url);
    const row = a ? rowInfo(a) : { name: "", size: "" };
    found.set(key, {
      url,
      name: own.name || row.name || (key.startsWith("bt:") ? key.slice(3, 15) : url.slice(0, 60)),
      size: own.size || row.size,
    });
    return true;
  }

  function validMagnet(url) {
    return /[?&]xt=urn:/i.test(url);
  }

  function scan() {
    let changed = false;
    // 先看 <a>：这样才能从那一行拿到名字和大小
    for (const a of document.querySelectorAll("a[href]")) {
      const href = a.href || "";
      if (href.startsWith("magnet:") ? validMagnet(href)
        : href.startsWith("ed2k:") || (/^https?:/i.test(href) && /\.torrent$/i.test(href.split(/[?#]/)[0]))) {
        changed = add(href, a) || changed;
      }
    }
    // 其余藏在 data 属性、复制按钮、脚本、输入框、纯文字里的，直接搜整页源码
    let html = document.documentElement.outerHTML;
    for (const el of document.querySelectorAll("input, textarea")) {
      if (el.value) html += "\n" + el.value;
    }
    html = html
      .replace(/&quot;|&#0?39;|&lt;|&gt;/g, '"')
      .replace(/&amp;|\\u0026/g, "&")
      .replace(/\\\//g, "/");
    for (const m of html.match(MAGNET) || []) {
      if (validMagnet(m)) changed = add(m) || changed;
    }
    for (const m of html.match(ED2K) || []) {
      if (/\|\/?$/.test(m)) changed = add(m) || changed;
    }
    return changed;
  }

  // 面板。全用 CSSOM 写样式：网页的内容安全策略可能禁止 <style> 和 style 属性
  let panel, countEl, hintEl, sendBtn;
  let booted = false;

  function buildPanel() {
    const view = window.ShFeedback.topRight({onClose:stop});
    panel=view.host;countEl=view.title;hintEl=view.message;sendBtn=view.action;
    sendBtn.hidden=false;sendBtn.textContent="交给闪电全能下载";sendBtn.onclick=send;
  }

  function pendingKeys() {
    return [...found.keys()].filter((k) => !sent.has(k));
  }

  function render() {
    if (!booted) return;
    const n = pendingKeys().length;
    // 都交出去了就不占地方；有没交的、或者一个都没抓到时才露面
    if (found.size && !n) {
      panel?.remove();
      panel = null;
      return;
    }
    if (!panel) buildPanel();
    countEl.textContent = !found.size ? "没有找到下载链接" : sent.size ? `又找到 ${n} 个下载链接` : `找到 ${n} 个下载链接`;
    hintEl.textContent = "要点击才显示的链接，点开后会自动加进来";
    sendBtn.textContent = "交给闪电全能下载";
    sendBtn.disabled = !n;
    sendBtn.style.setProperty("opacity", n ? "1" : ".5");
  }

  async function send() {
    const keys = pendingKeys();
    if (!keys.length) return;
    if (panel) {
      sendBtn.disabled = true;
      sendBtn.textContent = "正在交给闪电全能下载…";
    }
    const r = await chrome.runtime.sendMessage({ type: "grabSend", links: keys.map((k) => found.get(k)) }).catch(() => null);
    if (r?.ok) {
      keys.forEach((k) => sent.add(k));
      render();
      return;
    }
    if (!panel) buildPanel();
    countEl.textContent = `找到 ${keys.length} 个下载链接`;
    hintEl.textContent = r?.outdated
      ? "当前版本的闪电全能下载还不支持批量抓取，请先更新 App"
      : "没连上闪电全能下载，请打开 App 后再试";
    sendBtn.disabled = false;
    sendBtn.textContent = "重试";
  }

  // 新出现的链接：顶层直接并进来，子 frame 转交给顶层的面板
  function publish() {
    if (isTop) {
      // 第一批由启动时那次直接交出；之后新出现的走面板，不反复抢用户的焦点
      render();
    } else chrome.runtime.sendMessage({ type: "grabFrame", links: [...found.values()] }).catch(() => {});
  }

  let timer = 0;
  function schedule() {
    if (!active || timer) return;
    timer = setTimeout(() => {
      timer = 0;
      if (active && scan()) publish();
    }, 400);
  }

  const observer = new MutationObserver(schedule);
  const onClick = () => {
    schedule();
    // 点完后链接常常是请求回来才出现的
    setTimeout(schedule, 1500);
  };

  function start() {
    active = true;
    observer.observe(document.documentElement, {
      childList: true, subtree: true, attributes: true,
      attributeFilter: ["href", "value", "data-clipboard-text", "data-href", "data-url", "data-magnet"],
    });
    addEventListener("click", onClick, true);
  }

  function stop() {
    active = false;
    observer.disconnect();
    removeEventListener("click", onClick, true);
    panel?.remove();
    panel = null;
  }

  // 顶层等子 frame 报完再交，一次凑齐
  function firstSend() {
    booted = false;
    setTimeout(() => {
      scan();
      booted = true;
      if (found.size) send();
      else render();
    }, 600);
  }

  window.__sdGrab = () => {
    // 再点一次「抓取」：全部重新交一遍
    if (!active) start();
    scan();
    if (isTop) {
      sent.clear();
      firstSend();
    } else publish();
  };

  if (isTop) {
    chrome.runtime.onMessage.addListener((msg) => {
      if (msg.type !== "grabMerge") return;
      for (const l of msg.links) {
        if (!found.has(keyOf(l.url))) found.set(keyOf(l.url), l);
      }
      render();
    });
  }
  start();
  scan();
  if (isTop) firstSend();
  else publish();
})();
