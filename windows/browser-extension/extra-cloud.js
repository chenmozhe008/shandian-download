// SPDX-License-Identifier: AGPL-3.0-or-later
// Provider selection, endpoint and signing adaptations based on LinkSwift by
// Hmjz100, 油小猴 and contributors. See linkswift-NOTICE.md for source and license.
// Credentials remain transient in the extension; only file URLs/necessary download
// headers are sent to the local app. Requests run here to avoid page CORS restrictions.

// Serialized by chrome.scripting: keep this function independent of worker globals.
function extraCloudInPage(kind, refreshFiles = null) {
  try {
    const read = key => { const value = localStorage.getItem(key); try { return JSON.parse(value); } catch { return value; } };
    const share = kind === "mobile" ? /^\/(link|shareweb)/.test(location.pathname) :
      kind === "tianyi" ? /^\/web\/share/.test(location.pathname) : /^\/(s|share|123pan)\//.test(location.pathname);
    if (share && ["mobile", "xunlei", "pan123"].includes(kind)) return { needsSave: true, error: "先保存到自己的网盘，再选中文件点击「闪电下载」。" };
    let all = [], selected = [];
    if (kind === "mobile") {
      const list = document.querySelector(".main_file_list")?.__vue__;
      const home = document.querySelector(".home-page")?.__vue__;
      if (list) {
        selected = (list.selectList || []).map(value => value.item || value);
        all = list.fileList || list.list || [];
      } else if (home) {
        const files = home._computedWatchers?.fileList?.value || [];
        const dirs = home._computedWatchers?.dirList?.value || [];
        all = [...files, ...dirs];
        selected = [...files.filter((_, i) => home.selectedFile?.includes(i)), ...dirs.filter((_, i) => home.selectedDir?.includes(i))];
      }
    } else if (kind === "tianyi") {
      const list = document.querySelector(".c-file-list")?.__vue__;
      if (list) { selected = list.selectedList || []; all = list.fileList || list.list || []; }
      else {
        const detail = document.querySelector(".info-detail")?.__vue__?.fileDetail;
        if (detail) all = selected = [detail];
      }
    } else if (kind === "xunlei") {
      for (const node of document.querySelectorAll('[class*="SourceListItem__item--"]')) {
        const item = node.__vue__;
        if (!item?.info) continue;
        all.push(item.info);
        if (item.selected?.includes(item.info.id)) selected.push(item.info);
      }
    } else {
      const selector = kind === "guangya" ? '.swangpan-file-list__root[data-ui="file-list"]' :
        '.ant-table-wrapper, .tiled-list, .file-list, .single-file-sharing-container-content, .custom-table-wrapper, [class^="mfy_h-table-module__shell__"]';
      for (const node of document.querySelectorAll(selector)) {
        const key = Object.keys(node).find(key => /^__react(?:Fiber|InternalInstance|Props)\$/.test(key));
        let fiber = key && node[key];
        for (let depth = 0; fiber && depth < 25; depth++, fiber = fiber.return) {
          for (const props of [fiber.memoizedProps, fiber.pendingProps, fiber.stateNode?.props, fiber]) {
            if (!props) continue;
            const list = props.dataSource || props.loadedFileList || props.files;
            if (Array.isArray(list)) {
              const keys = props.rowSelection?.selectedRowKeys || props.selectedItems || [];
              const ids = new Set(keys.map(key => String(key?.fileId ?? key?.FileId ?? key)));
              all = list;
              selected = list.filter(file => file.checked === true || ids.has(String(file.FileId ?? file.fileId)));
            }
            if (kind === "pan123" && props.file?.S3KeyFlag != null) all = selected = [props.file];
            if (selected.length || (refreshFiles && all.length)) break;
          }
          if (selected.length || (refreshFiles && all.length)) break;
        }
        if (selected.length || (refreshFiles && all.length)) break;
      }
    }
    const normalized = file => ({
      id: file.FileId ?? file.fileId ?? file.contentID ?? file.id,
      name: file.FileName ?? file.fileName ?? file.contentName ?? file.name,
      size: Number(file.Size ?? file.fileSize ?? file.contentSize ?? file.size ?? 0),
      folder: kind === "pan123" ? Number(file.Type) !== 0 : kind === "tianyi" ? !!file.isFolder :
        kind === "mobile" ? !file.contentID : kind === "xunlei" ? file.kind !== "drive#file" : Number(file.resType) === 2,
      etag: file.Etag, s3: file.S3KeyFlag, shareId: file.shareId,
    });
    if (refreshFiles) {
      const list = all.map(normalized);
      selected = [];
      for (const expected of refreshFiles) {
        const name = expected.name.split(/[\\/]/).pop();
        const matches = list.filter(file => !file.folder && file.name === name && (!expected.size || !file.size || file.size === expected.size));
        if (matches.length !== 1) return { notFound: true, error: "请进入原文件夹，扩展将自动匹配未完成文件。" };
        selected.push({ ...matches[0], name: expected.name });
      }
    } else selected = selected.map(normalized);
    if (!selected.length) return { error: "先选中要下载的文件" };
    if (selected.some(file => file.folder)) return { error: "请打开所选文件夹，再选择其中的文件下载。" };
    if (selected.some(file => file.id == null || !file.name)) return { error: "未能完整读取所选文件，请刷新网盘页面后重试。" };
    const session = {};
    if (kind === "pan123") { session.token = read("authorToken"); session.uuid = read("LoginUuid"); }
    if (kind === "tianyi") session.token = read("accessToken");
    if (["xunlei", "guangya"].includes(kind)) {
      for (let i = 0; i < localStorage.length; i++) {
        const key = localStorage.key(i);
        if (key.startsWith("credentials_") && read(key)?.access_token) {
          const credentials = read(key);
          session.credentials = { access_token: credentials.access_token, token_type: credentials.token_type };
          const captcha = read(`captcha_${key.slice("credentials_".length)}`);
          session.captcha = { token: captcha?.token, captcha_token: captcha?.captcha_token };
          break;
        }
      }
      session.device = String(read("deviceid") || "").match(/\w{32}/)?.[0];
    }
    return { files: selected, session, share, shareId: location.pathname.split("/")[2],
      code: new URL(location.href).searchParams.get("code") || document.querySelector('input.ant-input[type="text"]')?.value || "",
      ua: navigator.userAgent };
  } catch (_) { return { error: "读取网盘页面失败，请刷新页面后重新选择文件。" }; }
}

