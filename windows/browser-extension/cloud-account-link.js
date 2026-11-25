// An explicit, one-time account association initiated by the native app.
// Public tickets may survive worker suspension; credentials never enter storage.
const ACCOUNT_LINK_NATIVE = "http://127.0.0.1:6800/extension/cloud-account/";
const ACCOUNT_LINK_FLIGHTS = new Map();
const ACCOUNT_LINK_DONE = new Map();
const ACCOUNT_LINK_CONTEXTS = new Map();
const ACCOUNT_LINK_CONTEXT_FLIGHTS = new Map();
const accountLinkKey = tabId => "cloudAccountLink:"+tabId;
const accountLinkTicket = value => typeof value==="string" && /^[a-f0-9]{32}$/.test(value);

function accountLinkPageTicket(value) {
  try { const page=new URL(value);return accountLinkTicket(page.searchParams.get("shandian-account-link")) ? page.searchParams.get("shandian-account-link") : ""; }
  catch { return ""; }
}
function accountLinkPublicContext(context, site) {
  if(!context?.ok || context.provider!==site.id || !["local","nas"].includes(context.target))return null;
  const login=cloudAccountLinkSite(context.loginUrl);
  if(!login || login.id!==site.id)return null;
  let official=null;
  if(context.official){
    const url=new URL(context.loginUrl);
    if(site.id!=="pikpak" || context.target!=="nas" || context.transferSession!==false || url.origin!=="https://mypikpak.com" || url.pathname!=="/drive/activate" || !/^[a-z0-9_-]{1,64}$/i.test(context.official.code || "") || !Number.isSafeInteger(context.official.expiresAt))return null;
    official={code:context.official.code,expiresAt:context.official.expiresAt};
  }else if(context.transferSession!==site.transferSession)return null;
  const resumeToken=!official && context.transferSession===false && accountLinkTicket(context.resumeToken) ? context.resumeToken : null;
  return {ok:true,provider:site.id,name:site.name,target:context.target,targetLabel:context.target==="local"?"本机":String(context.targetLabel || "NAS").slice(0,100),transferSession:context.transferSession,completed:context.state==="ready" || context.completed===true,...(official?{official}:{}),...(resumeToken?{resumeToken}:{})};
}
async function accountLinkBlocksLegacyPikPak(tab) {
  if(!tab?.id || cloudAccountLinkSite(tab.url)?.id!=="pikpak")return false;
  const page=new URL(tab.url);
  const stored=(await chrome.storage.session.get(accountLinkKey(tab.id)))[accountLinkKey(tab.id)];
  const blocked=page.searchParams.has("shandian-account-link") || (stored?.provider==="pikpak" && accountLinkTicket(stored.ticket));
  if(blocked)await chrome.storage.session.remove("pikpakLink"+tab.id);
  return blocked;
}
async function accountLinkNativeContext(ticket,site,fresh=false,retry=false) {
  const cached=ACCOUNT_LINK_CONTEXTS.get(ticket);
  if(!fresh && cached && cached.provider===site.id && Date.now()-cached.time<30000)return cached.context;
  if(ACCOUNT_LINK_CONTEXT_FLIGHTS.has(ticket))return ACCOUNT_LINK_CONTEXT_FLIGHTS.get(ticket);
  const pending=(async()=>{
    const response=await fetch(ACCOUNT_LINK_NATIVE+"link-context",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(retry?{ticket,retry:true}:{ticket}),cache:"no-store"});
    const value=await response.json();
    const context=accountLinkPublicContext(value,site);
    if(!response.ok || !context)throw new Error("关联请求已失效，请回闪电重新发起关联。");
    ACCOUNT_LINK_CONTEXTS.set(ticket,{provider:site.id,time:Date.now(),context});
    while(ACCOUNT_LINK_CONTEXTS.size>64)ACCOUNT_LINK_CONTEXTS.delete(ACCOUNT_LINK_CONTEXTS.keys().next().value);
    return context;
  })();
  ACCOUNT_LINK_CONTEXT_FLIGHTS.set(ticket,pending);
  try{return await pending}finally{ACCOUNT_LINK_CONTEXT_FLIGHTS.delete(ticket)}
}
async function accountLinkContextFor(tab,fresh=false,retry=false) {
  const site=cloudAccountLinkSite(tab?.url);
  if(!tab?.id || !site)return null;
  const pageTicket=accountLinkPageTicket(tab.url);
  const stored=(await chrome.storage.session.get(accountLinkKey(tab.id)))[accountLinkKey(tab.id)];
  const popupOverride=stored?.popupOverride===true && stored.provider===site.id && stored.urlTicket===pageTicket && accountLinkTicket(stored.ticket);
  let ticket=popupOverride?stored.ticket:pageTicket;
  if(ticket){if(!popupOverride)await chrome.storage.session.set({[accountLinkKey(tab.id)]:{ticket,provider:site.id,autoLink:true}});}
  else {
    if(!accountLinkTicket(stored?.ticket) || stored.provider!==site.id)return null;
    ticket=stored.ticket;
  }
  const context=await accountLinkNativeContext(ticket,site,fresh,retry);
  if(context.provider!==site.id)throw new Error("请在对应网盘页面完成关联。");
  if(context.official && new URL(tab.url).pathname!=="/drive/activate")return null;
  return {...context,ticket,autoLink:!popupOverride && (!!pageTicket || stored?.autoLink===true)};
}

