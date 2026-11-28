const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
(async()=>{
for(const kind of ['阿里云盘','夸克网盘','UC 网盘','百度网盘','123 云盘','天翼云盘','移动云盘','迅雷云盘','光鸭云盘']) for(const complete of [true,false]) for(const fileCount of [1,2]) {
 const wanted=[{name:'a.mp4',size:10},{name:'b.mp4',size:20}].slice(0,fileCount),job={token:'a'.repeat(32),at:Date.now()};let jobs=[job],applied=[];
 const hosts={'阿里云盘':'www.alipan.com','夸克网盘':'pan.quark.cn','UC 网盘':'drive.uc.cn','百度网盘':'pan.baidu.com','123 云盘':'www.123pan.com','天翼云盘':'cloud.189.cn','移动云盘':'yun.139.com','迅雷云盘':'pan.xunlei.com','光鸭云盘':'www.guangyapan.com'};const host=hosts[kind];
 const c=vm.createContext({URL,Set,Date,JSON,setTimeout,clearTimeout,APP:'http://127.0.0.1:6800/extension/add',BAIDU_UA:'pan.baidu.com',BAIDU_META_BATCH:100,
  aliInPage:async()=>{},quarkInPage:async()=>{},quarkSite:()=>({api:'api',pr:'test',domain:'example',page:'https://'+host+'/'}),
  extraCloudFiles:async(tab,files)=>{assert.deepEqual(files,wanted);return {files:(complete?wanted:wanted.slice(0,-1)).map(f=>({url:'https://cdn.example/'+f.name,name:f.name,headers:[]}))}},
  baiduToken:async()=>'fake-token',baiduList:async()=>wanted.map((f,i)=>({id:i+1,name:f.name,size:f.size})),baiduApi:async()=>({list:(complete?wanted:wanted.slice(0,-1)).map((f,i)=>({fs_id:i+1,dlink:'https://d.pcs.baidu.com/'+f.name,md5:'abcdef1234567890abcdef1234567890'}))}),
  fetch:async(url,opt)=>{assert.ok(url.startsWith('http://127.0.0.1:6800/extension/refresh/'));const body=JSON.parse(opt.body);
   if(url.endsWith('/info'))return {ok:true,json:async()=>({ok:true,info:{kind,files:wanted}})};
   if(url.endsWith('/apply')){applied.push(body);return {ok:true,json:async()=>({ok:true})}};
   return {ok:true,json:async()=>({ok:true})};
  },
  chrome:{runtime:{onMessage:{addListener(){}}},cookies:{getAll:async()=>[{name:'session',value:'fake-cookie'}]},storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({cloudRefreshJobs:jobs}),set:async v=>jobs=v.cloudRefreshJobs}},alarms:{create(){},clear(){},onAlarm:{addListener(){}}},tabs:{update:async()=>{},query:async()=>[{id:1,active:true,url:'https://'+host+'/drive/file/all/folder'}],onUpdated:{addListener(){}}},
  scripting:{executeScript:async options=>{if(kind==='百度网盘')return [{result:'/folder'}];assert.deepEqual(options.args.at(-1).map(f=>f.name),wanted.map(f=>f.name));return [{result:{files:(complete?wanted:wanted.slice(0,-1)).map(f=>({url:'https://cdn.example/'+f.name,name:f.name})),ua:'Browser'}}]}}
 }});
 vm.runInContext(fs.readFileSync('browser-extension/cloud-sites.js','utf8')+'\n'+fs.readFileSync('browser-extension/cloud-refresh.js','utf8'),c);await c.runCloudRefresh(job);
 assert.equal(applied.length,complete?1:0,kind);if(complete){assert.equal(applied[0].files.length,wanted.length);assert.equal(applied[0].token,job.token);if(kind==='百度网盘'){assert.equal(applied[0].files[0].md5,'abcdef1234567890abcdef1234567890');assert.equal(new URL(applied[0].files[0].url).searchParams.get('access_token'),'fake-token')}assert.equal(jobs.length,0)}
}
 for(const root of ['.']) {
 const s=fs.readFileSync(root+'/ui/app.js','utf8');const immediate=s.includes('if (act === "resume" && (canBaiduWebResume(t) || needsCloudRefresh(t)))');
 const a=s.indexOf(immediate?'    if (act === "resume" && (canBaiduWebResume(t) || needsCloudRefresh(t)))':'    if (act === "resume" && String(t.meta'),b=s.indexOf('    if (act === "open")',a);
 assert.ok(a>=0&&b>a,'定位实际恢复入口，避免测试空代码');
 let dialogs=0;const c=vm.createContext({act:'resume',id:'original',t:{status:'error',kind:'阿里云盘',meta:{req:{url:'shandian-multi:folder'}}},cloudHomes:{阿里云盘:'home'},needsCloudRefresh:()=>true,canBaiduWebResume:()=>false,cloudResumeAttempts:new Set(),beginCloudRefresh:async()=>dialogs++});
 vm.runInContext('async function replay(){'+s.slice(a,b)+'}',c);await c.replay();assert.equal(dialogs,immediate?1:0);await c.replay();assert.equal(dialogs,immediate?2:1);
 const defs = s.slice(s.indexOf('const cloudHomes ='), s.indexOf('const cloudResumeLinkProviders'));
 const check = vm.runInNewContext(defs+";needsCloudRefresh");
 const old = {id:"fixture",status:"error",kind:"百度网盘",meta:{req:{url:"https://d.pcs.baidu.com/file"}},shandianError:"connection 0 failed: retries=3, status=404"};
 assert.equal(check(old),true);
 assert.equal(check({...old,kind:"直链"}),false);
 assert.equal(check({...old,shandianError:"connection 404 timed out"}),false);
 const before = s.indexOf('async function beginCloudRefresh(');
 const after = s.indexOf('async function acceptCloudRefresh(', before);
 const calls = [];
 const baidu = vm.createContext({
   URL, isLocalBaiduHttp:()=>false, cloudHomes:{百度网盘:'https://pan.baidu.com/disk/main'}, canBaiduWebResume:()=>true,
   ACTIVE:new Set(), toast(){}, askChoice(){assert.fail('继续下载应直接打开网盘')},
   invoke:async (action,args)=>{calls.push({action,args}); return 'b'.repeat(32);}
 });
 vm.runInContext(s.slice(before,after),baidu);
 await baidu.beginCloudRefresh({id:'bd:original',kind:'百度网盘',status:'pause',meta:{req:{url:'https://pan.baidu.com/s/fixture'}}},true);
 assert.deepEqual(calls.map(call=>call.action), ['multi_refresh_prepare','open_url']);
 assert.equal(calls[0].args.id,'bd:original');
 assert.equal(new URL(calls[1].args.url).searchParams.get('shandian-refresh'),'b'.repeat(32));
 calls.length = 0;
 await baidu.beginCloudRefresh({id:'bd:original',kind:'百度网盘',status:'pause',meta:{req:{labels:{baiduCachedResume:'true'}}}},true);
 assert.deepEqual(calls.map(call=>call.action), ['resume_task']);
 const prompts = [];
 const stale = vm.createContext({
   URL, isLocalBaiduHttp:()=>true, cloudHomes:{百度网盘:"https://pan.baidu.com/disk/main"}, canBaiduWebResume:()=>false,
   needsCloudRefresh:check, askChoice:async options=>{prompts.push(options);return false;},
   invoke:()=>assert.fail("取消后不应修改原任务或打开网页")
 });
 vm.runInContext(s.slice(before,after),stale);
 await stale.beginCloudRefresh({...old,progress:{downloaded:0}},true);
 assert.equal(prompts.length,1);
 assert.match(prompts[0].desc,/字节为 0.*从头下载/);
 const bgCalls = [];
 const background = vm.createContext({
   URL, cloudHomes:{百度网盘:"https://pan.baidu.com/disk/main"}, canBaiduWebResume:()=>false,
   isLocalBaiduHttp:()=>true, pendingBaiduRenewals:new Set(), ACTIVE:new Set(), toast(){},
   needsCloudRefresh:check, askChoice:async()=>{assert.fail("已授权且进度有效时无需打开网页")},
   invoke:async (cmd,args)=>{bgCalls.push(cmd);assert.equal(args.id,old.id);return null;},
   refreshTasks:async()=>{}
 });
 vm.runInContext(s.slice(before,after),background);
 for(const status of ["pause","error"]){
   const source={...old,status,progress:{downloaded:100}};
   assert.equal(check(source),true);
   await background.beginCloudRefresh(source,true);
 }
 assert.deepEqual(bgCalls,["baidu_http_renew","baidu_http_renew"]);
 background.invoke=async()=> "当前授权已过期";
 background.askChoice=async opts=>{assert.equal(opts.buttons[1].label,"打开网盘"); return false;};
 await background.beginCloudRefresh({...old,progress:{downloaded:100}},true);
 assert.equal(calls[0].args.id, 'bd:original');
}
console.log('自动换链通过：九种网盘完整匹配才回填、缺项不提交、不依赖手动选择、验证当前恢复入口');
})().catch(e=>{console.error(e);process.exitCode=1});