// RFC 1321 MD5, needed by the Tianyi and Mobile request signatures.
function cloudMd5(value) {
  const data = new TextEncoder().encode(value), length = Math.ceil((data.length + 9) / 64) * 64;
  const bytes = new Uint8Array(length); bytes.set(data); bytes[data.length] = 128;
  const view = new DataView(bytes.buffer);
  view.setUint32(length - 8, data.length * 8, true);
  view.setUint32(length - 4, Math.floor(data.length / 0x20000000), true);
  const shifts = [7,12,17,22,5,9,14,20,4,11,16,23,6,10,15,21];
  let state = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
  for (let offset = 0; offset < length; offset += 64) {
    let [a,b,c,d] = state;
    for (let i = 0; i < 64; i++) {
      let f, g;
      if (i < 16) { f = (b & c) | (~b & d); g = i; }
      else if (i < 32) { f = (d & b) | (~d & c); g = (5 * i + 1) % 16; }
      else if (i < 48) { f = b ^ c ^ d; g = (3 * i + 5) % 16; }
      else { f = c ^ (b | ~d); g = (7 * i) % 16; }
      const sum = (a + f + Math.floor(Math.abs(Math.sin(i + 1)) * 0x100000000) + view.getUint32(offset + g * 4, true)) | 0;
      const shift = shifts[Math.floor(i / 16) * 4 + i % 4];
      [a,b,c,d] = [d, (b + ((sum << shift) | (sum >>> (32 - shift)))) | 0, b, c];
    }
    state = state.map((value, i) => (value + [a,b,c,d][i]) | 0);
  }
  return state.flatMap(value => [0,8,16,24].map(shift => ((value >>> shift) & 255).toString(16).padStart(2,"0"))).join("");
}

