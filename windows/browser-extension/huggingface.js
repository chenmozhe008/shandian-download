const HF_ORIGIN = "https://huggingface.co";

function huggingFaceSizeLabel(bytes) {
  if (!Number.isSafeInteger(bytes) || bytes < 0) return "";
  return bytes >= 1e9 ? (bytes / 1e9).toFixed(2) + " GB"
    : bytes >= 1e6 ? (bytes / 1e6).toFixed(2) + " MB"
    : bytes >= 1e3 ? (bytes / 1e3).toFixed(2) + " KB" : bytes + " B";
}

function huggingFaceHoverSize(value, root = document) {
  try {
    const url = new URL(value);
    if (url.origin !== HF_ORIGIN) return null;
    const [owner, model, route, revision, ...path] = url.pathname.slice(1).split("/").map(decodeURIComponent);
    if (route !== "resolve" || !revision || !path.length) return null;
    const repo = owner + "/" + model;
    const file = path.join("/");
    for (const node of root.querySelectorAll('[data-target="ViewerIndexTreeList"], [data-target="BlobDownloadMenu"]')) {
      const props = JSON.parse(node.getAttribute("data-props") || "{}");
      if (props.context?.repo?.name === repo && props.context.rev === revision) {
        const entry = props.entries?.find(entry => entry.type === "file" && entry.path === file);
        if (entry) return { sizeLabel: huggingFaceSizeLabel(entry.size), sizeBytes: entry.size };
      }
      if (props.repoId?.name === repo && props.revision === revision && props.filePath === file &&
          typeof props.size === "string" && /^\d+(?:\.\d+)?\s+(?:[KMGT]?B|Bytes?)$/i.test(props.size.trim())) {
        return { sizeLabel: props.size.trim().replace(/\s+/, " ").replace(/\bbytes?$/i, "B").toUpperCase() };
      }
    }
  } catch (_) {}
  return null;
}

function huggingFaceTree(value) {
  try {
    const url = new URL(value);
    if (url.origin !== HF_ORIGIN) return null;
    const [owner, repo, route, revision, ...directory] = url.pathname.slice(1).replace(/\/$/, "").split("/").map(decodeURIComponent);
    if (route !== "tree" || !revision || revision === "." || revision === ".." ||
        ![owner, repo].every(s => /^[\w.-]+$/.test(s) && s !== "." && s !== "..") ||
        ["api", "datasets", "spaces", "buckets"].includes(owner) ||
        directory.some(s => !s || s === "." || s === ".." || /[/\\]/.test(s))) return null;
    return { repo, revision, model: [owner, repo].map(encodeURIComponent).join("/"), directory };
  } catch (_) { return null; }
}

async function readHuggingFaceFiles(value, selected) {
  const tree = huggingFaceTree(value);
  if (!tree) throw Error("请打开 Hugging Face 模型文件页");
  const model = HF_ORIGIN + "/api/models/" + tree.model;
  const chosen = selected === undefined ? null : new Set(selected);
  if (chosen && !chosen.size) throw Error("请先选择文件");
  const revisionPrefix = HF_ORIGIN + "/" + tree.model + "/resolve/";
  const first = chosen?.values().next().value;
  const pin = first?.startsWith(revisionPrefix) ? first.slice(revisionPrefix.length).split("/")[0] : "";
  if (chosen && !/^[a-f0-9]{40}$/i.test(pin)) throw Error("文件列表已变化，请重新查找");
  const options = { credentials: "omit", redirect: "error", signal: AbortSignal.timeout(15000) };
  const info = await fetch(model + "/revision/" + encodeURIComponent(pin || tree.revision), options);
  if (!info.ok) throw Error("无法读取公开模型文件，请确认仓库可访问");
  const { sha, gated, private: isPrivate } = await info.json();
  if (gated || isPrivate || !/^[a-f0-9]{40}$/i.test(sha || "")) throw Error("此入口只下载公开模型文件");
  const api = model + "/tree/" + sha + (tree.directory.length ? "/" + tree.directory.map(encodeURIComponent).join("/") : "");
  const seen = new Set();
  const names = new Set();
  const items = [];
  let next = api + "?recursive=true";
  while (next) {
    if (seen.has(next) || seen.size >= 100) throw Error("模型文件列表分页异常");
    seen.add(next);
    const response = await fetch(next, options);
    if (!response.ok) throw Error("读取模型文件列表失败，请重试");
    const rows = await response.json();
    if (!Array.isArray(rows)) throw Error("模型文件列表格式有误");
    for (const row of rows) {
      if (row.type !== "file" || typeof row.path !== "string") continue;
      const parts = row.path.split("/");
      if (tree.directory.some((part, index) => parts[index] !== part) || parts.length <= tree.directory.length) continue;
      const url = revisionPrefix + sha + "/" + parts.map(encodeURIComponent).join("/");
      if (chosen && !chosen.has(url)) continue;
      const local = parts.slice(tree.directory.length);
      if (local.some(part => !part || /[\\:*?"<>|\u0000-\u001f]|[. ]$/.test(part) ||
          /^(?:con|prn|aux|nul|com[1-9¹²³]|lpt[1-9¹²³])(?:\.|$)/i.test(part))) {
        throw Error("模型含 Windows 无法保存的文件名");
      }
      const title = local.join("/");
      if (names.has(title.toLowerCase())) throw Error("模型含 Windows 下同名的文件");
      names.add(title.toLowerCase());
      const bytes = row.size;
      const sizeBytes = Number.isSafeInteger(bytes) && bytes >= 0 ? bytes : null;
      const size = huggingFaceSizeLabel(sizeBytes);
      items.push({ url, title, kind: "Hugging Face", size, sizeBytes });
      if (items.length > 1000) throw Error("此目录超过 1000 个文件，请分目录下载");
    }
    const page = response.headers.get("Link")?.match(/<([^>]+)>\s*;\s*rel="next"/)?.[1];
    next = page ? new URL(page, next).href : "";
    if (next && !next.startsWith(api + "?")) throw Error("模型文件列表的分页地址有误");
  }
  if (chosen && items.length !== chosen.size) throw Error("文件列表已变化，请重新查找");
  return { items, folderName: tree.directory.at(-1) || tree.repo };
}