// Serialized by chrome.scripting, and therefore self-contained. Check the
// origin again inside the page so a navigation cannot change the credential source.
function accountLinkReadPikPak(expectedOrigin) {
  if(location.origin!==expectedOrigin || location.protocol!=="https:")return null;
  const prefix="credentials_YUMx5nI8ZU8Ap8pm";
  const sessions=[];
  for(let i=0;i<localStorage.length;i++) {
    const key=localStorage.key(i);
    if(key!==prefix && !key.startsWith(prefix+"_"))continue;
    let value;try{value=JSON.parse(localStorage.getItem(key) || "null")}catch{continue}
    if(!value?.access_token || !value?.refresh_token)continue;
    const credentials={};
    for(const name of ["access_token","refresh_token","expires_in","expires_at","token_type","sub","user_id","username","email"])if(["string","number"].includes(typeof value[name]))credentials[name]=value[name];
    sessions.push({key,c:credentials,device:localStorage.getItem("deviceid") || ""});
  }
  const exact=sessions.find(session=>session.key===prefix);
  return exact || (sessions.length===1?sessions[0]:null);
}
function accountLinkReadBrowserToken(kind,expectedOrigin) {
  if(location.origin!==expectedOrigin || location.protocol!=="https:" || kind!=="pan123")return null;
  let token=localStorage.getItem("authorToken");
  try{const parsed=JSON.parse(token);if(typeof parsed==="string")token=parsed}catch{}
  if(typeof token!=="string")return null;
  token=token.trim().replace(/^Bearer\s+/i,"");
  if(!token || token.length>16384 || /[\s\x00-\x1f\x7f]/.test(token) || ["null","undefined"].includes(token))return null;
  return {AccessToken:token};
}
function accountLinkClearPikPak(expectedOrigin,key,access,refresh) {
  if(location.origin!==expectedOrigin || location.protocol!=="https:")return false;
  try {
    const value=JSON.parse(localStorage.getItem(key) || "null");
    if(value?.access_token!==access || value?.refresh_token!==refresh)return false;
    localStorage.removeItem(key);return true;
  } catch {return false}
}
function accountLinkCookieMatches(cookie,site,partition) {
  const host=site.currentHost || new URL(site.home).hostname,domain=String(cookie.domain || "").replace(/^\./,"").toLowerCase();
  const allowed=site.cookieDomains || [site.cookieDomain];
  if(!domain || !(host===domain || host.endsWith("."+domain)) || !allowed.some(root=>domain===root || domain.endsWith("."+root)))return false;
  if(cookie.partitionKey && (!partition || cookie.partitionKey.topLevelSite!==partition.topLevelSite || !!cookie.partitionKey.hasCrossSiteAncestor!==!!partition.hasCrossSiteAncestor))return false;
  return /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(cookie.name || "") && typeof cookie.value==="string" && !/[\x00-\x1f\x7f;]/.test(cookie.value);
}
async function accountLinkReadCookies(tab,site) {
  const stores=await chrome.cookies.getAllCookieStores();
  const store=stores.find(entry=>entry.tabIds?.includes(tab.id));
  if(!store)throw new Error("未能识别当前浏览器登录，请从软件重新打开关联页。");
  site={...site,currentHost:new URL(tab.url).hostname};
  const query={url:new URL(tab.url).origin+"/",storeId:store.id};
  let partition=null;
  if(typeof chrome.cookies.getPartitionKey==="function") {
    try {
      const result=await chrome.cookies.getPartitionKey({tabId:tab.id,frameId:0}), candidate=result?.partitionKey || result;
      const top=candidate?.topLevelSite && new URL(candidate.topLevelSite);
      if(top?.protocol==="https:" && ((site.cookieDomains || [site.cookieDomain]).includes(top.hostname) || site.hosts.includes(top.hostname)))partition=candidate;
    } catch {}
  }
  let cookies=await chrome.cookies.getAll(query);
  if(partition)cookies=cookies.concat(await chrome.cookies.getAll({...query,partitionKey:partition}));
  const seen=new Set();
  cookies=cookies.filter(cookie=>{
    if(!accountLinkCookieMatches(cookie,site,partition))return false;
    const identity=JSON.stringify([cookie.name,cookie.domain,cookie.path,cookie.partitionKey || null]);
    if(seen.has(identity))return false;seen.add(identity);return true;
  }).sort((a,b)=>(b.path || "/").length-(a.path || "/").length);
  if(site.id==="mobile") {
    const authorizationCookie=cookies.find(cookie=>cookie.name==="authorization");
    if(!authorizationCookie?.value)return null;
    let authorization=authorizationCookie.value;try{authorization=decodeURIComponent(authorization)}catch{}
    authorization=authorization.trim().replace(/^Basic\s+/i,"");
    if(authorization.length<8 || !/^[A-Za-z0-9+/]+={0,2}$/.test(authorization))return null;
    const fields={authorization},domain=cookies.find(cookie=>cookie.name==="ud_id")?.value;
    if(domain)fields.user_domain_id=domain;return fields;
  }
  if(site.id==="pan115") {
    if(!["UID","CID","SEID"].every(name=>cookies.some(cookie=>cookie.name===name && cookie.value)))return null;
  }else if(!cookies.some(cookie=>["__puus","__pus"].includes(cookie.name) && cookie.value))return null;
  return {cookie:cookies.map(cookie=>cookie.name+"="+cookie.value).join("; ")};
}
function accountLinkError(message,fields) {
  let text=String(message || "关联未完成，请在软件中检查登录状态。");
  const scrub=value=>{
    if(typeof value==="string" && value.length>3)text=text.split(value).join("<已隐藏>");
    else if(value && typeof value==="object")Object.values(value).forEach(scrub);
  };
  scrub(fields);
  if(/https?:|access_token|refresh_token|client_secret|authorization|cookie|eyJ[A-Za-z0-9_-]{12}/i.test(text))return "关联未完成，请在软件中检查登录状态。";
  return text.replace(/<[^>]*>/g,"").slice(0,180);
}
function accountLinkClearGuangya(expectedOrigin,key,access,refresh) {
  if(location.origin!==expectedOrigin || location.protocol!=="https:" || !String(key).startsWith("credentials_aMe-8VSlkrbQXpUR"))return false;
  try{const current=JSON.parse(localStorage.getItem(key)||"null");if(current?.access_token!==access || current?.refresh_token!==refresh)return false;localStorage.removeItem(key);return true;}catch{return false;}
}
async function accountLinkRun(tab,context) {
  const site=cloudAccountLinkSite(tab.url),origin=new URL(tab.url).origin;
  let fields=null,sourceSession=null;
  try {
    if(!site || site.id!==context.provider)return {ok:false,error:"请在对应网盘页面完成关联。"};
    if(context.official){
      if(!context.autoLink)return {ok:false,error:"请回到闪电打开的 PikPak 官方授权页。"};
      fields={};
    } else if(site.id==="pikpak") {
      const [result]=await chrome.scripting.executeScript({target:{tabId:tab.id},world:"MAIN",func:accountLinkReadPikPak,args:[origin]});
      fields=result?.result || null;
    } else if(site.id==="guangya") {
      const [result]=await chrome.scripting.executeScript({target:{tabId:tab.id},world:"MAIN",func:nasGuangyaSession,args:[origin]});
      sourceSession=result?.result || null;fields=sourceSession?.fields || null;
    } else if(site.id==="pan123") {
      const [result]=await chrome.scripting.executeScript({target:{tabId:tab.id},world:"MAIN",func:accountLinkReadBrowserToken,args:[site.id,origin]});fields=result?.result || null;
    } else fields=await accountLinkReadCookies(tab,site);
    if(!fields)return {ok:false,needsLogin:true,error:`请先在当前网页登录${site.name}，再点击关联。`};
    const latest=await chrome.tabs.get(tab.id);
    if(new URL(latest.url).origin!==origin || cloudAccountLinkSite(latest.url)?.id!==site.id)return {ok:false,error:"页面已切换，请在原网盘页面重新关联。"};
    const options={method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({ticket:context.ticket,provider:site.id,fields})};
    if(typeof AbortSignal!=="undefined" && typeof AbortSignal.timeout==="function")options.signal=AbortSignal.timeout(65000);
    const response=await fetch(ACCOUNT_LINK_NATIVE+"link",options),result=await response.json();
    if(context.official && response.ok && result?.ok && result.provider===site.id && result.target===context.target && result.state==="pending")return {ok:false,officialPending:true};
    if(!response.ok || !result?.ok || result.provider!==site.id || result.target!==context.target || (result.state && result.state!=="ready"))return {ok:false,error:accountLinkError(result?.error || result?.message,fields)};
    let sourceCleared=false;
    if(site.id==="pikpak" && context.transferSession) {
      const [cleared]=await chrome.scripting.executeScript({target:{tabId:tab.id},world:"MAIN",func:accountLinkClearPikPak,args:[origin,fields.key,fields.c.access_token,fields.c.refresh_token]}).catch(()=>[]);
      sourceCleared=cleared?.result===true;
    } else if(site.id==="guangya" && context.transferSession && sourceSession) {
      const [cleared]=await chrome.scripting.executeScript({target:{tabId:tab.id},world:"MAIN",func:accountLinkClearGuangya,args:[origin,sourceSession.key,fields.access_token,fields.refresh_token]}).catch(()=>[]);
      sourceCleared=cleared?.result===true;
    }
    return {ok:true,provider:site.id,target:context.target,state:"ready",sourceCleared,message:context.transferSession&&!sourceCleared?`已关联到${context.targetLabel}；当前网页登录状态已变化，未清除网页端登录。`:`已关联到${context.targetLabel}，可以关闭此页。`};
  } catch {return {ok:false,error:"关联未完成，请确认软件已打开，再检查当前网页登录状态。"}}
}
async function accountLinkSubmit(tab,retry=false) {
  let context;
  try{context=await accountLinkContextFor(tab,retry,retry)}catch{return {ok:false,requestExpired:true,error:"关联请求已失效，请回闪电重新发起关联。"}}
  if(!context)return {ok:false,error:"请先从软件的云盘配置发起关联。"};
  if(context.completed){
    if(context.autoLink)await accountLinkContinueInTab(tab,context);
    return {ok:true,provider:context.provider,target:context.target,state:"ready",alreadyLinked:true,message:"此次关联已完成，请回软件查看账号状态。"};
  }
  const done=ACCOUNT_LINK_DONE.get(context.ticket);
  if(done)return done.tabId===tab.id?done.result:{ok:false,error:"这次关联已在另一个标签页完成，请从软件重新发起。"};
  const flight=ACCOUNT_LINK_FLIGHTS.get(context.ticket);
  if(flight)return flight.tabId===tab.id?flight.promise:{ok:false,error:"这次关联正在另一个标签页进行。"};
  const promise=(async()=>{
    try {
      const fresh=retry ? context : await accountLinkContextFor(tab,true);
      if(!fresh || fresh.ticket!==context.ticket || fresh.provider!==context.provider || fresh.target!==context.target || fresh.targetLabel!==context.targetLabel)return {ok:false,error:"关联目标已变化，请从软件重新发起。"};
      return await accountLinkRun(tab,fresh);
    } catch {return {ok:false,error:"关联请求已失效，请回闪电重新发起关联。"}}
  })();
  ACCOUNT_LINK_FLIGHTS.set(context.ticket,{tabId:tab.id,promise});
  try {
    const result=await promise;
    if(result.ok){
      const cached=ACCOUNT_LINK_CONTEXTS.get(context.ticket);
      if(cached)cached.context={...cached.context,completed:true};
      ACCOUNT_LINK_DONE.set(context.ticket,{tabId:tab.id,result});while(ACCOUNT_LINK_DONE.size>64)ACCOUNT_LINK_DONE.delete(ACCOUNT_LINK_DONE.keys().next().value);
      if(context.autoLink && result.state==="ready")await accountLinkContinueInTab(tab,context);
    } else if(context.autoLink && context.resumeToken && !result.needsLogin && !result.requestExpired &&
        await startCloudRefreshInTab(context.resumeToken,tab).catch(()=>false)) {
      return {...result,resumeFallback:true,message:"账号关联未完成，正在尝试继续本次下载"};
    }
    return result;
  } finally {ACCOUNT_LINK_FLIGHTS.delete(context.ticket)}
}

