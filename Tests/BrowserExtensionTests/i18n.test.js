const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('browser-extension/i18n.js', 'utf8');
function load(language) {
  const context = { chrome: { i18n: { getUILanguage: () => language } } };
  context.globalThis = context;
  vm.runInNewContext(source + ';globalThis.T = T; globalThis.EN = SHANDIAN_EN;', context);
  return context;
}
const zh = load('zh-CN'), en = load('en-US');
assert.equal(zh.T('下载这个视频'), '下载这个视频');
assert.equal(en.T('下载这个视频'), 'Download This Video');
assert.equal(en.T('下载所选（{n}）', { n: 3 }), 'Download Selected (3)');
assert.equal(zh.T('下载所选（{n}）', { n: 3 }), '下载所选（3）');
// 代码里每条 T("…") 和网页里每段中文都要有英文
const han = /[一-鿿]/;
for (const file of ['popup.js', 'batch-ui.js', 'background.js', 'network-capture.js']) {
  const text = fs.readFileSync('browser-extension/' + file, 'utf8');
  for (const match of text.matchAll(/\bT\((["'])(.*?)\1/g)) if (han.test(match[2])) assert.ok(en.EN[match[2]], file + ': ' + match[2]);
}
const html = fs.readFileSync('browser-extension/popup.html', 'utf8').replace(/<style[\s\S]*?<\/style>/, '');
for (const match of html.matchAll(/>([^<>]*[一-鿿][^<>]*)</g)) assert.ok(en.EN[match[1].trim()], match[1]);
for (const match of html.matchAll(/(?:title|placeholder)="([^"]*[一-鿿][^"]*)"/g)) assert.ok(en.EN[match[1]], match[1]);
console.log('i18n ok');
