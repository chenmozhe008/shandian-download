const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('browser-extension/popup.js', 'utf8');
async function run(url, reply) {
  const element = () => ({
    textContent: '', style: {}, children: [],
    appendChild(child) { this.children.push(child); },
    replaceChildren() { this.children = []; }
  });
  const elements = Object.fromEntries(['title', 'dl', 'hint', 'takeover', 'scan', 'capture', 'capture-note', 'results', 'batch-actions', 'wechat', 'wechat-toggle', 'wechat-link', 'wechat-download', 'feishu-heading', 'feishu-rescan', 'feishu-selected', 'media-batch-actions', 'media-select-all', 'media-download-selected'].map(id => [id, element()]));
  let request;
  const chrome = { tabs: { query: async () => [{ id: 5, url, title: '页面标题' }] }, runtime: { sendMessage: async msg => { request = msg; return typeof reply === 'function' ? reply(msg) : reply; } }, storage: { local: { get: async () => ({}) } } };
  const page = { querySelector: key => elements[key.slice(1)] };
  const document = { getElementById: id => id === 'page' ? page : elements[id], createElement: element, body: { classList: { add() {}, remove() {} } } };
  vm.runInNewContext(source, { chrome, document, URL, setTimeout() {}, window: {} });
  await new Promise(resolve => setImmediate(resolve));
  return { elements, click: async (id = 'dl') => { await elements[id].onclick(); return request; } };
}
(async () => {
  let p = await run('https://movie.douban.com/trailer/326598/#content', { ok: true });
  assert.equal((await p.click()).requireVideo, false);
  assert.match(p.elements.dl.textContent, /已交给/);
  p = await run('https://example.com/article', { ok: false, noVideo: true });
  assert.equal((await p.click()).requireVideo, true);
  assert.match(p.elements.hint.textContent, /没有找到/);
  assert.equal(p.elements.dl.disabled, false);
  p = await run('edge://settings', {});
  assert.equal(p.elements.dl.style.display, 'none');
  p = await run('https://yuanbao.tencent.com/', { ok: true });
  assert.equal(p.elements['wechat-toggle'].hidden, false);
  p.elements.wechat.hidden = true;
  p.elements['wechat-toggle'].onclick();
  assert.equal(p.elements.wechat.hidden, false);
  p.elements['wechat-toggle'].onclick();
  assert.equal(p.elements.wechat.hidden, true);
  p.elements['wechat-toggle'].onclick();
  p.elements['wechat-link'].value = 'https://weixin.qq.com/sph/AnYC9T2a4d';
  await p.elements['wechat-download'].onclick();
  assert.match(p.elements.hint.textContent, /已交给/);
  p.elements['wechat-link'].value = 'https://example.com/sph/test';
  await p.elements['wechat-download'].onclick();
  assert.match(p.elements.hint.textContent, /完整的/);
  const first = { url: 'https://example.com/first.mp4', title: '第一段', kind: '视频', ext: 'mp4' };
  const second = { url: 'https://example.com/second.mp4', title: '第二段', kind: '视频', ext: 'mp4' };
  const items = [first, second];
  const labels = popup => popup.elements.results.children.map(card => {
    const info = card.children[0].children.find(child => child.className === 'media-info');
    return info.children.map(child => child.textContent);
  });
  p = await run('https://example.com/article', ({ type }) => type === 'scanMedia' ? { items } : { ok: true });
  assert.deepEqual(labels(p).map(([name]) => name), ['第一段', '第二段']);
  assert.doesNotMatch(labels(p)[0][1], /最新/);
  assert.equal((await p.click()).item, first);
  p = await run('https://example.com/article', ({ type }) => type === 'captureStatus' ? { active: true, items } : type === 'scanMedia' ? { items } : { ok: true });
  assert.deepEqual(labels(p).map(([name]) => name), ['第二段', '第一段']);
  assert.match(labels(p)[0][1], /最新/);
  assert.equal((await p.click()).item, second);
  await p.elements.scan.onclick();
  assert.deepEqual(labels(p).map(([name]) => name), ['第一段', '第二段']);
  assert.equal((await p.click()).item, first);
  const feishu = 'https://example.feishu.cn/wiki/TestDoc';
  const files = Array.from({ length: 24 }, (_, i) => ({ id: String(i), name: `视频 ${i + 1}.mp4`, size: 1024 }));
  p = await run(feishu, ({ type }) => type === 'scanFeishuMedia'
    ? { ok: true, title: '飞书文档标题', items: files }
    : { ok: true });
  assert.equal(p.elements.title.textContent, '飞书文档标题');
  assert.equal(p.elements.dl.hidden, true);
  assert.equal(p.elements.results.children.length, 24);
  assert.equal(p.elements['feishu-selected'].textContent, '已选 24 / 24');
  assert.equal(p.elements['media-download-selected'].textContent, '下载到文件夹');
  const selectAll = p.elements['media-select-all'];
  selectAll.checked = false;
  selectAll.onchange();
  assert.equal(p.elements['media-download-selected'].disabled, true);
  assert.equal(p.elements['feishu-selected'].textContent, '已选 0 / 24');
  const checked = p.elements.results.children[23].children[0];
  checked.checked = true;
  checked.onchange();
  assert.equal(selectAll.indeterminate, true);
  await p.elements['feishu-rescan'].onclick();
  const lastRow = p.elements.results.children[23];
  assert.equal(lastRow.children[1].textContent, '视频 24.mp4');
  assert.equal(lastRow.children[2].textContent, '1 KB');
  assert.equal(p.elements['feishu-selected'].textContent, '已选 1 / 24');
  assert.equal(p.elements['media-download-selected'].textContent, '下载原文件');
  const download = await p.click('media-download-selected');
  assert.equal(download.type, 'downloadFeishuMedia');
  assert.equal(download.tabId, 5);
  assert.deepEqual(Array.from(download.ids), ['23']);
  assert.equal(p.elements['media-download-selected'].disabled, true);
  console.log('扩展弹窗测试通过：豆瓣、普通网页无视频、浏览器内部页面');
})().catch(error => { console.error(error); process.exitCode = 1; });
