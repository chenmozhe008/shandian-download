// 轻雀按可视区域挂载播放器。使用当前文档的附件模型取得原文件，
// 包括尚未滚动到的附件。此函数由 executeScript 注入，不能引用外部变量。
async function collectQingqueMedia() {
  const page = new URL(location.href);
  if (page.origin !== "https://docs.qingque.cn") return [];
  const docId = page.pathname.match(/^\/d\/home\/([\w-]+)\/?$/)?.[1];
  if (!docId) return [];

  const identity = page.searchParams.get("identityId");
  const signal = AbortSignal.timeout(12000);
  const get = async (path, params = {}) => {
    const url = new URL(path, page.origin);
    for (const [key, value] of Object.entries(params)) url.searchParams.set(key, value);
    if (identity) url.searchParams.set("identityId", identity);
    const response = await fetch(url, { credentials: "same-origin", signal });
    if (!response.ok) throw new Error("Qingque document request failed");
    return response.json();
  };

  const metadata = await get(`/word/api/load/${docId}`);
  const revision = String(metadata.result?.maxSnapshotVersion ?? metadata.result?.snapshotRev ?? "");
  if (metadata.code !== 0 || !/^\d+$/.test(revision) || revision === "0") return [];
  // 请求完整快照，避免只读取当前页或分块快照的描述信息。
  const snapshot = await get(`/word/api/snapshot/${docId}/${revision}`, { supportChunkSnapshot: "false" });
  if (!Array.isArray(snapshot)) return [];
  const items = new Map();
  for (const chunk of snapshot) {
    if (!Array.isArray(chunk?.pk9)) continue;
    const positions = new Map(chunk.pk9.filter(node => node?.pk1 === "t6").map(node => [node.pk6, node.pk7]));
    const attachments = chunk.pk9.filter(node => node?.pk1 === "t3" && node.pk5?.aj_ao);
    attachments.sort((a, b) => (positions.get(a.pk6) ?? Infinity) - (positions.get(b.pk6) ?? Infinity));
    for (const node of attachments) {
      const media = node.pk5.aj_ao;
      const name = typeof media.fj_c === "string" ? media.fj_c.trim() : "";
      const ext = name.match(/\.([a-z0-9]{2,5})$/i)?.[1]?.toLowerCase();
      if (!ext || typeof media.fj_b !== "string") continue;
      const kind = ["mp4", "m4v", "mov", "webm", "mkv", "avi", "wmv", "flv"].includes(ext) ? "视频"
        : ["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus"].includes(ext) ? "音频" : "";
      if (!kind) continue;

      let url;
      try { url = new URL(media.fj_b, page.origin); } catch (_) { continue; }
      if (url.origin !== page.origin || url.username || url.password ||
          url.pathname !== "/merlot/api/blob-store/v2/download" || url.searchParams.get("docId") !== docId) continue;
      const key = url.searchParams.get("blobStoreKey");
      if (!key || items.has(key)) continue;
      url.searchParams.set("fileName", name);
      // 快照里可能留着上传者的旧分享标识，下载沿用当前页面的访问入口。
      url.searchParams.delete("identityId");
      if (identity) url.searchParams.set("identityId", identity);
      const filename = name.slice(0, -(ext.length + 1));
      const size = Number.isSafeInteger(media.fj_e) && media.fj_e > 0 ? media.fj_e : undefined;
      items.set(key, {
        url: url.href, referrer: page.href, kind, ext, title: document.title,
        filename, trackTitle: filename, size, source: "qingque-attachment"
      });
      if (items.size >= 100) return [...items.values()];
    }
  }
  return [...items.values()];
}
