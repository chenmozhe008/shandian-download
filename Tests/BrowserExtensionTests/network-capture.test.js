const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const code = fs.readFileSync('browser-extension/network-capture.js', 'utf8');
const listeners = () => ({ cb: null, addListener(fn) { this.cb = fn; }, removeListener() { this.cb = null; } });
const memory = {};
let granted = false;
const response = listeners(), alarm = listeners(), removed = listeners(), updated = listeners();
const badges = new Map();
const chrome = {
  webRequest: { onHeadersReceived: response },
  action: {
    setBadgeText({ tabId, text }) { badges.set(tabId, text); },
    setBadgeBackgroundColor() {}
  },
  storage: { session: { async get(key) { return { [key]: memory[key] }; }, async set(obj) { Object.assign(memory, obj); }, async remove(key) { delete memory[key]; } } },
  permissions: { async contains() { return granted; }, async remove() { granted = false; return true; } },
  runtime: { onStartup: listeners() },
  alarms: { onAlarm: alarm, async create() {}, async clear() {} },
  tabs: { onRemoved: removed, onUpdated: updated }
};
const ctx = { chrome, URL, Date, console }; vm.createContext(ctx); vm.runInContext(code, ctx);
(async () => {
  assert.equal(await ctx.startNetworkCapture(1, 'https://example.org/page', '示例'), false);
  granted = true;
  assert.equal(await ctx.startNetworkCapture(1, 'https://example.org/page', '示例'), true);
  assert.ok(response.cb);
  const event = (tabId, url, mime, extraHeaders = []) => ({ tabId, url, method: 'GET', statusCode: 200, responseHeaders: [{ name: 'Content-Type', value: mime }, ...extraHeaders] });
  response.cb(event(2, 'https://cdn.example/private', 'video/mp4'));
  response.cb(event(1, 'https://cdn.example/chunk.m4s', 'video/mp4'));
  response.cb(event(1, 'https://cdn.example/no-extension?sig=1', 'application/vnd.apple.mpegurl'));
  response.cb(event(1, 'https://cdn.example/movie', 'video/mp4'));
  response.cb(event(1, 'https://cdn.example/movie', 'video/mp4'));
  response.cb(event(1, 'https://cdn.example/sfx-rain.mp3?token=abc', 'application/octet-stream', [{ name: 'Content-Length', value: '204800' }]));
  response.cb(event(1, 'https://cdn.example/stream/play?id=9', 'audio/mp3', [{ name: 'Content-Disposition', value: 'inline; filename="thunder.mp3"' }, { name: 'Content-Range', value: 'bytes 0-1023/512000' }]));
  response.cb(event(1, 'https://cdn.example/a1b2c3d4e5f67890.wav', 'audio/x-wav'));
  let result = await ctx.capturedForTab(1);
  assert.equal(result.active, true); assert.equal(result.items.length, 5);
  assert.equal(badges.get(1), '5');
  assert.equal(result.items[0].stream, true); assert.equal(result.items[0].ext, 'm3u8');
  assert.equal(result.items[1].stream, false); assert.equal(result.items[1].ext, 'mp4');
  assert.equal(result.items[2].kind, '音频'); assert.equal(result.items[2].ext, 'mp3'); assert.equal(result.items[2].size, 204800); assert.equal(result.items[2].filename, 'sfx-rain');
  assert.equal(result.items[3].kind, '音频'); assert.equal(result.items[3].ext, 'mp3'); assert.equal(result.items[3].size, 512000); assert.equal(result.items[3].filename, 'thunder');
  assert.equal(result.items[4].kind, '音频'); assert.equal(result.items[4].ext, 'wav'); assert.equal(result.items[4].filename, undefined);
  assert.equal(result.items[0].referrer, 'https://example.org/page');
  assert.equal((await ctx.capturedForTab(2)).items.length, 0);
  await ctx.stopNetworkCapture();
  assert.equal(badges.get(1), '');
  assert.equal(response.cb, null); assert.equal(granted, false);
  result = await ctx.capturedForTab(1); assert.equal(result.active, false); assert.equal(result.items.length, 0);
  granted = true;
  assert.equal(await ctx.startNetworkCapture(1, 'https://example.org/page?private=1#section', '示例'), true);
  response.cb(event(1, 'https://cdn.example/manifest', 'application/dash+xml'));
  assert.equal((await ctx.capturedForTab(1)).items[0].referrer, 'https://example.org/page');
  vm.runInContext('captureState.expires = Date.now() - 1', ctx);
  assert.equal((await ctx.capturedForTab(1)).active, false);
  assert.equal(granted, false); assert.equal(response.cb, null);
  console.log('网络侦测测试通过：权限、当前标签、媒体识别、去重、停止及清除');
})().catch(e => { console.error(e); process.exitCode = 1; });
