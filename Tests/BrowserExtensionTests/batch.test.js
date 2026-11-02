const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const code = fs.readFileSync('browser-extension/background.js', 'utf8');
const fragment = code.slice(code.indexOf('function collectDownloadLinks()'), code.indexOf('async function extensionBatchRequest'));
const hash = '1234567890123456789012345678901234567890';
const anchor = (href, title) => ({ href, textContent: title, getAttribute: () => '' });
const anchors = [
  anchor('magnet:?xt=urn:btih:' + hash + '&dn=episode1', '第一集'),
  anchor('magnet:?xt=urn:btih:' + hash + '&tr=tracker', '同一集'),
  anchor('https://example.org/file.torrent?download=1', '种子'),
  anchor('https://example.org/archive.zip', '压缩包'),
  anchor('https://example.org/article', '普通网页'),
  anchor('javascript:alert(1)', '脚本')
];
const context = { URL, location: { href: 'https://example.org/page' }, document: { querySelectorAll: () => anchors }, chrome: { scripting: { executeScript: async () => [{ result: context.collectDownloadLinks() }, { result: context.collectDownloadLinks() }] } } };
vm.createContext(context); vm.runInContext(fragment, context);
(async () => {
  const results = await context.scanDownloadLinks(7);
  assert.equal(results.length, 3);
  assert.deepEqual(Array.from(results, x => x.kind), ['磁力', '种子', '直链']);
  assert.equal(results[0].title, '第一集');
  console.log('批量抓取测试通过：类型筛选、磁力去重、普通网页排除');
})().catch(error => { console.error(error); process.exitCode = 1; });
