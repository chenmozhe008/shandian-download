const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
async function run(url, reply, formats = [], root = '.') {
  const source = ['cloud-sites.js', 'huggingface.js', 'popup.js', 'baidu-ui.js']
    .map(file => fs.readFileSync(root + '/windows/browser-extension/' + file, 'utf8')).join('\n');
  const element = () => ({ textContent: '', value: '', children: [],
    append(child) { this.children.push(child); },
    contains(child) { return this.children.includes(child); },
    after() {}, replaceChildren() { this.children.length = 0; }, appendChild(child) { this.append(child); },
    style: {}, firstElementChild: { textContent: '' },
    classList: { toggle() {}, replace() {} }, addEventListener() {} });
  const elements = new Proxy({}, { get: (map, id) => map[id] ||= element() });
  const tools = elements['download-tools'], summary = element();
  tools.append(summary);
  tools.querySelector = () => summary;
  summary.after = child => tools.children.splice(1, 0, child);
  elements.dl.parentElement = element();
  for (const id of ['batch', 'more', 'download-tools', 'capture']) elements[id].hidden = true;
  elements['baidu-open'].hidden = true;
  const requests = [];
  let request;
  const chrome = { tabs: { query: async () => [{ id: 5, url, title: '页面标题' }] }, runtime: { sendMessage: async msg => {
    request = msg;
    requests.push(msg.type);
    if (msg.type === 'wechatFormats') return { formats };
    if (msg.type === 'cloudAccountLinkContext') return { ok: true, inactive: true, target: 'local' };
    if (msg.type === 'baiduAuth') return { ok: true, authed: true };
    return reply;
  } }, permissions: { request: async () => true, remove: async () => true },
    storage: { local: { get: async () => ({}), set: async () => {} } } };
  const page = { querySelector: key => elements[key.slice(1)] };
  const document = { body: element(), createElement: element, addEventListener() {}, querySelectorAll: () => [], getElementById: id => id === 'page' ? page : elements[id] };
  vm.runInNewContext(source, { chrome, document, URL, setTimeout() {}, window: {} });
  await new Promise(resolve => setImmediate(resolve));
  return { elements, requests, click: async () => { await elements.dl.onclick(); return request; } };
}
(async () => {
  let p = await run('https://movie.douban.com/trailer/326598/#content', { ok: true });
  assert.equal(p.elements['download-tools'].contains(p.elements.more), true);
  assert.equal(p.elements['download-tools'].contains(p.elements.batch), true);
  assert.equal((await p.click()).requireVideo, false);
  assert.match(p.elements.dl.textContent, /已交给/);
  p = await run('https://example.com/article', { ok: false, noVideo: true });
  assert.equal(p.elements.batch.hidden, false);
  assert.equal(p.elements.more.hidden, false);
  assert.equal(p.elements['download-tools'].hidden, true);
  assert.equal(p.elements.capture.hidden, true);
  const file = {};
  p.elements.results.append(file);
  await p.elements.scan.onclick();
  assert.equal(p.elements.capture.hidden, false);
  assert.deepEqual(p.elements.results.children, [file]);
  assert.match(p.elements['media-hint'].textContent, /暂未找到/);
  assert.equal((await p.click()).requireVideo, true);
  assert.match(p.elements.hint.textContent, /没(?:有)?找到/);
  assert.equal(p.elements.dl.disabled, false);
  p = await run('https://mp.weixin.qq.com/s/article', { ok: true }, [{ value: '854x480', label: '480p' }]);
  assert.equal(p.elements['mp-quality-row'].hidden, false);
  p.elements['mp-quality'].value = '854x480';
  assert.equal((await p.click()).quality, '854x480');
  p = await run('https://channels.weixin.qq.com/finder-preview/pages/sph?id=preview123', { ok: false, login: true, error: '请在腾讯元宝登录后继续下载' });
  assert.notEqual(p.elements.dl.hidden, true);
  assert.equal(p.elements.dl.textContent, '闪电下载');
  assert.match(p.elements.hint.textContent, /腾讯元宝/);
  assert.equal((await p.click()).requireVideo, false);
  assert.ok(p.elements.hint.textContent);
  p = await run('https://channels.weixin.qq.com/finder-preview/pages/sph?id=preview123', { ok: true, pending: true });
  await p.click(); assert.match(p.elements.hint.textContent, /确认名称和保存位置/);
  p = await run('https://mp.weixin.qq.com/s/article', { ok: true });
  assert.notEqual(p.elements.dl.hidden, true);
  assert.equal((await p.click()).requireVideo, true);
  p = await run('edge://settings', {});
  assert.equal(p.elements.dl.style.display, 'none');
  assert.equal(p.elements.more.hidden, true);
  assert.equal(p.elements['download-tools'].hidden, true);
  for (const url of ['https://pan.baidu.com/disk/main', 'https://www.alipan.com/drive/file/all', 'https://pan.quark.cn/']) {
    p = await run(url, {ok:true});
    assert.equal(p.elements.batch.hidden, true);
    assert.equal(p.elements.more.hidden, true);
  }
  assert.equal(p.elements.dl.textContent, '选择网盘文件下载');
  assert.equal((await p.click()).type, 'quarkSelected');
  for (const root of ['.']) {
    for (const path of ['/s/example?pwd=test', '/share/init?surl=example', '/share/link/?shareid=1&uk=2']) {
      p = await run('https://pan.baidu.com' + path, { ok: true }, [], root);
      assert.equal(p.elements['baidu-open'].hidden, true);
      assert.notEqual(p.elements.dl.hidden, true);
      assert.equal(p.elements.dl.textContent, '闪电下载');
      assert.equal(p.requests.includes('baiduAuth'), false);
      assert.equal((await p.click()).type, 'baiduShareDownload');
    }
    p = await run('https://pan.baidu.com/disk/main', { ok: true }, [], root);
    assert.equal(p.elements.dl.hidden, true);
    assert.equal(p.elements['baidu-open'].hidden, false);
    assert.equal(p.elements['baidu-open'].textContent, '选择网盘文件下载');
  }
  p = await run('https://www.alipan.com/drive/file/all', { ok: true, sent: 1 });
  assert.equal(p.elements.dl.textContent, '选择网盘文件下载');
  assert.equal((await p.click()).type, 'aliSelected');
  p = await run('https://115.com/?mode=wangpan', { ok: true });
  assert.equal(p.elements.dl.hidden, true);
  for (const url of ['https://www.123pan.com/', 'https://cloud.189.cn/web/main/', 'https://yun.139.com/w/', 'https://pan.xunlei.com/', 'https://www.guangyapan.com/']) {
    p = await run(url, {ok:true,sent:2});
    assert.equal(p.elements.batch.hidden, true);
    assert.equal(p.elements['download-tools'].hidden, true);
    assert.equal(p.elements.dl.textContent, '选择网盘文件下载');
    assert.equal((await p.click()).type, 'extraCloudSelected');
    assert.match(p.elements.hint.textContent, /确认文件/);
    assert.equal(p.elements.capture.hidden, true);
  }
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
  for (const url of ['https://pixabay.com/music/search/cinematic/', 'https://docs.qingque.cn/d/home/example']) {
    p = await run(url, {});
    assert.equal(p.elements.batch.hidden, false);
    assert.equal(p.elements['download-tools'].contains(p.elements.batch), false);
    assert.equal(p.elements['download-tools'].contains(p.elements.more), true);
  }
  p = await run('https://my.feishu.cn/wiki/Example', {});
  assert.equal(p.elements['download-tools'].contains(p.elements.batch), true);
  assert.equal(p.elements['download-tools'].contains(p.elements.more), true);
  console.log('扩展弹窗测试通过：视频网站、普通网页、网盘、文档站、Pixabay、视频号和浏览器内部页面');
})().catch(error => { console.error(error); process.exitCode = 1; });
