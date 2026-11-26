// The native app's verified one-time ticket is the user's association intent.
// Ordinary browsing and popup-created tickets never trigger automatic capture.
(() => {
  if(window.top!==window || window.__shandianAccountLinkPage || !cloudAccountLinkSite(location.href))return;
  window.__shandianAccountLinkPage=true;
  let view=null,context=null,busy=false,request=0,retry=0,started=0,queued=false,dismissed="",finished="",waiting=false;
  const stopRetry=()=>{clearTimeout(retry);retry=0};
  function show(title,message,manual=false) {
    if(dismissed===(context?.ticket || "invalid"))return;
    if(!view)view=window.ShFeedback.topRight({id:"shandian-account-link",onClose(){dismissed=context?.ticket || "invalid";view=null}});
    view.title.textContent=title;view.message.textContent=message;
    view.action.hidden=!manual;
    view.action.textContent=context?.official?"检查授权":`关联到${context?.targetLabel || "本机"}`;
    view.action.onclick=event=>{if(event.isTrusted)associate(true)};
  }
  async function associate(manual=false) {
    if(busy || !context?.autoLink || context.completed || (!manual && finished===context.ticket))return;
    stopRetry();busy=true;
    if(context.official && !waiting)show("授权 PikPak 给 NAS",`请在此页输入短码 ${context.official.code}，然后确认授权。`);
    else if(!context.official && (manual || !waiting))show(`正在关联${context.name}`,`正在验证并关联到${context.targetLabel}…`);
    const ticket=context.ticket;
    try {
      const result=await chrome.runtime.sendMessage(manual?{type:"cloudAccountLinkSubmit",retry:true}:{type:"cloudAccountLinkSubmit"});
      if(context?.ticket!==ticket)return;
      if(result?.ok){finished=ticket;context.completed=true;show("关联完成",context.resumeToken ? "已关联，正在获取新地址并继续原下载。" : result.message || `已关联到${context.targetLabel}，可以关闭此页。`);}
      else if(result?.resumeFallback){finished=ticket;show("正在尝试继续下载",result.message);}
      else if(result?.officialPending && Date.now()<context.official.expiresAt*1000){
        waiting=true;retry=setTimeout(associate,2000);
      } else if(result?.needsLogin && !manual && Date.now()-started<600000){
        waiting=true;
        show(`关联${context.name}`,`请在此网页登录，登录后会自动关联到${context.targetLabel}。`+(context.transferSession?"关联成功后，此次网页登录会退出。":""));
        // One bounded login wait, sharing the broker's in-flight request and
        // verified destination. Navigation/focus reuses this same timer.
        retry=setTimeout(associate,2000);
      } else {waiting=false;finished=ticket;show("关联未完成",result?.error || "请确认闪电全能下载已打开，再试一次。",!result?.requestExpired);}
    } catch {waiting=false;finished=ticket;show("关联未完成","请确认闪电全能下载已打开，再试一次。",true);}
    finally {busy=false;if(queued){queued=false;refresh()}}
  }
  async function refresh() {
    if(busy){queued=true;return;}
    const revision=++request;
    try {
      const value=await chrome.runtime.sendMessage({type:"cloudAccountLinkContext"});
      if(revision!==request)return;
      if(value?.ok && value.autoLink){
        if(context?.ticket!==value.ticket){stopRetry();started=Date.now();finished="";dismissed="";waiting=false;}
        context=value;
        if(value.completed){
          stopRetry();
          if(finished!==value.ticket){
            finished=value.ticket;
            // The app may have committed while the login page redirected.
            // The worker closes only the original, verified association tab.
            chrome.runtime.sendMessage({type:"cloudAccountLinkClose"}).catch(()=>{});
          }
          if(view)show("关联完成",context.resumeToken ? "正在获取新地址并继续原下载。" : "此次关联已完成，请回软件查看账号状态。");
        }
        else associate();
      } else if(value?.inactive || (value?.ok && !value.autoLink)) {stopRetry();context=null;view?.close();view=null;}
      else if(context || new URL(location.href).searchParams.has("shandian-account-link")){stopRetry();if(!finished)show("关联未完成",value?.error || "请回软件重新发起关联。");}
    } catch {if(context && !finished){stopRetry();show("关联未完成","请确认闪电全能下载已打开，再试一次。",true);}}
  }
  chrome.runtime.onMessage.addListener(message=>{if(message?.type==="cloudAccountLinkContextChanged")refresh();});
  window.addEventListener("focus",refresh);
  window.addEventListener("pagehide",stopRetry);
  refresh();
})();