async function accountLinkContinueInTab(tab,context) {
  if(context.resumeToken){
    await startCloudRefreshInTab(context.resumeToken,tab).catch(()=>false);
    return;
  }
  await accountLinkClosePage(tab,context);
}

async function accountLinkClosePage(tab,context) {
  // A user may repurpose the tab while the app verifies the account.
  // Close only the page that completed this app-opened request.
  try {
    const current=await chrome.tabs.get(tab.id);
    const currentUrl=current.pendingUrl || current.url;
    if(!accountLinkCanCloseAt(currentUrl,tab.url,context))return;
    const key=accountLinkKey(tab.id);
    const stored=(await chrome.storage.session.get(key))[key];
    if(stored?.autoLink && stored.ticket===context.ticket && stored.provider===context.provider){
      await chrome.tabs.remove(tab.id);
    }
  } catch { /* A closed or moved tab does not change the association result. */ }
}

function accountLinkCanCloseAt(current,submitted,context) {
  if(cloudAccountLinkSite(current)?.id!==context.provider)return false;
  const ticket=accountLinkPageTicket(current);
  if(ticket && ticket!==context.ticket)return false;
  const url=new URL(current), previous=new URL(submitted);
  // Some providers remove the query and replace the site route once logged in.
  // Keep tabs that have navigated to a file, a shared link, or another folder.
  if(url.origin!==previous.origin)return false;
  if(context.official)return url.pathname==="/drive/activate" && [...url.searchParams.keys()].every(key=>["client_id","scope","shandian-account-link"].includes(key));
  if(current===submitted && ticket===context.ticket)return true;
  if(context.provider==="pikpak" && url.pathname==="/drive/login"){
    return !url.hash && [null,"/all","/drive/all"].includes(url.searchParams.get("redirect")) && [...url.searchParams.keys()].every(key=>["redirect","shandian-account-link"].includes(key));
  }
  if([...url.searchParams.keys()].some(key=>key!=="mode" && key!=="shandian-account-link"))return false;
  const landing={pikpak:["/drive","/drive/all"],quark:["/","/list"],uc:["/","/list"],guangya:["/"],pan115:["/"],pan123:["/"],mobile:["/w"]}[context.provider];
  if(!landing?.includes(url.pathname.replace(/\/$/,"") || "/"))return false;
  if(url.hash && !(context.provider==="guangya" && url.hash==="#/home/all"))return false;
  return url.origin===previous.origin;
}

