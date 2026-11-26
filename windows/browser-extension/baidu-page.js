// 百度网盘页面的快捷入口：选中文件后，工具栏上多一个按钮，一点就交给闪电全能下载。
// 只往工具栏里放这一个按钮；页面结构变了就找不到工具栏，什么也不做，弹窗里的选择入口仍可用。
(() => {
  if (window.top !== window) return;
  const ID = "shandian-baidu-btn";
  const LABEL = "闪电下载";
  const LOGO = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAAIHUlEQVR4nIVXa4hdVxX+1j7n3MfcuXfmZm7GTKJS26atWMZCQqHVxEd9t7ZKqRGjQhHaPwH9IbZiMMSCUKMgIlYRO0VRSDX2h6EELNKmNUo1RQVj26SNUZqkSc087tznOXstWWufe+ckbfQOm3PuPWfv9a21vu/bewgAREBEEL1feuSuG2vl7E5hmhcXTTpygIsAvZJeI8DpfRgMwAkDOlivPh/63YOFQexXycnfOhwfmN75y2eLMWl0848Hb69fuYEfjEjujhJXgThdGnDxOJgBMTA5IAWisA2ArAX3Wf6bgsgAgynwme97oYWXz87e9/b7Hm4LQCR74I6v+8jk22p8MK7F2/yqB0BenCOiCOIiUDHguAqk72ky0OJJNiwEDL8bGPYQ8SBmgUgU1SNknezpkx132+YLh1Yd7QVvSob74gq2ZYv9IXsRCCJiOIg4YnHQIfmVOQyvQ+zKmTiutByXZxxPbHASTzh4n8+DC2shYhFJF/tDjTWXDPdpbDr2zVtuurqRHiaQE7Ks6dI+X8wByocDohjcWQLm70KyZUfI1g/R//WXkXROg6LYfivyQ8QLCWvj/MlVbHcbJ9JPJRFinzFgzxjMDPGCcC8QrwszREmlz0aj30VWaiGevwMUJ6BSFd0Xj2Bw+gVQVArzCmvp0BgaK46QtEqyI6Ysm5eIIExgJYsxgyDaY60geQg5SxoKnARCWiSHbLWNaNvn4KoNwA+Nh50/HcBkqQz2SkYlpowrIKoIUSDKmUx7Mh8zS40yRagBSTUFoQDEuGSt4JxXCoCtJTxcgZ++ApXrP2zBEZXQOfYU3KvPI55ZZwGs0qaGvA1KL+MiQ2MyoxZreVRt4UprgS34SLBaiUBsLYX+Zd0ekvfuACUVIO1BHKPz7K9QTaKQrbZs1HuMOLBWiZFIYmifFAC/PriaRCi5CiMYhvoC91YgG+ZRum67BUdSRe/lo5BTzyFpToGHaQiq5bago0rkRFQQGswrABYjKnsNkAe2fodsLXPSZ2xc0Hnp0KN682dBLoZkfcPbPvIoytCya/ZqRIX+51f1g1EFFAKYEBtTNXmv0pJCFQLZ7Le8BapQ374Ad80tKF25FTJog8qT6J9+Adnx36NWb0Cy1NTyxgCCPYtZjaoLiNlnFkiykbUqoJBx0H5og5XFD5H5CPV37QzOZ5kCS0/8BEn7LCRugdmbHIuBQ3DNeEREzZ+t6rGx0jSuNVcCKZA8oK6u99qPKEbWWUFp+xeQvPn68Cwuw6++ZjZXveFWUByDBl34f/9lTbYjAFbysFaoelBejCw8FLNwDegAjsblJ9M+GXqmBN3TL2HxZ7vDTsceGz52D2Y/vw/IBgao/eQjSP/+JJL6NIzg40rkm1XOAeNWphzIvBHCAEQKVtSU851Qc8sBqPyI4P96yCrmex1EV21F0pwDekvmDVl3GYtPLKDmksAFnwcvKEKVpRWw/TFTAJqJmoInK0DINhiOlkg9YCwPECqTDVSjGOlqCY3bvwhXqkJWu6DqNJYPPwqcPwk3M5tLUU1HzP2s9zkXzOL1GUfaAvUBRZX33tkpIUjB2B/aYhh04yUCdy4Ab3knStfeBPSWQUkJ3F9G++n9qFYmTAVj788DG2dGQHRvyE0p5ozBkQ8V4AikHBiV3QUlcHfVJpOeDRxh2OmiuXOnbUDSb4PqLSz+dgH++B/hZtYjXepY5Vy5WjAiO7kEYBLazt4qkJPCKpDZtj/KnthBOENy1Y1AqWK90wNHZXoTqu/YBmjwKAKGXQzPnET9hg8gKlcQ6fY97CN96TkDPPaAkR/Yre6MhFjRqHNx5sbZ2yTKpdfrguNJbNj1I1B+wjGlDHp5VgSkA8x9+v7xGUIowtnv3QvudRBXJ8bOaF5Q8AH9GltrDFju1dpn9QJlfSZwcQXtp/ajn3pc8aUfAINu2CMMbO4Vehn2gmLKNfzzu/ciPXIA9Zn14MEwlN58IHBh5E/6cVbVomuODiBZBvYpOB2i1lyP3uH9OPXD+0G1Rlgs76mN0V5fm7J39N1as2Vz1WnNbe1wotmHqRpTY4cTdeEkfdFQturEYR+NZgvtx3+Mfz28F1Sfzv0+17fab33anuk7jeaMzQknKXn9+oXvsd3YZrR2mL30Y532GaaaM1jc/21E1Qls+sxXIMvnbRJNtfDKL75lz5qt9ZBUF7v8x6qtMQWIM0bXtl0tTw6giEOKIMCYarZwbuEbcNU65u64x56deewhnFt4AM11LUD3lcsFLt4w6YmtG/eYjk1FeJ+6ph31Lwqo9hyNIYVDicN0cxZnHvoqSjNz9qveN5vrTT3BdEYLhRaOkhoBMKMF0GU6Rj/90NwH39OQx9mzEz1rjWcH/frOcs5elWZeP/X9wQC9fMEqdB8qB4kWgqusXW0qbOuF/HUjdJHjZ9ruo/p2cvTjGxeurqY7X+1z6oiScP4USJyg/u5PwtUaAUSxN9o2r36vB5UkV0WhfM6BOytoP/MYKEvD+5a9pG+quOREL/n5lt+cvjt+FODvnOCv7b5Grt2YpFvPD9QiiPWw4csTVP3ELkTTs5dktwZizKpLu+0i+KVzWPzDQbjeiiBKlENutkzJK4PSnzWmxqY9gNsLuFs3z75172Z+oBWld5ZIyvofWqoMzQQZxf+HVm8gH1KJZZiICKXYwRFhKDR4zScHdr9IXz904vypPUZFADkIZVvl+zfPvn9Lw982QXKdI0ywCOnR6XIS/V8f50gDq8C6XaHnj67g4K4j//kdgP4ewO8F+L/gLZ9bK0wjSgAAAABJRU5ErkJggg==";
  let enabled = true;
  const isShare = () => /^\/(s|share)(\/|$)/.test(location.pathname);
  const SHARE_TIP = "下载选中的文件。下载整份他人分享时，可在闪电中确认保存位置。";
  const entryTip = () => isShare() ? SHARE_TIP : "将所选文件交给闪电下载。文件相同；闪电便于统一管理，支持批量、续传和多连接，部分场景可能更快。实际速度取决于网络和网盘策略。";
  const shareCodeInput = 'input#accessCode, input[placeholder*="提取码"], input[aria-label*="提取码"]';
  function rememberShareCode(scope, input) {
    if (!isShare()) return;
    const field = input?.matches?.(shareCodeInput) ? input : scope.querySelector(shareCodeInput);
    const code = field?.value?.trim();
    if (/^[a-z0-9]{4}$/i.test(code || "") && field.getClientRects().length) {
      chrome.runtime.sendMessage({ type: "baiduShareCodeEntered", code }).catch(() => {});
    }
  }
  document.addEventListener("submit", event => {
    if (event.isTrusted) rememberShareCode(event.target);
  }, true);
  document.addEventListener("keydown", event => {
    if (event.isTrusted && event.key === "Enter" && event.target.matches?.(shareCodeInput)) {
      rememberShareCode(document, event.target);
    }
  }, true);
  document.addEventListener("click", event => {
    if (!event.isTrusted) return;
    const button = event.target.closest?.("button, a, [role=button]");
    if (!button || !/^(?:提取文件|提取|确认提取)$/.test(button.textContent.trim())) return;
    rememberShareCode(button.closest("form") || document);
  }, true);
  let clearFailure=()=>{};
  function showFailure(message,options) {
    clearFailure();
    const button=document.getElementById(ID);
    if(window.ShFeedback)clearFailure=window.ShFeedback.show(button,message,options);
    else if(button){button.querySelector("span").textContent="请重新加载扩展";button.title=message;}
  }

  function build() {
    const b = document.createElement("button");
    b.id = ID;
    b.type = "button";
    b.title = entryTip();
    b.innerHTML = `<img src="${LOGO}" alt="" style="width:18px;height:18px;border-radius:4px;display:block"><span style="display:block;line-height:18px"></span>`;
    const label = b.querySelector("span");
    label.textContent = LABEL;
    b.style.cssText = "display:inline-flex;align-items:center;gap:6px;align-self:center;margin:0 12px 0 0;height:36px;padding:0 16px 0 12px;border:0;border-radius:18px;background:rgba(128,128,128,.18);color:inherit;font-size:14px;font-weight:600;line-height:18px;box-sizing:border-box;white-space:nowrap;cursor:pointer;flex:none;transition:background .12s;";
    b.addEventListener("mouseenter", () => { b.style.background = "rgba(128,128,128,.32)"; });
    b.addEventListener("mouseleave", () => { b.style.background = "rgba(128,128,128,.18)"; });
    b.addEventListener("click", async (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (!e.isTrusted) return;
      const share = isShare();
      clearFailure();
      b.disabled = true;
      label.textContent = share ? "正在交给闪电…" : "正在获取…";
      let text = "交接失败", detail = "", action;
      try {
        const r = await chrome.runtime.sendMessage({ type: "baiduSelected" });
        if (r?.selectionHint) {
          label.textContent = LABEL;
          b.title = entryTip();
          b.disabled = false;
          showFailure(r.error, { brief: true, ms: 4500 });
          return;
        }
        if (r?.ok) text = r.needCode ? "请在闪电填提取码" : r.pending ? "请在闪电确认" : share ? "已交给闪电" : `已交给闪电（${r.sent}）`;
        else if (r?.noKey) {
          text = "需要 AppKey";
          detail = "百度直下需要你自己的 AppKey（免费）。到百度网盘开放平台创建应用后，在扩展设置中填写 AppKey；也可以使用 LinkSwift 的 Aria2 下载助手连接闪电。";
          action = {label:"打开开放平台",run:()=>window.open("https://pan.baidu.com/union/console","_blank","noopener")};
        } else if (r?.needAuth) text = "看弹窗提示";
        else { detail = r?.error || "获取失败，请刷新网盘页面后重试。"; text = "获取失败"; }
      } catch (_) {
        text = "请刷新页面后重试"; // 扩展刚更新过
      }
      label.textContent = text;
      b.title = detail || text;
      if (detail) showFailure(detail, { action, brief: !action, ms: 4500 });
      setTimeout(() => {
        label.textContent = LABEL;
        b.title = entryTip();
        b.disabled = false;
      }, 3000);
    });
    return b;
  }

  function mount() {
    const old = document.getElementById(ID);
    if (!enabled) return old?.remove();
    const share = isShare();
    const controls = share ? [...document.querySelectorAll('a, button, [role="button"]')] : [];
    const find = labels => controls.find(el =>
      el.id !== ID && el.getClientRects().length &&
      labels.includes((el.innerText || el.textContent || "").trim()));
    const anchor = find(["下载"]);
    const shareBar = find(["电脑端看", "电脑端查看", "取消分享"])?.parentElement ||
      (share && document.querySelector(".share-list-toolbar, .module-share-top-bar .x-button-box, .module-share-bottom-bar .x-button-box"));
    const bar = anchor?.parentElement || shareBar || document.querySelector(".wp-s-agile-tool-bar__header");
    if (!bar) return old?.remove();
    if (old?.parentElement === bar && (!anchor || anchor.previousElementSibling === old)) {
      const label = old.querySelector("span");
      if (!old.disabled && label.textContent !== LABEL) { label.textContent = LABEL; old.title = entryTip(); }
      return;
    }
    old?.remove();
    const b = build();
    if (anchor) {
      const cs = getComputedStyle(anchor), height = Math.max(28, Math.min(56, anchor.getBoundingClientRect().height || 36));
      b.style.height = `${height}px`;b.style.borderRadius = cs.borderRadius;
      b.style.background = cs.background;b.style.color = cs.color;b.style.border = cs.border;
      b.style.setProperty("width", "max-content", "important");b.style.setProperty("min-width", "max-content", "important");
      b.style.setProperty("align-items", "center", "important");b.style.setProperty("justify-content", "center", "important");
      b.style.setProperty("padding-top", "0", "important");b.style.setProperty("padding-bottom", "0", "important");
      b.title = SHARE_TIP;
      anchor.before(b);
    } else bar.prepend(b);
  }

  // 只响应应用生成的短期换链令牌，文件清单由本机网关核对。
  try {
    const page=new URL(location.href),token=page.searchParams.get("shandian-refresh");
    if(/^[a-f0-9]{32}$/.test(token || ""))chrome.runtime.sendMessage({type:"cloudRefreshBegin",token}).then(reply=>{
      if(reply?.ok){page.searchParams.delete("shandian-refresh");history.replaceState(null,"",page.href);}
    }).catch(()=>{});
  } catch {}
  chrome.storage.local.get("baiduButton").then(({ baiduButton = true }) => {
    enabled = baiduButton;
    mount();
  });
  chrome.storage.onChanged.addListener((c) => {
    if (c.baiduButton) {
      enabled = c.baiduButton.newValue !== false;
      mount();
    }
  });
  // 网盘是单页应用，工具栏会反复重建
  new MutationObserver(mount).observe(document.documentElement, { childList: true, subtree: true });
})();
