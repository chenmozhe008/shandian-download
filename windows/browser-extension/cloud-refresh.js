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
  if(jobs.length) chrome.alarms.create("cloud-refresh",{periodInMinutes:0.5});
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
    const data={list};
    const token=await baiduToken();
    const files=matches.map(x=>{const f=(data.list||[]).find(f=>String(f.fs_id)===String(x.f.id));return f?.dlink?{name:x.name,url:f.dlink+"&access_token="+encodeURIComponent(token),headers:[["User-Agent",BAIDU_UA]]}:null});
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
    if(folder && tabs.length){
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
        await wakeCloudTab(tab);
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
  const job={token,at:Date.now(),openerTabId:tab.id};
  jobs.push(job);await putRefreshJobs(jobs);
  runCloudRefresh(job).catch(()=>{});
  return true;
}
chrome.runtime.onMessage.addListener((message,sender,reply)=>{
  if(message.type!=="cloudRefreshBegin")return;
  startCloudRefreshInTab(message.token,sender.tab).then(ok=>reply({ok}),()=>reply({ok:false}));return true;
});
chrome.tabs.onUpdated.addListener((id,change)=>{if(change.url || change.status==="complete")refreshJobs().then(jobs=>jobs.forEach(job=>runCloudRefresh(job))).catch(()=>{});});
chrome.alarms.onAlarm.addListener(alarm=>{if(alarm.name==="cloud-refresh")refreshJobs().then(jobs=>jobs.forEach(job=>runCloudRefresh(job))).catch(()=>{});});
