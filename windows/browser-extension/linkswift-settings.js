// Open LinkSwift's own settings UI; never read or modify its stored credentials.
async function openLinkSwiftRpcInPage() {
  const opened = () => /Aria2.*设置/.test(document.querySelector(".swal2-title")?.textContent || "");
  if (opened()) return true;
  const rpc = () => document.querySelector('.swal2-popup .listener-open-aria2-setting, .swal2-popup button[action="settings"][type="aria2"]');
  let button = rpc();
  if (!button) {
    const settings = document.querySelector(".pl-button-mode.listener-open-setting, .pl-button .listener-open-setting");
    if (!settings) return false;
    settings.click();
    for (let i=0;i<10 && !button;i++) { await new Promise(resolve=>setTimeout(resolve,100)); button=rpc(); }
  }
  if (!button) return false;
  button.click();
  for (let i=0;i<10;i++) {
    if (opened()) return true;
    await new Promise(resolve=>setTimeout(resolve,100));
  }
  return false;
}

async function openLinkSwiftSettings(nonce, sender) {
  if (!sender.tab || new URL(sender.tab.url).origin !== "http://127.0.0.1:6800" || new URL(sender.tab.url).pathname !== "/linkswift-settings") return {ok:false};
  const ack = await fetch("http://127.0.0.1:6800/extension/linkswift-settings", {method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({nonce})});
  if (!ack.ok || !(await ack.json()).ok) return {ok:false,error:"请求已过期，请回到闪电重新点击「连接设置」。"};
  const hosts=["pan.baidu.com","yun.baidu.com","alipan.com","aliyundrive.com","pan.quark.cn","drive.uc.cn","cloud.189.cn","yun.139.com","caiyun.139.com","pan.xunlei.com","guangyapan.com","123pan.com","123pan.cn","123684.com","123865.com","123952.com","123912.com","123957.com"];
  const tabs=(await chrome.tabs.query({})).filter(tab=>{
    try {const u=new URL(tab.url);return u.protocol==="https:" && hosts.some(host=>u.hostname===host || u.hostname.endsWith("."+host));} catch {return false;}
  }).sort((a,b)=>(Number(b.active)-Number(a.active)) || ((b.lastAccessed || 0)-(a.lastAccessed || 0)));
  for (const tab of tabs) {
    try {
      const [result]=await chrome.scripting.executeScript({target:{tabId:tab.id},world:"MAIN",func:openLinkSwiftRpcInPage});
      if (!result?.result) continue;
      await chrome.tabs.update(tab.id,{active:true});
      if (tab.windowId != null && chrome.windows?.update) await chrome.windows.update(tab.windowId,{focused:true});
      return {ok:true};
    } catch (_) {}
  }
  return {ok:false,error:"请先打开已安装 LinkSwift 的网盘页面，再回到闪电点击「连接设置」。该设置位于网盘页的「下载助手 → 助手设置 → Aria2 服务器 → 配置」。"};
}
chrome.runtime.onMessage.addListener((message,sender,reply)=>{
  if (message.type!=="openLinkSwiftSettings") return;
  openLinkSwiftSettings(message.nonce,sender).then(reply,()=>reply({ok:false,error:"未能打开 LinkSwift 连接设置，请刷新网盘页面后重试。"}));
  return true;
});
