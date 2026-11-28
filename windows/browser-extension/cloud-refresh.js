// 用户从原任务发起的一次性换链意图；复用现有取链器与本机网关。
const CLOUD_REFRESH_BUSY = new Set();
const CLOUD_REFRESH_HOSTS = {"阿里云盘":["www.alipan.com","www.aliyundrive.com"],"夸克网盘":["pan.quark.cn"],"UC 网盘":["drive.uc.cn"],"百度网盘":["pan.baidu.com"]};
if (typeof EXTRA_CLOUD_SITES !== "undefined") for (const site of EXTRA_CLOUD_SITES) {
  CLOUD_REFRESH_HOSTS[site.name] = site.hosts.flatMap(host => [host, ...(site.subdomains || []).map(sub => `${sub}.${host}`)]);
}
async function refreshRpc(action, body) {
  const response = await fetch(APP.replace(/\/add$/, "/refresh/") + action, {method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(body)});
  if(!response.ok) throw new Error("无法连接闪电下载");
  return response.json();
}
async function refreshJobs() { return (await chrome.storage.session.get("cloudRefreshJobs")).cloudRefreshJobs || []; }
async function putRefreshJobs(jobs) {
  await chrome.storage.session.set({cloudRefreshJobs:jobs});
  if(jobs.some(job => !job.baidu)) chrome.alarms.create("cloud-refresh",{periodInMinutes:0.5});
  else chrome.alarms.clear("cloud-refresh");
}
async function refreshNotice(job,message) {
  if(job.lastMessage===message)return;
  job.lastMessage=message;
  await refreshRpc("status",{token:job.token,message}).catch(()=>{});
}
async function refreshedFilesInTab(info,tab) {
  if (typeof extraCloudSite === "function" && extraCloudSite(tab.url)?.name === info.kind) return extraCloudFiles(tab,info.files);
  if(info.kind==="阿里云盘") {
    const [reply]=await chrome.scripting.executeScript({target:{tabId:tab.id},world:"MAIN",func:aliInPage,args:[info.files]});
    const data=reply?.result;
    if(!data?.files?.length)return data || {notFound:true};
    return {files:data.files.map(f=>({url:f.url,name:f.name,headers:[["Referer",new URL(tab.url).origin+"/"],["User-Agent",data.ua]]}))};
  }
  if(info.kind==="夸克网盘" || info.kind==="UC 网盘") {
    const site=quarkSite(tab.url);
    const [reply]=await chrome.scripting.executeScript({target:{tabId:tab.id},world:"MAIN",func:quarkInPage,args:[site.api,site.pr,info.files]});
    const data=reply?.result;
    if(!data?.files?.length)return data || {notFound:true};
    const cookies=await chrome.cookies.getAll({domain:site.domain});
    if(!cookies.length)return {error:"请先在网盘页面登录"};
    const header=[["Cookie",cookies.map(c=>`${c.name}=${c.value}`).join("; ")],["User-Agent",data.ua],["Referer",site.page]];
    return {files:data.files.map(f=>({url:f.url,name:f.name,headers:header}))};
  }
  if(info.kind==="百度网盘") {
    if(!await baiduToken())return {error:"请先在扩展设置中完成百度网盘授权"};
    const [reply]=await chrome.scripting.executeScript({target:{tabId:tab.id},world:"MAIN",func:()=>{
      const url=new URL(location.href);let path=url.searchParams.get("path") || url.searchParams.get("dir");
      if(!path){const found=location.hash.match(/(?:^|[/?&#])(?:path|dir)=([^&]+)/);if(found)try{path=decodeURIComponent(found[1])}catch{}}
      return path || "/";
    }});
    const here=await baiduList(reply?.result || "/"),matches=[];
    for(const expected of info.files){
      const base=String(expected.name).split(/[\\/]/).pop();const found=here.filter(f=>!f.isdir && f.name===base && (!expected.size || !f.size || expected.size===f.size));
      if(found.length!==1)return {notFound:true};matches.push({f:found[0],name:expected.name});
    }
    const list=[];
    for(let i=0;i<matches.length;i+=BAIDU_META_BATCH){
      const part=await baiduApi("xpan/multimedia?method=filemetas",{fsids:JSON.stringify(matches.slice(i,i+BAIDU_META_BATCH).map(x=>x.f.id)),dlink:1},"更新下载地址");list.push(...(part.list || []));
    }
    const token = await baiduToken();
    if (!token) return {error:"请先在扩展设置中完成百度网盘授权"};
    const files = matches.map(({f: selected, name}) => {
      const file = list.find(entry => String(entry.fs_id) === String(selected.id));
      if (!file?.dlink) return null;
      const url = new URL(file.dlink);
      if (url.protocol !== "https:" || !/(^|\.)(baidu\.com|baidupcs\.com)$/.test(url.hostname) ||
          url.username || url.password || url.port || url.hash || url.searchParams.has("access_token")) return null;
      url.searchParams.set("access_token", token);
      return {name, url:url.href, md5:file.md5, headers:[["User-Agent",BAIDU_UA]]};
    });
    return files.every(Boolean)?{files}:{error:"部分文件暂未返回下载地址，请稍后重试"};
  }
  return {error:"此网盘尚未适配自动取链"};
}
function cloudFolderKey(kind,file){return JSON.stringify([kind,file.name,file.size || 0]);}
function cloudFolderScore(tab,kind){
 try {const u=new URL(tab.url);return ((kind==="阿里云盘" ? /^\/drive\/file\/[^/]+\/[^/]+/.test(u.pathname) : u.hash.length>10)?4:0)+(tab.active?1:0);}catch{return 0}
}
async function rememberCloudFolder(info,tab){
 const url=new URL(tab.url);url.searchParams.delete("shandian-refresh");url.searchParams.delete("shandian-account-link");
 const saved=(await chrome.storage.local.get("cloudRefreshFolders")).cloudRefreshFolders || {};
 for(const file of info.files)saved[cloudFolderKey(info.kind,file)]={url:url.href,at:Date.now()};
 const keep=Object.fromEntries(Object.entries(saved).sort((a,b)=>b[1].at-a[1].at).slice(0,1000));
 await chrome.storage.local.set({cloudRefreshFolders:keep});
}
async function wakeCloudTab(tab){
 if(!tab.active || tab.discarded || tab.frozen)await chrome.tabs.update(tab.id,{active:true});
 if(chrome.tabs.get)for(let n=0;n<40;n++){const live=await chrome.tabs.get(tab.id);if(live.status!=="loading")break;await new Promise(r=>setTimeout(r,200));}
}
async function boundedCloudRead(info,tab){
 let timer;
 try{return await Promise.race([refreshedFilesInTab(info,tab),new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error("网盘页面暂未就绪")),10000)})]);}
 finally{clearTimeout(timer)}
}
async function baiduResumeInTab(tab) {
  const job = (await refreshJobs()).find(j => j.baidu && j.openerTabId === tab.id);
  if (!job) return null;
  return runBaiduRefresh(job, tab);
}

