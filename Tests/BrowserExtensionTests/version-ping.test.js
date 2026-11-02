const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const code = fs.readFileSync('browser-extension/background.js', 'utf8');
const ping = code.slice(code.indexOf('async function ping()'), code.indexOf('chrome.alarms.create("shandian-ping"'));
let calls = 0, reloads = 0;
const context = {
  PING: 'http://127.0.0.1:6800/extension/ping',
  fetch: async () => { calls++; return { json: async () => ({ version: '99.0.0' }) }; },
  chrome: { runtime: { reload: () => reloads++, getManifest: () => ({ version: '1.0.0' }) } }
};
vm.createContext(context); vm.runInContext(ping, context);
(async () => {
  await context.ping(); await context.ping();
  assert.equal(calls, 2);
  assert.equal(reloads, 0, '版本不一致不能自动触发重新加载循环');
  context.fetch = async () => { throw new Error('offline'); };
  await context.ping();
  assert.equal(reloads, 0);
  console.log('扩展连接测试通过：版本不一致及应用离线均不会循环重载');
})().catch(err => { console.error(err); process.exitCode = 1; });
