// 商店版接管下载：种子不论大小都接、小文件不接、别的扩展发起的不接、同一链接只接一次、应用没开不取消、开关关掉不接
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const path = require("node:path");

const source = fs.readFileSync(path.join(__dirname, "../browser-extension-store/background.js"), "utf8");
let listener, app = "ready", takeover = true;
const added = [], cancelled = [];
const context = vm.createContext({
  URL, Date, Map,
  chrome: {
    runtime: { id: "self", onInstalled: { addListener() {} } },
    contextMenus: { create() {}, onClicked: { addListener() {} } },
    storage: { local: { async set() {}, async get() { return { takeover }; } } },
    action: { async setBadgeText() {} },
    downloads: { onCreated: { addListener(fn) { listener = fn; } }, async cancel(id) { cancelled.push(id); }, async erase() {} }
  },
  async fetch(url, options) {
    if (app === "offline") throw new Error("offline");
    if (url.endsWith("ping")) return { ok: true, async json() { return { ready: app !== "not-ready" }; } };
    added.push(JSON.parse(options.body));
    return { ok: app !== "rejected" };
  }
});
vm.runInContext(source, context);
const handle = item => context.handleDownload(item);
const reset = () => { added.length = 0; cancelled.length = 0; };

(async () => {
  // 种子文件只有几十 KB 也接；按地址、文件名或类型认
  await handle({ id: 1, url: "https://bt.example/a.torrent", mime: "", totalBytes: 30e3, referrer: "https://bt.example/" });
  await handle({ id: 2, url: "https://bt.example/get?id=7", filename: "Sintel.torrent", totalBytes: 40e3, referrer: "https://bt.example/" });
  await handle({ id: 3, url: "https://bt.example/x", mime: "application/x-bittorrent", totalBytes: 20e3, referrer: "https://bt.example/" });
  assert.deepEqual(added.map(x => Object.keys(x)), [["url"], ["url"], ["url"]], "只发 {url}");
  assert.deepEqual(cancelled, [1, 2, 3]);
  reset();
  // 5 MB 以下的 PDF、图片、网页不接；大文件接
  await handle({ id: 4, url: "https://example.org/readme.pdf", mime: "application/pdf", totalBytes: 40e3, referrer: "https://example.org/" });
  await handle({ id: 5, url: "https://example.org/photo.jpg", mime: "image/jpeg", totalBytes: 50e6, referrer: "https://example.org/" });
  await handle({ id: 6, url: "https://example.org/big.zip", mime: "application/zip", totalBytes: 50e6, referrer: "https://example.org/" });
  assert.deepEqual(added.map(x => x.url), ["https://example.org/big.zip"]);
  assert.deepEqual(cancelled, [6]);
  reset();
  // 别的扩展在后台发起、反复重试：一次都不接；扩展页面里点的也不接
  for (let i = 0; i < 20; i++) await handle({ id: 10 + i, url: "https://cdn.example/wall.bin", totalBytes: 0, byExtensionId: "infinity" });
  await handle({ id: 40, url: "https://cdn.example/wall2.bin", totalBytes: 0, referrer: "chrome-extension://infinity/newtab.html" });
  assert.equal(added.length, 0);
  // 网页上同一链接反复出现：2 分钟内只接第一次
  for (let i = 0; i < 20; i++) await handle({ id: 50 + i, url: "https://example.org/same.iso", totalBytes: 0, referrer: "https://example.org/" });
  assert.equal(added.length, 1);
  assert.deepEqual(cancelled, [50]);
  reset();
  // 应用没开、没就绪或拒收（例如网关不收的链接）：不取消，浏览器照常下载
  for (const state of ["offline", "not-ready", "rejected"]) {
    app = state;
    await handle({ id: 100, url: `https://example.org/${state}.zip`, totalBytes: 50e6, referrer: "https://example.org/" });
  }
  assert.deepEqual(cancelled, []);
  app = "ready";
  // 视频站地址不接（会被桌面应用改走视频解析）
  await handle({ id: 101, url: "https://www.youtube.com/file.zip", totalBytes: 50e6, referrer: "https://www.youtube.com/" });
  assert.deepEqual(cancelled, []);
  // 开关关掉不接
  takeover = false;
  await handle({ id: 102, url: "https://example.org/off.zip", totalBytes: 50e6, referrer: "https://example.org/" });
  assert.deepEqual(cancelled, []);
  console.log("商店版接管下载测试通过：种子不论大小、小文件不接、别的扩展不接、同一链接只接一次、应用没开不取消、开关关掉不接");
})().catch(error => { console.error(error); process.exitCode = 1; });
