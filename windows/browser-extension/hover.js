// 悬停下载按钮：鼠标移到网页视频或下载链接上，角落出现一个闪电按钮，点一下交给闪电全能下载。
// 鼠标放上去就出现，移开就把元素从页面上拿掉；平时页面里没有任何多余节点，不拦截页面事件。
// 按钮放在目标外面（上方），不盖住页面自己的按钮和播放器控件。shadow DOM 和页面样式互不影响。
// 按视频所在位置判断而不是 mouseover：很多播放器在视频上盖了一层透明控件，事件到不了 <video>。
(() => {
  const nativeCloudHosts=["123pan.com","123pan.cn","123684.com","123865.com","123952.com","123912.com","123957.com","cloud.189.cn","yun.139.com","caiyun.139.com","pan.xunlei.com","guangyapan.com"];
  const cloudPage = nativeCloudHosts.some(host => location.hostname === host || location.hostname.endsWith("." + host));
  const isXhs = /(^|\.)xiaohongshu\.com$/.test(location.hostname);
  const isFeishu = /(^|\.)feishu\.cn$/i.test(location.hostname);
  const isPixabay = /(^|\.)pixabay\.com$/.test(location.hostname);
  const isWeChatMedia = /^(mp\.weixin\.qq\.com|channels\.weixin\.qq\.com)$/.test(location.hostname);
  const audioOnly = cloudPage || (window.top !== window && !isWeChatMedia);
  const isNativeMediaSite = !audioOnly && (/(^|\.)aigei\.com$/.test(location.hostname) || (isPixabay && /^\/(?:[a-z]{2}\/)?(music|sound-effects|videos)\//.test(location.pathname)));
  const LINK = /^(magnet:|ed2k:)|\.(zip|rar|7z|tar|gz|bz2|xz|tgz|txz|tbz|tbz2|zst|deb|rpm|appimage|snap|flatpak|flatpakref|flatpakrepo|run|bin|iso|dmg|pkg|exe|msi|msix|apk|torrent|mp4|mkv|avi|mov|webm|flv|wmv|m4v|ts|mp3|flac|wav|m4a|aac|ogg|oga|opus|aif|aiff|pdf)([?#].*)?$/i;
  const MIN_W = 200;
  const MIN_H = 120;
  const LEAVE = 250;
  const SIZE = 38;
  const GAP = 4;
  const LOGO = "data:image/png;base64,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";
  let enabled = true;
  let target = null;
  let host = null;
  let button = null;
  let hideTimer = 0;
  let lastMove = 0, nativeScanAt = 0;
  let hoverBusySince = 0, hoverRequest = 0;

  // 适配过的视频站：按钮放进站点自己的操作栏（点赞那一排），不再悬停。
  // 找不到操作栏（改版、不是播放页）就什么也不放，退回悬停按钮
  const INLINE_SITES = [
    { host: /(^|\.)bilibili\.com$/, path: /^\/(video|bangumi\/play)\//, bars: [".video-toolbar-left-main", ".video-toolbar-left"], where: "append", collection: true },
    { host: /(^|\.)youtube\.com$/, path: /^\/(watch|shorts)/, bars: ["ytd-watch-metadata #top-level-buttons-computed", "ytd-watch-metadata #actions-inner #menu"], where: "prepend", collection: true },
    // Vimeo 的类名是编译出来的，会变；认分享按钮的 aria-label，外面第二层是操作栏（每个按钮各包一层）
    { host: /(^|\.)vimeo\.com$/, path: /^\/\d+/, anchor: 'button[aria-label="Share button"]', where: "append" },
    // 新片场：类名是编译出来的，按按钮文字找这一排；样式照抄同排按钮。站点自己有「下载」，这里写全名区分
    { host: /(^|\.)xinpianchang\.com$/, path: /^\/a\d+/, where: "append", label: "闪电下载", copyNeighbor: true,
      find: () => [...document.querySelectorAll("button")].find((x) => x.offsetParent && x.innerText.trim() === "创作"), row: 5 },
    // 抖音：右侧竖排，放在分享下面。信息流里有好几屏，只放当前屏；data-e2e 比类名稳定。只加按钮，不碰页面请求
    { host: /(^|\.)douyin\.com$/, path: /./, column: true,
      find: () => [...document.querySelectorAll('[data-e2e="video-player-share"]')].find((e) => {
        const r = e.getBoundingClientRect();
        return r.height > 0 && r.top >= 0 && r.bottom <= innerHeight;
      }) },
    // 豆瓣预告片：「分享到、推荐」那一排的最前面。这一排字小，按钮也小一号
    { host: /^movie\.douban\.com$/, path: /^\/trailer\/\d+/, bars: [".btns"], where: "prepend", small: true },
  ];
  const INLINE_ID = "shandian-inline-btn";
  const INLINE_COLLECTION_ID = "shandian-inline-collection";
  const INLINE_COLLECTION_GROUP_ID = "shandian-inline-collection-group";
  const isX = /(^|\.)(x|twitter)\.com$/.test(location.hostname);
  const inlineShown = () => !!document.getElementById(INLINE_ID);
  function fitInline(b, site, ref) {
    if (site.column) {
      const img = b.querySelector("img"), label = b.querySelector("span");
      img.style.setProperty("width", "32px", "important");
      img.style.setProperty("height", "32px", "important");
      img.style.setProperty("margin", "0", "important");
      img.style.setProperty("padding", "0", "important");
      label.style.setProperty("margin", "4px 0 0", "important");
      label.style.setProperty("padding", "0", "important");
      label.style.setProperty("line-height", "16px", "important");
      return;
    }
    const refHeight = ref?.getBoundingClientRect().height || (site.small ? 24 : 36);
    const h = Math.max(site.small ? 24 : 28, Math.min(56, refHeight));
    b.style.height = b.style.minHeight = b.style.maxHeight = `${h}px`;
    b.style.setProperty("width", "max-content", "important");
    b.style.setProperty("min-width", "max-content", "important");
    b.style.maxWidth = "none";
    b.style.setProperty("display", "inline-flex", "important");
    b.style.setProperty("justify-content", "center", "important");
    b.style.setProperty("align-items", "center", "important");
    b.style.setProperty("padding-top", "0", "important");
    b.style.setProperty("padding-bottom", "0", "important");
    b.style.lineHeight = "1";
    const img = b.querySelector("img"), label = b.querySelector("span");
    const size = `${Math.min(24, Math.max(16, Math.round(h * 0.6)))}px`;
    img.style.setProperty("width", size, "important");
    img.style.setProperty("height", size, "important");
    img.style.flexShrink = "0";
    label.style.setProperty("font", "inherit", "important");
    label.style.setProperty("line-height", "1", "important");
    for (const child of [img, label]) {
      child.style.setProperty("margin", "0", "important");
      child.style.setProperty("padding", "0", "important");
      child.style.setProperty("align-self", "center", "important");
    }
  }
  function styleCollectionToolbar(element, bar) {
    const candidates = bar.querySelectorAll("button, .video-toolbar-left-item, .toolbar-left-item");
    const neighbor = [...candidates].find(candidate =>
      !candidate.closest(`#${INLINE_ID}, #${INLINE_COLLECTION_GROUP_ID}`) && candidate.getBoundingClientRect().height > 0);
    const height = neighbor ? Math.round(neighbor.getBoundingClientRect().height) : 36;
    const look = neighbor && getComputedStyle(neighbor);
    const props = {
      "--shandian-inline-height": Math.max(24, Math.min(56, height)) + "px",
      "--shandian-inline-icon": Math.max(16, Math.min(24, Math.round(height * 0.56))) + "px",
      "--shandian-inline-radius": Math.max(0, parseFloat(look?.borderTopLeftRadius) || 0,
        parseFloat(look?.borderTopRightRadius) || 0, parseFloat(look?.borderBottomLeftRadius) || 0,
        parseFloat(look?.borderBottomRightRadius) || 0) || height / 2,
      "--shandian-inline-font-size": look?.fontSize || "14px",
      "--shandian-inline-font-family": look?.fontFamily || "inherit",
      "--shandian-segment-color": look?.color || "inherit",
    };
    for (let source = neighbor; source && source !== bar; source = source.parentElement) {
      const background = getComputedStyle(source).backgroundColor;
      if (background !== "transparent" && background !== "rgba(0, 0, 0, 0)") {
        props["--shandian-segment-background"] = background;
        break;
      }
    }
    element.style.removeProperty("--shandian-segment-background");
    props["--shandian-inline-radius"] += "px";
    for (const [key, value] of Object.entries(props)) element.style.setProperty(key, value);
  }

  function inlineCollectionGroup(button) {
    return button?.parentElement?.id === INLINE_COLLECTION_GROUP_ID ? button.parentElement : null;
  }

  function syncInlineCollection(button, site) {
    if (!site.collection) return;
    const page = location.href;
    const group = inlineCollectionGroup(button);
    const choice = group?.querySelector("#" + INLINE_COLLECTION_ID);
    const currentChoice = choice?.dataset.page === page ? choice : null;
    if (currentChoice) {
      styleCollectionToolbar(group, group.parentElement);
      if (currentChoice.dataset.count) return;
    } else if (group) {
      group.before(button);
      group.remove();
      button.style.cssText = button._shSingleStyle;
      button.classList.add("shandian-inline-single");
      button._shCollectionLabel = "";
      button.querySelector("span").textContent = site.label || "闪电下载";
      button.title = "用闪电全能下载";
      button.setAttribute("aria-label", "闪电下载");
    }
    const previous = button._shCollectionProbe;
    const attempts = previous?.page === page ? previous.attempts : 0;
    if (attempts && (previous.pending || attempts >= 2 || Date.now() - previous.checked < 3000)) return;
    const probe = { page, pending: true, attempts: attempts + 1 };
    button._shCollectionProbe = probe;
    chrome.runtime.sendMessage({ type: "videoCollectionAction", page }).then((action) => {
      if (!action || action.explicit || !button.isConnected || location.href !== page || button._shCollectionProbe !== probe) return;
      if (currentChoice) {
        if (action.count) {
          currentChoice.dataset.count = String(action.count);
          currentChoice.textContent = action.label;
        }
        return;
      }
      const select = document.createElement("button");
      select.id = INLINE_COLLECTION_ID;
      select.type = "button";
      select.dataset.page = page;
      if (action.count) select.dataset.count = String(action.count);
      select.textContent = action.label;
      button._shSingleStyle = button.style.cssText;
      button.style.cssText = "";
      button.classList.remove("shandian-inline-single");
      button._shCollectionLabel = action.currentLabel;
      const caption = button.querySelector("span");
      caption.textContent = action.currentLabel;
      caption.style.setProperty("line-height", "inherit", "important");
      button.title = action.currentLabel;
      button.setAttribute("aria-label", action.currentLabel);
      select.addEventListener("click", async (event) => {
        event.preventDefault();
        event.stopPropagation();
        if (select.disabled || location.href !== page) return;
        select.disabled = true;
        nativeFeedback(select, "正在打开新建下载…", true);
        try {
          const result = await chrome.runtime.sendMessage({ type: "downloadCollection", page });
          if (select.isConnected) nativeFeedback(select, result?.ok ? "请在「新建下载」中勾选视频" : "没能打开合集，请重试");
        } catch {
          if (select.isConnected) nativeFeedback(select, "请刷新页面后重试");
        } finally {
          select.disabled = false;
        }
      });
      const segments = document.createElement("div");
      segments.id = INLINE_COLLECTION_GROUP_ID;
      segments.className = "shandian-segments shandian-inline-segments";
      segments.setAttribute("role", "group");
      segments.setAttribute("aria-label", "视频下载");
      styleCollectionToolbar(segments, button.parentElement);
      button.before(segments);
      segments.append(button, select);
    }).catch(() => {}).finally(() => {
      probe.pending = false;
      probe.checked = Date.now();
      const current = inlineCollectionGroup(button)?.querySelector("#" + INLINE_COLLECTION_ID);
      if (probe.attempts === 1 && current?.dataset.page === page && !current.dataset.count) {
        setTimeout(() => {
          if (button.isConnected && location.href === page) syncInlineCollection(button, site);
        }, 3200);
      }
    });
  }

  function mountInline() {
    if (audioOnly) return;
    mountNativeDownloads();
    const old = document.getElementById(INLINE_ID);
    const site = INLINE_SITES.find((s) => s.host.test(location.hostname) && s.path.test(location.pathname));
    if (!old || !enabled || !site) document.getElementById(INLINE_COLLECTION_GROUP_ID)?.remove();
    if (!enabled || !site) return old?.remove();
    const near = site.find?.();
    let columnAnchor = null;
    if (site.column && near) {
      // 分享按钮外面可能包了几层，往上找到竖排的直接子元素
      columnAnchor = near;
      while (columnAnchor.parentElement && columnAnchor.parentElement.childElementCount < 3) columnAnchor = columnAnchor.parentElement;
      if (old?.previousElementSibling === columnAnchor) return;
      old?.remove();
    } else if (site.column) return;
    else if (old?.isConnected) {
      if (!inlineCollectionGroup(old)) {
        if (site.collection) styleCollectionToolbar(old, old.parentElement);
        else fitInline(old, site, near || old.parentElement?.querySelector(`button:not(#${INLINE_ID}, #${INLINE_COLLECTION_ID})`));
      }
      syncInlineCollection(old, site);
      return;
    }
    // row：往外找到装着整排按钮的那层（至少这么多个子元素）。有的按钮外面还包了一层，直接用父元素会塞进别的格子里
    let rowEl = near?.parentElement;
    while (site.row && rowEl && rowEl.childElementCount < site.row && rowEl.parentElement) rowEl = rowEl.parentElement;
    const bar = site.column ? columnAnchor.parentElement : site.find ? rowEl : site.anchor ? document.querySelector(site.anchor)?.parentElement?.parentElement : site.bars.map((s) => document.querySelector(s)).find(Boolean);
    if (!bar) return;
    const b = document.createElement("button");
    b.id = INLINE_ID;
    b.type = "button";
    b.setAttribute("aria-label", "闪电下载");
    b.title = "用闪电全能下载";
    b.innerHTML = `<img src="${LOGO}" alt="" style="width:18px;height:18px;border-radius:4px;display:block"><span style="display:block;line-height:18px">${site.label || "闪电下载"}</span>`;
    b.style.cssText = "display:inline-flex;align-items:center;gap:6px;flex:none;align-self:center;height:36px;margin:0 8px;padding:0 14px;border:0;border-radius:18px;background:rgba(128,128,128,.18);color:inherit;font:inherit;font-size:14px;line-height:18px;box-sizing:border-box;cursor:pointer;white-space:nowrap;transition:background .12s;";
    const label = b.querySelector("span");
    let rest = "rgba(128,128,128,.18)";
    if (site.small) {
      b.style.cssText = "display:inline-flex;vertical-align:middle;align-items:center;gap:4px;height:24px;margin:0 10px 0 0;padding:0 10px;border:0;border-radius:12px;background:rgba(128,128,128,.18);color:inherit;font-size:12px;line-height:16px;box-sizing:border-box;cursor:pointer;white-space:nowrap;transition:background .12s;";
      const img = b.querySelector("img");
      img.style.width = img.style.height = "16px";
      img.style.borderRadius = "3px";
      label.style.lineHeight = "16px";
    }
    if (site.column) {
      rest = "none";
      b.style.cssText = "display:flex;flex-direction:column;align-items:center;width:100%;padding:0;margin:6px 0;border:0;background:none;color:#fff;cursor:pointer;";
      const img = b.querySelector("img");
      img.style.width = img.style.height = "32px";
      img.style.borderRadius = "8px";
      label.hidden = true;
      label.style.display = "none";
    }
    if (site.copyNeighbor && near) {
      // 外距交给站点的排版规则（这一排靠子元素规则留间距），边框、圆角、高度、文字色跟同排一致
      const cs = getComputedStyle(near);
      rest = "transparent";
      b.style.margin = "";
      b.style.background = rest;
      b.style.border = `1px solid ${cs.borderTopColor}`;
      b.style.borderRadius = cs.borderTopLeftRadius;
      b.style.height = `${near.offsetHeight}px`;
      b.style.padding = "0 10px";
      b.style.color = cs.color;
    }
    if (site.collection) {
      b.style.cssText = "";
      b.classList.add("shandian-inline-single");
      label.style.lineHeight = "inherit";
      styleCollectionToolbar(b, bar);
    } else fitInline(b, site, near || bar.querySelector("button:not(#" + INLINE_ID + ")"));
    // 网站的样式表可能盖掉 :hover，写在元素上最稳
    b.addEventListener("mouseenter", () => {
      if (site.collection) return;
      if (site.column) b.style.filter = "brightness(1.15)"; else b.style.background = "rgba(128,128,128,.25)";
    });
    b.addEventListener("mouseleave", () => {
      if (site.collection) return;
      if (site.column) b.style.filter = ""; else b.style.background = rest;
    });
    b.addEventListener("click", async (e) => {
      e.preventDefault();
      e.stopPropagation();
      label.textContent = "正在交接…";
      let text = "已交给闪电";
      try {
        const r = await chrome.runtime.sendMessage({ type: "hoverDownload", video: true });
        if (r?.noVideo) text = "没找到视频";
        else if (r?.ok === false) text = "交接失败";
      } catch {
        text = "请刷新页面"; // 扩展刚更新过
      }
      label.textContent = text;
      b.title = text;
      setTimeout(() => { label.textContent = b._shCollectionLabel || site.label || "闪电下载"; }, 2500);
    });
    if (site.column) columnAnchor.after(b);
    else if (site.where === "prepend") bar.prepend(b);
    else bar.append(b);
    syncInlineCollection(b, site);
  }
  let inlineQueued = false;
  if (!audioOnly) new MutationObserver(() => {
    if (inlineQueued) return;
    inlineQueued = true;
    setTimeout(() => { inlineQueued = false; mountInline(); }, 500);
  }).observe(document.documentElement, { childList: true, subtree: true });

  function build() {
    host = document.createElement("span");
    host.id = "shandian-hover-host";
    host.style.cssText = "position:fixed;left:0;top:0;width:0;height:0;z-index:2147483647;pointer-events:none;overflow:visible!important;display:block!important;visibility:visible!important;";
    const root = host.attachShadow({ mode: "closed" });
    const style = document.createElement("style");
    style.textContent = `
      button { position: fixed; display: none; width: ${SIZE}px; height: ${SIZE}px; padding: 0; border: 0; border-radius: 8px; cursor: pointer;
        background: none; box-shadow: 0 2px 8px rgba(0,0,0,.35); }
      button:hover { filter: brightness(1.12); }
      button:focus { outline: none; }
      /* 交出去之后整块盖一层：绿底对勾＝已交给 App，红底感叹号＝没交出去 */
      button.ok::after, button.fail::after { content: "✓"; position: absolute; inset: 0; border-radius: 8px; display: flex; align-items: center; justify-content: center;
        background: #2f9e44; color: #fff; font: 700 ${Math.round(SIZE * 0.6)}px/1 system-ui, sans-serif; }
      button.fail::after { content: "!"; background: #c92a2a; }
      img { display: block; width: 100%; height: 100%; border-radius: 8px; }
    `;
    button = document.createElement("button");
    button.title = "闪电下载"; button.setAttribute("aria-label", "闪电下载");
    button.style.pointerEvents = "auto";
    const icon = document.createElement("img"); icon.src = LOGO; icon.alt = "";
    button.appendChild(icon); root.appendChild(style); root.appendChild(button);
    button.addEventListener("pointerdown", (event) => {
      if (!target?.feishuBlock || !feishuDownloadControl(target.feishuBlock)) return;
      event.preventDefault();
      event.stopPropagation();
    });
    button.addEventListener("click", onClick);
    button.addEventListener("mouseenter", () => clearTimeout(hideTimer));
    button.addEventListener("mouseleave", scheduleHide);
    (isXhs && document.fullscreenElement || document.documentElement || document.body).appendChild(host);
  }

  const isChannelPreview = () => location.hostname === "channels.weixin.qq.com" && /\/finder-preview\//.test(location.pathname);
  function syncChannelButton() {
    if (!isChannelPreview() || !enabled) return;
    const candidates = [...document.querySelectorAll('video'), ...[...document.querySelectorAll('iframe')].filter(el => {
      try { const u = new URL(el.getAttribute('src') || '', location.href); return /(^|\.)qq\.com$/.test(u.hostname) && /video|player|finder/.test(u.pathname); } catch (_) { return false; }
    })];
    const media = candidates.find(el => {
      const r = el.getBoundingClientRect();
      return r.width >= MIN_W && r.height >= MIN_H && r.bottom > 0 && r.top < innerHeight
        && (el.tagName === "VIDEO" || !(r.width >= innerWidth * .9 && r.height >= innerHeight * .85));
    });
    if (!host?.isConnected || !button) { host?.remove(); host = null; button = null; build(); }
    const mount = document.fullscreenElement || document.documentElement || document.body;
    if (host.parentElement !== mount) mount.appendChild(host);
    target = { el: media || document.documentElement, video: true };
    let left = innerWidth - SIZE - 12, top = 12;
    if (media) {
      const r = media.getBoundingClientRect();
      left = r.right - SIZE - 8; top = r.top + 8;
    }
    button.style.left = `${Math.max(4, Math.min(left, innerWidth - SIZE - 4))}px`;
    button.style.top = `${Math.max(4, Math.min(top, innerHeight - SIZE - 4))}px`;
    button.style.display = "flex";
    if(button.disabled && Date.now()-hoverBusySince>45000){hoverRequest++;button.disabled=false;button.style.opacity="1";button.style.cursor="pointer";button.title="解析超时，请点击重试，也可刷新页面后再试";}
  }
  if (location.hostname === "channels.weixin.qq.com") { setInterval(syncChannelButton, 500); syncChannelButton(); document.addEventListener("fullscreenchange", syncChannelButton); }

  if (window.top === window && location.hostname === "channels.weixin.qq.com") {
    const auto = new URLSearchParams(location.hash.slice(1));
    const stamp = auto.get("shandian-auto");
    if(stamp && /^[a-f0-9]{32}$/.test(stamp)) {
      history.replaceState(null,"",location.pathname+location.search);
      const request=++hoverRequest;hoverBusySince=Date.now();
      if(button){button.disabled=true;button.title="正在自动解析视频…";button.style.opacity=".55";}
      chrome.runtime.sendMessage({type:"downloadPage",tabId:undefined,autoChannel:true,autoToken:stamp}).then(r=>{
        if(request!==hoverRequest || !button)return;
        button.disabled=false;button.style.opacity="1";button.className=r?.ok?"ok":"fail";
        button.title=r?.ok?"已交给闪电全能下载":`${r?.error || "解析失败"}。可点击重试，也可刷新页面后再试；仍失败时使用扩展中的闪电下载。`;
      }).catch(()=>{if(request===hoverRequest && button){button.disabled=false;button.style.opacity="1";button.title="扩展连接已失效，请刷新页面后重试";}});
    }
  }

  function linkUrl(el) {
    const a = el.closest?.("a[href]");
    if (!a) return null;
    if(/^(magnet:|ed2k:)/i.test(a.href))return a.href;
    let file=false;
    try {
      const u=new URL(a.href);
      if(!["http:","https:"].includes(u.protocol))return null;
      if(u.hostname==="github.com" && /^\/[^/]+\/[^/]+\/tree\//.test(u.pathname))return null;
      if(u.hostname==="github.com" && /^\/[^/]+\/[^/]+\/blob\//.test(u.pathname)){u.pathname=u.pathname.replace('/blob/','/raw/');return u.href;}
      if(["http:","https:"].includes(u.protocol))file=a.hasAttribute("download")
        || (u.hostname==="github.com" && /^\/[^/]+\/[^/]+\/(releases\/(?:latest\/)?download\/|raw\/|archive\/)/.test(u.pathname))
        || u.hostname==="raw.githubusercontent.com";
    } catch (_) {}
    return file || LINK.test(a.href) ? a.href : null;
  }

  function audioSource(audio) {
    const current = audio.currentSrc || audio.src;
    if (current) return /^https?:\/\//i.test(current) ? current : "";
    return [...audio.querySelectorAll("source[src]")].find(source => /^https?:\/\//i.test(source.src))?.src || "";
  }

  function audioHit(el) {
    const feishuPlayer = isFeishu && el.closest(".docx-audio-v2-container");
    if (feishuPlayer) {
      const feishuBlock = feishuPlayer.closest(".file-block.preview-type-audio");
      const players = feishuPlayer.querySelectorAll("audio");
      return feishuBlock && players.length === 1
        ? { el: feishuPlayer, audio: players[0], feishuBlock } : null;
    }
    const bounds = el.getBoundingClientRect();
    if (bounds.width < 96 || bounds.height < 20 || bounds.height > 160) return null;
    let audio = el.tagName === "AUDIO" ? el : null;
    if (!audio && !el.querySelector("video") && el.querySelector('button, [role="button"], [role="slider"]')) {
      const players = el.querySelectorAll("audio");
      if (players.length === 1 && players[0].getBoundingClientRect().height === 0) audio = players[0];
    }
    const url = audio && audioSource(audio);
    return url ? { el, audio, url } : null;
  }

  function pick(x, y) {
    const elements = document.elementsFromPoint(x, y).filter(el => el !== host);
    for (const el of elements) {
      const sound = audioHit(el);
      if (sound) return sound;
      if (!audioOnly && el.tagName === "VIDEO" && !inlineShown()) {
        const r = el.getBoundingClientRect();
        if (r.width >= MIN_W && r.height >= MIN_H) {
          // X：信息流一屏好几条，交这条推文自己的地址（页面地址只是 /home）
          const tweet = isX && el.closest("article")?.querySelector('a[href*="/status/"] time')?.parentElement?.href;
          const attachment = location.hostname === "docs.qingque.cn"
            && el.closest(".k-frame-attachment-preview-inner")?.querySelector(".header__name");
          const qingqueName = attachment?.textContent?.trim().replace(/\s*\([\d.]+\s*[KMGT]?(?:i?B)?\)\s*$/i, "");
          return tweet ? { el, url: tweet } : { el, video: true, qingqueName };
        }
      }
    }
    if (audioOnly) return null;
    const top = document.elementFromPoint(x, y);
    const url = top && linkUrl(top);
    return url ? { el: top.closest("a[href]"), url } : null;
  }

  function feishuDownloadControl(block) {
    const selected = document.querySelectorAll(".file-block.preview-selected");
    if (selected.length !== 1 || selected[0] !== block) return null;
    const controls = [...document.querySelectorAll('.block-toolbar__overlay [data-name="download"]')]
      .filter(el => el.getClientRects().length && !el.matches('[disabled],[aria-disabled="true"],.menu-item-disabled'));
    return controls.length === 1 ? controls[0] : null;
  }

  async function downloadFeishuAudio(block, control, isCurrent) {
    if (!block.isConnected || !isCurrent()) return false;
    for (let attempt = 0; attempt < 12 && !control; attempt++) {
      await new Promise(resolve => setTimeout(resolve, 50));
      if (!block.isConnected || !isCurrent()) return false;
      control = feishuDownloadControl(block);
    }
    if (!control) return false;
    control.click();
    return true;
  }

  function place(el, audio = false) {
    const r = el.getBoundingClientRect();
    // 检测到视频时统一放在画面右上角，链接入口仍靠近链接。
    let top, left;
    if (audio) {
      const fitsRight = r.right + GAP + SIZE + 4 <= innerWidth;
      left = fitsRight ? r.right + GAP : r.right - SIZE;
      top = fitsRight ? r.top + (r.height - SIZE) / 2 : r.top - SIZE - GAP;
      if (top < 4 && !fitsRight) top = r.bottom + GAP;
    } else if (el.tagName === "VIDEO") {
      top = Math.max(4, r.top) + 8;
      left = r.right - SIZE - 8;
    } else {
      top = r.top - SIZE - GAP;
      left = r.left;
      if (top < 4) top = r.bottom + GAP;
    }
    button.style.left = `${Math.max(4, Math.min(left, innerWidth - SIZE - 4))}px`;
    button.style.top = `${Math.max(4, Math.min(top, innerHeight - SIZE - 4))}px`;
    button.style.display = "flex";
  }

  function hide() {
    target = null;
    // 整个拿掉，页面上不留节点
    host?.remove();
    host = null;
    button = null;
  }

  function scheduleHide() {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(hide, LEAVE);
  }

  function onMove(e) {
    // Some pages dispatch a plain Event with no mouse coordinates.
    const { clientX: x, clientY: y } = e;
    if (!Number.isFinite(x) || !Number.isFinite(y)) return;
    const now = Date.now();
    if (!enabled || now - lastMove < 30) return;
    lastMove = now;
    if (isNativeMediaSite && now-nativeScanAt>400) { nativeScanAt=now; mountNativeDownloads(); }
    if (isChannelPreview()) { syncChannelButton(); return; }
    if (document.fullscreenElement && !isXhs) return hide();
    // 鼠标在按钮上：不算移开
    if (host && document.elementFromPoint(x, y) === host) return clearTimeout(hideTimer);
    let hit = pick(x, y);
    if ((hit?.video && isPixabay) || (hit?.audio && isNativeMediaSite)) {
      let row = hit.el;
      for (let i=0; row && i<6; i++, row=row.parentElement) {
        if (row.querySelector?.(".shandian-site-download")) { hit=null; break; }
      }
    }
    if (!hit) {
      if (target) scheduleHide();
      return;
    }
    if (target?.el === hit.el && host?.isConnected) return clearTimeout(hideTimer);
    clearTimeout(hideTimer);
    if (!host?.isConnected) build();
    target = hit;
    // 将浮标挂在原生音频标题下，使用户本次点击同时走飞书的附件选择逻辑。
    const title = hit.feishuBlock?.querySelector(".audio-title-context");
    const mount = title || (isXhs && document.fullscreenElement) || document.documentElement || document.body;
    if (host.parentElement !== mount) mount.appendChild(host);
    button.className = "";
    button.title = "用闪电全能下载";
    place(hit.el, Boolean(hit.audio));
  }

  async function onClick(e) {
    if (!target || !button || button.disabled) return;
    const { url, video, audio, qingqueName, feishuBlock } = target;
    const control = feishuBlock && feishuDownloadControl(feishuBlock);
    if (!feishuBlock || control) {
      e.preventDefault();
      e.stopPropagation();
    }
    const clicked = button;
    clicked.className = "";
    clicked.disabled = true; clicked.title = "正在获取下载地址…"; clicked.style.opacity=".55"; clicked.style.cursor="progress";
    hoverBusySince=Date.now(); const request=++hoverRequest;
    const feedback=typeof window!=="undefined"?window.ShFeedback:null,context=feedback?.context(clicked);
    const show=(message,ms=3500)=>{clicked._shFeedbackClose?.();clicked._shFeedbackClose=feedback?.show(clicked,message,{brief:true,ms,context});};
    show("正在解析…",0);
    const timeout=setTimeout(()=>{if(request===hoverRequest && clicked.disabled)show("解析时间较长，可刷新页面后重试",0)},45000);
    try {
      let r;
      if (feishuBlock) {
        const ok = await downloadFeishuAudio(feishuBlock, control, () => request === hoverRequest);
        r = { ok, error: "飞书未打开这条音频的原件下载按钮" };
      } else {
        const source = audio ? audioSource(audio) : url;
        r = audio && !source
          ? { ok: false, error: "音频地址已变化，请先播放后重试" }
          : await chrome.runtime.sendMessage({ type: "hoverDownload", url: source, video: !!video, qingqueName });
      }
      if(request!==hoverRequest) return;
      clicked.className = r?.ok ? "ok" : "fail";
      const successLabel = feishuBlock ? "已发起飞书原文件下载" : "已交给闪电全能下载";
      clicked.title = r?.ok ? successLabel : r?.noVideo
        ? "没找到可直接下载的视频：请先播放，再用扩展的「开始侦测本页播放资源」"
        : r?.error || "没交给闪电全能下载，请确认应用已打开";
      if (!r?.ok && isChannelPreview()) clicked.title += "。可点击重试，也可刷新页面后再试；仍失败时使用扩展中的闪电下载。";
    } catch {
      if(request!==hoverRequest) return;
      clicked.className = "fail"; // 扩展刚更新过，这个页面要刷新才连得上
      clicked.title = "扩展刚更新过，刷新页面后再试";
    }
    clearTimeout(timeout);
    show(clicked.title,clicked.className==="ok"?2500:7000);
    clicked.disabled = false; clicked.style.opacity="1"; clicked.style.cursor="pointer";
    if (!isChannelPreview()) setTimeout(hide, 1200);
  }

  function pixabayItem(row) {
    const tool=/^(在新标签页中打开链接|在新分页开启连结|open link in (?:a )?new tab|download|下载)$/i;
    const links=[...row.querySelectorAll('a[href]')].filter(a=>{
      try{const u=new URL(a.href,location.href);return /^(www\.)?pixabay\.com$/.test(u.hostname) && /^\/(?:[a-z]{2}\/)?(music|sound-effects|videos)\/(?!search\/)[^/]+-\d+\/?$/.test(u.pathname)}catch(_){return false}
    });
    const link=links.find(a=>a.querySelector('img')?.alt) || links.find(a=>!tool.test(a.textContent.trim()));
    let url,title;
    if(link){
      const u=new URL(link.href,location.href);url=u.origin+u.pathname;
      title=link.querySelector('img')?.alt || link.title || link.getAttribute?.('aria-label') || link.textContent.trim();
      if(!title || tool.test(title))title=u.pathname.split('/').filter(Boolean).pop().replace(/-\d+$/,'').replace(/-/g,' ');
      const artist=row.querySelector('a[href*="/users/"]')?.textContent?.trim();
      title=[title,artist].filter(Boolean).join(" - ");
    } else if(/^\/(?:[a-z]{2}\/)?(music|sound-effects|videos)\/[^/]+-\d+\/?$/.test(location.pathname)){
      url=location.origin+location.pathname;
      title=(document.querySelector('meta[property="og:title"]')?.content || document.querySelector('h1')?.textContent || "").replace(/\.?\s*Free Stock Video.*$/i, "").replace(/\s*[-|]\s*Pixabay.*$/i, "").trim();
    }
    if(!url || !title)return null;
    return {url,title,kind:/\/videos\//.test(new URL(url).pathname)?"Pixabay视频 · 原站选画质":"Pixabay音频",resolve:true};
  }
  function pixabayTitle(row) { return pixabayItem(row)?.title || ""; }
  function loadAigeiMenu(native) {
    native.dispatchEvent(new MouseEvent("mouseover",{bubbles:true,view:window,relatedTarget:null}));
  }
  const nativeButtons = new Map();
  let pendingSiteNativeClick=null;
  function trySiteFlowNativeClick(){
    const pending=pendingSiteNativeClick;if(!pending)return;
    const canonical=value=>{try{const u=new URL(value,location.href);return u.origin+u.pathname.replace(/\/$/,'')}catch(_){return ''}};
    if(canonical(location.href)!==canonical(pending.itemHref))return;
    const candidates=[...document.querySelectorAll('button,a,[role="button"]')].filter(el=>!el.closest('.shandian-site-download') && el.getClientRects().length && !el.disabled);
    const native=candidates.find(el=>/^(免费下载|Free download)$/i.test(el.innerText?.trim() || ''))
      || [...nativeButtons].find(([el,b])=>candidates.includes(el) && canonical(siteCatalogItem(el,b)?.url)===canonical(pending.itemHref))?.[0];
    if(!native)return;
    pendingSiteNativeClick=null;clearTimeout(pending.timer);console.info('Shandian Pixabay native download button ready');pending.reply({ok:true});
    setTimeout(()=>native.click(),0);
  }
  function nativeFeedback(button,text,busy=false) {
    button.title=text;button.setAttribute("aria-busy",String(busy));
    button._shFeedbackClose?.();
    button._shFeedbackClose=window.ShFeedback?.show(button,text,{brief:true,ms:busy?0:5000});
  }
  function mountNativeDownloads() {
    if (!isNativeMediaSite) return;
    for (const [native, b] of nativeButtons) if (!native.isConnected || !b.isConnected || !enabled) { clearInterval(b._shandianKeepAlive); b._shFeedbackClose?.(); b._shandianRestore?.(); b._shandianHoverRow?.classList.remove("shandian-download-row"); b.remove(); nativeButtons.delete(native); }
    if (!enabled) return;
    if(!document.getElementById("shandian-site-download-style")) {
      const style=document.createElement("style");style.id="shandian-site-download-style";
      style.textContent=`.shandian-site-download{opacity:0!important;visibility:hidden!important;pointer-events:none!important;transition:opacity .1s ease!important}.shandian-site-download[aria-busy="true"],.shandian-download-row:hover .shandian-site-download,.shandian-download-row:has(:focus-visible) .shandian-site-download{opacity:1!important;visibility:visible!important;pointer-events:auto!important}`;
      (document.head || document.documentElement).append(style);
    }
    const isAigei = /(^|\.)aigei\.com$/.test(location.hostname);
    const selectors = isAigei ? '[id="audioDropMenu"].js-down-btn-with-drop-down'
      : 'a[download],a[href*="/download"],[aria-label*="download" i],[title*="download" i],[aria-label*="下载"],[title*="下载"],[class*="download" i]';
    const candidates=new Set();
    document.querySelectorAll(selectors).forEach(el => {
      if(el.closest(".shandian-site-download") || el.classList.contains("shandian-download-row"))return;
      const native=isAigei ? el : el.closest('button,a,[role="button"]') || el.querySelector('button,a,[role="button"]');
      if(native)candidates.add(native);
    });
    for(const native of candidates) {
      if(nativeButtons.has(native) || native.closest('.shandian-site-download') || !native.getClientRects().length)continue;
      let row=isAigei ? native.closest('[id^="audioItemBox_"]') : native.parentElement,found=!!(isAigei && row);
      for(let i=0;!found && row && i<7;i++,row=row.parentElement){
        const r=row.getBoundingClientRect();
        if(r.height>650)break;
        if(r.width>180 && r.height>40 && row.querySelector('audio,video,canvas,[class*="wave"],a[href*="/item/"],a[href*="/music/"],a[href*="/sound-effects/"],a[href*="/videos/"]')){found=true;break;}
      }
      if(!found)continue;
      const b=document.createElement("button");b.type="button";b.className="shandian-site-download";b.title="通过网站下载并交给闪电";b.setAttribute("aria-label","闪电下载");
      b._shandianNative=native;b._shandianItemRow=row;
      b.innerHTML=`<img src="${LOGO}" alt="" style="width:20px;height:20px;border-radius:4px"><span>闪电下载</span>`;
      b.style.cssText="display:inline-flex;align-items:center;justify-content:center;gap:5px;flex:none;vertical-align:middle;margin:0 0 0 8px;padding:0 9px;height:32px;border:1px solid currentColor;border-radius:7px;background:transparent;color:inherit;font:inherit;font-size:12px;cursor:pointer;white-space:nowrap;box-sizing:border-box";
      b.querySelector("span").style.display="none";
      b.style.width=b.style.minWidth="40px";b.style.height="40px";b.style.padding="0";
      b.style.border="0";b.style.outline="none";b.style.background="transparent";b.style.boxShadow="none";
      b.querySelector("img").style.width=b.querySelector("img").style.height="32px";
      if(isPixabay)b.style.margin="0";
      b.addEventListener("mouseenter",()=>{b.style.filter="brightness(1.15)";});
      b.addEventListener("mouseleave",()=>{b.style.filter="";b.style.background="transparent";});
      b.addEventListener("click",async event=>{
        event.preventDefault();event.stopPropagation();if(b.disabled)return;
        if(isAigei && b.dataset.shandianRequest && !b.dataset.shandianBatchFormat){loadAigeiMenu(native);native.click();return;}
        clearInterval(b._shandianKeepAlive);
        b.disabled=true;if(!b._shandianBatchResolve)nativeFeedback(b,"正在准备下载…",true);const requestId=crypto.randomUUID();b.dataset.shandianRequest=requestId;
        try {
          const name=isAigei ? row.querySelector('.title-name')?.textContent?.trim() || "" : pixabayTitle(row);
          let readyTimer;
          const reply=await Promise.race([
            chrome.runtime.sendMessage({type:"siteDownload",requestId,name,itemHref:isPixabay?pixabayItem(row)?.url:'',collectOnly:b.dataset.shandianCollectOnly==="true",batchId:b.dataset.shandianCollectOnly==="true"?siteBatchJob?.id:null}).finally(()=>clearTimeout(readyTimer)),
            new Promise((_,reject)=>{readyTimer=setTimeout(()=>reject(Error("扩展连接超时，请刷新页面后重试")),8000)})
          ]);
          if(!reply?.ok)throw Error(reply?.error || "扩展连接已失效，请刷新页面后重试");
          if(b.dataset.shandianRequest!==requestId)return;
          if(reply.directUrl){await handoffSiteDownload(b,requestId,reply.directUrl);return;}
          if(reply.needsOriginal && b._shandianBatchResolve){
            nativeFeedback(b,reply.reason || '需要原站取链');
            b._shandianBatchResolve(reply);return;
          }
          const batchFormat=b.dataset.shandianBatchFormat;
          b.querySelector("span").textContent=reply.flowUrl?'等待原站准备':batchFormat?"正在自动选格式":"等待原站操作";
          if(!reply.flowUrl)nativeFeedback(b,batchFormat?"正在自动选择 "+batchFormat.toUpperCase()+"…":isAigei?"请在原站选择下载格式":"请按原站提示选择画质或下载",!!batchFormat || !!reply.flowUrl);
          const heartbeat=setInterval(()=>{
            if(!b.isConnected || b.dataset.shandianRequest!==requestId){clearInterval(heartbeat);return;}
            chrome.runtime.sendMessage({type:"siteDownloadKeepAlive",requestId}).then(r=>{if(!r?.ok)clearInterval(heartbeat);}).catch(()=>clearInterval(heartbeat));
          },20000);
          b._shandianKeepAlive=heartbeat;
          if(reply.flowUrl){
            siteBatchJob.canView=true;
            publishSiteBatch();
            return;
          }
          if(isAigei)loadAigeiMenu(native);
          if(!batchFormat || !native.closest('.dropdown')?.querySelector('.dropdown-menu [item-down-type]'))native.click();
          setTimeout(()=>{clearInterval(heartbeat);if(b.isConnected && b.dataset.shandianRequest===requestId){delete b.dataset.shandianRequest;b.disabled=false;b.querySelector("span").textContent="闪电下载";nativeFeedback(b,"网站尚未开始下载，可重试；仍无反应请刷新页面");}},120000);
          if(isAigei){
            b.disabled=false;
            const format=b.dataset.shandianBatchFormat;
            if(format)await selectAigeiBatchFormat(native,b,row,requestId,format);
          }

        } catch (error) {if(b._shandianBatchResolve){b._shandianBatchResolve({error:error.message});return;}clearInterval(b._shandianKeepAlive);delete b.dataset.shandianRequest;b.disabled=false;b.querySelector("span").textContent="获取失败";nativeFeedback(b,/[\u4e00-\u9fff]/.test(error?.message || "")?error.message:"扩展连接已失效，请刷新页面后重试");}
      });
      let anchor=native;
      for(let i=0;i<3 && anchor.parentElement!==row;i++){
        const parent=anchor.parentElement;if(!parent)break;
        const cs=getComputedStyle(parent);
        if(cs.display.includes("flex") && !cs.flexDirection.startsWith("column"))break;
        if(parent.childElementCount>1)break;
        anchor=parent;
      }
      if(isAigei) {
        // Compact cards have a fixed-width download/favourite strip; keep that strip intact.
        b.querySelector("span").style.display="none";
        b.style.width=b.style.minWidth="32px";b.style.height="32px";b.style.padding="0";
        b.querySelector("img").style.width=b.querySelector("img").style.height="28px";
        if(!native.closest('.audio-download-box-common')) {
          b.style.position="absolute";b.style.right="6px";b.style.top="6px";b.style.margin="0";b.style.zIndex="2";
          const heading=row.querySelector('.audio-item-row1');
          const oldPadding=heading?.style.paddingRight;
          if(heading)heading.style.paddingRight="44px";
          b._shandianRestore=()=>{if(heading)heading.style.paddingRight=oldPadding;};
          row.append(b);
        } else {
          const bar=native.closest('.audio-download-box'),wrap=native.closest('.dropdown');
          if(!bar || !wrap)continue;
          const oldWidth=bar.style.width;
          bar.style.width=`${Math.ceil(bar.getBoundingClientRect().width)+44}px`;
          b.style.cssFloat="left";b.style.margin="0 4px";
          b._shandianRestore=()=>{bar.style.width=oldWidth;};
          const favourite=bar.querySelector('.fav-btn');
          (favourite || wrap).after(b);
        }
      } else anchor.after(b);
      const hoverRow=isAigei && !native.closest('.audio-download-box-common') ? row.closest('.mpic-tpl') || row : row;
      hoverRow.classList.add("shandian-download-row");b._shandianHoverRow=hoverRow;
      nativeButtons.set(native,b);
    }
    trySiteFlowNativeClick();
  }
  if(isNativeMediaSite)chrome.runtime.onMessage.addListener(msg=>{
    if(msg.type==='siteBatchBrowserTask'){
      console.warn('Shandian Pixabay browser download task observed');
      if(siteBatchJob){siteBatchJob.browserDownloads++;siteBatchJob.lastError='检测到浏览器下载任务，批量已停止';}
      for(const b of nativeButtons.values())if(b._shandianBatchResolve)b._shandianBatchResolve({error:'检测到浏览器下载任务，批量已停止'});
      return;
    }
    if(msg.type!=="siteDownloadResult")return;
    for(const b of nativeButtons.values())if(b.dataset.shandianRequest===msg.requestId){
      if(msg.collected && (!b._shandianBatchResolve || msg.batchId!==siteBatchJob?.id))continue;
      clearInterval(b._shandianKeepAlive);
      if(b._shandianBatchResolve){
        b._shandianBatchResolve(msg.ok && msg.collected ? msg : {error:msg.error});
        continue;
      }
      if(!msg.ok){nativeFeedback(b,msg.error || "未能取得直链，批量已停止");b._shandianBatchResolve?.({error:msg.error});b.disabled=false;delete b.dataset.shandianRequest;continue;}
      if(isNativeMediaSite && !msg.collected && msg.ok && b._shandianNative?.getAttribute('aria-expanded')==='true')b._shandianNative.click();
      if(msg.collected){console.info('Shandian Pixabay original URL handed off before browser download');b._shandianBatchResolve(msg);}
      b.disabled=false;b.querySelector("span").textContent=msg.collected?"已收集":msg.ok?"已交给闪电":"交接失败";b.title=msg.ok?"已交给闪电全能下载":"仍由网站下载，请确认闪电全能下载已打开";
      nativeFeedback(b,msg.collected?"已收集":msg.ok?"已交给闪电":"交接失败，请确认应用已打开后重试");
      delete b.dataset.shandianRequest;
    }
  });
  async function handoffSiteDownload(b,requestId,url){
    nativeFeedback(b,"正在交给闪电…",true);
    try {
      const result=await chrome.runtime.sendMessage({type:"siteDownloadDirect",requestId,url});
      if(b.dataset.shandianRequest!==requestId)return;
      if(!result?.ok)b._shandianBatchResolve?.({error:result?.error || '文件链接未能交接，批量已停止'});
      if(result?.ok && !result.collected && b._shandianNative?.getAttribute('aria-expanded')==='true')b._shandianNative.click();
      if(result?.collected)b._shandianBatchResolve?.(result);
      clearInterval(b._shandianKeepAlive);b.disabled=false;delete b.dataset.shandianRequest;
      b.querySelector("span").textContent=result?.collected?"已收集":result?.ok?"已交给闪电":"交接失败";
      nativeFeedback(b,result?.collected?"已收集，批量完成后交给应用":result?.ok?"已交给闪电":"交接失败，请确认应用已打开后重试");
    } catch(_){b._shandianBatchResolve?.({error:'扩展连接已失效，请刷新页面后重试'});clearInterval(b._shandianKeepAlive);b.disabled=false;delete b.dataset.shandianRequest;nativeFeedback(b,"扩展连接已失效，请刷新页面后重试");}
  }
  if(isNativeMediaSite)window.addEventListener("message",async event=>{
    const data=event.data;
    if(event.source!==window || event.origin!==location.origin || !["shandian-site-download","shandian-site-download-error"].includes(data?.type))return;
    const b=[...nativeButtons.values()].find(b=>b.dataset.shandianRequest===data.requestId);
    if(!b)return;
    if(data.type==="shandian-site-download-error"){nativeFeedback(b,data.error);b._shandianBatchResolve?.({error:data.error});b.disabled=false;clearInterval(b._shandianKeepAlive);return;}
    await handoffSiteDownload(b,data.requestId,data.url);
  });
  let siteBatchJob=null;
  const siteBatchPopupPorts=new Set();
  if(isPixabay)chrome.runtime.onConnect?.addListener(port=>{
    if(port.name!=='pixabayBatchPopup' || port.sender?.url!==chrome.runtime.getURL('popup.html'))return;
    siteBatchPopupPorts.add(port);
    updateSiteBatchCard();
    port.onDisconnect.addListener(()=>{siteBatchPopupPorts.delete(port);updateSiteBatchCard();});
  });
  function siteBatchSnapshot(job){
    return {id:job.id,running:job.running,total:job.items.length,done:job.ready.size,failed:job.failed,items:job.items,
      ready:[...job.ready],delivered:[...job.delivered],unchecked:[...job.unchecked],lastError:job.lastError || '',
      canView:!!job.canView,failedKey:job.failedKey,browserDownload:!!job.browserDownloads,submitting:!!job.submitting};
  }
  function updateSiteBatchCard(){
    const job=siteBatchJob;
    if(!job)return;
    const available=job.browserDownloads ? 0 : job.ready.size-job.delivered.size;
    const show=!siteBatchPopupPorts.size && !job.cardDismissed && (job.running || available>0);
    if(show && !job.card){
      job.card=window.ShFeedback?.topRight({id:'shandian-pixabay-batch',title:'Pixabay 批量下载',withProgress:true,
        onClose:()=>{job.cardDismissed=true;}});
      if(job.card)job.card.host.getBoundingClientRect();
    }
    if(!job.card)return;
    job.card.message.textContent=job.submitting?'正在交给闪电…':job.running
      ? `已解析 ${job.ready.size} / ${job.items.length} 项`
      : `有 ${available} 项可下载，点扩展图标选择。`;
    job.card.progress.max=job.items.length;
    job.card.progress.value=job.ready.size+job.failed;
    job.card.setVisible(show);
  }
  function publishSiteBatch(){
    updateSiteBatchCard();
    return chrome.runtime.sendMessage({type:'siteBatchProgress',batch:siteBatchSnapshot(siteBatchJob)}).catch(()=>{});
  }
  function aigeiFormatOption(menu,format) {
    const options=[...(menu?.querySelectorAll('[item-down-type]') || [])];
    return options.find(a=>(a.querySelector?.('.td-format')?.textContent || '').trim().toLowerCase()===format)
      || options.find(a=>!a.querySelector?.('.td-format') && a.getAttribute('item-down-type')===(format==='wav'?'item_audio':'item_audio_mp3'));
  }
  async function selectAigeiBatchFormat(native,b,row,requestId,format) {
    const deadline=Date.now()+15000;
    while(b.isConnected && b.dataset.shandianRequest===requestId && Date.now()<deadline){
      const menu=native.closest('.dropdown')?.querySelector('.dropdown-menu');
      if(menu?.querySelector('[item-down-type]')){
        const status=aigeiFormatState(row,format);
        if(status.blocked){
          if(siteBatchJob)siteBatchJob.lastError=status.blockedReason;
          nativeFeedback(b,status.blockedReason);
          chrome.runtime.sendMessage({type:"siteBatchProgress",key:"aigei:"+native.getAttribute('itemid'),format,reason:status.blockedReason}).catch(()=>{});
          b._shandianBatchResolve?.({blocked:true,error:status.blockedReason});return;
        }
        const option=aigeiFormatOption(menu,format);
        if(option){nativeFeedback(b,"正在获取 "+format.toUpperCase()+"…",true);option.click();return;}
      }
      await new Promise(r=>setTimeout(r,100));
    }
    if(b.dataset.shandianRequest===requestId){nativeFeedback(b,"格式菜单未加载，请刷新页面后重试");b._shandianBatchResolve?.(null);}
  }
  function aigeiFormatState(row,format) {
    const menu=row.querySelector('.dropdown-menu'),text=menu?.innerText || "";
    const options=[...(menu?.querySelectorAll('[item-down-type]') || [])];
    const option=aigeiFormatOption(menu,format);
    let reason="";
    if(option){
      const info=option.textContent || "";
      if(/购买授权/.test(info))reason="需购买授权";
      else if(/(?:需|仅限|开通).*(?:VIP|会员)|(?:VIP|会员)专享/i.test(info))reason="需 VIP";
    }else if(menu && options.length && format==='wav' && /购买授权/.test(text))reason="需购买授权";
    else if(menu && options.length)reason="不提供 "+format.toUpperCase()+" 格式";
    return {blocked:!!reason,blockedReason:reason};
  }
  function siteCatalogItem(native,b) {
    const row=b._shandianItemRow || b._shandianHoverRow;if(!row)return null;
    if(!isPixabay){
      const id=native.getAttribute("itemid"),title=row.querySelector('.title-name')?.textContent?.trim();
      if(!id || !title)return null;
      return {url:"aigei:"+id,title,kind:"爱给",resolve:true,formats:{mp3:aigeiFormatState(row,"mp3"),wav:aigeiFormatState(row,"wav")}};
    }
    return pixabayItem(row);
  }
  async function runSiteBatch(items,format,batchId) {
    siteBatchJob?.card?.close();
    const keys=items.map(item=>item.url);
    const job=siteBatchJob={id:batchId,items,format,failed:0,browserDownloads:0,ready:new Set(),delivered:new Set(),unchecked:new Set()};
    let next=0, stopped=false;
    job.running=true;job.lastError='';
    publishSiteBatch();
    const entries=new Map();
    for(const [native,b] of nativeButtons){
      const key=siteCatalogItem(native,b)?.url;
      if(key && !entries.has(key))entries.set(key,[native,b]);
    }
    const resolveItem=async key=>{
      const entry=entries.get(key);
      if(!entry || entry[1].isConnected===false)return {error:'所选素材已从页面移除，请重新抓取'};
      const [native,b]=entry, item=siteCatalogItem(native,b);
      if(!item || (item.formats?.[format] || item).blocked || (isPixabay && b.dataset.shandianRequest))return {error:'所选素材当前无法取链，未提交本批下载'};
      return new Promise(resolve=>{
        let ended=false;
        const finish=value=>{if(ended)return;ended=true;clearTimeout(timer);job.canView=false;if(!value?.url && b.dataset.shandianRequest)chrome.runtime.sendMessage({type:"releaseSiteIntent",requestId:b.dataset.shandianRequest}).catch(()=>{});clearInterval(b._shandianKeepAlive);b.disabled=false;b._shandianBatchResolve=null;delete b.dataset.shandianRequest;delete b.dataset.shandianCollectOnly;delete b.dataset.shandianBatchFormat;resolve(value);};
        const timer=setTimeout(()=>finish({error:"等待 Pixabay 返回下载地址超时。"}),120000);
        b._shandianBatchResolve=finish;b.dataset.shandianCollectOnly="true";if(!isPixabay)b.dataset.shandianBatchFormat=format;
        b.click();
      });
    };
    const worker=async()=>{
      while(!stopped && !job.cancelled && next<keys.length){
        const key=keys[next++];
        const result=await resolveItem(key).catch(error=>({error:error?.message}));
        if(result?.url)job.ready.add(key);
        else if(!job.cancelled) {
          job.failed++;job.failedKey=key;job.lastError=job.lastError || result?.error || '未能取得文件链接';
          if(!result?.blocked)stopped=true;
        }
        publishSiteBatch();
      }
    };
    try {
      await worker();
    } catch(error){job.lastError=error?.message || '批量取链失败';}
    finally {
      const autoSubmit=!job.cancelled && !job.browserDownloads && !job.failed && job.ready.size===keys.length;
      if(autoSubmit && keys.some(key=>!job.delivered.has(key) && !job.unchecked.has(key))){
        job.submitting=true;publishSiteBatch();
      }
      const result=await chrome.runtime.sendMessage({type:'siteBatchFinished',batchId:job.id,review:!job.browserDownloads && job.ready.size>0,autoSubmit}).catch(()=>null);
      if(result?.autoError)job.lastError=result.autoError;
      for(const key of result?.keys || [])job.delivered.add(key);
      if(!result?.ok && job.ready.size>job.delivered.size){job.ready=new Set(job.delivered);job.lastError='解析结果已过期，请重新解析';}
      job.submitting=false;
      job.running=false;publishSiteBatch();
    }
  }
  if(isNativeMediaSite)chrome.runtime.onMessage.addListener((msg,sender,reply)=>{
    if(msg.type==='runSiteFlowNative' && isPixabay){
      if(pendingSiteNativeClick){reply({ok:false,error:'原站下载正在准备'});return;}
      const pending=pendingSiteNativeClick={itemHref:msg.itemHref,reply};
      pending.timer=setTimeout(()=>{if(pendingSiteNativeClick!==pending)return;pendingSiteNativeClick=null;reply({ok:false,error:'原站下载按钮未加载，请检查素材页'});},10000);
      mountNativeDownloads();return true;
    }
    if(!isPixabay && ['collectSiteCatalog','runSiteBatch'].includes(msg.type)){reply({ok:false,items:[],error:'爱给暂不支持批量下载，请逐项下载'});return;}
    if(isPixabay && msg.type==='getSiteBatchStatus'){reply({batch:siteBatchJob?siteBatchSnapshot(siteBatchJob):null});return;}
    if(isPixabay && ['siteBatchDelivered','setSiteBatchSelection'].includes(msg.type)){
      const job=siteBatchJob;
      if(!job || job.id!==msg.batchId){reply({ok:false});return;}
      if(msg.type==='siteBatchDelivered'){
        for(const key of msg.keys || [])if(job.items.some(item=>item.url===key))job.delivered.add(key);
        publishSiteBatch();
      }else{
        const unchecked=new Set(Array.isArray(msg.unchecked)?msg.unchecked:[]);
        job.unchecked=new Set(job.items.map(item=>item.url).filter(key=>unchecked.has(key)));
      }
      reply({ok:true});return;
    }
    if(isPixabay && msg.type==='cancelSiteBatch'){
      const job=siteBatchJob;
      if(!job?.running || job.submitting || job.id!==msg.batchId){reply({ok:false});return;}
      job.cancelled=true;job.lastError='已停止批量获取';
      for(const b of nativeButtons.values())b._shandianBatchResolve?.({error:job.lastError});
      reply({ok:true});return;
    }
    if(msg.type==="collectSiteCatalog"){
      mountNativeDownloads();const items=new Map();
      for(const [native,b] of nativeButtons){const item=siteCatalogItem(native,b);if(item && !items.has(item.url))items.set(item.url,item);}
      reply({items:[...items.values()].slice(0,100),batch:siteBatchJob?siteBatchSnapshot(siteBatchJob):null});return;
    }
    if(msg.type==="runSiteBatch"){
      if(siteBatchJob?.running){reply({ok:false,error:"当前页正在批量处理"});return;}
      const items=(msg.items || []).slice(0,100);
      if(!items.length){reply({ok:false});return;}
      runSiteBatch(items,msg.format,msg.batchId).catch(()=>{if(siteBatchJob)siteBatchJob.running=false;});reply({ok:true,queued:items.length});
    }
  });

  if(isPixabay && window===top)chrome.runtime.sendMessage({type:'siteFlowDocumentReady'}).then(result=>console.info('Shandian Pixabay flow document ready: '+!!result?.ok)).catch(()=>{});

  document.addEventListener("fullscreenchange",()=>{
    if(isXhs && host && target){(document.fullscreenElement || document.documentElement).appendChild(host);place(target.el, Boolean(target.audio));}
  });

  chrome.storage.local.get("hoverButton").then(({ hoverButton = true }) => { enabled = hoverButton; mountInline(); });
  chrome.storage.onChanged.addListener((c) => {
    if (c.hoverButton) {
      enabled = c.hoverButton.newValue !== false;
      if (!enabled) hide();
      mountInline();
    }
  });
  document.addEventListener("mousemove", onMove, { passive: true, capture: true });
  addEventListener("scroll", () => target && hide(), { passive: true, capture: true });
})();