async function runBaiduRefresh(job, tab = null) {
  const failed = error => ({ok:false, selectionHint:true, error});
  if (CLOUD_REFRESH_BUSY.has(job.token)) return failed("正在获取原文件地址，请稍等");
  CLOUD_REFRESH_BUSY.add(job.token);
  try {
    const {info} = await refreshRpc("info", {token:job.token});
    if (!info || Date.now()-job.at > 600000) {
      await putRefreshJobs((await refreshJobs()).filter(j => j.token !== job.token));
      return failed("续传请求已过期，请回到闪电的原任务点「继续下载」");
    }
    if (!await baiduToken()) {
      return failed("请先在扩展设置中完成百度网盘授权，再点网页里的「闪电下载」");
    }
    const expected = info.files[0];
    let fsId = baiduFileId(expected?.fsId);
    if (tab) {
      const base = String(expected?.name || "").split(/[\\/]/).pop();
      const items = await baiduPageSelection(tab.id);
      const matches = items.filter(item => !item.isdir && item.name === base);
      fsId = matches.length === 1 ? baiduFileId(matches[0].id) : "";
      if (!fsId) return failed("请选中原文件，再点「闪电下载」继续原任务");
    } else if (!fsId) {
      const name = String(expected?.name || "").split(/[\\/]/).pop();
      if (!name) return failed("缺少原文件名，请在网盘里选择文件");
      const data = await baiduApi("xpan/file?method=search", {dir:"/", recursion:1, num:100, key:name}, "查找原文件");
      const entries = data.list || [];
      const files = entries.filter(f => !Number(f.isdir) && f.server_filename === name && Number(f.size) === expected.size);
      if (files.length !== 1 || entries.length >= 100 || Number(data.has_more) > 0) {
        return failed("无法唯一确认原文件，请在网盘里选中原文件，再点「闪电下载」");
      }
      fsId = baiduFileId(files[0].fs_id);
      if (!fsId) return failed("未取得有效的文件编号，请在网盘里选择文件");
    }

    const data = await baiduApi("xpan/multimedia?method=filemetas", {
      fsids: "[" + fsId + "]", dlink: 1,
    }, "续传原文件");
    const match = (data.list || []).find(file => baiduFileId(file.fs_id) === fsId);
    if (!match?.dlink || Number(match.size) !== expected.size || !match.md5) {
      return failed(tab ? "这个文件无法与原任务匹配，请确认选中的是同一份文件" :
        "无法自动取得原文件地址。请在已打开的网盘标签内选中原文件，再点「闪电下载」");
    }

    const url = new URL(match.dlink);
    if (url.protocol !== "https:" || !/(^|\.)(baidu\.com|baidupcs\.com)$/.test(url.hostname) ||
        url.username || url.password || url.port || url.hash || url.searchParams.has("access_token")) {
      return failed("百度网盘返回的下载地址无效，请稍后重试");
    }
    const token = await baiduToken();
    if (!token) return failed("百度网盘授权已过期，请在扩展设置中重新授权");
    url.searchParams.set("access_token", token);
    const reply = await refreshRpc("apply", {token:job.token,files:[{
      name:expected.name, url:url.href, md5:match.md5, headers:[["User-Agent",BAIDU_UA]],
    }]});
    if (!reply.ok) return failed(reply.error || "未能继续原任务，已保留进度");
    await putRefreshJobs((await refreshJobs()).filter(j => j.token !== job.token));
    return {ok:true,resumed:true};
  } catch (error) {
    if (error.needAuth) return failed("百度网盘授权已过期，请在扩展设置中重新授权");
    return failed(tab ? "暂时无法取得这个文件的下载地址，请确认有下载权限" :
      "获取原文件地址未成功。请在刚打开的网盘标签内选中原文件，点「闪电下载」");
  } finally {
    CLOUD_REFRESH_BUSY.delete(job.token);
  }
}

