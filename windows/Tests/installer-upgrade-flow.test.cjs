const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const flush=()=>new Promise(resolve=>setImmediate(resolve));
class Element {
  constructor(tag='div'){this.tagName=tag.toUpperCase();this.listeners={};this.children=[];this.style={};this.dataset={};this.textContent='';this.value='';this.hidden=false;this.disabled=false;this.checked=false;this.classList={add(){},remove(){},toggle(){}};}
  addEventListener(type,callback){(this.listeners[type] ||= []).push(callback);}
  async fire(type){for(const callback of this.listeners[type] || [])await callback({target:this});}
  click(){return this.fire('click');}
  focus(){this.focused=true;}
  set innerHTML(value){this.html=value;this.children=[];}
  get innerHTML(){return this.html;}
  get lastElementChild(){return this.children.at(-1);}
  appendChild(child){this.children.push(child);return child;}
  insertBefore(child,before){this.children.splice(this.children.indexOf(before),0,child);return child;}
  getBoundingClientRect(){return {height:this.height ?? 320};}
}
function installer(html,handler){
  const elements=new Map(),calls=[],copied=[];
  for(const node of html.matchAll(/<([a-z][a-z0-9]*)\b[^>]*\bid="([^"]+)"([^>]*)>/gi)){
    const element=new Element(node[1]);element.id=node[2];element.hidden=/\bhidden\b/.test(node[3]);element.checked=/\bchecked\b/.test(node[3]);elements.set(element.id,element);
  }
  const get=id=>{assert.ok(elements.has(id),'unknown element '+id);return elements.get(id);};
  const pages=[...elements.values()].filter(element=>element.id.startsWith('p-'));
  const documentEvents={};
  const document={getElementById:get,createElement:tag=>new Element(tag),addEventListener:(event,fn)=>{documentEvents[event]=fn;},querySelectorAll:selector=>selector==='.page'?pages:[],querySelector:selector=>selector==='.page:not([hidden])'?pages.find(page=>!page.hidden):null};
  const context=vm.createContext({document,navigator:{clipboard:{writeText:async value=>copied.push(value)}},window:{__TAURI__:{core:{invoke:(command,args)=>{calls.push({command,args});return handler(command,args,get);}},event:{listen(){}}}},console});
  const script=[...html.matchAll(/<script(?:\s[^>]*)?>([\s\S]*?)<\/script>/g)].map(match=>match[1]).find(script=>script.includes('const { invoke }'));
  new vm.Script(script);vm.runInContext(script,context);
  return {context,get,calls,copied,pages,documentEvents};
}
const defaults={mode:'install',version:'0.1.30',edition:'nas',installedVersion:null,installedEdition:null,versionCmp:null,extensionVersion:'1.15.50',installedExtensionVersion:null,dir:'C:\Users\Fixture\Programs\闪电全能下载',machine:false,userDir:'C:\Users\Fixture\Programs\闪电全能下载',machineDir:'C:\Program Files\闪电全能下载',sizeMb:420,auto:null};
async function fresh(html){
  let release;
  const f=installer(html,(command,args)=>{
    if(command==='setup_info')return new Promise(resolve=>{release=resolve;});
    if(command==='fit_window')return Promise.resolve();
    throw Error('unexpected '+command);
  });
  assert.equal(f.get('p-check').hidden,false);assert.equal(f.get('p-install').hidden,true,'识别完成前不显示新安装选项');
  release(defaults);await flush();
  assert.equal(f.get('p-install').hidden,false);assert.equal(f.get('i-scopes').hidden,false);assert.equal(f.get('i-dir').value,defaults.userDir);
  f.get('i-machine').checked=true;f.get('i-user').checked=false;await f.get('i-machine').fire('change');assert.equal(f.get('i-dir').value,defaults.machineDir,'所有用户即时联动默认目录');
  f.get('i-machine').checked=false;f.get('i-user').checked=true;await f.get('i-user').fire('change');assert.equal(f.get('i-dir').value,defaults.userDir,'仅为我即时联动默认目录');
}
async function upgrade(html,automatic,needsAdmin){
  let release;
  const info={...defaults,installedVersion:'0.1.29',installedEdition:'nas',versionCmp:'upgrade',machine:true,dir:defaults.machineDir,auto:automatic?{dir:'D:\Wrong\闪电全能下载',desktop:false,machine:false}:null};
  const f=installer(html,(command,args,get)=>{
    if(command==='setup_info')return new Promise(resolve=>{release=resolve;});
    if(command==='fit_window')return Promise.resolve();
    if(command==='running_in')return Promise.resolve([]);
    if(command==='needs_admin'){
      assert.equal(get('i-scopes').hidden,true,'自动入口也必须先初始化升级模式');
      assert.match(get('i-sub').textContent,/已安装.*升级/);assert.equal(get('i-dir').value,info.dir);assert.equal(args.machine,true);
      return Promise.resolve(needsAdmin);
    }
    if(command==='elevate_install'){
      assert.equal(args.opts.machine,true);assert.equal(args.opts.dir,info.dir,'自动参数不能改变原安装位置或范围');
      return Promise.reject('cancelled');
    }
    if(command==='install'){
      assert.equal(needsAdmin,false,'需要管理员确认时不能直接install');
      assert.equal(args.opts.machine,true);assert.equal(args.opts.dir,info.dir);
      return Promise.resolve({exe:info.dir+'\闪电全能下载.exe',extensionChanged:false});
    }
    throw Error('unexpected '+command);
  });
  assert.equal(f.get('p-check').hidden,false);release(info);await flush();await flush();
  assert.equal(f.get('i-dir').readOnly,true);assert.equal(f.get('i-pick').hidden,true);assert.equal(f.get('i-user').disabled,true);assert.equal(f.get('i-machine').disabled,true);
  f.get('i-machine').checked=false;f.get('i-user').checked=true;await f.get('i-user').fire('change');assert.equal(f.get('i-dir').value,info.dir,'升级不能由隐藏scope事件换目录');
  if(!automatic)await f.get('i-go').fire('click');await flush();
  assert.equal(f.calls.filter(call=>call.command==='needs_admin').length,1);
  if(needsAdmin){
    assert.equal(f.calls.filter(call=>call.command==='elevate_install').length,1);assert.equal(f.calls.filter(call=>call.command==='install').length,0,'UAC取消不执行安装/关闭旧程序');
    assert.match(f.get('i-note').textContent,/更新未开始/);assert.doesNotMatch(f.get('i-note').textContent,/仅为我/);assert.equal(f.get('p-install').hidden,false);
  }else{
    assert.equal(f.calls.filter(call=>call.command==='install').length,1);assert.equal(f.calls.filter(call=>call.command==='elevate_install').length,0);assert.equal(f.get('r-title').textContent,'更新完成');
  }
}
async function automaticFresh(html){
  const info={...defaults,auto:{dir:defaults.machineDir,desktop:false,machine:true}};
  const f=installer(html,command=>{
    if(command==='setup_info')return Promise.resolve(info);
    if(command==='fit_window')return Promise.resolve();
    if(command==='needs_admin')return Promise.resolve(true);
    if(command==='elevate_install')return Promise.reject('cancelled');
    throw Error('unexpected '+command);
  });
  await flush();await flush();
  assert.equal(f.get('i-machine').checked,true);assert.equal(f.get('i-scopes').hidden,false);assert.equal(f.get('i-dir').value,defaults.machineDir);
  f.get('i-machine').checked=false;f.get('i-user').checked=true;await f.get('i-user').fire('change');
  assert.equal(f.get('i-dir').value,defaults.userDir,'auto取消提权后范围联动仍已绑定');
  assert.equal(f.calls.filter(call=>call.command==='install').length,0);
}
function backendOrdering(source){
  const install=source.slice(source.indexOf('fn do_install('),source.indexOf('#[derive(Serialize)]',source.indexOf('fn do_install(')));
  assert.ok(install.indexOf('prepare_install(&target, payload, places.needs_admin())?')<install.indexOf('kill_procs('),'所有预检成功后才可关闭进程');
  assert.doesNotMatch(install.slice(0,install.indexOf('prepare_install(')),/kill_procs\(/);
  const preflight=source.slice(source.indexOf('fn prepare_install('),source.indexOf('fn do_install('));
  assert.ok(preflight.indexOf('if needs_elevation')<preflight.indexOf('open_zip(payload)?'));
  assert.ok(preflight.includes('probe_install_write(target)?'));assert.ok(preflight.includes('has_main'));assert.doesNotMatch(preflight,/kill_procs/);
  const probe=source.slice(source.indexOf('fn probe_install_write('),source.indexOf('fn prepare_install('));
  assert.match(probe,/create_new\(true\)/,'写入预检只能创建唯一新文件');assert.doesNotMatch(probe,/fs::write|File::create\(/,'不能截断已有文件');
  assert.ok(install.includes('locked_install_opts(opts)'),'后台自动入口也锁原目录/范围');
  const elevation=source.slice(source.indexOf('fn elevate_install('),source.indexOf('struct InstallResult'));
  assert.ok(elevation.indexOf('w.hide()')<elevation.indexOf('run_as_admin('));
  assert.match(elevation,/Err\(error\)[\s\S]*ELEVATION_HANDOFF\.store\(false/);
  assert.match(source,/if !ELEVATION_HANDOFF\.load\([^\n]+&& !window\.is_visible/,'启动兜底不会把交接后的旧窗口重新显示');
}
async function extensionGuide(html){
  const f=installer(html,(command)=>{
    if(command==='setup_info')return Promise.resolve(defaults);
    if(command==='fit_window' || command==='launch' || command==='close_app')return Promise.resolve();
    if(command==='install')return Promise.resolve({exe:'fixture.exe',extensionChanged:true,extensionFrom:'1.15.55',extensionTo:'1.15.56'});
    if(command==='ext_info')return Promise.resolve({dir:defaults.machineDir,browsers:[]});
    throw Error(command);
  });
  await flush();
  await f.context.runInstall({dir:defaults.machineDir,desktop:false,machine:false});
  f.get('r-open').checked=true;
  await f.get('r-foot').lastElementChild.click();
  assert.equal(f.get('p-ext').hidden,false);
  assert.equal(f.get('e-foot').children[0].textContent,'复制 Chrome 地址');
  assert.equal(f.get('e-foot').children[1].textContent,'复制 Edge 地址');
  assert.equal(f.get('e-foot').lastElementChild.focused,true);
  assert.equal(f.calls.some(call=>call.command==='launch'),false,'指南出现前不打开主窗口');
  f.documentEvents.keydown({key:'Enter',target:f.get('e-dir'),preventDefault(){}});
  await flush();
  assert.deepEqual(f.calls.filter(call=>['launch','close_app'].includes(call.command)).map(call=>call.command),['launch','close_app'],'指南回车先启动主程序再结束安装');
}
async function completionResponsiveness(html){
  const start=kind=>{
    let release;
    const f=installer(html,command=>{
      if(command==='setup_info')return Promise.resolve(defaults);
      if(command==='fit_window' || command==='close_app')return Promise.resolve();
      if(command==='install')return Promise.resolve({exe:'unused',extensionChanged:false});
      if(command===kind)return new Promise(resolve=>{release=resolve;});
      throw Error(command);
    });
    return {f,release:value=>release(value)};
  };
  const launch=start('launch');
  await flush();
  const a=launch.f;
  a.get('r-open').checked=true;
  a.get('r-ext-row').hidden=true;
  a.get('r-foot').appendChild(new Element('button'));
  const pending=a.context.completeInstall({},defaults.userDir+'\\闪电全能下载.exe');
  assert.equal(a.get('close').disabled,false,'安装结束后等待主程序启动时仍可关闭');
  assert.equal(a.get('r-foot').lastElementChild.textContent,'正在打开…');
  await a.context.completeInstall({},defaults.userDir+'\\闪电全能下载.exe');
  assert.equal(a.calls.filter(call=>call.command==='launch').length,1,'等待中重复点击不会再次启动');
  await a.get('close').click();
  assert.equal(a.calls.filter(call=>call.command==='close_app').length,1);
  launch.release();
  await pending;
  assert.equal(a.calls.filter(call=>call.command==='close_app').length,1);

  for (const closeWhileLoading of [false,true]) {
    const discover=start('ext_info');
    await flush();
    const b=discover.f;
    b.get('p-ext').height=305;
    await b.context.runInstall({dir:defaults.userDir,desktop:false,machine:false});
    const fits=()=>b.calls.filter(call=>call.command==='fit_window');
    const priorFitCount=fits().length;
    const button=b.get('r-foot').lastElementChild;
    assert.equal(b.calls.filter(call=>call.command==='ext_info').length,1,'完成页提前检查浏览器');
    const guide=button.click();
    assert.equal(b.get('p-result').hidden,false,'资料未齐时停留在完成页');
    assert.equal(button.textContent,'正在准备…');
    assert.equal(button.disabled,true);
    assert.equal(fits().length,priorFitCount);
    assert.equal(b.get('close').disabled,false);
    await button.click();
    assert.equal(b.calls.filter(call=>call.command==='ext_info').length,1,'重复点击复用同一次检查');
    if(closeWhileLoading) {
      b.documentEvents.keydown({key:'Escape',target:button});
      assert.equal(b.calls.filter(call=>call.command==='close_app').length,1);
    }
    discover.release({dir:defaults.userDir,browsers:[['edge','Edge',true]]});
    await guide;
    if(closeWhileLoading) {
      assert.equal(b.get('p-ext').hidden,true,'关闭后不再渲染迟到的浏览器结果');
      assert.equal(fits().length,priorFitCount);
    } else {
      assert.equal(b.get('p-ext').hidden,false);
      assert.equal(b.get('e-foot').children[0].textContent,'打开 Edge 扩展页');
      assert.deepEqual(fits().slice(priorFitCount).map(call=>[call.args.height,call.args.center]),[[305,false]],'扩展引导整体只调整一次高度');
    }
  }
}
async function positionOnlyOnFirstReadyPage(html){
  let resolveSetup;
  const f=installer(html,command=>{
    if(command==='setup_info')return new Promise(resolve=>{resolveSetup=resolve;});
    if(command==='fit_window')return Promise.resolve();
    throw Error(command);
  });
  const requests=()=>f.calls.filter(call=>call.command==='fit_window').map(call=>[call.args.height,call.args.center]);
  assert.deepEqual(requests(),[],'准备状态不提前显示窗口');
  resolveSetup(defaults);
  await flush();
  assert.deepEqual(requests(),[[320,true]],'正式首屏量好尺寸再居中显示');
  for(const [page,height] of [['p-progress',200],['p-result',280],['p-ext',360],['p-install',320]]){
    f.get(page).height=height;
    f.context.show(page);
  }
  assert.deepEqual(requests().slice(1),[[200,false],[280,false],[360,false],[320,false]],'安装操作后的步骤只改变高度');
}
async function extensionManager(html){
  let complete;
  const f=installer(html,(command)=>{
    if(command==='setup_info')return Promise.resolve(defaults);
    if(command==='fit_window')return Promise.resolve();
    if(command==='ext_info')return Promise.resolve({dir:defaults.machineDir,browsers:[['chrome','Chrome',false],['edge','Edge',true]]});
    if(command==='open_ext_page')return new Promise(resolve=>{complete=resolve;});
    throw Error(command);
  });
  await flush();
  f.context.showExtGuide(true,()=>{},await f.context.readExtInfo(defaults.machineDir));
  assert.equal(f.get('e-foot').children[0].textContent,'复制 Chrome 地址');
  const edge=f.get('e-foot').children[1];
  assert.equal(edge.textContent,'打开 Edge 扩展页');
  const pending=edge.click();
  assert.equal(edge.disabled,true);
  assert.equal(edge.textContent,'正在打开…');
  complete(true);await pending;
  assert.match(f.get('e-note').textContent,/已交给 Edge/);
  assert.equal(edge.disabled,false);
  const fallback=edge.click();complete(false);await fallback;
  assert.equal(f.copied.at(-1),'edge://extensions/');
  assert.match(f.get('e-note').textContent,/已复制地址/);
  const initial=installer(html,command=>{
    if(command==='setup_info')return Promise.resolve(defaults);
    if(command==='fit_window')return Promise.resolve();
    if(command==='ext_info')return Promise.resolve({dir:defaults.machineDir,browsers:[['chrome','Chrome',true],['edge','Edge',false]]});
    if(command==='open_ext_page')return Promise.resolve(true);
    throw Error(command);
  });
  await flush();
  initial.context.showExtGuide(false,()=>{},await initial.context.readExtInfo(defaults.machineDir));
  const chrome=initial.get('e-foot').children[0];
  assert.equal(chrome.textContent,'打开 Chrome 扩展页');
  await chrome.click();
  assert.match(initial.get('e-note').textContent,/已交给 Chrome/);
  const firstEdge=initial.get('e-foot').children[1];
  assert.equal(firstEdge.textContent,'复制 Edge 地址');
  await firstEdge.click();
  assert.deepEqual(initial.copied,['edge://extensions/']);
  assert.equal(initial.calls.filter(call=>call.command==='open_ext_page').length,1);
}
(async()=>{
  for(const root of ['.']){
    const html=fs.readFileSync(root+'/installer/app/ui/index.html','utf8');
    assert.match(html,/<section class="page" id="p-check">/);assert.match(html,/<section class="page" id="p-install" hidden>/);
    await fresh(html);await automaticFresh(html);await extensionGuide(html);await completionResponsiveness(html);await positionOnlyOnFirstReadyPage(html);await extensionManager(html);for(const automatic of [false,true])for(const needsAdmin of [false,true])await upgrade(html,automatic,needsAdmin);
    backendOrdering(fs.readFileSync(root+'/installer/app/src/main.rs','utf8'));
  }
  console.log('安装升级通过：检查loading、新装范围/目录联动、手动与auto升级锁目录范围、auto先提权、UAC取消不install、backend预检先于kill、唯一写入探测');
})().catch(error=>{console.error(error);process.exitCode=1;});
