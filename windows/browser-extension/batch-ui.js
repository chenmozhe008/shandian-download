// The list remains usable while Pixabay resolves the selected materials.
(async function () {
  const [tab] = await chrome.tabs.query({active:true,currentWindow:true});
  const $ = id => document.getElementById(id);
  const batch=$('batch'), actions=$('batch-actions'), results=$('results');
  const selectAll=$('select-all'), download=$('download-selected'), hint=$('hint');
  const progress=$('site-batch-progress');
  const controls=$('site-batch-controls'), verify=$('site-batch-verify'), stop=$('site-batch-cancel');
  const formatRow=$('site-batch-format-row'), formatSelect=$('site-batch-format');
  let url;
  try { url=new URL(tab?.url || ''); } catch (_) {}
  if(!url || !['http:','https:'].includes(url.protocol) || url.hostname==='mypikpak.com'){
    batch.style.display='none';return;
  }
  const host=url.hostname.replace(/^www\./,'');
  if(host==='aigei.com'){
    batch.hidden=true;
    formatRow.hidden=true;actions.hidden=true;
    return;
  }
  formatRow.hidden=true;

  const isPixabay=host==='pixabay.com';
  const batchLabel=isPixabay?'查找本页素材':host==='docs.qingque.cn'?'查找文档附件':'查找本页下载链接';
  batch.textContent=batchLabel;
  let ownerTabId=tab.id, batchState=null, checkedRows=[], unchecked=new Set();
  let pending=false, scanning=false, sending=false;
  const selectedRows=()=>checkedRows.filter(row=>!row.check.disabled && row.check.checked);
  function updateButtons(){
    const eligible=checkedRows.filter(row=>!row.check.disabled).length;
    const selected=selectedRows().length;
    selectAll.disabled=!eligible || pending || sending;
    selectAll.checked=eligible>0 && selected===eligible;
    selectAll.indeterminate=selected>0 && selected<eligible;
    download.disabled=pending || sending || !!batchState?.submitting || !selected;
    download.textContent=pending?'正在准备…':sending || batchState?.submitting?'正在交给闪电…':`下载所选（${selected}）`;
    const label=$('site-batch-select-label');
    if(label)label.textContent=batchState?'全选已解析':'全选';
    batch.disabled=scanning || pending || sending || !!batchState?.running;
  }
  function drawRows(items,known=new Set()){
    results.replaceChildren();
    checkedRows=items.map(item=>{
      const row=document.createElement('label');row.className='batch-row';
      const check=document.createElement('input');check.type='checkbox';check.value=item.url;
      const name=document.createElement('span');name.className='batch-name';
      const title=document.createElement('span');title.className='name';title.textContent=item.title;title.title=item.title;
      const reason=document.createElement('small');reason.className='batch-reason';
      const kind=document.createElement('small');kind.textContent=ShSources.label(item.url,tab.url) || item.kind || '';
      const detail=item.formats?.[formatSelect.value] || item;
      const state=detail.blocked ? detail.blockedReason || '需原站授权' : known.has(item.url)?'已添加':'';
      reason.textContent=state;reason.hidden=!state;
      check.disabled=!!state;check.checked=!check.disabled;
      name.append(title,reason);row.append(check,name,kind);results.append(row);
      check.onchange=()=>{
        if(batchState){if(check.checked)unchecked.delete(item.url);else unchecked.add(item.url);saveSelection();}
        updateButtons();
      };
      return {check,item,reason,kind,known:known.has(item.url)};
    });
    actions.hidden=!checkedRows.length;
  }
  formatSelect.onchange=()=>{
    if(batchState)return;
    for(const {check,item,reason,known} of checkedRows){
      const detail=item.formats?.[formatSelect.value] || item, wasDisabled=check.disabled;
      const status=detail.blocked ? detail.blockedReason || '需原站授权' : known?'已添加':'';
      check.disabled=!!status;
      if(check.disabled)check.checked=false;else if(wasDisabled)check.checked=true;
      reason.textContent=status;reason.hidden=!status;
    }
    updateButtons();
  };
  function saveSelection(){
    chrome.runtime.sendMessage({type:'setSiteBatchSelection',tabId:ownerTabId,batchId:batchState.id,unchecked:[...unchecked]}).catch(()=>{});
  }
  function showBatch(state){
    const newBatch=batchState?.id!==state.id;
    batchState=state;
    if(newBatch){
      unchecked=new Set(state.unchecked || []);
      document.body.classList.add('batch-mode');
      drawRows(state.items || []);
    }
    const ready=new Set(state.ready), delivered=new Set(state.delivered);
    for(const {check,item,reason,kind} of checkedRows){
      const sent=delivered.has(item.url), resolved=ready.has(item.url);
      check.disabled=sent || !resolved || !!state.browserDownload || !!state.submitting;
      check.checked=!sent && !unchecked.has(item.url) && (resolved || state.running);
      kind.textContent=sent?'已交给闪电':resolved && !state.browserDownload?'已解析':state.running?'等待解析':'未解析';
      reason.hidden=item.url!==state.failedKey;
      reason.textContent=reason.hidden?'':state.lastError;
    }
    if(progress){progress.hidden=!state.running;progress.max=state.total;progress.value=state.done+(state.failed || 0);}
    if(controls){
      controls.hidden=!state.running || !!state.submitting;
      verify.hidden=!state.canView;verify.disabled=false;stop.disabled=false;
    }
    const available=state.browserDownload?0:state.done-delivered.size;
    const count=`已解析 ${state.done} / ${state.total} 项`;
    const sent=delivered.size?` · 已交给闪电 ${delivered.size} 项`:'';
    const last=state.lastError?` · ${state.lastError}`:'';
    hint.textContent=sending || state.submitting?'正在交给闪电…':state.running?count+sent
      : available>0?`${count}${sent} · 还有 ${available} 项可下载${last}`:`${count}${sent}${last}`;
    batch.textContent=batchLabel;
    updateButtons();
  }

  selectAll.onchange=()=>{
    for(const {check,item} of checkedRows){
      if(check.disabled)continue;
      check.checked=selectAll.checked;
      if(batchState){if(check.checked)unchecked.delete(item.url);else unchecked.add(item.url);}
    }
    if(batchState)saveSelection();
    updateButtons();
  };
  if(controls)for(const [button,type] of [[verify,'focusSiteFlow'],[stop,'cancelSiteBatch']]){
    button.onclick=async()=>{
      if(!batchState?.id)return;
      button.disabled=true;
      try{
        const result=await chrome.runtime.sendMessage({type,tabId:ownerTabId,batchId:batchState.id});
        if(!result?.ok)throw Error(result?.error || '这次批量已结束');
      }catch(error){hint.textContent=error.message;}
      finally{button.disabled=false;}
    };
  }
  if(isPixabay){
    chrome.runtime.onMessage?.addListener(message=>{
      if(message.type==='siteBatchProgressUI' && message.tabId===ownerTabId && message.batch)showBatch(message.batch);
    });
    try{
      const current=await chrome.runtime.sendMessage({type:'getSiteBatchStatus',tabId:tab.id});
      ownerTabId=current?.tabId ?? tab.id;
      if(current?.batch && !batchState)showBatch(current.batch);
    }catch(_){}
    chrome.tabs.connect?.(ownerTabId,{name:'pixabayBatchPopup',frameId:0});
  }
  batch.onclick=async()=>{
    if(scanning || pending || sending || batchState?.running)return;
    scanning=true;batchState=null;
    document.body.classList.add('batch-mode');
    batch.textContent='正在抓取…';results.replaceChildren();actions.hidden=true;
    updateButtons();
    try{
      const response=await chrome.runtime.sendMessage({type:'scanLinks',tabId:ownerTabId});
      if(response?.error)throw Error(typeof response.error === "string" ? response.error : "无法读取当前页面，请刷新后重试。");
      const items=response?.items || [];
      drawRows(items,new Set(response?.known || []));
      const unavailable = response?.unavailable || 0;
      hint.textContent = items.length ? (unavailable ? `找到 ${items.length} 个附件，${unavailable} 个暂无原文件地址。`
        : `找到 ${items.length} 个项目。`) : "当前网页没有找到可下载的链接。";
    }catch(error){hint.textContent=error.message;}
    finally{scanning=false;batch.textContent=batchLabel;updateButtons();}
  };
  download.onclick=async()=>{
    const items=selectedRows().map(row=>row.item);
    if(!items.length || pending || sending)return;
    const keys=items.map(item=>item.url);
    if(batchState){
      const id=batchState.id;
      sending=true;hint.textContent='正在交给闪电…';updateButtons();
      try{
        const result=await chrome.runtime.sendMessage({type:'downloadCollectedSiteBatch',tabId:ownerTabId,batchId:id,keys});
        if(!result?.ok)throw Error(result?.error || '未能交给闪电，请重试');
        sending=false;
        if(batchState?.id===id)showBatch({...batchState,delivered:[...new Set([...batchState.delivered,...result.keys])]});
      }catch(error){hint.textContent=error.message;}
      finally{sending=false;updateButtons();}
      return;
    }
    pending=true;hint.textContent='正在准备…';updateButtons();
    try{
      const resolve=isPixabay && items.some(item=>item.resolve);
      if(resolve && !(await chrome.permissions.request({permissions:['webRequest']}))){
        throw Error('需要允许闪电获取 Pixabay 的原站下载地址。');
      }
      const result=await chrome.runtime.sendMessage({type:'downloadLinks',tabId:ownerTabId,urls:keys,items,format:formatSelect.value});
      if(!result?.ok)throw Error(result?.error || '未能连到闪电，请确认应用已打开。');
      if(!resolve){
        for(const row of checkedRows)if(keys.includes(row.item.url))row.check.checked=false;
        hint.textContent=`已交给闪电 ${items.length} 个链接，请在应用中确认任务。`;
      }else if(!batchState){
        hint.textContent='正在解析所选项目…';
      }
    }catch(error){hint.textContent=error.message;}
    finally{pending=false;updateButtons();}
  };
})();
