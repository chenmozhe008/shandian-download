// 网盘网页入口：读取选择、取链和本机交接由后台处理。
(() => {
  if (window.top !== window || window.__shandianExtraCloud) return;
  const site = extraCloudSite(location.href);
  if (!site) return;
  window.__shandianExtraCloud = true;
  const ID = "shandian-extra-cloud-btn";
  const LOGO = "data:image/png;base64,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";
  let enabled = true, queued = false;
  let saveRequiredPath = "";
  const SAVE_GUIDE = "先在分享页点击「保存至云盘」或「转存」，再进入自己的网盘选中文件，点击「闪电下载」。下载条件以网盘页面提示为准。";
  const isShare = () => site.id === "tianyi" ? /^\/web\/share/.test(location.pathname) : /^\/(s|share|123pan|link|shareweb)(\/|$)/.test(location.pathname);
  const ownShare = () => site.id === "guangya" && isShare() && (document.body?.textContent || "").includes("不能转存自己分享的文件");
  const saveGuide = () => ownShare() ? "这是你自己分享的文件，无需转存。请进入个人网盘找到原文件，再点击「闪电下载」。" : SAVE_GUIDE;
  const needsSave = () => isShare() && (["mobile","xunlei","pan123"].includes(site.id) || saveRequiredPath === location.pathname || ownShare());
  function label() { return needsSave() ? (ownShare() ? "到网盘下载" : "保存后再下载") : "闪电下载"; }

  let clearNotice=()=>{};
  function notice(message) {
    clearNotice();
    const button=document.getElementById(ID);
    if(window.ShFeedback)clearNotice=window.ShFeedback.show(button,message);
    else if(button){button.querySelector("span").textContent="请重新加载扩展";button.title=message;}
  }
  function build(reference) {
    const button = document.createElement("button");
    button.id = ID; button.type = "button";
    button.title = "将所选文件交给闪电下载，可在应用中确认名称和保存位置。";
    button.style.cssText = "display:inline-flex;align-items:center;justify-content:center;gap:6px;flex:none;width:max-content;min-width:max-content;height:32px;padding:0 14px;margin:0 10px 0 0;border:0;border-radius:8px;background:rgba(128,128,128,.18);color:inherit;font:600 14px/1 system-ui;white-space:nowrap;cursor:pointer;box-sizing:border-box;";
    button.innerHTML = `<img src="${LOGO}" alt="" style="display:block;flex:none;width:18px;height:18px;object-fit:contain;border-radius:4px"><span aria-live="polite"></span>`;

    const text = button.querySelector("span"); text.textContent = label();
    button.dataset.mode = label();
    button.addEventListener("click", async event => {
      event.preventDefault(); event.stopPropagation();
      if (button.disabled) return;
      if (needsSave()) { notice(saveGuide()); return; }
      button.disabled = true; text.textContent = "正在获取…";
      button.setAttribute("aria-busy", "true");
      try {
        const result = await chrome.runtime.sendMessage({ type: "extraCloudSelected" });
        text.textContent = result?.ok ? `已交给闪电（${result.sent}）` : result?.needsSave ? "请先保存到网盘" : "获取失败";
        if (result?.ok) clearNotice();
        else {
          if (result?.needsSave) saveRequiredPath = location.pathname;
          notice(result?.needsSave ? saveGuide() : result?.error || "获取下载地址失败，请刷新网盘页面后重试。");
        }
      } catch (_) { text.textContent = "请刷新页面"; notice("扩展连接已失效，请刷新网盘页面后重试。"); }
      finally {
        button.removeAttribute("aria-busy");
        setTimeout(() => { button.disabled = false; text.textContent = label(); button.dataset.mode = label(); }, 3000);
      }
    });
    return button;
  }
  function syncAppearance(button, reference) {
    if (!reference) return;
    const style = getComputedStyle(reference);
    for (const key of ["background", "color", "border", "borderRadius", "fontFamily", "fontSize", "fontWeight", "padding"]) button.style[key] = style[key];
    const height = Math.min(56,Math.max(24,reference.getBoundingClientRect().height || 32));
    button.style.height=height+"px";
    button.style.margin=style.margin;
    button.style.setProperty("float",style.getPropertyValue?.("float") || style.cssFloat || "none");
    button.style.verticalAlign="middle";
    const inset=Math.min(24,Math.max(10,parseFloat(style.paddingLeft) || 14));
    button.style.padding=`0 ${inset}px`;
    // Ant/Vue buttons often place an icon span before the text span.
    const spans=[...(reference.querySelectorAll?.("span") || [])];
    const text=spans.filter(el=>el.textContent.trim() && !el.querySelector("svg,img,i") && !/icon|badge/i.test(el.className || "") && (!el.getClientRects || el.getClientRects().length)).at(-1);
    const font=getComputedStyle(text || reference);
    button.querySelector("span").style.font=font.font;
    const size=Math.min(24,Math.max(16,Math.round(height*0.6)));
    const image=button.querySelector("img");image.style.width=size+"px";image.style.height=size+"px";
  }
  function mount() {
    queued = false;
    const old = document.getElementById(ID);
    if (!enabled) { old?.remove(); return; }
    // Selector alternatives are ordered fallbacks, not document-order competitors.
    const shareAnchor = site.id === "guangya" && isShare() ? [...document.querySelectorAll("button")].find(el=>el.textContent.trim()==="打开客户端") : null;
    const bar = shareAnchor?.parentElement || site.bar.split(",").map(selector=>document.querySelector(selector.trim())).find(Boolean);
    if (!bar) { old?.remove(); return; }
    // Prefer the existing helper as a positional anchor, then the native toolbar.
    const helper = bar.querySelector(".pl-button");
    const nativeXunlei = site.id === "xunlei" ? bar.querySelector('[class*="FileMenu__item--"].primary, .td-button:not(.active):not(.color-button)') : null;
    const controls=[...(bar.querySelectorAll?.("button,[role=button],a,.td-button") || [])].filter(el=>el.id!==ID && !el.closest?.(".pl-button") && el.getBoundingClientRect().height);
    const native=controls.find(el=>/^(下载|浏览器下载|立即下载|直接下载)$/.test(el.textContent.trim()) && !el.disabled) || controls.find(el=>!el.disabled);
    const reference = shareAnchor || nativeXunlei || native || helper?.querySelector("button") || helper || bar.querySelector("button, [role=button], a");
    if (old && !old.disabled && old.dataset.mode !== label()) { old.querySelector("span").textContent = label(); old.dataset.mode = label(); }
    const button = old || build(reference);
    syncAppearance(button,reference);
	if(site.id==="guangya" && new URL(location.href).searchParams.has("shandian-nas-account") && !document.getElementById("shandian-link-nas-account")) {
	  const link=document.createElement("button");link.id="shandian-link-nas-account";link.type="button";link.className=reference?.className || "";link.style.cssText=button.style.cssText;link.textContent="将当前登录关联到 NAS";
	  link.title="一次性将登录交给已在闪电 NAS 版配置的 NAS。成功后当前网页登录退出，后续由 NAS 独立续期。";
	  link.onclick=async()=>{
	    if(!confirm("将当前光鸭登录交给闪电 NAS 版中已配置的 NAS？成功后当前网页登录会退出，避免两端刷新令牌相互失效；以后电脑关闭也能自动换链。"))return;
	    link.disabled=true;link.textContent="NAS 正在关联…";
	    try{const r=await chrome.runtime.sendMessage({type:"linkGuangyaToNas"});link.textContent=r?.ok?"NAS 已关联，可以关闭此页":"关联未完成";if(!r?.ok){link.disabled=false;link.title=r?.error || "请在 NAS 检查登录状态";notice(link.title)}}
	    catch{link.disabled=false;link.textContent="请刷新网页后重试"}
	  };bar.append(link);
	}
    if (old && (shareAnchor ? shareAnchor.previousElementSibling===old : helper ? helper.previousElementSibling === old : old.parentElement === bar)) return;
    if (shareAnchor) shareAnchor.before(button); else if (helper) helper.before(button); else bar.prepend(button);
  }
  function schedule() { if (!queued) { queued = true; queueMicrotask(mount); } }
  chrome.storage.local.get("extraCloudButton").then(saved => { enabled = saved.extraCloudButton !== false; schedule(); });
  chrome.storage.onChanged.addListener(changes => {
    if (changes.extraCloudButton) { enabled = changes.extraCloudButton.newValue !== false; schedule(); }
  });
  new MutationObserver(schedule).observe(document.documentElement, { childList: true, subtree: true });
  try {
    const page = new URL(location.href), token = page.searchParams.get("shandian-refresh");
    if (/^[a-f0-9]{32}$/.test(token || "")) chrome.runtime.sendMessage({ type: "cloudRefreshBegin", token }).then(reply => {
      if (reply?.ok) { page.searchParams.delete("shandian-refresh"); history.replaceState(null,"",page.href); }
    }).catch(() => {});
  } catch (_) {}
})();
