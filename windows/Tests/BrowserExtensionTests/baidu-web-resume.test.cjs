const {test} = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");

const token = "b".repeat(32);
const originalId = "1234567890123456789";
const digest = "900150983cd24fb0d6963f7d28e17f72";
const tab = {id: 7, url: "https://pan.baidu.com/disk/main?shandian-refresh=" + token};

function fixture(root, automatic) {
  const updates = [], alarms = [], applied = [], notices = [], lookups = [];
  let jobs = [];
  const context = vm.createContext({
    URL, Date, JSON, Set, setTimeout, clearTimeout,
    APP: "http://127.0.0.1:6800/extension/add", BAIDU_UA: "pan.baidu.com",
    baiduToken: async () => "fixture-token",
    baiduFileId: value => /^[1-9][0-9]{0,19}$/.test(value || "") ? String(value) : "",
    baiduPageSelection: async () => [{id: "42", name: "fixture.zip", isdir: false}],
    baiduApi: async (_, params) => {
      lookups.push(params.fsids);
      const fsId = params.fsids.slice(1, -1);
      if (fsId === originalId && !automatic) return {list: []};
      return {list: [{fs_id: fsId, size: 3, md5: digest, dlink: "https://d.pcs.baidu.com/file"}]};
    },
    fetch: async (url, opts) => {
      const body = JSON.parse(opts.body);
      if (url.endsWith("/info")) return {ok: true, json: async () => ({
        info: {kind: "百度网盘", baiduResume: true, files: [{name: "folder/fixture.zip", size: 3, fsId: originalId}]},
      })};
      if (url.endsWith("/apply")) applied.push(body);
      if (url.endsWith("/status")) notices.push(body.message);
      return {ok: true, json: async () => ({ok: true})};
    },
    chrome: {
      runtime: {onMessage: {addListener() {}}},
      storage: {session: {
        get: async () => ({cloudRefreshJobs: jobs}),
        set: async values => { jobs = values.cloudRefreshJobs; },
      }},
      tabs: {onUpdated: {addListener(listener) { updates.push(listener); }}, update() { assert.fail("续传不能切换标签"); }},
      alarms: {onAlarm: {addListener(listener) { alarms.push(listener); }}, create() { assert.fail("百度续传不能定时重试"); }, clear() {}},
    },
  });
  vm.runInContext(fs.readFileSync(root + "/browser-extension/cloud-refresh.js","utf8"),context);
  return {context, updates, alarms, applied, notices, lookups};
}

const flush = () => new Promise(resolve => setImmediate(resolve));
for (const root of ["."]) {
  test(root + " 按文件编号续传，无需访问原目录", async () => {
    const f = fixture(root,true);
    assert.equal(await f.context.startCloudRefreshInTab(token,tab),true);
    await flush();
    assert.deepEqual(f.lookups,["[" + originalId + "]"]);
    assert.equal(f.applied[0].files[0].name,"folder/fixture.zip");
    assert.equal(f.applied[0].files[0].md5,digest);
    assert.equal(f.notices.length,0);
  });

  test(root + " 自动取链失败后只响应原标签的手动操作", async () => {
    const f = fixture(root,false);
    await f.context.startCloudRefreshInTab(token,tab);
    await flush();
    assert.equal(f.notices.length,1);
    for (let i = 0; i < 4; i++) {
      f.updates[0](7,{url:"https://pan.baidu.com/disk/main#/new"});
      f.alarms[0]({name:"cloud-refresh"});
    }
    await flush();
    assert.equal(f.lookups.length,1);
    assert.equal(await f.context.baiduResumeInTab({id:99}),null);
    assert.equal((await f.context.baiduResumeInTab(tab)).resumed,true);
    assert.deepEqual(f.lookups,["[" + originalId + "]","[42]"]);
    assert.equal(f.notices.length,1);
    assert.equal(f.applied.length,1);
  });

  test(root + " 清理续传参数时保留用户已打开的目录", async () => {
    const source = fs.readFileSync(root + "/browser-extension/baidu-page.js","utf8");
    const start = source.indexOf("// 只响应应用生成的短期换链令牌");
    const end = source.indexOf('chrome.storage.local.get("baiduButton")',start);
    const location = {href:tab.url};
    let reply;
    const calls = [];
    const history = {state:{fixture:true},replaceState(...args) { calls.push(args); }};
    vm.runInNewContext(source.slice(start,end),{URL,location,history,chrome:{runtime:{sendMessage:()=>new Promise(r=>{reply=r;})}}});
    location.href = tab.url + "#/next-folder";
    reply({ok:true});
    await flush();
    assert.equal(calls.length,1);
    assert.equal(calls[0][0].fixture,true);
    assert.equal(calls[0][2],"https://pan.baidu.com/disk/main#/next-folder");
  });
}
