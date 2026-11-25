
// Only the original attachment links issued for the active shared document are sent to the app.
async function getQingqueDocumentFiles(pageUrl, selected = null) {
  const page = new URL(pageUrl);
  const here = new URL(location.href);
  const docId = here.pathname.match(/^\/d\/home\/([\w-]+)\/?$/)?.[1];
  if (!docId || here.protocol !== "https:" || here.hostname !== "docs.qingque.cn" ||
      here.origin !== page.origin || here.pathname !== page.pathname || here.search !== page.search) {
    return { error: "文档已切换，请重新抓取。" };
  }
  const identity = here.searchParams.get("identityId");
  const documentUrl = path => {
    const url = new URL(path, here.origin);
    if (identity) url.searchParams.set("identityId", identity);
    return url;
  };
  const read = async url => {
    const response = await fetch(url, { credentials: "same-origin", redirect: "error", signal: AbortSignal.timeout(10000) });
    if (!response.ok) throw new Error();
    return response.json();
  };

  let snapshot;
  try {
    // Reuse the document snapshot the editor itself requested, including its current revision.
    const prefix = "/word/api/snapshot/" + docId + "/";
    const urls = performance.getEntriesByType("resource").map(entry => new URL(entry.name, here.origin));
    const snapshots = urls.filter(url => url.origin === here.origin && url.pathname.startsWith(prefix) &&
      /^\d+$/.test(url.pathname.slice(prefix.length)) && url.searchParams.get("identityId") === identity);
    let url = snapshots.reduce((latest, value) =>
      !latest || Number(value.pathname.slice(prefix.length)) >= Number(latest.pathname.slice(prefix.length)) ? value : latest, null);
    if (!url) {
      const loaded = await read(documentUrl("/word/api/load/" + docId));
      const revision = loaded.result?.snapshotRev;
      if (!loaded.ok || !Number.isSafeInteger(revision)) throw new Error();
      url = documentUrl(prefix + revision);
      url.searchParams.set("supportChunkSnapshot", "true");
    }
    snapshot = await read(url);
    if (!Array.isArray(snapshot)) throw new Error();
  } catch (_) {
    return { error: "无法读取轻雀文档的附件清单，请确认该文档可访问后重试。" };
  }

  const files = new Map();
  const unavailable = [];
  for (const part of snapshot) {
    for (const operation of Array.isArray(part?.pk9) ? part.pk9 : []) {
      // The editor stores attachment metadata separately from rendered video and image previews.
      const attachment = operation?.pk5?.aj_ao;
      if (typeof attachment?.fj_b !== "string") continue;
      let url;
      try { url = new URL(attachment.fj_b, here.href); } catch (_) { continue; }
      const key = url.searchParams.get("blobStoreKey");
      const name = url.searchParams.get("fileName") || attachment.fj_c;
      if (url.origin !== here.origin || url.username || url.password ||
          url.searchParams.get("docId") !== docId || typeof name !== "string" || !name.trim()) continue;
      if (key && ["/merlot/api/blob-store/v2/download", "/merlot/api/blob-store/download"].includes(url.pathname)) {
        if (!files.has(key)) files.set(key, { key, url: url.href, title: name, kind: "轻雀文档" });
      } else if (!key && url.pathname === here.pathname) {
        unavailable.push({
          url: "qingque-attachment:" + docId + ":" + unavailable.length,
          title: name, kind: "轻雀文档", blocked: true, blockedReason: "文档未提供原文件地址",
        });
      }
    }
  }

  const items = [...files.values()];
  if (selected === null) return {
    items: [...items.map(({ key, ...item }) => item), ...unavailable],
    unavailable: unavailable.length,
  };
  let chosen;
  if (typeof selected === "string") {
    chosen = items.filter(item => item.title === selected);
    if (chosen.length !== 1 || unavailable.some(item => item.title === selected)) {
      return { error: "无法唯一确认原文件，请在批量列表中选择附件。" };
    }
  } else {
    if (!Array.isArray(selected) || selected.length === 0 || selected.length > 1000) {
      return { error: "请选择要下载的附件，每批最多 1000 个。" };
    }
    const urls = new Set(selected);
    chosen = items.filter(item => urls.has(item.url));
    if (chosen.length !== urls.size) return { error: "附件清单已变化，请重新抓取。" };
  }

  const links = new Array(chosen.length);
  let index = 0;
  let failed = null;
  await Promise.all(Array.from({ length: Math.min(chosen.length, 6) }, async () => {
    while (index < chosen.length && !failed) {
      const i = index++;
      const item = chosen[i];
      const api = documentUrl("/merlot/api/docs/yfile/download-url/" + docId);
      api.searchParams.set("blobStoreKey", item.key);
      api.searchParams.set("fileName", item.title);
      try {
        const response = await read(api);
        if (!response.ok || typeof response.result?.fileUrl !== "string") throw new Error();
        const url = new URL(response.result.fileUrl, here.origin);
        if (url.origin !== here.origin || url.username || url.password ||
            url.pathname !== "/yfile/api/stream/download" ||
            url.searchParams.get("docId") !== docId || url.searchParams.get("blobStoreKey") !== item.key) throw new Error();
        links[i] = { url: url.href, name: item.title };
      } catch (_) {
        failed ||= item.title;
      }
    }
  }));
  if (failed) return { error: "无法获取「" + failed + "」的原文件。请确认附件可下载后重试。" };

  const names = new Set();
  for (const link of links) {
    const name = link.name.replace(/[\\/:*?"<>|\x00-\x1F]/g, " ").trim().slice(0, 240).replace(/[ .]+$/, "") || "附件";
    const dot = name.lastIndexOf(".");
    const stem = dot > 0 ? name.slice(0, dot) : name;
    const ext = dot > 0 ? name.slice(dot) : "";
    link.name = name;
    for (let n = 2; names.has(link.name.toLowerCase()); n++) link.name = stem + " (" + n + ")" + ext;
    names.add(link.name.toLowerCase());
  }
  return { links };
}

async function readQingqueFiles(tab, selected = null) {
  if (tab.pendingUrl) return { error: "文档正在切换，请重新抓取。" };
  const [frame] = await chrome.scripting.executeScript({
    target: { tabId: tab.id },
    func: getQingqueDocumentFiles,
    args: [tab.url, selected],
  });
  return frame?.result || { error: "无法读取轻雀文档，请刷新页面后重试。" };
}
