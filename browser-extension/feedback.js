// Shared presentation helpers. No requests, credentials or application-state changes.
(() => {
  const scope=typeof window!=="undefined"?window:globalThis;
  if (scope.ShFeedback || typeof document==="undefined") return;
  let recent=null,tracking=false;
  const active=new WeakMap();
  function remember(event) {
    const target=event.target?.closest?.('button,a[href],[role="button"],[role="menuitem"],input,textarea');
    if (target) recent={anchor:target,rect:target.getBoundingClientRect(),at:Date.now()};
  }
  function track() {
    if (tracking) return;tracking=true;
    document.addEventListener("click",remember,true);
    document.addEventListener("keydown",event=>{if(event.key==="Enter")remember(event)},true);
  }
  function context(anchor) {
    if (anchor) return {anchor,rect:anchor.getBoundingClientRect()};
    return recent && Date.now()-recent.at<30000 ? recent : null;
  }
  function position(el,ctx) {
    if (!ctx) return;
    const current=ctx.anchor?.isConnected?ctx.anchor.getBoundingClientRect():null;
    const rect=current?.width || current?.height ? current : ctx.rect;
    if (!rect) return;
    Object.assign(el.style,{position:"fixed",right:"auto",bottom:"auto",transform:"none",animation:"none",margin:"0",maxWidth:Math.min(420,innerWidth-24)+"px"});
    const size=el.getBoundingClientRect();
    const top=rect.bottom+8+size.height>innerHeight-8 ? rect.top-size.height-8 : rect.bottom+8;
    el.style.left=Math.max(8,Math.min(rect.left,innerWidth-size.width-8))+"px";
    el.style.top=Math.max(8,Math.min(top,innerHeight-size.height-8))+"px";
  }
  function reset(el) {for(const key of ["left","right","top","bottom","transform","animation","margin","maxWidth"])el.style[key]="";}
  function inline(anchor,message,ms=2500) {
    const text=String(message);
    if (!anchor?.isConnected || anchor.tagName?.toUpperCase()!=="BUTTON" || /circle|switch/.test(anchor.className || "") || text.length>12 || /[\n\r]/.test(text)) return null;
    const direct=[...(anchor.childNodes || [])].find(node=>node.nodeType===3&&node.textContent.trim());
    const leaf=[...(anchor.querySelectorAll?.("span") || [])].find(node=>!node.children?.length && node.textContent.trim() && !/icon|badge|count/.test(node.className || ""));
    const target=direct || leaf;if(!target)return null;
    const width=anchor.getBoundingClientRect().width;
    if(anchor.style.width!=="max-content" && width && text.length>Math.max(4,Math.floor((width-20)/14)))return null;
    const previous=active.get(anchor),original=previous?.target===target&&target.textContent===previous.message ? previous.original : target.textContent;
    previous?.close();target.textContent=text;
    let timer;const close=()=>{clearTimeout(timer);if(target.textContent===text)target.textContent=original;if(active.get(anchor)?.close===close)active.delete(anchor)};
    active.set(anchor,{original,target,message:text,close});timer=setTimeout(close,ms);return close;
  }
  function show(anchor,message,options={}) {
    const short=!options.action && !options.brief && inline(anchor,message);if(short)return short;
    const ctx=options.context || context(anchor);
    const id=(anchor?.id || "shandian-feedback")+"-message";
    document.getElementById(id)?.remove();
    const host=document.createElement("div");
    host.id=id;
    host.setAttribute("role",options.brief?"status":"alert");
    host.style.cssText="all:initial;position:fixed;z-index:2147483647;";
    const root=host.attachShadow({mode:"closed"});
    const dark=typeof matchMedia==="function" && matchMedia("(prefers-color-scheme: dark)").matches;
    const bg=dark?"#252320":"#FFFFFF";
    const border=dark?"rgba(255,255,255,0.12)":"rgba(0,0,0,0.08)";
    const fg=dark?"#F3F0EB":"#1D1D1F";
    const sub=dark?"#A8A29E":"#6E6E73";
    const style=document.createElement("style");
    style.textContent=`
      .card{width:320px;max-width:calc(100vw - 24px);box-sizing:border-box;padding:14px 16px;border-radius:12px;background:${bg};color:${fg};border:1px solid ${border};box-shadow:0 10px 32px rgba(0,0,0,${dark?0.45:0.16});font:13px/1.6 -apple-system,"PingFang SC","SF Pro Text",system-ui,sans-serif;}
      .head{display:flex;align-items:center;gap:8px;margin-bottom:6px;}
      .dot{width:8px;height:8px;border-radius:50%;background:#E0992F;flex:none;}
      .title{font-size:13.5px;font-weight:600;color:${fg};flex:1;}
      .x{border:0;background:transparent;color:${sub};font-size:16px;line-height:1;padding:2px 4px;cursor:pointer;border-radius:4px;}
      .x:hover{color:${fg};}
      .msg{color:${sub};white-space:pre-line;word-break:break-word;}
      .foot{display:flex;justify-content:flex-end;gap:8px;margin-top:12px;}
      .btn{border:0;border-radius:7px;padding:6px 14px;font:600 12.5px/1.2 inherit;cursor:pointer;background:#E0992F;color:#fff;}
      .btn:hover{filter:brightness(1.06);}
      .btn.sec{background:${dark?"rgba(255,255,255,0.1)":"rgba(0,0,0,0.06)"};color:${fg};}
    `;
    const card=document.createElement("div");
    card.className="card";
    if(options.brief){
      card.style.padding="8px 12px";
      card.style.width="auto";
      card.textContent=message;
      host.style.pointerEvents="none";
    }else{
      const head=document.createElement("div");head.className="head";
      head.innerHTML=`<span class="dot"></span><span class="title">闪电全能下载</span>`;
      const x=document.createElement("button");x.type="button";x.className="x";x.textContent="×";x.title="关闭";
      head.append(x);
      const msg=document.createElement("div");msg.className="msg";msg.textContent=message;
      const foot=document.createElement("div");foot.className="foot";
      card.append(head,msg,foot);
      x.onclick=()=>dismiss();
      if(options.action){
        const act=document.createElement("button");act.type="button";act.className="btn";act.textContent=options.action.label;
        act.onclick=()=>{dismiss();options.action.run()};
        foot.append(act);
      }
      const ok=document.createElement("button");ok.type="button";ok.className=options.action?"btn sec":"btn";ok.textContent="知道了";
      ok.onclick=()=>dismiss();
      foot.append(ok);
    }
    root.append(style,card);
    let timer;
    const place=()=>position(host,ctx || {rect:{left:8,top:0,bottom:0}});
    const dismiss=()=>{clearTimeout(timer);host.remove();if(anchor?.getAttribute?.("aria-describedby")===id)anchor.removeAttribute("aria-describedby");scope.removeEventListener("scroll",place,true);scope.removeEventListener("resize",place)};
    if(options.brief && options.ms!==0)timer=setTimeout(dismiss,options.ms || 3500);
    document.body.append(host);anchor?.setAttribute("aria-describedby",id);
    scope.addEventListener("scroll",place,true);scope.addEventListener("resize",place);place();return dismiss;
  }

  // The existing page-capture panel is also the home for association feedback.
  // Keep it isolated from site CSS and reuse one instance per operation.
  function topRight({id,title,message,onClose,withProgress=false}={}) {
    const css=(el,values)=>{for(const [key,value] of Object.entries(values))el.style.setProperty(key,value);return el};
    const dark=typeof matchMedia==="function" && matchMedia("(prefers-color-scheme: dark)").matches;
    const [bg,text,sub]=dark?["#2A2723","#EEE","#AAA"]:["#FBF9F6","#222","#777"];
    const host=css(document.createElement("div"),{all:"initial",position:"fixed",right:"20px",top:"20px","z-index":"2147483647","max-width":"calc(100vw - 40px)"});
    if(id)host.id=id;
    const root=host.attachShadow({mode:"closed"});
    const box=css(document.createElement("div"),{width:"360px","max-width":"100%","box-sizing":"border-box",padding:"16px 18px","border-radius":"12px",background:bg,color:text,border:"2px solid #E0992F","box-shadow":"0 12px 36px rgba(0,0,0,.35)",font:'13px/1.45 -apple-system, "PingFang SC", "Microsoft YaHei", sans-serif'});
    const head=css(document.createElement("div"),{display:"flex","align-items":"center",gap:"12px"});
    const heading=css(document.createElement("b"),{flex:"1","font-size":"16px"});heading.textContent=title || "";
    const close=css(document.createElement("button"),{border:"0",background:"transparent",color:sub,cursor:"pointer","font-size":"16px",padding:"0 2px"});
    close.type="button";close.textContent="×";close.title="关闭";close.setAttribute("aria-label","关闭提示");
    const note=css(document.createElement("div"),{color:sub,"font-size":"12px",margin:"4px 0 0","overflow-wrap":"anywhere"});note.textContent=message || "";note.setAttribute("role","status");note.setAttribute("aria-live","polite");
    const action=css(document.createElement("button"),{width:"100%",padding:"7px 0",border:"0","border-radius":"8px",background:"#E0992F",color:"#fff","font-weight":"600",cursor:"pointer",font:"inherit","margin-top":"10px"});action.type="button";action.hidden=true;
    head.append(heading,close);box.append(head,note);
    let progress;
    if(withProgress){
      css(box,{width:"290px",padding:"12px 14px",border:"1px solid "+(dark?"#ffffff24":"#00000022")});
      css(heading,{"font-size":"14px"});
      css(host,{opacity:"0",visibility:"hidden","pointer-events":"none",transition:"opacity 180ms ease, visibility 0s linear 180ms"});
      progress=css(document.createElement("progress"),{width:"100%",height:"6px",display:"block",margin:"8px 0 0",border:"0","border-radius":"3px",overflow:"hidden",appearance:"none"});
      progress.setAttribute("aria-label","解析下载地址");
      const style=document.createElement("style");
      style.textContent=`progress::-webkit-progress-bar { background:${dark?"#3A3631":"#EAE5DD"}; border:0; } progress::-webkit-progress-value { background:#E0992F; border:0; }`;
      root.append(style);
      box.append(progress);
    }
    box.append(action);root.append(box);document.documentElement.append(host);
    close.onclick=()=>{host.remove();onClose?.()};
    if(!withProgress)box.animate?.([{transform:"translateY(-16px)",opacity:0},{transform:"none",opacity:1}],{duration:220,easing:"ease-out"});
    function setVisible(visible){
      const fade=matchMedia("(prefers-reduced-motion: reduce)").matches?0:180;
      host.style.transition=visible?`opacity ${fade}ms ease`:`opacity ${fade}ms ease, visibility 0s linear ${fade}ms`;
      host.style.visibility=visible?"visible":"hidden";
      host.style.pointerEvents=visible?"auto":"none";
      host.style.opacity=visible?"1":"0";
      host.inert=!visible;
    }
    return {host,title:heading,message:note,action,progress,setVisible,close:()=>host.remove()};
  }
  scope.ShFeedback={track,context,position,reset,inline,show,topRight};
})();