async function runCloudRefresh(job) {
  if(CLOUD_REFRESH_BUSY.has(job.token))return;
  CLOUD_REFRESH_BUSY.add(job.token);
  try {
    const reply=await refreshRpc("info",{token:job.token});const info=reply.info;
    if(!info || Date.now()-job.at>600000){await putRefreshJobs((await refreshJobs()).filter(x=>x.token!==job.token));return;}
    const hosts=CLOUD_REFRESH_HOSTS[info.kind] || [];
    let tabs=(await chrome.tabs.query({})).filter(tab=>{try{const u=new URL(tab.url);return hosts.includes(u.hostname) && !/^\/(s|share)(\/|$)/.test(u.pathname)}catch{return false}});
    const remembered=(await chrome.storage.local.get("cloudRefreshFolders")).cloudRefreshFolders || {};
    const folder=info.files.map(f=>remembered[cloudFolderKey(info.kind,f)]?.url).find(Boolean);
    if(info.kind !== "百度网盘" && folder && tabs.length){
      const target=new URL(folder);
      if(hosts.includes(target.hostname) && !tabs.some(t=>t.url===folder)){
        const tab=tabs.find(t=>t.id===job.openerTabId) || tabs.find(t=>t.active) || tabs[0];
        await chrome.tabs.update(tab.id,{url:folder,active:true});tabs=tabs.map(t=>t.id===tab.id?{...t,url:folder,active:true,status:"loading"}:t);
      }
    }
    tabs=tabs.sort((a,b)=>cloudFolderScore(b,info.kind)-cloudFolderScore(a,info.kind)).slice(0,8);
    let error="请进入原文件夹，扩展将自动匹配未完成文件，无需手动勾选。";
    for(const tab of tabs){
      let result;try{
        if (info.kind !== "百度网盘") await wakeCloudTab(tab);
        for(let attempt=0;attempt<3;attempt++){
          result=await boundedCloudRead(info,tab);if(!result?.notFound)break;
          if(attempt<2)await new Promise(r=>setTimeout(r,600));
        }
      }catch{continue}
      if(!result?.files?.length){if(result?.error && !result.notFound)error=result.error;continue;}
      const names=new Set(result.files.map(f=>f.name));
      if(result.files.length!==info.files.length || names.size!==info.files.length || !info.files.every(f=>names.has(f.name)))continue;
      const applied=await refreshRpc("apply",{token:job.token,files:result.files});
      if(applied.ok){await rememberCloudFolder(info,tab);await putRefreshJobs((await refreshJobs()).filter(x=>x.token!==job.token));return;}
      error=applied.error || "换链未完成，原进度保留";if(!/不是同一资源|文件名.*不匹配/.test(error))break;
    }
    await refreshNotice(job,error);
    const jobs=await refreshJobs();const i=jobs.findIndex(x=>x.token===job.token);if(i>=0){jobs[i].lastMessage=job.lastMessage;await putRefreshJobs(jobs)}
  } catch { await refreshNotice(job,"自动取链暂未完成，请确认闪电下载已打开及网盘已登录。"); }
  finally {CLOUD_REFRESH_BUSY.delete(job.token);}
}
async function startCloudRefreshInTab(token,tab) {
  if(!tab?.id || !/^[a-f0-9]{32}$/.test(token || ""))return false;
  const data=await refreshRpc("info",{token});
  if(!data.info || !(CLOUD_REFRESH_HOSTS[data.info.kind] || []).includes(new URL(tab.url).hostname))return false;
  const jobs=await refreshJobs();
  if(jobs.some(job=>job.token===token))return true;
  const job={token,at:Date.now(),openerTabId:tab.id,baidu:data.info.kind === "百度网盘" && (data.info.baiduResume === true || data.info.files?.length === 1)};
  const keep = job.baidu ? jobs.filter(j => !j.baidu || j.openerTabId !== tab.id) : jobs;
  keep.push(job);
  await putRefreshJobs(keep);
  if (job.baidu) {
    runBaiduRefresh(job).then(result => {
      if (!result.ok) return refreshNotice(job, result.error);
    }).catch(()=>{});
  } else {
    runCloudRefresh(job).catch(()=>{});
  }
  return true;
}
chrome.runtime.onMessage.addListener((message,sender,reply)=>{
  if(message.type!=="cloudRefreshBegin")return;
  startCloudRefreshInTab(message.token,sender.tab).then(ok=>reply({ok}),()=>reply({ok:false}));return true;
});
chrome.tabs.onUpdated.addListener((id,change)=>{if(change.url || change.status==="complete")refreshJobs().then(jobs=>jobs.filter(job=>!job.baidu).forEach(job=>runCloudRefresh(job))).catch(()=>{});});
chrome.alarms.onAlarm.addListener(alarm=>{if(alarm.name==="cloud-refresh")refreshJobs().then(jobs=>jobs.filter(job=>!job.baidu).forEach(job=>runCloudRefresh(job))).catch(()=>{});});
