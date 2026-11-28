const assert = require("node:assert/strict");
const fs = require("node:fs");
const { test } = require("node:test");
const vm = require("node:vm");

for (const root of ["."]) {
  test(root + " scans a whole Qingque document and delivers the selected originals as one batch", async () => {
    const origin = "https://docs.qingque.cn";
    const docId = "sample-doc";
    const page = origin + "/d/home/" + docId + "?identityId=share";
    const documentName = "生态大会素材脚本汇总";
    const prefix = origin + "/word/api/snapshot/" + docId + "/";
    const first = "轻雀素材.mov";
    const second = "另一段.mp4";
    const attachment = (key, name, id = docId) => ({
      pk5: { aj_ao: { fj_c: name, fj_b: "/merlot/api/blob-store/v2/download?" +
        new URLSearchParams({ blobStoreKey: key, fileName: name, docId: id }),
      } },
    });
    const snapshot = [{ pk9: [attachment("one", first), attachment("one", first),
      attachment("two", second), attachment("foreign", "其他文档.mov", "different-doc"),
      { pk5: { aj_ao: { fj_c: "缺失的原件.MOV", fj_b: "?docId=" + docId } } },
    ] }];
    const reads = [], requests = [];
    const context = vm.createContext({
      URL, AbortSignal, location: new URL(page),
      performance: { getEntriesByType: () => [
        { name: prefix + "5?identityId=share" },
        { name: prefix + "6?identityId=share" },
        { name: prefix + "7?identityId=old" },
      ] },
      fetch: async (value, options) => {
        const url = new URL(value);
        reads.push(url.pathname);
        assert.equal(url.origin, origin);
        assert.equal(url.searchParams.get("identityId"), "share");
        assert.equal(options.credentials, "same-origin");
        assert.equal(options.redirect, "error");
        let data = snapshot;
        if (url.pathname === "/word/api/load/" + docId) data = { ok: true, result: { snapshotRev: 6 } };
        if (url.pathname === "/merlot/api/docs/yfile/download-url/" + docId) {
          const fileUrl = origin + "/yfile/api/stream/download?" + new URLSearchParams({
            docId, blobStoreKey: url.searchParams.get("blobStoreKey"), fileName: url.searchParams.get("fileName"),
          });
          data = { ok: true, result: { fileUrl } };
        }
        return { ok: true, json: async () => data };
      },
      chrome: {
        tabs: { get: async () => ({ id: 7, url: page, title: documentName + " - 轻雀文档" }) },
        scripting: { executeScript: async ({ target, func, args }) => {
          assert.equal(target.tabId, 7);
          return [{ result: await func(...args) }];
        } },
        runtime: { getURL: name => "chrome-extension://example/" + name },
      },
      appReady: async () => true,
      headersFor: async (_, referrer) => ({ Referer: referrer }),
      extensionBatchRequest: async (path, body) => {
        requests.push({ path, body });
        return { ok: true };
      },
    });
    vm.runInContext(fs.readFileSync(root + "/browser-extension/qingque.js", "utf8"), context);
    vm.runInContext(fs.readFileSync(root + "/browser-extension/huggingface.js", "utf8"), context);
    const list = await context.getQingqueDocumentFiles(page);
    assert.deepEqual(Array.from(list.items, item => item.title), [first, second, "缺失的原件.MOV"]);
    assert.equal(list.unavailable, 1);
    assert.equal(list.items[2].blocked, true);
    assert.deepEqual(reads, ["/word/api/snapshot/" + docId + "/6"]);
    const changed = await context.getQingqueDocumentFiles(page, [origin + "/out-of-scope"]);
    assert.match(changed.error, /重新抓取/);
    assert.match((await context.getQingqueDocumentFiles(page, [list.items[2].url])).error, /重新抓取/);
    assert.equal((await context.getQingqueDocumentFiles(page, first)).links[0].name, first);

    const background = fs.readFileSync(root + "/browser-extension/background.js", "utf8");
    const start = background.indexOf('  if (msg.type === "downloadLinks")');
    const end = background.indexOf("  if(msg.type==='downloadCollectedSiteBatch')", start);
    vm.runInContext("globalThis.dispatch = (msg, _sender) => new Promise((reply) => {" +
      background.slice(start, end) + "});", context);
    const result = await context.dispatch({ type: "downloadLinks", tabId: 7, items: list.items.filter(item => !item.blocked) },
      { url: "chrome-extension://example/popup.html" });
    assert.equal(result.ok, true);
    assert.equal(requests.length, 1);
    assert.equal(requests[0].path, "batch");
    assert.equal(requests[0].body.folderName, documentName);
    assert.deepEqual(Array.from(requests[0].body.links, link => link.name), [first, second]);
    for (const link of requests[0].body.links) {
      assert.equal(new URL(link.url).pathname, "/yfile/api/stream/download");
      assert.equal(link.headers.Referer, page);
    }
    const host = fs.readFileSync(root + "/ui/app.js", "utf8");
    const batchStart = host.indexOf("async function submitBatch(");
    const batchEnd = host.indexOf("/// 重复已经问过了", batchStart);
    const grabStart = host.indexOf("const grabNames = new Map()");
    const grabEnd = host.indexOf("// ---------- 通用确认框", grabStart);
    const dialogs = [];
    const shown = requests[0].body.links.map(link => ({ ...link, kind: "轻雀文档" }));
    const confirm = vm.createContext({
      extInfo: new Map(), state: { engineRunning: true, settings: {} },
      BAIDU_SHARE: /^https:\/\/pan\.baidu\.com\/s\//, isFeishuDocument: () => false,
      invoke: async () => null, collectionFolderName: name => name,
      askConfirm: async (url, opts) => dialogs.push({ url, opts }),
    });
    const hfSource = host.slice(host.indexOf("function isHuggingFaceFilePage("), host.indexOf("async function runBatch("));
    vm.runInContext(hfSource + host.slice(batchStart, batchEnd) + host.slice(grabStart, grabEnd), confirm);
    await confirm.openGrab(shown, { folderName: requests[0].body.folderName, sourceUrl: page });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(dialogs.length, 1);
    assert.equal(dialogs[0].opts.folderName, documentName);
    assert.equal(dialogs[0].opts.sourceUrl, page);
    assert.equal(dialogs[0].opts.kinds[0], "轻雀文档");
    assert.deepEqual(Array.from(dialogs[0].opts.batch, file => file.name), [first, second]);

    const hostStart = host.indexOf("function isHuggingFaceFilePage(");
    const hostEnd = host.indexOf("/// 投一条链接", hostStart);
    const jobs = [];
    const links = requests[0].body.links;
    const app = vm.createContext({
      extInfo: new Map(links.map(link => [link.url, link])),
      multiCommonHead: () => "",
      invoke: async (command, args) => {
        jobs.push({ command, args });
        return "new-batch";
      },
      settleMulti: async () => {}, switchView: () => {}, refreshTasks: () => {}, toast: () => {},
    });
    vm.runInContext(host.slice(hostStart, hostEnd), app);
    const dir = root === "." ? "C:\\Downloads" : "/video";
    await app.runBatch(links, dir, null, null, "轻雀文档", {});
    assert.equal(jobs.length, 1);
    assert.equal(jobs[0].command, "add_multi");
    assert.equal(jobs[0].args.dir, dir);
    assert.deepEqual(Array.from(jobs[0].args.files, file => file.name), [first, second]);

    context.performance.getEntriesByType = () => [];
    assert.equal((await context.getQingqueDocumentFiles(page)).items.length, 3);
    assert.deepEqual(reads.slice(-2), ["/word/api/load/" + docId, "/word/api/snapshot/" + docId + "/6"]);
  });
}
