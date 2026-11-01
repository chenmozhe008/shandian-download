const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('browser-extension/background.js', 'utf8');
const start = source.indexOf('function grabPikPakSession()');
const end = source.indexOf('// PikPak 网页一打开', start);
const linkStart = source.indexOf('async function linkPikPak(');
const code = source.slice(start, end) + '\n' + source.slice(linkStart);
async function run(accept) {
  let stored = JSON.stringify({access_token:'test-access',refresh_token:'test-refresh'});
  const localStorage = { getItem: key => key === 'deviceid' ? 'device' : stored, removeItem: () => { stored = null; }, credentials_test: true };
  const document = {title: ''};
  const ctx = {localStorage, document, alert: () => {}, setTimeout, fetch: async () => ({ok:accept, json:async()=>({ok:accept})})};
  ctx.chrome = {scripting: {executeScript: async ({func,args=[]}) => [{result:func(...args)}]}};
  vm.createContext(ctx);
  vm.runInContext(code, ctx);
  const result = await ctx.linkPikPak(1);
  assert.equal(result, accept);
  assert.equal(stored === null, accept);
}
Promise.all([run(true),run(false)]).then(() => console.log('PikPak 关联成功与失败保留会话测试通过')).catch(e => { console.error(e); process.exit(1); });
