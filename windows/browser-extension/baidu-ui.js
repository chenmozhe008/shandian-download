// 弹窗里的百度网盘入口：只在百度网盘页面显示
(async function () {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  let url;
  try { url = new URL(tab?.url || ""); } catch (_) {}
  const open = document.getElementById("baidu-open");
  if (!open) return;
  if (url?.hostname !== "pan.baidu.com" || isBaiduSharePage(url)) return;
  open.hidden = false;
  const results = document.getElementById("results");
  const actions = document.getElementById("baidu-actions");
  const selectAll = document.getElementById("baidu-all");
  const download = document.getElementById("baidu-dl");
  const hint = document.getElementById("hint");
  const send = (m) => chrome.runtime.sendMessage(m);
  const size = (n) => (n >= 1 << 30 ? (n / (1 << 30)).toFixed(2) + " GB" : n >= 1 << 20 ? (n / (1 << 20)).toFixed(1) + " MB" : Math.ceil(n / 1024) + " KB");
  // 打开网盘时所在的文件夹，从地址里取
  const here = () => {
    const m = /[?&#]path=([^&#]*)/.exec(tab.url);
    try { return m ? decodeURIComponent(m[1]) : "/"; } catch (_) { return "/"; }
  };
  const needAuth = () => {
    open.textContent = "授权后下载百度网盘文件";
    hint.textContent = "会打开百度官方授权页，同意后回到这里再点一次。不需要在扩展里输入密码。";
    open.onclick = async () => {
      const r = await send({ type: "baiduStartAuth" });
      if (!r?.ok) hint.textContent = r?.error || "打开授权页失败";
    };
  };
  let selected = new Map();
  async function show(dir) {
    results.replaceChildren();
    actions.hidden = true;
    hint.textContent = "正在读取文件列表…";
    const r = await send({ type: "baiduList", dir });
    if (!r?.ok) {
      hint.textContent = r?.error || "读取失败";
      if (r?.needAuth) needAuth();
      return;
    }
    hint.textContent = dir;
    if (dir !== "/") {
      const up = document.createElement("button");
      up.className = "scan";
      up.textContent = "返回上一级";
      up.onclick = () => show(dir.replace(/\/[^/]*$/, "") || "/");
      results.appendChild(up);
    }
    const checks = [];
    const update = () => {
      download.disabled = !selected.size;
      download.textContent = `下载所选（${selected.size}）`;
      const on = checks.filter((c) => c.checked).length;
      selectAll.checked = checks.length > 0 && on === checks.length;
      selectAll.indeterminate = on > 0 && on < checks.length;
    };
    for (const f of r.list) {
      const row = document.createElement("label");
      row.className = "batch-row";
      const c = document.createElement("input");
      c.type = "checkbox";
      c.checked = selected.has(f.id);
      c.onchange = () => { if (c.checked) selected.set(f.id, f); else selected.delete(f.id); update(); };
      c._file = f;
      const name = document.createElement("span");
      name.className = "name";
      name.textContent = f.name;
      name.title = f.name;
      if (f.isdir) {
        name.style.cursor = "pointer";
        name.onclick = (e) => { e.preventDefault(); show(f.path); };
      }
      const kind = document.createElement("small");
      kind.textContent = f.isdir ? "文件夹 ›" : size(f.size);
      row.append(c, name, kind);
      results.appendChild(row);
      checks.push(c);
    }
    selectAll.onchange = () => {
      for (const c of checks) {
        c.checked = selectAll.checked;
        if (c.checked) selected.set(c._file.id, c._file); else selected.delete(c._file.id);
      }
      update();
    };
    download.onclick = async () => {
      download.disabled = true;
      hint.textContent = "正在获取下载地址…";
      const d = await send({ type: "baiduDownload", items: [...selected.values()], referrer: tab.url, tabId: tab.id });
      if (d?.ok) {
        hint.textContent = `已交给闪电全能下载（${d.sent} 个文件）。`;
        selected = new Map();
        for (const c of checks) c.checked = false;
      } else {
        hint.textContent = d?.error || "获取下载地址失败";
        if (d?.needAuth) needAuth();
      }
      update();
    };
    actions.hidden = false;
    update();
  }
  const state = await send({ type: "baiduAuth" });
  if (!state?.authed) return needAuth();
  open.textContent = "选择网盘文件下载";
  open.onclick = () => show(here());
})();
