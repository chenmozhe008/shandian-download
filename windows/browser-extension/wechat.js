// 用户点下载后才把视频号分享链接提交给腾讯元宝；复用浏览器现有登录，不读取登录凭据。
async function parseWeChatInTab(tabId, share, originTabId = tabId, limits = {}, job = null) {
  let shared;
  try { shared = new URL(share); } catch (_) {}
  if (!shared || shared.protocol !== "https:" || shared.hostname !== "weixin.qq.com" || !/^\/sph\/[\w-]{4,64}$/.test(shared.pathname)) {
    return { ok: false, error: "请使用完整的视频号分享链接。" };
  }
  const tab = await chrome.tabs.get(tabId);
  if (new URL(tab.url).origin !== "https://yuanbao.tencent.com") return { ok: false, error: "请打开腾讯元宝页面后重试。" };
  const deadline = Date.now() + (limits.timeout || 45000);
  let workerTimer;
  const injection = chrome.scripting.executeScript({ target: { tabId }, world: "MAIN", args: [share, limits], func: async (share, limits = {}) => {
    try {
      for (let attempt = 0; attempt < (limits.attempts || 3); attempt++) {
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), limits.timeout || 15000);
      let response;
      try { response = await fetch("/api/weixin/get_parse_result", {
        method: "POST", credentials: "include", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ type: "video_channel_url", url: share, scene: 1 }), signal: controller.signal,
      }); } catch (_) { if (attempt + 1 < (limits.attempts || 3)) { await new Promise(r => setTimeout(r, 800)); continue; } return {}; } finally { clearTimeout(timer); }
      if (response.status === 401) { if (attempt + 1 < (limits.attempts || 3)) { await new Promise(r => setTimeout(r, 1200)); continue; } return { login: true }; }
      if (!response.ok) { if (attempt + 1 < (limits.attempts || 3)) { await new Promise(r => setTimeout(r, 800)); continue; } return {}; }
      const data = await response.json();
      if (data?.data?.playable_url) return { playable: data.data.playable_url, title: data.data.desc || "微信视频号" };
      if (attempt + 1 < (limits.attempts || 3)) await new Promise(r => setTimeout(r, 800));
      }
      return {};
    } catch (_) { return {}; }
  } });
  let result;
  try { result = await Promise.race([injection,new Promise(resolve=>{workerTimer=setTimeout(()=>resolve([]),Math.max(1,deadline-Date.now()));})]); }
  finally { clearTimeout(workerTimer); }
  if (Date.now() >= deadline || job?.cancelled || (job && Date.now() > job.expires)) return {ok:false,error:"解析已超时，请重新点击下载。"};
  const parsed = result?.[0]?.result;
  if (!parsed?.playable) return { ok: false, login: !!parsed?.login, error: parsed?.login
    ? "元宝页面尚未就绪，请查看页面是否需要初始化或验证后重试。"
    : "腾讯元宝暂时未能解析此视频，请稍后重试。" };
  if (job?.beforeHandoff) { await job.beforeHandoff(); job.beforeHandoff=null; }
  if (!(await appReady()) && !(await launchApp(originTabId))) return { ok: false, resolved: true, error: "未能打开闪电全能下载，请打开应用后重试。" };
  if (job?.cancelled || (job && Date.now() > job.expires)) return {ok:false,error:"本次解析已取消或超时，请重试。"};
  if (job) { if (job.delivered) return {ok:true,pending:true,resolved:true}; job.delivered=true; }
  return { ...await extensionBatchRequest("wechat", { share, playable: parsed.playable, title: parsed.title }), resolved: true };
}

