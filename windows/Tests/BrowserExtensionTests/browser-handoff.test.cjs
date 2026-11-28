const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const test = require("node:test");

function fixture(root, { states = [], ready = true, accepted = true, resumeFails = false } = {}) {
  const source = fs.readFileSync(root + "/browser-extension/background.js", "utf8");
  const recovery = source.slice(source.indexOf("function githubRecoveryDownload("), source.indexOf("// tabId：传了就表示"));
  const handoff = source.slice(source.indexOf("const finishingBrowserHandoffs"), source.indexOf("// 交给 App 后盯"));
  const activity = [];
  const tasks = new Map();
  const local = Object.create(null);
  const alarms = new Set();
  const makeStorage = data => ({
    get: async key => key ? { [key]: data[key] } : { ...data },
    set: async values => Object.assign(data, values),
    remove: async key => { delete data[key]; },
  });
  const session = Object.create(null);
  const handlers = {};
  const context = vm.createContext({
    URL, AbortSignal,
    crypto: { randomUUID: () => "11223344-5566-4788-99aa-bbccddeeff00" },
    PING: "ping",
    BROWSER_HANDOFF: "handoff",
    setTimeout: queueMicrotask,
    knownFileBytes: size => size,
    feishuAttachmentDownload: () => false,
    sendToApp: async (...args) => {
      activity.push("send");
      assert.equal(args[6].browserHandoffId, "112233445566478899aabbccddeeff00");
      return accepted;
    },
    fetch: async (url, opts) => {
      if (url === "ping") return { ok: true, json: async () => ({ ready: true, browserHandoff: ready }) };
      const data = JSON.parse(opts.body);
      if (data.showRetryHint) {
        activity.push("retry-hint:" + data.browserNext);
        return { ok: true, json: async () => ({}) };
      }
      const next = states.shift() || "released";
      activity.push("state:" + next);
      return { ok: true, json: async () => ({ state: next }) };
    },
    chrome: {
      action: { setTitle: async () => {} },
      runtime: {
        onStartup: { addListener: fn => { handlers.restart = fn; } },
        getPlatformInfo: async () => ({}),
      },
      storage: { local: makeStorage(local), session: makeStorage(session) },
      alarms: {
        onAlarm: { addListener: fn => { handlers.alarm = fn; } },
        create: async key => alarms.add(key),
        clear: async key => alarms.delete(key),
      },
      downloads: {
        onChanged: { addListener: fn => { handlers.changed = fn; } },
        search: async ({ id }) => tasks.has(id) ? [{ ...tasks.get(id) }] : [],
        pause: async id => { activity.push("pause"); tasks.get(id).paused = true; },
        resume: async id => {
          activity.push("resume");
          if (resumeFails) throw Error("expired");
          tasks.get(id).paused = false;
        },
        cancel: async id => { activity.push("cancel"); tasks.delete(id); },
        erase: async () => { activity.push("erase"); },
      },
    },
  });
  vm.runInContext(recovery + handoff, context);
  const item = { id: 1, state: "in_progress", url: "https://files.example.test/single-use?token=old", referrer: "https://example.test/exports", totalBytes: 1234 };
  tasks.set(item.id, { ...item });
  return { context, activity, item, local, tasks, handlers, alarms, session };
}

for (const root of ["."]) {
  test(root + " cancels the original only after the new task receives data", async () => {
    const f = fixture(root, { states: ["pending", "waiting", "received"] });
    await f.context.startBrowserHandoff(f.item, f.item.url);
    assert.deepEqual(f.activity, ["pause", "send", "state:pending", "state:waiting", "state:received", "cancel", "erase"]);
    assert.equal(f.alarms.size, 0);
    assert.deepEqual(Object.keys(f.local), []);
  });

  test(root + " resumes the same browser item after a failed handoff", async () => {
    const f = fixture(root, { states: ["failed"] });
    await f.context.startBrowserHandoff(f.item, f.item.url);
    assert.deepEqual(f.activity, ["pause", "send", "state:failed", "resume"]);
    assert.equal(f.tasks.get(f.item.id).paused, false);
  });

  test(root + " guides a retry to the browser once if the original expired", async () => {
    const f = fixture(root, { states: ["failed"], resumeFails: true });
    await f.context.startBrowserHandoff(f.item, f.item.url);
    assert.deepEqual(f.activity, ["pause", "send", "state:failed", "resume", "retry-hint:true"]);
    const fresh = { ...f.item, url: "https://files.example.test/single-use?token=fresh" };
    assert.equal(await f.context.useBrowserForRetry({ ...fresh, referrer: "https://another.example.test/" }), false);
    assert.equal(await f.context.useBrowserForRetry(fresh), true);
    assert.equal(await f.context.useBrowserForRetry(fresh), false);
  });

  test(root + " guides a browser retry when a resumed download fails asynchronously", async () => {
    const f = fixture(root, { states: ["failed"] });
    await f.context.startBrowserHandoff(f.item, f.item.url);
    const delta = { id: f.item.id, state: { current: "interrupted" }, error: { current: "SERVER_BAD_CONTENT" } };
    await f.handlers.changed(delta);
    await f.handlers.changed(delta);
    assert.deepEqual(f.activity, ["pause", "send", "state:failed", "resume", "retry-hint:true"]);
  });

  test(root + " leaves older App versions and browser-held items alone", async () => {
    const f = fixture(root, { ready: false });
    await f.context.startBrowserHandoff(f.item, f.item.url);
    await f.context.startBrowserHandoff({ ...f.item, paused: true }, f.item.url);
    assert.deepEqual(f.activity, []);
  });

  test(root + " recovers a held original after the extension restarts", async () => {
    const f = fixture(root, { states: ["starting", "released"] });
    const key = "shandian-browser-handoff:1";
    f.tasks.get(1).paused = true;
    f.local[key] = { id: 1, token: "112233445566478899aabbccddeeff00" };
    await f.handlers.restart();
    assert.deepEqual(f.activity, ["state:starting"]);
    assert(f.alarms.has(key));
    await f.context.recoverBrowserHandoff(key);
    assert.deepEqual(f.activity, ["state:starting", "state:released", "resume"]);
    assert.deepEqual(Object.keys(f.local), []);
  });
}
