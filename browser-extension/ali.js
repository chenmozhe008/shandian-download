// 阿里云盘自己的文件页：点击时读取本页选中项，使用网页当前会话取直链。
// 登录令牌只用于阿里云盘接口，不存入扩展，也不交给下载器。
async function aliInPage(refreshFiles = null) {
  const shareId = location.pathname?.match(/^\/s\/([^/]+)/)?.[1];
  if (shareId) return { needsSave:true, error:"先点「立即保存」，再到自己网盘的列表视图选中文件，点「闪电下载」。\n文件相同；闪电便于统一管理，支持批量、续传和多连接，部分场景可能更快。实际速度取决于网络和网盘策略。" };
  // 原生视图切换保留选中项；只在网格视图时切换一次，等待列表真正出现。
  const gridView = () => document.querySelector?.('[class*="node-list-grid-view--"]');
  const visible = el => el && el.offsetParent !== null;
  if (visible(gridView())) {
    const toggle = document.querySelector?.('[class^="switch-wrapper--"]');
    if (!toggle?.click) return {error:"请先切换到列表视图，再点击闪电下载。"};
    toggle.click();
    let ready = false;
    for (let wait = 0; wait < 30; wait++) {
      if (!visible(gridView()) && visible(document.querySelector('[class*="node-list-table-view--"]'))) { ready = true; break; }
      await new Promise(resolve => setTimeout(resolve,100));
    }
    if (!ready) return {error:"列表视图尚未加载完成，请稍候再点击闪电下载。"};
  }
  let shareToken = "";
  const nodes = document.querySelectorAll('[class*="node-list"], [class*="file-list"]');
  let chosen = null;
  for (const node of nodes) {
    const key = Object.keys(node).find((k) => /^__react(?:Fiber|InternalInstance|Props)\$/.test(k));
    let fiber = key && node[key];
    for (let depth = 0; fiber && depth < 15; depth++, fiber = fiber.return) {
      const props = [fiber, fiber.memoizedProps, fiber.pendingProps, fiber.stateNode?.props]
        .find((p) => Array.isArray(p?.dataSource) && (refreshFiles || p.selectedKeys != null));
      if (!props) continue;
      if (refreshFiles) {
        const matched=[];
        for (const expected of refreshFiles) {
          const base=String(expected.name).split(/[\\/]/).pop();
          const found=props.dataSource.filter(f=>(f.name || f.file_name)===base && f.type!=="folder");
          if(found.length!==1 || (expected.size>0 && Number(found[0].size || found[0].file_size)>0 && Number(found[0].size || found[0].file_size)!==expected.size)) break;
          matched.push({...found[0],__refreshName:expected.name});
        }
        if(matched.length!==refreshFiles.length) continue;
        chosen=matched;break;
      }
      const keys = new Set(Array.isArray(props.selectedKeys) ? props.selectedKeys : String(props.selectedKeys).split(','));
      if (shareId) shareToken = props.shareToken || props.share_token || shareToken;
      chosen = props.dataSource.filter((f) => keys.has(f.fileId || f.file_id));
      break;
    }
    if (chosen) break;
  }
  if(refreshFiles && !chosen) return {notFound:true,error:"请进入原文件夹，扩展会自动匹配文件，无需勾选。"};
  // 网格视图不总是把选择保存在同一层组件，按当前目录与可见选中名称核对文件。
  const names = chosen === null ? new Set([...document.querySelectorAll('[data-is-selected="true"]')]
    .map((el) => el.closest('[title]')?.getAttribute('title')).filter(Boolean)) : new Set();
  if (!chosen?.length && !names.size) return { error: "请切换到列表视图，选中要下载的文件后重试。" };
  let token;
  try { token = JSON.parse(localStorage.getItem("token") || "null"); } catch (_) {}
  if (!token?.access_token && !shareId) return { error: "请先登录阿里云盘" };
  token ||= {};
  const api = `https://${location.hostname.replace(/^www\./, 'api.')}`;
  const callOnce = async (path, body) => {
    let response;
    try {
      response = await fetch(api + path, {
        method: "POST", credentials: "include",
        headers: { "Content-Type": "application/json", ...(token.access_token ? {Authorization: `${token.token_type || 'Bearer'} ${token.access_token}`} : {}), ...(shareToken ? {"X-Share-Token":shareToken} : {}),
          ...(path.endsWith('get_download_url') ? { "X-Canary": "client=windows,app=adrive,version=v6.0.0" } : {}) },
        body: JSON.stringify(body),
      });
    } catch (_) { return { retryable:true, error: "无法连接阿里云盘，请检查网络后重试。" }; }
    let result;
    try { result = await response.json(); }
    catch (_) { return { retryable:response.status === 429 || response.status >= 500, error: "阿里云盘返回了异常内容，请刷新页面后重试。" }; }
    if (response.ok && !result.code) return result;
    const code = String(result.code || '').toLowerCase();
    if (/share.*(?:pwd|password|token)/.test(code)) return {error:"分享访问验证未通过，请先在分享页完成提取码或访问验证后重试。"};
    if (/share.*(?:save|transfer)|notsaved|savefirst/.test(code)) return {needsSave:true,error:"网盘要求先保存这些文件，请使用分享页的保存功能，保存后在个人文件页选择文件并点击闪电下载。"};
    if (/vip|membership/.test(code)) return {error:"阿里云盘返回会员权益限制，请查看网盘页面的具体要求。"};
    if (response.status === 401 || /access.?token|authentication|invalid.*token|token.*(?:invalid|expired)|unauthorized/.test(code)) {
      return { error: "阿里云盘登录已过期，请刷新页面；若仍失败，请重新登录后再下载。" };
    }
    if (response.status === 429 || /toomany|ratelimit|throttl/.test(code)) return { retryable:true, error: "请求过于频繁，请稍等片刻再下载。" };
    if (/signature|device|session/.test(code)) return { error: "阿里云盘身份校验未通过，请刷新页面后重试。" };
    if (/forbidden|permission|accessdenied|notallowed/.test(code)) return { error: "当前账号没有此文件的下载权限，请在阿里云盘确认文件及账号权限。" };
    if (response.status === 403) return { error: "阿里云盘拒绝了下载请求，请刷新页面并确认登录状态及文件权限。" };
    if (response.status === 404 || /notfound|filenotexist/.test(code)) return { error: "文件已被移走或删除，请刷新文件列表后重试。" };
    if (response.status >= 500) return { retryable:true, error: "阿里云盘服务暂时异常，请稍后重试。" };
    const hint = shareId ? "分享页直接取链未成功。可使用网页保存功能，保存后在个人文件页选择文件并点击闪电下载。" : "请刷新页面后重试。";
    const publicCode = /^[a-zA-Z0-9_.-]{1,64}$/.test(String(result.code || "")) ? `（${result.code}）` : "";
    return { error: `阿里云盘未能获取下载地址${publicCode}。${hint}` };
  };
  // 只重试临时失败的取链请求，不重启整批任务，也不重试登录或权限错误。
  const call = async (path, body) => {
    for (let attempt = 0; attempt < 3; attempt++) {
      const result = await callOnce(path, body);
      const missingUrl = path.endsWith('get_download_url') && !result.error && !result.url && !result.download_url;
      if (!result.retryable && !missingUrl) return result;
      if (attempt === 2) return missingUrl
        ? {error:"暂未获取到下载地址，已自动重试。请稍后重试，或分批选择文件。"}
        : result;
      await new Promise(resolve => setTimeout(resolve, 600 * (attempt + 1)));
    }
  };
  if (shareId && !shareToken) {
    const shared=await call('/v2/share_link/get_share_token',{share_id:shareId,share_pwd:""});
    if(shared.error)return shared;
    if(!shared.share_token)return {error:"分享需要提取码或访问验证，请先在分享页面完成验证。"};
    shareToken=shared.share_token;
  }
  if (chosen === null) {
    const parent = (shareId ? location.pathname.match(/\/folder\/([^/]+)/)?.[1] : location.pathname.match(/\/drive\/file\/[^/]+\/([^/]+)/)?.[1]) || "root";
    const matches = [];
    let marker = "";
    do {
      const result = await call(shareId ? '/adrive/v2/file/list_by_share' : '/adrive/v3/file/list', { ...(shareId ? {share_id:shareId} : {drive_id:token.default_drive_id}), parent_file_id: parent, limit:100, marker });
      if (result.error) return result;
      matches.push(...(result.items || []).filter((f) => names.has(f.name)));
      marker = result.next_marker || "";
    } while (marker);
    if (matches.length !== names.size) return { error: "无法确认选中的文件，请切换列表视图后重试" };
    chosen = matches;
  }
  if (chosen.some((f) => f.type === "folder")) return { error: "请打开文件夹后选择要下载的文件" };
  const files = [];
  for (const file of chosen) {
    const result = await call(shareId ? '/v2/file/get_share_link_download_url' : '/v2/file/get_download_url', { ...(shareId ? {share_id:shareId} : {drive_id:file.driveId || file.drive_id || token.default_drive_id}), file_id:file.fileId || file.file_id });
    if (result.error) return result;
    result.url ||= result.download_url;
    if (!result.url) return { error: "阿里云盘未返回下载地址，请刷新页面后重试。" };
    let url;
    try { url = new URL(result.url); } catch (_) {}
    if (!url || !["https:", "http:"].includes(url.protocol)) return { error: "网盘返回的下载地址无效" };
    files.push({ url: url.href, name: file.__refreshName || file.name || file.file_name || result.file_name });
  }
  return { files, ua: navigator.userAgent };
}

