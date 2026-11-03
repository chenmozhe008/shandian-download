const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const path = require("node:path");

const source = fs.readFileSync(path.join(__dirname, "../browser-extension-store/background.js"), "utf8");
const manifest = JSON.parse(fs.readFileSync(path.join(__dirname, "../browser-extension-store/manifest.json"), "utf8"));
const seen = [], state = {};
let mode = "success";
const context = vm.createContext({
  URL,
  chrome: {
    runtime: { onInstalled: { addListener() {} } },
    contextMenus: { create() {}, onClicked: { addListener() {} } },
    storage: { local: { async set(data) { Object.assign(state, data); } } },
    action: { async setBadgeText() {} }
  },
  async fetch(url, options) {
    seen.push({ url, options });
    if (mode === "offline") throw new Error("offline");
    if (url.endsWith("ping")) return { ok: true, async json() { return { ready: mode !== "not-ready" }; } };
    return { ok: mode !== "rejected" };
  }
});
vm.runInContext(source, context);
const allowed = value => vm.runInContext(`supportedFileLink(${JSON.stringify(value)})`, context);
const run = url => context.handleLink({ menuItemId: "shandian-file-link", linkUrl: url });

(async () => {
  assert.deepEqual(manifest.permissions, ["contextMenus", "storage"]);
  assert.deepEqual(manifest.host_permissions, ["http://127.0.0.1/*"]);
  assert.equal(manifest.default_locale, "en");
  for (const locale of ["en", "zh_CN"]) {
    const messages = JSON.parse(fs.readFileSync(path.join(__dirname, `../browser-extension-store/_locales/${locale}/messages.json`), "utf8"));
    assert.ok(messages.extensionName.message && messages.extensionDescription.message);
  }
  assert.equal(allowed("https://example.org/manual.pdf?download=1"), true);
  const linksSource = fs.readFileSync(path.join(__dirname, "../Sources/Shandian/Links.swift"), "utf8");
  const hosts = linksSource.match(/static let videoHosts\s*=\s*\[([^\]]+)\]/s)?.[1].match(/"([^"]+)"/g)?.map(s => s.slice(1, -1));
  assert.ok(hosts?.length, "Cannot verify the desktop video-site list");
  for (const host of hosts) assert.equal(allowed(`https://${host}/file.zip`), false, `${host} would route to the video downloader`);
  for (const url of ["https://youtube.com/file.zip", "https://r.googlevideo.com/file.pdf", "https://vimeo.com/file.zip", "https://www.bilibili.com/file.pdf", "https://example.org/video.mp4", "https://example.org/page", "https://user:password@example.org/file.zip", "file:///tmp/file.zip"]) assert.equal(allowed(url), false);
  assert.equal(allowed("https://notyoutube.com/file.zip"), true);
  await run("https://example.org/file.zip");
  assert.equal(state.lastResult, "sent");
  assert.equal(seen.length, 2);
  assert.deepEqual(JSON.parse(seen[1].options.body), { url: "https://example.org/file.zip" });
  for (const failure of ["offline", "not-ready", "rejected"]) {
    mode = failure;
    await run("https://example.org/file.zip");
    assert.equal(state.lastResult, "unavailable");
  }
  seen.length = 0;
  await run("https://youtube.com/video.mp4");
  assert.equal(state.lastResult, "unsupported");
  assert.equal(seen.length, 0);
  console.log("商店版权限、链接筛选、发送与失败处理测试通过");
})().catch(error => { console.error(error); process.exitCode = 1; });
