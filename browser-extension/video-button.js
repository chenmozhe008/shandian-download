// 视频上悬停出现「下载」按钮（照迅雷）：只在支持的视频站、只对足够大的视频显示，点了走工具栏面板同一条「下载这个视频」。
// 按钮放在独立的 Shadow DOM 里叠在页面上方，不改页面自己的元素和脚本（抖音会检查脚本被改动，见 douyin-hook.js）。
// 不引用 i18n.js：它加载时会翻译整页文字，只能给扩展自己的面板用。
(() => {
  const zh = (chrome.i18n?.getUILanguage?.() || "zh").toLowerCase().startsWith("zh");
  const words = {
    idle: zh ? "下载" : "Download",
    busy: zh ? "正在交给闪电…" : "Sending…",
    done: zh ? "已交给闪电全能下载" : "Sent to Shandian",
    fail: zh ? "未能添加，可点工具栏图标重试" : "Couldn't add. Retry from the toolbar icon",
  };
  // 信息流里的小预览不出按钮：点了下的是整页，对不上预览里的那条
  const MIN_WIDTH = 400, MIN_HEIGHT = 225;
  const host = document.createElement("div");
  host.style.cssText = "all: initial; position: fixed; z-index: 2147483647; top: 0; left: 0; display: none; transform: translateX(-100%);";
  const root = host.attachShadow({ mode: "closed" });
  root.innerHTML = `<style>
    button { display: flex; align-items: center; gap: 5px; padding: 6px 12px; border: 0; border-radius: 8px; cursor: pointer;
      font: 600 13px "PingFang SC", -apple-system, sans-serif; color: #fff; background: rgba(224, 153, 47, .95);
      box-shadow: 0 2px 10px rgba(0, 0, 0, .3); white-space: nowrap; }
    button:hover { background: rgb(210, 135, 30); }
    button:disabled { cursor: default; opacity: .9; }
  </style><button type="button"></button>`;
  const button = root.querySelector("button");
  let video = null, busy = false, hideTimer = 0, lastMove = 0;

  const idle = () => { button.textContent = "⬇ " + words.idle; button.disabled = false; };
  idle();

  function largeVideoAt(x, y) {
    for (const v of document.querySelectorAll("video")) {
      const r = v.getBoundingClientRect();
      if (r.width >= MIN_WIDTH && r.height >= MIN_HEIGHT && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) return v;
    }
    return null;
  }

  function place() {
    if (!video?.isConnected) return hide(true);
    const r = video.getBoundingClientRect();
    if (r.width < MIN_WIDTH || r.height < MIN_HEIGHT || r.bottom < 40 || r.top > innerHeight - 40) return hide(true);
    host.style.top = Math.max(8, r.top + 12) + "px";
    host.style.left = Math.min(innerWidth - 8, r.right - 12) + "px";
    if (!host.isConnected) document.documentElement.appendChild(host);
    host.style.display = "block";
  }

  function hide(now) {
    if (busy) return;
    clearTimeout(hideTimer);
    const go = () => { host.style.display = "none"; video = null; };
    if (now) go(); else hideTimer = setTimeout(go, 500);
  }

  document.addEventListener("mousemove", (e) => {
    if (e.composedPath().includes(host)) { clearTimeout(hideTimer); return; }
    const now = Date.now();
    if (now - lastMove < 120) return;
    lastMove = now;
    const found = largeVideoAt(e.clientX, e.clientY);
    if (found) { clearTimeout(hideTimer); video = found; place(); } else if (video) hide(false);
  }, { capture: true, passive: true });
  addEventListener("scroll", () => { if (video) place(); }, { capture: true, passive: true });
  addEventListener("resize", () => { if (video) place(); }, { passive: true });

  button.addEventListener("click", async (e) => {
    e.preventDefault(); e.stopPropagation();
    if (busy) return;
    busy = true; button.disabled = true; button.textContent = words.busy;
    let ok = false;
    try { ok = (await chrome.runtime.sendMessage({ type: "downloadPage", requireVideo: false }))?.ok === true; } catch (_) {}
    button.textContent = ok ? "✓ " + words.done : words.fail;
    setTimeout(() => { busy = false; idle(); if (!video) hide(true); }, ok ? 2500 : 4000);
  });
})();
