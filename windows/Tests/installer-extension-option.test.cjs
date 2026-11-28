const fs = require('node:fs'), vm = require('node:vm'), assert = require('node:assert/strict'), path = require('node:path');
const base = path.resolve(__dirname, '..');
for (const root of [base].filter(r => fs.existsSync(path.join(r, 'installer/app/ui/index.html')))) {
  const source = fs.readFileSync(path.join(root, 'installer/app/ui/index.html'), 'utf8').replace(/\r\n/g, '\n');
  const a = source.indexOf('function result('), b = source.indexOf('\n}\n', a) + 3;
  for (const [installedVersion, extensionChanged, openRow, visible] of [['0.1.23', false, true, false], ['0.1.23', true, true, true], [null, false, true, true], ['0.1.23', true, false, false]]) {
    const elements = new Map(), get = id => { if (!elements.has(id)) elements.set(id, { classList: { toggle() {} }, appendChild() {} }); return elements.get(id); };
    const c = vm.createContext({ $: get, info: { installedVersion }, OK_ICON: '', BAD_ICON: '', show() {} });
    vm.runInContext(source.slice(a, b), c);
    c.result({ ok: true, title: '', text: '', buttons: [], openRow, extensionChanged, extensionFrom:installedVersion?'1.15.35':null,extensionTo:'1.15.36' });
    assert.equal(get('r-ext-row').hidden, !visible); assert.equal(get('r-ext').checked, visible);
  }
}
console.log('安装扩展选项通过：同内容升级隐藏、扩展变化和全新安装显示、失败页不显示');
