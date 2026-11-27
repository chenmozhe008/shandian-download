// 夸克网盘 / UC 网盘页面的快捷入口：工具栏「上传文件」左边多一个「闪电下载」，选中文件后一点就交给闪电全能下载。
// 位置和外观都和百度网盘那个一致（LinkSwift 的「下载助手」也在这里）。页面结构变了找不到工具栏，就什么也不做。
(() => {
  if (window.top !== window) return;
  const isAli = /(^|\.)(alipan\.com|aliyundrive\.com)$/.test(location.hostname);
  const isUcShare = location.hostname === "drive.uc.cn" && /^\/(s|share)\//.test(location.pathname);
  const ID = isAli ? "shandian-ali-btn" : "shandian-quark-btn";
  const setting = isAli ? "aliButton" : "quarkButton";
  const LABEL = "闪电下载";
  const LOGO = "data:image/png;base64,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";
  let enabled = true;
  const SHARE_GUIDE = "请先点页面上的「立即保存」存到自己的阿里云盘，\n再到个人网盘的列表视图勾选文件，点击「闪电下载」。";
  const entryTip = () => isAliShare() ? SHARE_GUIDE : "将选中的文件交给闪电全能下载。";
  const isAliShare = () => isAli && /^\/s\//.test(location.pathname);
  const entryLabel = () => isAliShare() ? "保存后再下载" : LABEL;

  let clearFailure=()=>{};
  function showFailure(message) {
    clearFailure();
    const button=document.getElementById(ID);
    if(window.ShFeedback)clearFailure=window.ShFeedback.show(button,message);
    else if(button){button.querySelector("span").textContent="请重新加载扩展";button.title=message;}
  }

  function build() {
    const b = document.createElement("button");
    b.id = ID;
    b.type = "button";
    b.title = entryTip();
    b.innerHTML = `<img src="${LOGO}" alt="" style="width:18px;height:18px;border-radius:4px;display:block;flex-shrink:0;object-fit:contain"><span style="display:block;line-height:1;margin:0;padding:0"></span>`;
    const label = b.querySelector("span");
    label.textContent = entryLabel();
    b.style.cssText = "display:inline-flex;align-items:center;justify-content:center;gap:6px;align-self:center;margin:0 10px 0 0;width:max-content;min-width:max-content;max-width:none;height:32px;padding:0 14px;border:0;border-radius:16px;background:rgba(128,128,128,.18);color:inherit;font-size:14px;font-weight:600;line-height:1;box-sizing:border-box;white-space:nowrap;cursor:pointer;flex:none;vertical-align:middle;transition:background .12s;";
    b.addEventListener("click", async (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (isAliShare()) {
        showFailure(SHARE_GUIDE);
        return;
      }
      b.disabled = true;
      label.textContent = "正在获取…";
      let text = "交接失败";
      let detail = "";
      try {
        const r = await chrome.runtime.sendMessage({ type: isAli ? "aliSelected" : "quarkSelected" });
        if (r?.ok) {
          text = `已交给闪电（${r.sent}）`;
          clearFailure();
        } else {
          const reason = r?.error && /[\u4e00-\u9fff]/.test(r.error) ? r.error : "获取下载地址失败，请刷新网盘页面后重试。";
          text = r?.needsSave ? "请先保存到网盘" : /登录/.test(reason) ? "需要登录" : "获取失败";
          detail = reason;

        }
      } catch (_) {
        text = "请刷新页面";
        detail = "扩展连接已失效，请刷新当前网盘页面后重试。";
      }
      label.textContent = text;
      b.title = detail || text;
      if(detail)showFailure(detail);
      setTimeout(() => {
        label.textContent = entryLabel();
        b.title = entryTip();
        b.disabled = false;
      }, 3000);
    });
    return b;
  }

  function isSupportedPage() {
    const p = location.pathname || "/";
    if (isAli) {
      if (!/^\/(drive\/file|s)(\/|$)/.test(p)) return false;
      return !!document.querySelector('[class*="node-list"], [class*="file-list"]');
    }
    return /^\/(list|s|share)(\/|$)/.test(p);
  }

  function mount() {
    const oldExisting = document.getElementById(ID);
    if (!isSupportedPage()) { oldExisting?.remove(); return; }
    const ucDownload = isUcShare ? document.querySelector(".file-info-share-buttom > .share-downloa") : null;
    const quarkShare = !isAli && !isUcShare && /^\/(s|share)\//.test(location.pathname);
    // 个人网盘和阿里云盘放助手左侧；只有夸克分享页按约定放右侧。
    const helper = document.querySelector(isAli ? ".ali-button.pl-button, .pl-button" : ".share-btns > .pl-button, .btn-operate .btn-main > .pl-button");
    const nativeAli = isAli ? [...document.querySelectorAll('button, [role="button"], a, [class*="download"]')].filter(el =>
      el.id !== ID && el.offsetParent && /^(下载移动端|下载桌面端|下载客户端|保存到网盘|保存至网盘)$/.test((el.innerText || el.textContent || "").trim())).sort((a,b) => a.getBoundingClientRect().left - b.getBoundingClientRect().left)[0] : null;
    const bar = ucDownload?.parentElement || helper?.parentElement || nativeAli?.parentElement || document.querySelector(isAli
      ? '[class^="header--"] > [class^="actions--"], [class*="share"] [class*="actions"]'
      : '.share-btns, .btn-operate .btn-main');
    const old = document.getElementById(ID);
    if (!enabled) return old?.remove();
    if (!bar) return;
    const inPosition = old && (nativeAli && !helper ? nativeAli.previousElementSibling === old : ucDownload ? ucDownload.previousElementSibling === old : helper
      ? (quarkShare ? helper.nextElementSibling : helper.previousElementSibling) === old : bar.contains(old));
    const b = inPosition ? old : build();
    if (!inPosition) old?.remove();
    // .pl-button 有时只是外层下拉容器，真正的背景、圆角在里面的 button 上。
    const assistant = helper?.querySelector("button") || helper;
    const ref = assistant || nativeAli || (isUcShare ? bar.querySelector(".pl-button button") : null) || ucDownload || bar.querySelector("button");
    if (ref) {
      const s = getComputedStyle(ref);
      const h = Math.min(56, Math.max(24, ref.getBoundingClientRect().height || parseFloat(s.height) || 32));
      b.style.height = `${h}px`;
      b.style.minHeight = `${h}px`;
      b.style.maxHeight = `${h}px`;
      b.style.borderRadius = s.borderRadius === "0px" ? "8px" : s.borderRadius;
      b.style.fontSize = `${Math.min(parseFloat(s.fontSize) || 14, h - 6)}px`;
      b.style.fontWeight = s.fontWeight;
      b.style.fontFamily = s.fontFamily;
      b.style.color = s.color;
      b.style.background = s.background;
      b.style.border = s.border;
      // 参考按钮可能是固定宽度的 div。只取横向间距，自己的内容宽度和上下居中独立计算。
      const inset = Math.min(20, Math.max(10, parseFloat(s.paddingLeft) || 14));
      b.style.padding = `0 ${inset}px`;
      const icon = b.querySelector("img");
      const size = `${Math.min(24, Math.max(16, Math.round(h * 0.6)))}px`;
      icon.style.setProperty("width", size, "important");
      icon.style.setProperty("height", size, "important");
      b.style.boxShadow = s.boxShadow;
    } else if (isAli) {
      b.style.background = "#574ab8";
      b.style.color = "#fff";
      b.style.fontWeight = "400";
    }
    // 不继承网页对 button 的窄宽度或上下 padding，否则图标文字会伸出背景。
    b.style.setProperty("width", "max-content", "important");
    b.style.setProperty("min-width", "max-content", "important");
    b.style.setProperty("box-sizing", "border-box", "important");
    b.style.setProperty("display", "inline-flex", "important");
    b.style.setProperty("align-items", "center", "important");
    b.style.setProperty("justify-content", "center", "important");
    b.style.setProperty("padding-top", "0", "important");
    b.style.setProperty("padding-bottom", "0", "important");
    const image = b.querySelector("img"), label = b.querySelector("span");
    for (const el of [image, label]) {
      el.style.setProperty("margin", "0", "important");
      el.style.setProperty("padding", "0", "important");
      el.style.setProperty("align-self", "center", "important");
    }
    label.style.setProperty("font", "inherit", "important");
    label.style.setProperty("line-height", "1", "important");
    // 页面和主题样式可能晚于按钮加载。位置正确时也同步外观，避免保留初始的默认样式。
    if (!b.disabled && label.textContent !== entryLabel()) { label.textContent = entryLabel(); b.title = entryTip(); }
    if (inPosition) return;
    if (nativeAli && !helper) {
      b.style.position = "static"; b.style.margin = "0 12px 0 0"; nativeAli.before(b);
    } else if (ucDownload) {
      b.style.position = "static";
      ucDownload.before(b);
    } else if (helper || bar.matches(".share-btns")) {
      b.style.margin = quarkShare ? "0 0 0 12px" : "0 12px 0 0";
      b.style.position = "static";
      if (helper) { if (quarkShare) helper.after(b); else helper.before(b); }
      else bar.append(b);
    } else bar.prepend(b);
  }

  // 只响应应用生成的短期换链令牌，文件清单由本机网关核对。
  try {
    const page=new URL(location.href),token=page.searchParams.get("shandian-refresh");
    if(/^[a-f0-9]{32}$/.test(token || ""))chrome.runtime.sendMessage({type:"cloudRefreshBegin",token}).then(reply=>{
      if(reply?.ok){page.searchParams.delete("shandian-refresh");history.replaceState(null,"",page.href);}
    }).catch(()=>{});
  } catch {}
  chrome.storage.local.get(setting).then((saved) => {
    enabled = saved[setting] !== false;
    mount();
  });
  chrome.storage.onChanged.addListener((c) => {
    if (c[setting]) {
      enabled = c[setting].newValue !== false;
      mount();
    }
  });
  // 网盘是单页应用，工具栏随选择反复重建
  new MutationObserver(mount).observe(document.documentElement, { childList: true, subtree: true });
})();
