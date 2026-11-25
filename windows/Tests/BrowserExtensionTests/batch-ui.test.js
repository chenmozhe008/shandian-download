const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const nodes = {};
function element() {
  return { style: {}, children: [], append(...xs) { this.children.push(...xs); }, appendChild(x) { this.children.push(x); }, replaceChildren() { this.children = []; } };
}
for (const id of ['batch', 'batch-actions', 'results', 'select-all', 'download-selected', 'hint', 'site-batch-format-row', 'site-batch-format']) nodes[id] = element();
nodes['site-batch-format'].value='mp3';
const document = { getElementById: id => nodes[id], createElement: element, body: { classList: { add() {} } } };
const chrome = { tabs: { query: async () => [{ id: 1, url: 'https://example.com/' }] }, runtime: { sendMessage: async message => message.type === 'scanLinks' ? { items: [{ url: 'magnet:?xt=urn:btih:123', title: '第01集', kind: '磁力' },{url:'aigei:paid',title:'森林鸟叫',kind:'爱给',blocked:true,blockedReason:'需购买授权'},{url:'aigei:mixed',title:'不同格式权限',kind:'爱给',formats:{mp3:{blocked:false},wav:{blocked:true,blockedReason:'需购买授权'}}}], checked: true } : { ok: true } } };
vm.runInNewContext(fs.readFileSync('windows/browser-extension/source-labels.js','utf8')+'\n'+fs.readFileSync('windows/browser-extension/batch-ui.js', 'utf8'), { document, chrome, URL });
setImmediate(async () => {
  await nodes.batch.onclick();
  const check = nodes.results.children[0].children[0];
  assert.equal(check.checked, true);
  const paid=nodes.results.children[1];assert.equal(paid.children[0].checked,false);assert.equal(paid.children[0].disabled,true);assert.equal(paid.children[1].children[1].textContent,'需购买授权');
  const mixed=nodes.results.children[2];assert.equal(mixed.children[0].disabled,false);
  nodes['site-batch-format'].value='wav';nodes['site-batch-format'].onchange();assert.equal(mixed.children[0].disabled,true);assert.equal(mixed.children[1].children[1].textContent,'需购买授权');
  nodes['site-batch-format'].value='mp3';nodes['site-batch-format'].onchange();assert.equal(mixed.children[0].disabled,false);assert.equal(mixed.children[0].checked,true);
  await nodes['download-selected'].onclick();
  assert.equal(check.disabled, false, '交接不应假定用户已确认任务');
  assert.equal(check.checked, false);
  assert.match(nodes.hint.textContent, /确认任务/);
  console.log('批量交接测试通过：提示确认，取消后可以重新选择');
});