const channelRequests = new Map();
function downloadChannel(tab, forceWake = false) {
  const share=wechatShareOf(tab.url);
  if (!share) return Promise.resolve({ok:false,error:"请打开具体的视频号预览页后下载。"});
  const previous=channelRequests.get(share);
  if (previous && Date.now() < previous.expires && !forceWake) return previous.promise;
  if (previous) previous.cancelled=true;
  const job={cancelled:false,delivered:false,expires:Date.now()+60000};
  channelRequests.set(share,job);
  job.promise=runChannel(tab,forceWake,job).finally(()=>{if(channelRequests.get(share)===job)channelRequests.delete(share);});
  return job.promise;
}
async function runChannel(tab, forceWake, job) {
  const share = wechatShareOf(tab.url);
  if (!share) return { ok: false, error: "请打开具体的视频号预览页后下载。" };
  const existing = await chrome.tabs.query({ url: "https://yuanbao.tencent.com/*" });
  let parser = existing.find((t) => t.status === "complete") || existing[0];
  if (!parser) parser = await chrome.tabs.create({ url: "https://yuanbao.tencent.com/", active: false });
  const session = chrome.storage.session;
  const { channelParserWoken = false } = await session.get("channelParserWoken");
  const { channelBackgroundWait = 5 } = await chrome.storage.local.get("channelBackgroundWait");
  const waitMs = ([3,5,10].includes(Number(channelBackgroundWait)) ? Number(channelBackgroundWait) : 5) * 1000;
  const waitReady = async (timeout) => new Promise(resolve => {
    let timer;
    const finish = ok => { clearTimeout(timer); chrome.tabs.onUpdated.removeListener(updated); resolve(ok); };
    const updated = (id, info) => { if (id === parser.id && info.status === "complete") finish(true); };
    chrome.tabs.onUpdated.addListener(updated);
    timer = setTimeout(() => finish(false), timeout);
    chrome.tabs.get(parser.id).then(t => { if (t.status === "complete") finish(true); }).catch(() => finish(false));
  });
  const parse = async limits => {
    try { return await parseWeChatInTab(parser.id, share, tab.id, limits, job); }
    catch (_) { return {ok:false,error:"腾讯元宝页面未就绪，请稍后重试。"}; }
  };
  let result;
  if (channelParserWoken && !forceWake) {
    const began = Date.now();
    if (parser.discarded) await chrome.tabs.reload(parser.id);
    const loaded = parser.status === "complete" && !parser.discarded || await waitReady(waitMs);
    result = loaded ? await parse({attempts:1,timeout:Math.max(100,waitMs-(Date.now()-began))}) : {ok:false};
  }
  if (job.cancelled || Date.now()>job.expires) return {ok:false,error:"解析已超时，请重新点击下载。"};
  if (!result?.ok && !result?.resolved) {
    // 首次直接唤醒；后续只有后台解析超时或失败时才唤醒。
    await session.set({channelParserWoken:true});
    await chrome.tabs.update(parser.id,{active:true});
    if (chrome.windows?.update && parser.windowId != null) {
      const windowState=await chrome.windows.get(parser.windowId).catch(()=>null);
      await chrome.windows.update(parser.windowId,{focused:true,...(windowState?.state==="minimized"?{state:"normal"}:{})}).catch(()=>{});
    }
    const restore = async () => {
      if(job.cancelled || job.restored) return;
      job.restored=true;
      if(tab.id!==parser.id) await chrome.tabs.update(tab.id,{active:true}).catch(()=>{});
      if(chrome.windows?.update && tab.windowId != null && tab.windowId!==parser.windowId) await chrome.windows.update(tab.windowId,{focused:true}).catch(()=>{});
    };
    job.beforeHandoff=restore;

    try {
      if (parser.status !== "complete" || parser.discarded) await waitReady(15000);
      result = await parse({});
    } finally {
      await restore();
    }
  }
  if (result.login) {
    // 首次登录时保留一个明确的继续入口，不让用户再去找扩展面板或粘贴链接。
    await chrome.scripting.executeScript({ target: { tabId: parser.id }, args: [share, parser.id], func: (share, tabId) => {
      document.getElementById("shandian-channel-login")?.remove();
      const box = document.createElement("div"); box.id = "shandian-channel-login"; box.setAttribute("role", "status");
      box.style.cssText = "position:fixed;right:24px;bottom:24px;z-index:2147483647;max-width:340px;padding:16px;border-radius:12px;background:#27272e;color:#fff;font:14px/1.6 system-ui;box-shadow:0 6px 24px #0005;";
      const text = document.createElement("div"); text.textContent = "首次解析会打开元宝完成初始化；页面就绪后点击闪电下载继续。";
      const button = document.createElement("button"); button.type = "button"; button.textContent = "闪电下载";
      button.style.cssText = "display:block;margin-top:12px;padding:8px 12px;border:0;border-radius:8px;background:#e0992f;color:#fff;cursor:pointer;font:inherit;";
      button.onclick = async () => {
        button.disabled = true; button.textContent = "正在解析…";
        try {
          const result = await chrome.runtime.sendMessage({ type: "downloadWeChat", tabId, share });
          text.textContent = result?.pending ? "新建下载窗口已打开，请在应用中确认名称和保存位置。" : result?.ok ? "已加入闪电全能下载，可以关闭本页。" : result?.error || "解析未成功，请确认页面已就绪后重试。";
          button.textContent = result?.pending ? "请在应用中确认" : result?.ok ? "已加入下载" : "闪电下载";
          button.disabled = !!result?.ok;
        } catch (_) { text.textContent = "扩展已更新，请回到视频页刷新后重试。"; button.disabled = false; }
      };
      box.append(text, button); document.body.append(box);
    } });
    return { ...result, error: "元宝页面尚未就绪，请查看页面提示，处理后点击该页的闪电下载继续。" };
  }
  return result;
}

// 公众号卡片可能只带内部 feed ID；只接受卡片里真实存在的公开分享链接，不拼造短链接。
async function articleChannelLinks(tabId) {
  const result = await chrome.scripting.executeScript({target:{tabId},world:"MAIN",func:()=>{
    const cards=[...document.querySelectorAll('mp-common-videosnap, .js_wechannel_video_card')];
    const nodes=[...cards,...document.querySelectorAll('a[href], iframe[src]')];
    const links=new Set();
    for(const node of nodes) for(const key of ['href','src','data-share-url','data-shareurl','data-url','data-href']) {
      const value=node.getAttribute(key); if(!value) continue;
      try { const u=new URL(value.replace(/&amp;/g,'&'),location.href);
        const id=u.hostname==='weixin.qq.com' ? u.pathname.match(/^\/sph\/([\w-]{4,64})\/?$/)?.[1]
          : u.hostname==='channels.weixin.qq.com' && u.pathname.startsWith('/finder-preview/') ? u.searchParams.get('id'):null;
        if(id && /^[\w-]{4,64}$/.test(id)) links.add('https://channels.weixin.qq.com/finder-preview/pages/sph?id='+id);
      } catch(_) {}
    }
    return {links:[...links],embedded:cards.length>0};
  }});
  return result?.[0]?.result || {links:[],embedded:false};
}
