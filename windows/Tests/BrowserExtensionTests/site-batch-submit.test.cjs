const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
(async()=>{
  const code=fs.readFileSync('windows/browser-extension/background.js','utf8');
  const start=code.indexOf('const siteBatchStorageKey');
  const end=code.indexOf('let siteFlowObserverAttached',start);
  const sizeStart=code.indexOf('function knownFileBytes(');
  const sizeEnd=code.indexOf('// tabId：',sizeStart);
  const page='https://pixabay.com/music/', store=new Map(), messages=[], batches=[];
  const run={id:'current',page,keys:['first','second','pending'],stage:'original',sent:new Set()};
  const runs=new Map([[7,run]]), intents=new Map();
  for(const key of ['first','second'])intents.set(key,{
    tabId:7,batchId:'current',itemHref:key,collected:true,capturedURL:'https://cdn.pixabay.com/download/audio/'+key+'.mp3',name:key
  });
  let accepted=true, unblock, delay;
  const ctx=vm.createContext({
    chrome:{
      tabs:{get:async id=>({id,url:page,active:false}),sendMessage:async(id,msg)=>messages.push(msg)},
      storage:{session:{get:async key=>({[key]:store.get(key)}),set:async values=>Object.entries(values).forEach(([key,value])=>store.set(key,value))}}
    },
    siteBatchRuns:runs,siteDownloadIntents:intents,siteBatchSubmissions:new Map(),
    siteDownloadFileName:(_,name)=>name+'.mp3',isSiteDownloadURL:url=>url.startsWith('https://cdn.pixabay.com/'),
    capturedForTab:async()=>({items:[
      {url:intents.get('first')?.capturedURL,size:24000},
      {url:'https://cdn.pixabay.com/preview/second.mp3',size:16000},
    ]}),
    headersFor:async()=>({Referer:page}),
    extensionBatchRequest:async (_,body)=>{batches.push(body);if(delay)await delay;return {ok:accepted};}
  });
  vm.runInContext(code.slice(sizeStart,sizeEnd)+code.slice(start,end),ctx);
  const submit=(keys,id='current')=>ctx.submitSiteBatchSelection(7,id,keys);
  assert.equal((await submit(['pending'])).ok,false);
  assert.equal((await submit(['first','first'])).ok,false);
  assert.equal((await submit(['first'],'other')).ok,false);
  assert.equal(batches.length,0);
  delay=new Promise(resolve=>unblock=resolve);
  const first=submit(['first']);
  await new Promise(setImmediate);
  assert.equal((await submit(['first'])).ok,false,'在途提交不能重复交接');
  unblock();assert.equal((await first).ok,true);
  delay=null;
  assert.equal(run.stage,'original','分批下载后继续解析');
  assert.deepEqual([...run.sent],['first']);
  assert.equal((await submit(['first'])).ok,false);
  assert.equal(batches.length,1);
  assert.equal(batches[0].links[0].name,'first.mp3');
  assert.equal(batches[0].links[0].sizeBytes,24000);
  assert.deepEqual(Array.from(messages[0].keys),['first']);

  run.stage='review';
  await ctx.saveSiteBatch(7,run);
  runs.clear();intents.clear();
  assert.equal((await submit(['first'])).ok,false,'后台休眠后仍记得已交接项');
  accepted=false;
  assert.equal((await submit(['second'])).ok,false);
  accepted=true;
  assert.equal((await submit(['second'])).ok,true,'已解析结果在后台休眠后仍可下载，失败可安全重试');
  assert.equal(batches.at(-1).links[0].sizeBytes,undefined,'试听地址大小不能用来显示原文件大小');
  assert.deepEqual(Array.from(store.get('pixabayBatch:7').sent),['first','second']);
  const serial={id:'next',page,keys:['a','b','c'],stage:'original',sent:new Set(),unchecked:new Set(['b'])};
  runs.set(7,serial);
  for(const key of serial.keys)intents.set(key,{
    tabId:7,batchId:'next',itemHref:key,collected:true,capturedURL:'https://cdn.pixabay.com/download/audio/'+key+'.mp3',name:key
  });
  delay=new Promise(resolve=>unblock=resolve);
  const early=submit(['a'],'next');
  await new Promise(setImmediate);
  const auto=ctx.submitSiteBatchSelection(7,'next',null,true);
  assert.equal((await submit(['c'],'next')).ok,false,'末项自动交接期间，不能再重复点');
  unblock();
  assert.equal((await early).ok,true);
  assert.deepEqual(Array.from((await auto).keys),['c'],'非活动标签页自动提交剩余勾选项');
  assert.equal(batches.at(-1).links[0].name,'c.mp3');
  assert.deepEqual([...serial.sent],['a','c']);
  delay=null;
  assert.deepEqual(Array.from((await ctx.submitSiteBatchSelection(7,'next',null,true)).keys),[]);
  assert.equal(batches.filter(batch=>batch.links[0].name==='c.mp3').length,1);
  intents.delete('c');
  serial.links.delete('c');
  const incomplete=await ctx.submitSiteBatchSelection(7,'next',null,true);
  assert.equal(incomplete.ok,false,'原站部分失败时不自动交接');
  console.log('后台自动提交、保留手动选择、串行防重及休眠恢复通过');
})().catch(error=>{console.error(error);process.exitCode=1});