function extraCloudError(data, status) {
  const code = data?.code ?? data?.res_code ?? data?.error_code ?? data?.errorcode;
  const raw = String(data?.message || data?.msg || data?.error_description || "");
  if (status === 401 || ["5112", "InvalidSessionKey", "AccessTokenInvalid"].includes(String(code))) return "网盘登录已过期，请在网页重新登录后重试。";
  if (String(code) === "5113") return "123 云盘提示本月免费流量不足，请在网盘页面查看账号权益。";
  if (String(code) === "ShareNotFoundFlatDir") return "网盘要求先转存此文件，请保存到自己的网盘后下载。";
  if (["9", "Unauthenticated", "CaptchaInvalid"].includes(String(code))) return "网盘页面验证已过期，请刷新网页完成验证后重试。";
  if (status === 429) return "网盘请求过于频繁，请稍后重试。";
  const detail = /[\u3400-\u9fff]/.test(raw) ? raw.replace(/<[^>]*>/g, "").slice(0, 140) : "";
  if (detail) return `网盘获取地址失败：${detail}`;
  if (status === 403) return "网盘拒绝下载，请在网页检查文件权限及账号状态。";
  return `网盘未返回有效的下载地址${code != null ? `（错误码 ${String(code).replace(/[^\w.-]/g, "").slice(0,40)}）` : ""}，请刷新页面后重试。`;
}

