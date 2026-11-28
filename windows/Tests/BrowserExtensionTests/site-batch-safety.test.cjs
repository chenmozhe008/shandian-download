const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
(async()=>{
 const code=fs.readFileSync('browser-extension/site-download.js','utf8');let install,rules=[],events=[],alarm;
 const chrome={declarativeNetRequest:{getSessionRules:async()=>rules,updateSessionRules:async update=>{events.push('guard');rules=rules.filter(r=>!update.removeRuleIds.includes(r.id)).concat(update.addRules||[])}},
  alarms:{create:async()=>assert.fail('不再创建批量阻止规则的闹钟'),onAlarm:{addListener:fn=>alarm=fn}},scripting:{executeScript:async opts=>{events.push(opts.func.name==='pixabayOriginalFromPage'?'lookup':'hooks');install=opts.func}}};
 const worker=vm.createContext({URL,chrome});vm.runInContext(code,worker);await worker.prepareSiteDownload(7,'manual-one',false);
 assert.deepEqual(events,['lookup','hooks'],'单项保留请求前捕获，批量仅解析已提供的正式地址');
 assert.equal(rules.length,0,'不生成导致 CDN 错误页的网络阻止规则');
 const file='https://cdn.pixabay.com/download/audio/2026/11/test.mp3?filename=test.mp3';
 let networkStarts=0;const messages=[],listeners=new Map();
 class Element {setAttribute(name,value){if(name==='src'){this._src=value;if(/^https:/.test(value))networkStarts++}}}
 class Frame extends Element {}
 Object.defineProperty(Frame.prototype,'src',{configurable:true,get(){return this._src},set(value){this._src=value;if(/^https:/.test(value))networkStarts++}});
 function Anchor(){}Anchor.prototype.click=function(){networkStarts++};
 const originalFrame=Object.getOwnPropertyDescriptor(Frame.prototype,'src').set,originalAttribute=Element.prototype.setAttribute;
 const window={postMessage:m=>messages.push(m),open(){networkStarts++},navigation:{addEventListener:(k,v)=>listeners.set(k,v),removeEventListener:k=>listeners.delete(k)},jsInterface_iframeDown(){networkStarts++},jsInterface_locationHrefDown(){networkStarts++}};
 const document={documentElement:{},addEventListener:(k,v)=>listeners.set(k,v),removeEventListener:k=>listeners.delete(k)};
 const page=vm.createContext({URL,Object,Element,HTMLIFrameElement:Frame,HTMLAnchorElement:Anchor,window,document,location:{href:'https://pixabay.com/music/',origin:'https://pixabay.com'},MutationObserver:class{observe(){}disconnect(){}},setTimeout:()=>1,clearTimeout(){}});
 const setup=()=>vm.runInContext(`(${install.toString()})('batch-one',true)`,page);
 setup();new Frame().src=file;assert.equal(networkStarts,0,'src 赋值必须在网络启动前截获');assert.equal(messages.at(-1).url,file);
 assert.equal(Object.getOwnPropertyDescriptor(Frame.prototype,'src').set,originalFrame);
 setup();new Frame().setAttribute('src',file);assert.equal(networkStarts,0,'setAttribute 也不能先启动浏览器请求');assert.equal(Element.prototype.setAttribute,originalAttribute);
 setup();window.jsInterface_iframeDown({url:file});assert.equal(networkStarts,0,'原站 iframe 下载回调在创建下载前交接');
 setup();window.jsInterface_locationHrefDown({url:file});assert.equal(networkStarts,0,'原站地址跳转回调在创建下载前交接');
 setup();const a=new Anchor();a.href=file;a.click();assert.equal(networkStarts,0);
 setup();window.open('https://example.test/unrecognized-download');assert.equal(messages.at(-1).type,'shandian-site-download-error','不认识的批量弹窗不得回退到浏览器');
 const other={id:900000001,condition:{tabIds:[8]},action:{type:'block'}},existing={id:18901,condition:{tabIds:[7]},action:{type:'modifyHeaders'}};
 rules.push({id:900000002,condition:{tabIds:[7]},action:{type:'block'}},other,existing);await worker.clearSiteDownloadGuards(7);
 assert(rules.some(r=>r.id===other.id));assert(rules.some(r=>r.id===existing.id));assert.equal(rules.length,2,'收尾只移除当前标签页的批量规则');
 alarm({name:'shandian-site-guard:'+other.id});assert.equal(rules.length,1,'过期规则可由闹钟清理');
 const source=fs.readFileSync('browser-extension/background.js','utf8');const start=source.indexOf('chrome.downloads.onCreated.addListener'),end=source.indexOf('\nfunction grabPage',start);
 const calls=[];let created;
 const recovery=source.slice(source.indexOf('function githubRecoveryDownload('),source.indexOf('// tabId：传了就表示'));
 const context=vm.createContext({URL,isSiteDownloadURL:()=>true,console:{warn(){}},handedBack:new Set(),siteBatchRuns:new Map(),takeSiteDownloadIntent:()=>({collectOnly:true,tabId:7,requestId:'batch-one'}),siteDownloadFileName:()=> 'test.mp3',sendToApp(){assert.fail('意外浏览器下载不能算作批量取链成功')},chrome:{runtime:{id:'ours'},tabs:{sendMessage:async(_,m)=>calls.push(m)},downloads:{onCreated:{addListener:fn=>created=fn},cancel:async()=>calls.push('cancel'),erase:async()=>calls.push('erase'),pause(){assert.fail('不能暂停浏览器下载来收集链接')},resume(){assert.fail('不能恢复浏览器下载')},download(){assert.fail('不能降级为浏览器下载')}}}});
 vm.runInContext(recovery+source.slice(start,end),context);
 const protectedUrl='https://github.com/settings/two_factor_authentication/recovery_download';
 await created({id:2,url:protectedUrl,finalUrl:'https://example.invalid/redirected'});
 await created({id:3,url:'https://example.invalid/download',filename:'C:\\Downloads\\github-recovery-codes (1).txt'});
 assert.deepEqual(calls,[],'恢复码由浏览器处理，扩展不会暂停、取消或交接');
 assert(!context.githubRecoveryDownload('https://github.com/example/project/releases/download/v1/tool.zip'));
 await created({id:1,url:file,filename:'test.mp3'});
 assert.equal(calls[0],'cancel');assert.equal(calls[1],'erase');const failure=calls.find(m=>m?.type==='siteDownloadResult');assert.equal(failure.ok,false);assert(!failure.collected);assert(calls.some(m=>m?.type==='siteBatchBrowserTask'));
 await worker.clearSiteDownloadGuards(7);assert.equal(rules.length,1);assert.equal(rules[0].id,18901);
 console.log('下载隔离通过：不创建 CDN 阻止规则、保留单项捕获、无浏览器批量兜底、旧规则定向清理');
})().catch(e=>{console.error(e);process.exitCode=1});
