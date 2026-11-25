// Capture only official file URLs produced by the site's own download action, never preview playback.
function isSiteDownloadURL(value) {
  try {
    const u = new URL(value);
    const aigei = (u.hostname === "aigei.com" || u.hostname.endsWith(".aigei.com")) && /^\/src\/aud\//.test(u.pathname) && /\.(mp3|wav|m4a|ogg|aac|flac|mp4)$/i.test(u.pathname) && /^\?download[=/]/i.test(u.search);
    return u.protocol === "https:" && (aigei || (u.hostname === "cdn.pixabay.com"
      && /\.(mp3|wav|m4a|ogg|aac|flac|mp4|webm|mov)$/i.test(u.pathname)
      && (/^\/download\/(audio|video)\//.test(u.pathname)
        || ((!!u.searchParams.get("filename") || u.searchParams.has("download")) && /^\/(audio|video)\//.test(u.pathname)))));
  } catch (_) { return false; }
}

function siteDownloadFileName(url, title, fallback) {
  const u = new URL(url);
  let original;
  try { original = decodeURIComponent(u.search.match(/^\?download[=/]([^&]+)/i)?.[1] || ""); } catch (_) {}
  const supplied = u.searchParams.get("filename") || original || fallback || u.pathname.split("/").pop() || "";
  const ext = supplied.match(/\.(mp3|wav|m4a|ogg|aac|flac|mp4|webm|mov)$/i)?.[1]
    || u.pathname.match(/\.([a-z0-9]+)$/i)?.[1] || "mp3";
  const clean = value => String(value || "").replace(/[<>:"/\\|?*\u0000-\u001f]/g," ").replace(/\s+/g," ").trim().replace(/[. ]+$/g,"").slice(0,170);
  const base = title ? clean(title).replace(/\.(mp3|wav|m4a|ogg|aac|flac|mp4|webm|mov)$/i,"") : "";
  const safe = clean(supplied);
  return base ? `${base}.${ext}` : new RegExp(`\\.${ext}$`,"i").test(safe) ? safe : `${safe || "音频"}.${ext}`;
}

const SITE_GUARD_MIN=900000000,SITE_GUARD_MAX=1800000000;
// Read the selected card's original-file URL already supplied to Pixabay's own
// download component. Do not invoke its navigation callback or use player src.
async function pixabayOriginalFromPage(requestId,itemHref,collectOnly=false,allowOriginalFlow=false) {
  if(!/^(www\.)?pixabay\.com$/.test(location.hostname))return null;
  const canonical=value=>{if(typeof value!=='string'||!value)return '';try{const u=new URL(value,location.href);return u.origin+u.pathname.replace(/\/$/,'')}catch(_){return ''}};
  const expected=canonical(itemHref);
  const selected=[...document.querySelectorAll('[data-shandian-request]')].find(el=>el.getAttribute('data-shandian-request')===requestId);
  if(!selected || !expected)return {pixabay:true};
  const originalURL=value=>{try{const u=new URL(value,location.href);return u.protocol==='https:' && u.hostname==='cdn.pixabay.com' && /\.(mp3|wav|m4a|ogg|aac|flac|mp4|webm|mov)$/i.test(u.pathname)
    && (/^\/download\/(audio|video)\//.test(u.pathname) || (/^\/(audio|video)\//.test(u.pathname) && (u.searchParams.has('download') || !!u.searchParams.get('filename')))) ? u.href : ''}catch(_){return ''}};
  const seen=new Set();
  for(let node=selected.parentElement,depth=0;node && depth<8;node=node.parentElement,depth++){
    const key=Object.keys(node).find(k=>k.startsWith('__reactFiber$') || k.startsWith('__reactInternalInstance$'));
    for(let fiber=key && node[key],level=0;fiber && level<60;fiber=fiber.return,level++){
      if(seen.has(fiber))break;seen.add(fiber);
      const props=fiber.memoizedProps;
      if(!props || typeof props!=='object')continue;
      for(const media of [props.track,props.result,props.media,props.item,props]){
        if(!media || canonical(media.href)!==expected || typeof media.sources?.downloadUrl!=='string')continue;
        const url=media.sources.downloadUrl;
        try{
          const u=new URL(url,location.href);
          const directUrl=originalURL(u.href);
          if(directUrl)return {pixabay:true,directUrl};
          // This endpoint returns an HTML handoff page, whose script would
          // navigate to the CDN. Read it as inert text instead of opening it.
          if(u.origin===location.origin && /^\/(music|sound-effects|videos)\/download\//.test(u.pathname)){
            if(collectOnly && allowOriginalFlow)return {pixabay:true,flowUrl:new URL(itemHref,location.href).href};
            const response=await fetch(u.href,{method:'GET',credentials:'same-origin',redirect:'manual',signal:AbortSignal.timeout(7000)});
            if(response.type==='opaqueredirect')return {pixabay:true,error:'原站要求跳转验证，已停止批量取链'};
            if(!response.ok)return {pixabay:true,error:`原站拒绝取链（HTTP ${response.status}），请回素材页检查`};
            if(!response.headers.get('content-type')?.includes('text/html')){await response.body?.cancel();return {pixabay:true,error:'原站取链接口已变化，请使用单项下载'};}
            const html=(await response.text()).replace(/\\\//g,'/').replace(/\\u([0-9a-f]{4})/gi,(_,hex)=>String.fromCharCode(parseInt(hex,16))).replace(/&amp;/g,'&');
            if(/recaptcha|hcaptcha|grecaptcha|turnstile|challenges\.cloudflare\.com|Just a moment|cf-chl-/i.test(html))return {pixabay:true,error:'原站需要验证，请在原站继续'};
            const candidates=[...new Set((html.match(/(?:https:)?\/\/cdn\.pixabay\.com\/[^\s"'<>\\]+/g)||[]).map(originalURL).filter(Boolean))];
            const named=candidates.filter(value=>new URL(value).searchParams.get('filename')===media.sources.filename);
            const finalUrl=candidates.length===1?candidates[0]:named.length===1?named[0]:'';
            return finalUrl?{pixabay:true,directUrl:finalUrl}:{pixabay:true,error:'原站未返回可交接的原文件地址'};
          }
        }catch(_){return {pixabay:true,error:'原站取链失败，请回素材页确认可正常下载'};}
      }
    }
  }
  return {pixabay:true};
}
async function siteFlowNavigation() {
  const originalOpen=window.open,originalClick=HTMLAnchorElement.prototype.click;
  const download=value=>{try{const u=new URL(value,location.href);return (u.protocol==='https:' && ((u.hostname===location.hostname && /^\/(music|sound-effects|videos)\/download\//.test(u.pathname)) || (u.hostname==='cdn.pixabay.com' && (/^\/download\/(audio|video)\//.test(u.pathname) || u.searchParams.has('download') || u.searchParams.has('filename')))))?u.href:''}catch(_){return ''}};
  window.open=function(value,...args){const url=download(value);if(url){location.assign(url);return window;}return originalOpen.call(this,value,...args)};
  HTMLAnchorElement.prototype.click=function(){const url=download(this.href);if(url){location.assign(url);return;}return originalClick.call(this)};
  // Server-rendered controls can appear before React attaches their handler.
  const until=Date.now()+10000;
  while(Date.now()<until){
    const button=[...document.querySelectorAll('button')].find(el=>/^(Free download|免费下载)$/i.test(el.innerText?.trim() || ''));
    const props=button && Object.keys(button).find(key=>key.startsWith('__reactProps$'));
    if(props && typeof button[props]?.onClick==='function' && typeof window.turnstile?.render==='function')return;
    await new Promise(resolve=>setTimeout(resolve,100));
  }
  throw Error('原站下载按钮尚未初始化');
}
async function clearSiteDownloadGuards(tabId) {
  const rules=await chrome.declarativeNetRequest.getSessionRules();
  const ids=rules.filter(r=>r.id>=SITE_GUARD_MIN&&r.id<SITE_GUARD_MAX&&(tabId==null || r.condition.tabIds?.includes(tabId))).map(r=>r.id);
  if(ids.length)await chrome.declarativeNetRequest.updateSessionRules({removeRuleIds:ids});
  if(tabId!=null)await chrome.alarms.clear?.('shandian-site-tab-guard:'+tabId);
}
// Retire only this adapter's old blocking rules. Batch resolution no longer starts
// site navigation or installs a network block that can turn it into an error page.
if(typeof chrome!=='undefined' && chrome.declarativeNetRequest?.getSessionRules)clearSiteDownloadGuards().catch(()=>{});
if(typeof chrome!=="undefined" && chrome.alarms?.onAlarm)chrome.alarms.onAlarm.addListener(alarm=>{
  if(alarm.name.startsWith('shandian-site-tab-guard:')){
    const tabId=Number(alarm.name.split(':')[1]);
    if(Number.isInteger(tabId)&&tabId>=0)return clearSiteDownloadGuards(tabId).catch(()=>{});
    return;
  }
  if(!alarm.name.startsWith("shandian-site-guard:"))return;
  const id=Number(alarm.name.split(":")[1]);
  if(id>=SITE_GUARD_MIN&&id<SITE_GUARD_MAX)chrome.declarativeNetRequest.updateSessionRules({removeRuleIds:[id]}).catch(()=>{});
});

async function prepareSiteDownload(tabId, requestId, collectOnly=false,itemHref='',allowOriginalFlow=false) {
  try {
  const lookup=await chrome.scripting.executeScript({target:{tabId},world:'MAIN',args:[requestId,itemHref,collectOnly,allowOriginalFlow],func:pixabayOriginalFromPage});
  const original=lookup?.[0]?.result;
  if(original?.directUrl && isSiteDownloadURL(original.directUrl))return {directUrl:original.directUrl};
  if(collectOnly && allowOriginalFlow && original?.flowUrl)return {flowUrl:original.flowUrl};
  if(collectOnly && !allowOriginalFlow && original?.pixabay)return {needsOriginal:true,reason:original.error || '当前页面未提供正式下载地址'};
  if(collectOnly)throw Error(original?.error || '此素材暂不支持无跳转批量取链，请使用单项下载');
  await chrome.scripting.executeScript({target:{tabId},world:"MAIN",args:[requestId,collectOnly],func:(requestId,collectOnly)=>{
    // One explicit click, two-minute ceiling; restore every hook after the URL is handed off.
    window.__shandianSiteCleanup?.();
    let active=true,timer,frameObserver;
    const previousClick=HTMLAnchorElement.prototype.click, previousOpen=window.open;
    const ownClick=Object.prototype.hasOwnProperty.call(HTMLAnchorElement.prototype,"click");
    const frameSrc=Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype,"src");
    const elementProto=typeof Element!=="undefined"?Element.prototype:null,previousSetAttribute=elementProto?.setAttribute;
    const nativeFrameDownload=window.jsInterface_iframeDown,nativeLocationDownload=window.jsInterface_locationHrefDown;
    const allowed=value=>{
      try { const u=new URL(value,location.href);const aigei=(u.hostname==="aigei.com" || u.hostname.endsWith(".aigei.com")) && /^\/src\/aud\//.test(u.pathname) && /\.(mp3|wav|m4a|ogg|aac|flac|mp4)$/i.test(u.pathname) && /^\?download[=/]/i.test(u.search);return u.protocol==="https:" && (aigei || (u.hostname==="cdn.pixabay.com"
        && /\.(mp3|wav|m4a|ogg|aac|flac|mp4|webm|mov)$/i.test(u.pathname)
        && (/^\/download\/(audio|video)\//.test(u.pathname) || ((!!u.searchParams.get("filename") || u.searchParams.has("download")) && /^\/(audio|video)\//.test(u.pathname))))); }
      catch(_){return false;}
    };
    const cleanup=()=>{
      if(!active)return;active=false;clearTimeout(timer);
      if(HTMLAnchorElement.prototype.click===anchorClick){if(ownClick)HTMLAnchorElement.prototype.click=previousClick;else delete HTMLAnchorElement.prototype.click;}
      if(window.open===openFile)window.open=previousOpen;
      if(frameSrc && Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype,"src")?.set===frameSource)Object.defineProperty(HTMLIFrameElement.prototype,"src",frameSrc);
      if(elementProto?.setAttribute===setAttribute)elementProto.setAttribute=previousSetAttribute;
      if(window.jsInterface_iframeDown===frameDownload)window.jsInterface_iframeDown=nativeFrameDownload;
      if(window.jsInterface_locationHrefDown===locationDownload)window.jsInterface_locationHrefDown=nativeLocationDownload;
      document.removeEventListener("click",onClick,true);
      window.navigation?.removeEventListener("navigate",onNavigate);
      frameObserver?.disconnect();
      if(window.__shandianSiteCleanup===cleanup)delete window.__shandianSiteCleanup;
    };
    const capture=value=>{
      if(!active || !allowed(value))return false;
      const url=new URL(value,location.href).href;cleanup();
      window.postMessage({type:"shandian-site-download",requestId,url},location.origin);
      return true;
    };
    function anchorClick(){if(capture(this.href))return;if(collectOnly&&(this.download || this.target==="_blank")){rejectBatch();return;}return previousClick.call(this);}
    const rejectBatch=()=>{cleanup();window.postMessage({type:"shandian-site-download-error",requestId,error:"原站需要手动处理，已停止批量取链"},location.origin);};
    function openFile(url,...args){if(capture(url))return null;if(collectOnly){rejectBatch();return null;}return previousOpen.call(this,url,...args);}
    function onClick(e){const a=e.target.closest?.("a[href]");if(a && capture(a.href)){e.preventDefault();e.stopImmediatePropagation();}}
    function onNavigate(e){if(e.cancelable && !e.formData && e.navigationType!=="traverse" && allowed(e.destination.url)){e.preventDefault();capture(e.destination.url);}}
    function frameSource(value){if(!capture(value))frameSrc.set.call(this,value);}
    function setAttribute(name,value){if(this instanceof HTMLIFrameElement && String(name).toLowerCase()==="src" && capture(value))return;return previousSetAttribute.call(this,name,value);}
    function frameDownload(info,...args){if(capture(info?.url))return;if(collectOnly){rejectBatch();return;}return nativeFrameDownload.call(this,info,...args);}
    function locationDownload(info,...args){if(capture(info?.url))return;if(collectOnly){rejectBatch();return;}return nativeLocationDownload.call(this,info,...args);}
    if(frameSrc?.configurable && frameSrc.set)Object.defineProperty(HTMLIFrameElement.prototype,"src",{...frameSrc,set:frameSource});
    if(previousSetAttribute)elementProto.setAttribute=setAttribute;
    if(typeof nativeFrameDownload==="function")window.jsInterface_iframeDown=frameDownload;
    if(typeof nativeLocationDownload==="function")window.jsInterface_locationHrefDown=locationDownload;
    HTMLAnchorElement.prototype.click=anchorClick;window.open=openFile;
    document.addEventListener("click",onClick,true);window.navigation?.addEventListener("navigate",onNavigate);
    // Some authorized downloads use a hidden frame. Hand off its formal file URL
    // without waiting for the first CDN response or replacing the visible source page.
    const inspectFrame=frame=>{
      if(frame instanceof HTMLIFrameElement && active && allowed(frame.src)){
        const src=frame.src;if(capture(src))frame.setAttribute("src","about:blank");
      }
    };
    frameObserver=new MutationObserver(records=>{
      for(const record of records){
        if(record.type==="attributes")inspectFrame(record.target);
        for(const node of record.addedNodes||[]){inspectFrame(node);node.querySelectorAll?.("iframe").forEach(inspectFrame);}
      }
    });
    frameObserver.observe(document.documentElement,{subtree:true,childList:true,attributes:true,attributeFilter:["src"]});
    window.__shandianSiteCleanup=cleanup;timer=setTimeout(cleanup,120000);
  }});
  } catch(error) {if(collectOnly)await clearSiteDownloadGuards(tabId).catch(()=>{});throw error; }
}