// The extension popup uses the same broker as the page, including older tabs
// whose content scripts were not reinjected after the extension was reloaded.
async function accountLinkPopupContext(tab) {
  const site=cloudAccountLinkSite(tab?.url);
  if(!site || !tab?.id)return {ok:false,inactive:true};
  try{const attached=await accountLinkContextFor(tab);if(attached)return attached;}catch{}
  const response=await fetch(ACCOUNT_LINK_NATIVE+"popup-context",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({provider:site.id})});
  if(response.status===404 && site.id==="pikpak")return {ok:true,legacy:true,provider:site.id,name:site.name,target:"local",targetLabel:"本机",transferSession:true};
  const value=await response.json();
  if(response.ok && value?.ok && value.inactive)return {ok:true,inactive:true,provider:site.id,name:site.name,target:"local",targetLabel:"本机",transferSession:site.transferSession};
  const context=accountLinkPublicContext(value,site);
  if(!response.ok || !context || !accountLinkTicket(value.ticket))throw new Error(value?.error || "请先打开闪电全能下载，再点关联");
  await chrome.storage.session.set({[accountLinkKey(tab.id)]:{ticket:value.ticket,provider:site.id,popupOverride:true,urlTicket:accountLinkPageTicket(tab.url)}});
  ACCOUNT_LINK_CONTEXTS.set(value.ticket,{provider:site.id,time:Date.now(),context});
  return {...context,ticket:value.ticket};
}
async function accountLinkPopupSubmit(tab,expectedTarget) {
  let context=await accountLinkPopupContext(tab);
  if(!context?.ok)return {ok:false,error:"请打开对应网盘网页再关联"};
  if(context.target!==expectedTarget)return {ok:false,error:"关联目标已变化，请重新打开扩展确认"};
  if(context.legacy && context.provider==="pikpak"){
    const accepted=await linkPikPak(tab.id);
    return {ok:accepted===true,state:accepted?"ready":"error",provider:"pikpak",target:"local",message:accepted?"已关联到本机，可以关闭此窗口。":"",error:accepted?"":"请先在PikPak网页完成登录，再重试关联。"};
  }
  if(context.inactive){
    if(expectedTarget!=="local")return {ok:false,error:"NAS关联请求已失效，请从软件重新发起"};
    const response=await fetch(ACCOUNT_LINK_NATIVE+"begin-local",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({provider:context.provider})});
    const value=await response.json(),site=cloudAccountLinkSite(tab.url),fresh=accountLinkPublicContext(value,site);
    if(!response.ok || !fresh || fresh.target!==expectedTarget || !accountLinkTicket(value.ticket))return {ok:false,error:value?.error || "关联入口未就绪，请重试"};
    await chrome.storage.session.set({[accountLinkKey(tab.id)]:{ticket:value.ticket,provider:site.id,popupOverride:true,urlTicket:accountLinkPageTicket(tab.url)}});
    ACCOUNT_LINK_CONTEXTS.set(value.ticket,{provider:site.id,time:Date.now(),context:fresh});
  }
  return accountLinkSubmit(tab);
}
chrome.runtime.onMessage.addListener((message,sender,reply)=>{
  if(!["cloudAccountLinkContext","cloudAccountLinkSubmit","cloudAccountLinkClose"].includes(message?.type))return;
  const popup=!sender.tab && sender.id===chrome.runtime.id && sender.url===`chrome-extension://${chrome.runtime.id}/popup.html`;
  if((!sender.tab?.id && !popup) || (sender.id && sender.id!==chrome.runtime.id)){reply({ok:false,error:"请在软件打开的网盘页面关联。"});return;}
  const task=(async()=>{
    const tab=popup?await chrome.tabs.get(message.tabId):sender.tab;
    if(popup){
      if(message.type==="cloudAccountLinkClose")return {ok:false,inactive:true};
      return message.type==="cloudAccountLinkContext"?accountLinkPopupContext(tab):accountLinkPopupSubmit(tab,message.target);
    }
    if(sender.frameId && sender.frameId!==0)return {ok:false,inactive:true};
    if(message.type==="cloudAccountLinkClose"){
      const context=await accountLinkContextFor(tab,true);
      if(context?.autoLink && context.completed)await accountLinkContinueInTab(tab,context);
      return {ok:context?.completed===true};
    }
    return message.type==="cloudAccountLinkContext"?accountLinkContextFor(tab):accountLinkSubmit(tab,message.retry===true);
  })();
  task.then(value=>reply(value || {ok:false,inactive:true}),()=>reply({ok:false,error:"关联未完成，请确认软件已打开，再重试"}));return true;
});
chrome.tabs.onUpdated.addListener(async(tabId,change,tab)=>{
  const site=cloudAccountLinkSite(change.url || tab?.url),ticket=accountLinkPageTicket(change.url || tab?.url);
  if(ticket && site){
    const stored=(await chrome.storage.session.get(accountLinkKey(tabId)))[accountLinkKey(tabId)];
    if(!(stored?.popupOverride && stored.provider===site.id && stored.urlTicket===ticket))await chrome.storage.session.set({[accountLinkKey(tabId)]:{ticket,provider:site.id,autoLink:true}});
  }
  if((change.status==="complete" || change.url) && site)chrome.tabs.sendMessage(tabId,{type:"cloudAccountLinkContextChanged"}).catch(()=>{});
});
chrome.tabs.onRemoved?.addListener(tabId=>{chrome.storage.session.remove(accountLinkKey(tabId)).catch(()=>{});});