async function aliSend(tab) {
  const url = tab?.url ? new URL(tab.url) : null;
  const host = url?.hostname;
  if (!["www.alipan.com", "www.aliyundrive.com"].includes(host) || !/^\/(drive|s)(\/|$)/.test(url.pathname)) {
    return { ok: false, error: "请在阿里云盘文件或分享页面选择文件" };
  }
  const [result] = await chrome.scripting.executeScript({ target: { tabId: tab.id }, world: "MAIN", func: aliInPage });
  const data = result?.result;
  if (!data?.files?.length || data.error) return { ok: false, error: data?.error || "未找到下载地址", needsSave:!!data?.needsSave };
  let sent = 0;
  for (const file of data.files) {
    const body = JSON.stringify({ url: file.url, name: file.name, page: tab.url, group: true,
      headers: { Referer: `https://${host}/`, "User-Agent": data.ua } });
    const post = () => fetch(APP, { method: "POST", headers: { "Content-Type": "application/json" }, body }).then((r) => r.ok);
    let ok = await post().catch(() => null);
    if (ok === null && await launchApp(tab.id)) ok = await post().catch(() => false);
    if (!ok) return { ok: false, error: "没能交给闪电全能下载，请确认应用已打开" };
    sent++;
  }
  return { ok: true, sent };
}

chrome.runtime.onMessage.addListener((message, sender, reply) => {
  if (message.type !== "aliSelected") return;
  Promise.resolve().then(() => cloudDownloadTab(sender, message.tabId)).then(aliSend)
    .then(reply, () => reply({ ok: false, error: "读取阿里云盘页面失败，请刷新页面后重新选择文件。" }));
  return true;
});
