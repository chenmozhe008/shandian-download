const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const code = fs.readFileSync('browser-extension/background.js', 'utf8');
const fragment = code.slice(code.indexOf('const SMALL ='), code.indexOf('chrome.contextMenus.onClicked'));
let listener, sent = 0, cancelled = 0;
const context = {
  URL, Date, Map, Set, setTimeout: () => {},
  sendToApp: async () => { sent++; return true; },
  enabled: async () => true,
  chrome: {
    runtime: { id: 'self', getPlatformInfo: async () => ({}) },
    downloads: { onCreated: { addListener: fn => { listener = fn; } }, cancel: async () => { cancelled++; }, erase: async () => {}, download: () => {} },
  },
  fetch: async () => ({ json: async () => ({ state: 'none' }) }),
};
vm.createContext(context);
vm.runInContext(fragment, context);
const wallpaper = { id: 1, url: 'https://cdn.example/img/MzA1Njk5NTc0MjYwNjk1MTE1MDMxNTI4Nw==', mime: '', totalBytes: 0 };
(async () => {
  // 别的扩展在后台发起、并反复重试同一个下载：一次都不接
  for (let i = 0; i < 30; i++) await listener({ ...wallpaper, byExtensionId: 'infinity' });
  assert.equal(sent, 0);
  // 扩展页面里触发的下载也不接
  await listener({ ...wallpaper, referrer: 'chrome-extension://infinity/newtab.html' });
  assert.equal(sent, 0);
  // 网页上的同一个链接被反复创建：2 分钟内只接第一次，之后交给浏览器自己下
  for (let i = 0; i < 30; i++) await listener({ ...wallpaper, referrer: 'https://example.com/' });
  assert.equal(sent, 1);
  assert.equal(cancelled, 1);
  // 用户正常点的另一个下载照常接
  await listener({ id: 2, url: 'https://example.com/file.zip', mime: 'application/zip', totalBytes: 50e6, referrer: 'https://example.com/' });
  assert.equal(sent, 2);
  console.log('接管防循环测试通过：别的扩展发起的不接，同一链接 2 分钟内只接一次');
})().catch(e => { console.error(e); process.exitCode = 1; });
