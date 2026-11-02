const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const nodes = {};
function element() {
  return { style: {}, children: [], append(...xs) { this.children.push(...xs); }, appendChild(x) { this.children.push(x); }, replaceChildren() { this.children = []; } };
}
for (const id of ['batch', 'batch-actions', 'results', 'select-all', 'download-selected', 'hint']) nodes[id] = element();
const document = { getElementById: id => nodes[id], createElement: element, body: { classList: { add() {} } } };
const chrome = { tabs: { query: async () => [{ id: 1, url: 'https://example.com/' }] }, runtime: { sendMessage: async message => message.type === 'scanLinks' ? { items: [{ url: 'magnet:?xt=urn:btih:123', title: '第01集', kind: '磁力' }], checked: true } : { ok: true } } };
vm.runInNewContext(fs.readFileSync('browser-extension/batch-ui.js', 'utf8'), { document, chrome, URL });
setImmediate(async () => {
  await nodes.batch.onclick();
  const check = nodes.results.children[0].children[0];
  assert.equal(check.checked, true);
  await nodes['download-selected'].onclick();
  assert.equal(check.disabled, false, '交接不应假定用户已确认任务');
  assert.equal(check.checked, false);
  assert.match(nodes.hint.textContent, /确认任务/);
  console.log('批量交接测试通过：提示确认，取消后可以重新选择');
});