async function extraCloudRequest(url, headers = {}, body) {
  let response;
  try {
    response = await fetch(url, { method: body === undefined ? "GET" : "POST", credentials: "include",
      headers: { Accept: "application/json", ...headers, ...(body === undefined ? {} : { "Content-Type": "application/json" }) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(30000) });
  } catch (_) { throw new Error("连接网盘服务失败或超时，请检查网络后重试。"); }
  let data;
  try { data = await response.json(); } catch (_) {
    const type=(response.headers?.get?.("content-type") || "未知格式").split(";")[0];
    let login=false;try{login=/login|oauth|sso/i.test(new URL(response.url || url).pathname)}catch{}
    throw new Error(login ? "网盘接口跳转到了登录验证页，请刷新网盘页面完成登录后重试。" : `网盘接口返回了 ${type}，未返回文件信息（HTTP ${response.status}）。请刷新页面后重试。`);
  }
  const code = data?.code ?? data?.res_code ?? data?.error_code ?? data?.errorcode;
  if (!response.ok || data?.success === false || (code != null && !["0", "200"].includes(String(code)))) {
    const error=new Error(extraCloudError(data,response.status));error.code=String(code);error.needsSave=String(code)==="207" || String(code)==="ShareNotFoundFlatDir";throw error;
  }
  return data;
}

let pan123HeadersReady;
let tianyiHeadersReady;
async function prepareTianyiHeaders() {
  if (!tianyiHeadersReady) tianyiHeadersReady=chrome.declarativeNetRequest.updateSessionRules({
    removeRuleIds:[18901],addRules:[{id:18901,priority:1,
      condition:{urlFilter:"|https://api.cloud.189.cn/open/",initiatorDomains:[chrome.runtime.id],resourceTypes:["xmlhttprequest"]},
      action:{type:"modifyHeaders",requestHeaders:[
        {header:"Origin",operation:"set",value:"https://cloud.189.cn"},
        {header:"Referer",operation:"set",value:"https://cloud.189.cn/"},
      ]},
    }],
  }).catch(()=>{tianyiHeadersReady=null;throw new Error("天翼云盘请求配置未就绪，请重新加载闪电扩展后重试。");});
  await tianyiHeadersReady;
}
async function preparePan123Headers() {
  // Chrome fetch drops User-Agent/Sec-* headers. A session rule applies only to
  // this extension's own 123 API requests; ordinary browsing is unaffected.
  if (!pan123HeadersReady) pan123HeadersReady = chrome.declarativeNetRequest.updateSessionRules({
    removeRuleIds: [12301], addRules: [{ id: 12301, priority: 1,
      condition: { urlFilter: "|https://api.123pan.cn/api/v2/", initiatorDomains: [chrome.runtime.id], resourceTypes: ["xmlhttprequest"] },
      action: { type: "modifyHeaders", requestHeaders: [
        { header: "User-Agent", operation: "set", value: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) 123pan/3.2.3 Chrome/108.0.5359.215 Electron/22.3.27 Safari/537.36 WebView/1.0" },
        { header: "Sec-CH-UA", operation: "set", value: '"Not?A_Brand";v="8", "Chromium";v="108"' },
      ] },
    }],
  }).catch(() => { pan123HeadersReady = null; throw new Error("123 云盘请求配置未就绪，请重新加载闪电扩展后重试。"); });
  await pan123HeadersReady;
}

async function extraCloudFiles(tab, refreshFiles = null) {
  const site = extraCloudSite(tab.url);
  if (!site) return { error: "请在支持的网盘文件页面选择文件。" };
  const [reply] = await chrome.scripting.executeScript({ target: { tabId: tab.id }, world: "MAIN", func: extraCloudInPage, args: [site.id, refreshFiles] });
  const page = reply?.result;
  if (!page?.files?.length || page.error) return page || { error: "未能读取网盘页面，请刷新后重试。" };
  const session = page.session || {}, files = [];
  try {
    let auth = {}, shareToken = "";
    if (site.id === "tianyi") await prepareTianyiHeaders();
    if (site.id === "pan123") {
      if (!session.token) throw new Error("请先登录 123 云盘后重试。");
      await preparePan123Headers();
      auth = { Authorization: `Bearer ${session.token}`, Platform: "web", Platform_os: "123pan/v323(Windows10.0.26200)", "App-Version": "323", LoginUuid: session.uuid || "" };
    }
    if (site.id === "tianyi" && !session.token) {
      let response;
      try { response = await fetch("https://api.cloud.189.cn/open/oauth2/ssoH5.action", { credentials: "include", signal: AbortSignal.timeout(30000) }); }
      catch (_) { throw new Error("无法获取天翼云盘登录状态，请刷新网盘页面后重试。"); }
      session.token = new URL(response.url).searchParams.get("accessToken");
      if (!session.token) throw new Error("请先在天翼云盘网页登录后重试。");
    }
    if (site.id === "mobile") {
      const cookies = await chrome.cookies.getAll({ url: new URL(tab.url).origin + "/" });
      session.authorization = cookies.find(cookie => cookie.name === "authorization")?.value;
      if (!session.authorization) throw new Error("请先登录移动云盘后重试。");
    }
    if (["xunlei", "guangya"].includes(site.id)) {
      const credentials = session.credentials, captcha = session.captcha;
      if (!credentials?.access_token || !session.device) throw new Error(`请先登录${site.name}后重试。`);
      const verification = captcha?.captcha_token || captcha?.token;
      if (!verification) throw new Error("网盘页面验证尚未就绪，请刷新网页完成验证后重试。");
      auth = { Authorization: `${credentials.token_type || "Bearer"} ${credentials.access_token}`, "X-Captcha-Token": verification, "X-Device-Id": session.device };
      if (site.id === "guangya" && page.share) {
        const result = await extraCloudRequest("https://api.guangyapan.com/nd.bizuserres.s/v1/get_share_access_token", auth, { shareId: page.shareId, code: page.code });
        shareToken = result.data?.accessToken;
        if (!shareToken) throw new Error("未能获取分享访问权限，请在网盘页面完成提取码验证后重试。");
      }
    }
    for (const file of page.files) {
      let result, url;
      if (site.id === "pan123") {
        const body = page.share ? { ShareKey: page.shareId, FileID: file.id, S3keyFlag: file.s3, Size: file.size, Etag: file.etag } :
          { driveId: 0, etag: file.etag, fileId: file.id, s3keyFlag: file.s3, type: 0, fileName: file.name, size: file.size };
        result = await extraCloudRequest(`https://api.123pan.cn/api/v2/${page.share ? "share/download/info" : "file/download_info"}`, auth, body);
        if (result.data?.dispatchList?.length && result.data.downloadPath) {
          url = result.data.dispatchList[0].prefix + result.data.downloadPath;
          const params = new URL(url).searchParams.get("params");
          if (params) { try { url = decodeURIComponent(escape(atob(params))); } catch (_) { throw new Error("123 云盘返回的地址无法解码，请刷新页面后重试。"); } }
        } else url = result.data?.DownloadUrl || result.data?.downloadUrl;
      } else if (site.id === "tianyi") {
        const time = String(Date.now()), shareId = file.shareId;
        const query = new URLSearchParams({ fileId: file.id, ...(shareId ? { dt: "1", shareId } : {}) });
        const sign = `AccessToken=${session.token}&Timestamp=${time}${shareId ? "&dt=1" : ""}&fileId=${file.id}${shareId ? `&shareId=${shareId}` : ""}`;
        result = await extraCloudRequest(`https://api.cloud.189.cn/open/file/getFileDownloadUrl.action?${query}`, { Accept:"application/json;charset=UTF-8", "Sign-Type": "1", Accesstoken: session.token, Timestamp: time, Signature: cloudMd5(sign) });
        url = result.fileDownloadUrl;
      } else if (site.id === "mobile") {
        const body = { fileId: file.id }, time = new Date(Date.now() + 8 * 3600000).toISOString().slice(0,19).replace("T", " ");
        const key = crypto.randomUUID().replace(/-/g, "").slice(0,16);
        const sorted = encodeURIComponent(JSON.stringify(body).replace(/\s/g, "")).split("").sort().join("");
        const sign = cloudMd5(cloudMd5(btoa(sorted)) + cloudMd5(time + ":" + key)).toUpperCase();
        result = await extraCloudRequest("https://personal-kd-njs.yun.139.com/hcy/file/getDownloadUrl", {
          Authorization: session.authorization, Caller: "web", "CMS-DEVICE": "default", "Mcloud-Channel": "1000101", "Mcloud-Client": "10701", "Mcloud-Sign": `${time},${key},${sign}`, "Mcloud-Version": "7.14.2",
          "X-DeviceInfo": "||9|7.17.0|edge||||windows 10||zh-CN|||", "X-Huawei-ChannelSrc": "10000034", "X-Inner-Ntwk": "2", "X-M4C-Caller": "PC", "X-M4C-Src": "10002", "X-SvcType": "1", "X-Yun-Api-Version": "v1", "X-Yun-App-Channel": "10000034", "X-Yun-Channel-Source": "10000034", "X-Yun-Client-Info": "||9|7.17.0|edge||||windows 10||zh-CN|||||", "X-Yun-Module-Type": "100", "X-Yun-Svc-Type": "1", "X-Yun-Url-Type": "3",
        }, body);
        url = result.data?.url;
      } else if (site.id === "xunlei") {
        result = await extraCloudRequest(`https://api-pan.xunlei.com/drive/v1/files/${encodeURIComponent(file.id)}`, auth);
        url = result.web_content_link;
      } else {
        result = await extraCloudRequest(`https://api.guangyapan.com/nd.bizuserres.s/v1/${page.share ? "get_share_download_url" : "get_res_download_url"}`, auth,
          { fileId: file.id, ...(page.share ? { accessToken: shareToken } : {}) });
        url = result.data?.signedURL || result.data?.downloadUrl;
      }
      let parsed;
      try { parsed = new URL(url); } catch (_) {}
      if (!parsed || !["https:", "http:"].includes(parsed.protocol) || parsed.username || parsed.password) throw new Error(extraCloudError(result));
      files.push({ url: parsed.href, name: file.name, headers: [["User-Agent", page.ua], ["Referer", new URL(tab.url).origin + "/"]] });
    }
    return { files };
  } catch (error) {
    if(site.id==="guangya" && error.code==="254")return {error:"光鸭限制这份文件的网页下载，请在光鸭官方客户端取回。"};
    return { error: /[\u3400-\u9fff]/.test(error.message) ? error.message : "获取网盘下载地址失败，请刷新页面后重试。", needsSave: page.share && (!!error.needsSave || /先转存|保存到自己的网盘/.test(error.message)) };
  }
}

const EXTRA_CLOUD_BUSY = new Set();
async function extraCloudSend(tab) {
  if (!tab?.id || !extraCloudSite(tab.url)) return { ok: false, error: "请在网盘文件页面重试。" };
  if (EXTRA_CLOUD_BUSY.has(tab.id)) return { ok: false, error: "正在获取所选文件，请稍候。" };
  EXTRA_CLOUD_BUSY.add(tab.id);
  try {
    const result = await extraCloudFiles(tab);
    if (!result.files?.length) return { ok: false, error: result.error, needsSave: !!result.needsSave };
    let sent = 0;
    for (const file of result.files) {
      const body = JSON.stringify({ url: file.url, name: file.name, page: tab.url, group: true, headers: Object.fromEntries(file.headers) });
      const post = () => fetch(APP, { method: "POST", headers: { "Content-Type": "application/json" }, body }).then(response => response.ok);
      let ok = await post().catch(() => null);
      if (ok === null && await launchApp(tab.id)) ok = await post().catch(() => false);
      if (!ok) return { ok: false, error: `没能交给闪电全能下载，请确认应用已打开${sent ? `（已提交 ${sent} 个文件）` : ""}。` };
      sent++;
    }
    return { ok: true, sent };
  } finally { EXTRA_CLOUD_BUSY.delete(tab.id); }
}
chrome.runtime.onMessage.addListener((message, sender, reply) => {
  if (message.type !== "extraCloudSelected") return;
  // Popup may specify a tab; page messages always use their own sender tab.
  const run = async () => {
    const tab = sender.tab || (message.tabId != null ? await chrome.tabs.get(message.tabId) : null);
    return extraCloudSend(tab);
  };
  run().then(reply, () => reply({ ok: false, error: "读取网盘页面失败，请刷新网页后重试。" }));
  return true;
});

// Explicit one-time transfer of this login to the NAS configured in the native
// client. Neither the webpage nor a URL parameter can choose another recipient.
function nasGuangyaSession(expectedOrigin) {
  if(expectedOrigin && (location.origin!==expectedOrigin || location.protocol!=="https:"))return null;
  const sessions=[];
  for(let i=0;i<localStorage.length;i++) {
    const key=localStorage.key(i);if(key!=="credentials_aMe-8VSlkrbQXpUR" && !key.startsWith("credentials_aMe-8VSlkrbQXpUR_"))continue;
    let c;try{c=JSON.parse(localStorage.getItem(key))}catch{continue}
    if(!c?.access_token || !c?.refresh_token)continue;
    const device=String(localStorage.getItem("deviceid") || "").match(/[a-f0-9]{32}/i)?.[0] || "";
    sessions.push({key,fields:{access_token:c.access_token,refresh_token:c.refresh_token,client_id:"aMe-8VSlkrbQXpUR",device_id:device}});
  }
  return sessions.find(session=>session.key==="credentials_aMe-8VSlkrbQXpUR") || (sessions.length===1?sessions[0]:null);
}
async function linkGuangyaToNas(tab) {
  if(extraCloudSite(tab?.url)?.id!=="guangya")return {ok:false,error:"请先打开光鸭个人网盘并登录"};
  const [value]=await chrome.scripting.executeScript({target:{tabId:tab.id},world:"MAIN",func:nasGuangyaSession});
  const session=value?.result;if(!session)return {ok:false,error:"请先在光鸭网页完成登录"};
  let response;
  try{const r=await fetch(APP.replace(/\/add$/,"/cloud-account/link"),{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({provider:"guangya",fields:session.fields})});response=await r.json()}
  catch{return {ok:false,error:"请打开已配置 NAS 的闪电 NAS 版后重试，网页登录已保留"}}
  if(!response.ok)return {ok:false,error:response.error || "NAS 尚未完成关联，网页登录已保留"};
  await chrome.scripting.executeScript({target:{tabId:tab.id},world:"MAIN",func:(key,access)=>{
    try{const current=JSON.parse(localStorage.getItem(key) || "null");if(current?.access_token===access)localStorage.removeItem(key)}catch{}
  },args:[session.key,session.fields.access_token]});
  return {ok:true};
}
chrome.runtime.onMessage.addListener((message,sender,reply)=>{
  if(message.type!=="linkGuangyaToNas")return;
  (async()=>{const tab=sender.tab || (message.tabId!=null?await chrome.tabs.get(message.tabId):null);return linkGuangyaToNas(tab)})().then(reply,()=>reply({ok:false,error:"NAS 关联失败，网页登录已保留"}));return true;
});
