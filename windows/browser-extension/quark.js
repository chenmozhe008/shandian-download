// 夸克网盘 / UC 网盘直接下载（两家同一套接口，只是域名和参数不同）。
// 没有官方开放接口：在用户登录着的网盘页面里调网页自己用的接口列文件、取下载地址，
// 下载时把这个网盘的登录 Cookie 交给 App（下载地址不带登录会回 412）。Cookie 只交给本机 App，不进日志。
// 实测（11-07）：带 Cookie、用浏览器自己的 User-Agent 就能下，不用冒充网盘客户端。
const QUARK_SITES = {
  "pan.quark.cn": { api: "https://drive-pc.quark.cn", pr: "ucpro", domain: "quark.cn", page: "https://pan.quark.cn/", name: "夸克网盘" },
  "drive.uc.cn": { api: "https://pc-api.uc.cn", pr: "UCBrowser", domain: "uc.cn", page: "https://drive.uc.cn/", name: "UC 网盘" },
};

function quarkSite(url) {
  try {
    return QUARK_SITES[new URL(url).hostname] || null;
  } catch (_) {
    return null;
  }
}

// 在网盘页面里跑：读选中的文件，文件夹展开，再取下载地址。都用页面自己的登录（credentials: include）
async function quarkInPage(api, pr, refreshFiles = null) {
  try {
    const q = `pr=${pr}&fr=pc&uc_param_str=`;
    const call = async (path, body) => {
      const r = await fetch(`${api}/1/clouddrive/${path}${path.includes("?") ? "&" : "?"}${q}`, {
        method: body ? "POST" : "GET", credentials: "include",
        headers: body ? { "Content-Type": "application/json" } : {},
        body: body ? JSON.stringify(body) : undefined,
      });
      const j = await r.json();
      if (r.ok === false || (j.code != null && Number(j.code) !== 0) || (j.status != null && Number(j.status) !== 200)) {
        const msg = String(j.message || j.msg || "");
        if (/封禁|违规|封号|banned|blocked/i.test(msg)) throw new Error("网盘提示账号或分享受到限制，请在网盘页面查看限制及申诉信息。");
        if (Number(j.code) === 31001 || r.status === 401) throw new Error("网盘下载接口登录验证失败，请重新登录后重试。");
        // 23018 是下载通道限制，不能据此推断用户未登录或网盘空间不足。
        if (Number(j.code) === 23018) {
          const reason = /[\u3400-\u9fff]/.test(msg) && !/游客|访客|未登录|登录/.test(msg)
            ? msg.replace(/<[^>]*>/g, "").slice(0, 140) : "";
          throw new Error(reason ? `网盘下载受限（23018）：${reason}` : "当前网页或分享下载受到网盘限制（23018）。请查看网盘页面的具体下载条件。");
        }
        if (/登录|login|auth.*expir/i.test(msg)) throw new Error("网盘下载接口登录验证失败，请重新登录后重试。");
        if (r.status === 429 || /频繁|限流|too many/i.test(msg)) throw new Error("网盘请求过于频繁，请稍后重试。");
        if (/权限|无权|禁止|forbidden|denied/i.test(msg) || r.status === 403) throw new Error("网盘未授权下载这些文件，请检查分享权限和账号状态。");
        const detail = /[\u3400-\u9fff]/.test(msg) ? msg.replace(/<[^>]*>/g, "").slice(0, 140) : "网盘获取文件信息失败";
        throw new Error(detail + (Number.isFinite(Number(j.code)) ? `（错误码 ${j.code}）` : ""));
      }
      return j;
    };
    const rows = [...document.querySelectorAll("[data-row-key]")].filter((r) =>
      r.querySelector(".ant-checkbox-checked, input[type=checkbox]:checked") || r.classList.contains("ant-table-row-selected"));
    let picked = [...new Set(rows.map((r) => r.getAttribute("data-row-key")).filter(Boolean))];
    if (!picked.length && !refreshFiles) return { error: "先选中文件" };
    const shareId = location.pathname.match(/^\/(?:s|share)\/([a-zA-Z0-9]+)/)?.[1];
    let share = null;
    if (shareId) {
      const node = document.querySelector(".file-list");
      const key = node && Object.keys(node).find((k) => /^__react(?:Fiber|InternalInstance|Props)\$/.test(k));
      let fiber = key && node[key];
      for (let n = 0; fiber && n < 15; n++, fiber = fiber.return) {
        for (const props of [fiber, fiber.memoizedProps, fiber.pendingProps, fiber.stateNode?.props]) {
          if (Array.isArray(props?.list) && props.stoken) { share = props; break; }
        }
        if (share) break;
      }
      if (!share) return { error: "未读到分享信息，请刷新分享页后重试" };
    }
    const pdir = location.hash.match(/\/list\/(?:all|share)\/([^/?#]+)/)?.[1]?.split("-")[0] || "0";
    const listDir = async (fid) => {
      const all = [], seen = new Set();
      for (let page = 1; ; page++) {
        const shared = share ? `share/sharepage/detail?pwd_id=${encodeURIComponent(shareId)}&stoken=${encodeURIComponent(share.stoken)}&` : "file/sort?";
        const j = await call(`${shared}pdir_fid=${encodeURIComponent(fid)}&_page=${page}&_size=50&_fetch_total=1&_sort=file_type:asc,file_name:asc`);
        const list = j.data?.list;
        if (!Array.isArray(list)) throw new Error("网盘未返回有效的文件列表，请稍后重试。");
        const totalValue = j.metadata?._total ?? j.data?.total;
        const count = Number(totalValue);
        const total = totalValue != null && Number.isFinite(count) && count >= 0 ? count : null;
        const fresh = list.filter((f) => f.fid && !seen.has(f.fid));
        for (const f of fresh) { seen.add(f.fid); all.push(f); }
        if (Number.isFinite(total) && total !== null && all.length >= total) return all;
        if (!fresh.length && (list.length || (total != null && all.length < total))) throw new Error("网盘文件列表未能完整读取，请稍后重试或缩小所选范围。");
        if (total == null && list.length < 50) return all;
      }
    };
    let here = share ? share.list : await listDir(pdir);
    if (picked.some((fid) => !here.some((f) => f.fid === fid))) here = await listDir(pdir);
    const renewNames=new Map();
    if(refreshFiles){
      picked=[];
      for(const expected of refreshFiles){
        const base=String(expected.name).split(/[\\/]/).pop();
        const found=here.filter(f=>f.file_name===base && !f.dir && f.file_type!==0);
        if(found.length!==1 || (expected.size>0 && Number(found[0].size || found[0].file_size)>0 && Number(found[0].size || found[0].file_size)!==expected.size)) return {notFound:true,error:"请进入原文件夹，将自动匹配文件。"};
        picked.push(found[0].fid);renewNames.set(found[0].fid,expected.name);
      }
    }
    const byId = new Map(here.map((f) => [f.fid, f]));
    if (picked.some((fid) => !byId.has(fid))) return { error: "部分选中文件不在当前目录，请重新选择文件后重试。" };
    const files = [], visited = new Set();
    const queue = picked.map((fid) => ({ f: byId.get(fid), base: "" }));
    while (queue.length) {
      const { f, base } = queue.shift();
      if (visited.has(f.fid)) continue;
      visited.add(f.fid);
      const rel = base + f.file_name;
      if (f.dir || f.file_type === 0) {
        for (const child of await listDir(f.fid)) queue.push({ f: child, base: rel + "/" });
      } else files.push({ fid: f.fid, rel, token: f.share_fid_token });
    }
    if (!files.length) return { error: "所选文件夹内没有可下载的文件。" };
    const out = [];
    // 分享接口按小批次取地址；全部核对成功后才交给应用，避免漏掉部分文件。
    for (let i = 0; i < files.length; i += 15) {
      const part = files.slice(i, i + 15);
      const body = { fids: part.map((x) => x.fid) };
      if (share) {
        if (part.some((x) => !x.token)) return { error: "分享文件信息已过期，请重新打开分享页后重试。" };
        Object.assign(body, { pwd_id: shareId, stoken: share.stoken, fids_token: part.map((x) => x.token) });
      }
      const j = await call("file/download", body);
      const downloads = new Map((Array.isArray(j.data) ? j.data : []).map((d) => [d.fid, d]));
      for (const f of part) {
        const d = downloads.get(f.fid);
        if (!d?.download_url) throw new Error(`网盘未返回「${f.rel}」的下载地址，本次未创建下载任务。请检查该文件的下载权限。`);
        out.push({ url: d.download_url, name: renewNames.get(f.fid) || f.rel || d.file_name });
      }
    }
    return { files: out, ua: navigator.userAgent };
  } catch (e) {
    const msg = String(e?.message || "");
    return { error: /[\u3400-\u9fff]/.test(msg) ? msg : "连接网盘服务失败，请检查网络后重试。" };
  }
}

async function quarkSend(tab) {
  const site = quarkSite(tab?.url);
  if (!site) return { ok: false, error: "不是夸克或 UC 网盘页面" };
  const [res] = await chrome.scripting.executeScript({ target: { tabId: tab.id }, world: "MAIN", func: quarkInPage, args: [site.api, site.pr] });
  const r = res?.result;
  if (!r || r.error) return { ok: false, error: r?.error || "没取到下载地址，请刷新页面后重试" };
  if (!r.files.length) return { ok: false, error: "没取到下载地址" };
  // 下载域名（dl-pc-*.drive.quark.cn）用的是整个 quark.cn 的登录 Cookie，含页面里读不到的 httpOnly 项
  const cookies = await chrome.cookies.getAll({ domain: site.domain });
  const cookie = cookies.map((c) => `${c.name}=${c.value}`).join("; ");
  if (!cookie) return { ok: false, error: `请先登录${site.name}` };
  const headers = { Cookie: cookie, "User-Agent": r.ua, Referer: site.page };
  let sent = 0;
  for (const f of r.files) {
    const body = JSON.stringify({ url: f.url, headers, name: r.files.length === 1 ? f.name.split("/").pop() : f.name, page: tab.url, group: true });
    const post = () => fetch(APP, { method: "POST", headers: { "Content-Type": "application/json" }, body }).then((x) => x.ok);
    let ok = await post().catch(() => null);
    if (ok === null && (await launchApp(tab.id))) ok = await post().catch(() => false);
    if (!ok) return { ok: false, error: "没能交给闪电全能下载，请确认应用已打开" };
    sent++;
  }
  return { ok: true, sent };
}

chrome.runtime.onMessage.addListener((msg, sender, reply) => {
  if (msg.type !== "quarkSelected") return;
  Promise.resolve().then(() => cloudDownloadTab(sender, msg.tabId)).then(quarkSend)
    .then(reply, (e) => reply({ ok: false, error: e.message }));
  return true;
});
