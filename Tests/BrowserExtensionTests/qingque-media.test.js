const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");

const support = fs.readFileSync("browser-extension/qingque-media.js", "utf8");
const background = fs.readFileSync("browser-extension/background.js", "utf8");
const scan = background.slice(background.indexOf("async function scanPageMedia("), background.indexOf("// 只在用户点击时读取"));
const docId = "test-doc";
const origin = "https://docs.qingque.cn";
const loadPath = `/word/api/load/${docId}`;
const snapshotPath = `/word/api/snapshot/${docId}/13`;
const baseDownload = "/merlot/api/blob-store/v2/download";
const records = Array.from({ length: 60 }, (_, index) => {
  const url = `${baseDownload}?docId=${docId}&blobStoreKey=key-${index}&identityId=expired`;
  return { pk1: "t3", pk6: `attachment-${index}`, pk5: { aj_ao: {
    fj_b: url, fj_c: `原始附件-${index}.MOV`, fj_e: 1000 + index
  }}};
});
const snapshot = [{ pk9: [
  ...records.slice().reverse(),
  ...records.map((attachment, index) => ({ pk1: "t6", pk6: attachment.pk6, pk7: index })),
  records[0],
  { pk1: "t3", pk5: { aj_ao: { fj_c: "无法下载.mov", fj_b: "?docId=test-doc" } } },
  { pk1: "t3", pk5: { aj_ao: { fj_c: "外站.mov", fj_b: `https://example.org${baseDownload}?docId=${docId}&blobStoreKey=foreign` } } },
  { pk1: "t3", pk5: { aj_ao: { fj_c: "另一文档.mov", fj_b: `${baseDownload}?docId=other-doc&blobStoreKey=foreign` } } }
] }];
const requests = [];
const preview = [{ url: "https://cdn.example/preview.mp4" }];
const context = {
  URL, AbortSignal,
  location: { href: `${origin}/d/home/${docId}?identityId=active-share&via=notHome` },
  document: { title: "附件测试" },
  collectPageMedia: () => preview,
  fetch: async (url, options) => {
    requests.push({ url, options });
    if (url.pathname === loadPath) return { ok: true, json: async () => ({ code: 0, result: { snapshotRev: 13 } }) };
    assert.equal(url.pathname, snapshotPath);
    return { ok: true, json: async () => snapshot };
  },
  chrome: { scripting: { executeScript: async ({ func }) => [{ result: await func() }] } }
};
vm.createContext(context);
vm.runInContext(support + "\n" + scan, context);

(async () => {
  const found = await context.scanPageMedia(7, context.location.href);
  assert.equal(found.length, 60);
  assert.equal(found[0].filename, "原始附件-0");
  assert.equal(found[59].filename, "原始附件-59");
  assert.equal(found[0].size, 1000);
  assert.equal(found[0].ext, "mov");
  assert.equal(new URL(found[0].url).searchParams.get("identityId"), "active-share");
  assert.ok(found.every(item => item.source === "qingque-attachment"));
  assert.ok(requests.every(request => request.options.credentials === "same-origin"));
  assert.equal(requests[1].url.searchParams.get("supportChunkSnapshot"), "false");

  const general = await context.scanPageMedia(8, "https://example.org/d/home/test-doc");
  assert.equal(general[0], preview[0]);
  assert.equal(requests.length, 2);

  context.fetch = async () => ({ ok: false });
  const fallback = await context.scanPageMedia(9, context.location.href);
  assert.equal(fallback[0], preview[0]);
  console.log("轻雀媒体测试通过：60 项、原附件顺序、页面分享标识、跨站限制和扫描回退");
})().catch(error => { console.error(error); process.exitCode = 1; });
