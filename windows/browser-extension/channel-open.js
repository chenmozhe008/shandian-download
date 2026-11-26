// App-origin loopback landing page only. Keep the nonce out of history and never log it.
(() => {
  if (window.top !== window || location.origin !== "http://127.0.0.1:6800" || !["/channel-open","/linkswift-settings"].includes(location.pathname)) return;
  if (globalThis.__shandianChannelOpened) return;
  globalThis.__shandianChannelOpened = true;
  const params = new URLSearchParams(location.hash.slice(1));
  const autoToken = params.get("wechat-auto"), share = params.get("share"), action = params.get("action");
  history.replaceState(null, "", location.pathname);
  const status = text => {
    const update = () => { const el = document.getElementById("channel-status"); if (el) el.textContent = text; };
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", update, {once:true}); else update();
  };
  if (location.pathname === "/linkswift-settings") {
    const nonce=params.get("nonce");
    if (!/^[a-f0-9]{32}$/.test(nonce || "")) {status("请求已过期，请回到闪电重新点击「连接设置」。");return;}
    chrome.runtime.sendMessage({type:"openLinkSwiftSettings",nonce}).then(result=>{
      status(result?.ok ? "已在网盘页面打开 LinkSwift 的 Aria2 RPC 连接设置。" : result?.error || "未能打开设置，请重新加载闪电扩展后重试。");
    }).catch(()=>status("请重新加载闪电扩展，再回到 App 点击「连接设置」。"));
    return;
  }
  if (!/^[a-f0-9]{32}$/.test(autoToken || "")) { status("请求已过期，请重新复制视频号链接。"); return; }
  chrome.runtime.sendMessage({type:"autoChannel", autoToken, share, action}).then(result => {
    status(result?.ok ? "已交给闪电全能下载。" : result?.error || "扩展未能完成交接，请重新复制链接。");
  }).catch(() => status("请重新加载闪电扩展后，再复制视频号链接。"));
})();
