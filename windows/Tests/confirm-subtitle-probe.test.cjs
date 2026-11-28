const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
(async()=>{
 for(const file of ['ui/app.js']){
  const source=fs.readFileSync(file,'utf8'),start=source.indexOf('function renderCfSubtitleLanguageNote('),end=source.indexOf('/// 关掉确认框。',start);
  const elements=new Map(),states=[];let resolveProbe,modelRequest=()=>Promise.resolve({available:false,local:true,id:'ollama'});
  const $=id=>{if(!elements.has(id))elements.set(id,{hidden:id!=='confirm',setAttribute(){},addEventListener(){}});return elements.get(id)};
  const ctx=vm.createContext({cf:null,setCfDefaultName(){},$,normSubLangs:v=>v||'zh',document:{querySelector:()=>null},
    setProbeState:(el,text)=>{el.textContent=text;el.hidden=false},setQualityButtons(){},
    renderCfVideo:()=>{states.push(ctx.cf.subtitleAvailability);ctx.renderCfSubtitleLanguageNote(ctx.cf);},
    invoke:command=>command==='probe_video'?new Promise(resolve=>resolveProbe=resolve):modelRequest()});
  vm.runInContext(source.slice(start,end),ctx);
  for(const [result,expected] of [[{subs:true},'available'],[{subs:false},'none'],[null,'failed'],[{},'failed']]){
   ctx.cf={url:'selected-video',supportsSubs:true,nameLoading:true,video:{subtitles:true}};
   const pending=ctx.probeCfSubs();assert.equal(ctx.cf.subtitleAvailability,'checking');assert.equal(states.at(-1),'checking');
   resolveProbe(result);await pending;assert.equal(ctx.cf.nameLoading,false);assert.equal(ctx.cf.subtitleAvailability,expected);assert.equal(ctx.cf.supportsSubs,true);assert.equal(ctx.cf.video.subtitles,true,'查询结果不能改写用户保存的字幕偏好');
  }
  ctx.cf={url:'old',supportsSubs:true,video:{subtitles:true}};const pending=ctx.probeCfSubs();const oldResolve=resolveProbe;
  ctx.cf={url:'new',supportsSubs:true,subtitleAvailability:'checking',video:{subtitles:true}};oldResolve({subs:true,title:'旧资源'});await pending;
  assert.equal(ctx.cf.subtitleAvailability,'checking','旧资源迟到的结果不能使新资源出现字幕选项');

  let finishModels;
  modelRequest=()=>new Promise(resolve=>finishModels=resolve);
  const englishOnly={url:'english',supportsSubs:true,video:{subtitles:true,subLangs:'zh+en'}};
  ctx.cf=englishOnly;
  const probing=ctx.probeCfSubs();
  resolveProbe({subs:true,sub_zh:false,sub_en:true});
  await probing;
  const text=$('cf-sub-language-text'),retry=$('cf-sub-model-retry');
  assert.match(text.textContent,/正在检测字幕服务/);
  assert.equal(retry.hidden,true);
  finishModels({available:false,local:true,id:'ollama'});
  await new Promise(resolve=>setImmediate(resolve));
  assert.match(text.textContent,/保留英文/);
  assert.match(text.textContent,/配置第三方 API/);
  assert.equal(retry.hidden,false);
  modelRequest=()=>Promise.resolve({available:true,local:true,id:'ollama'});
  await ctx.checkCfPolish(englishOnly);
  assert.match(text.textContent,/本机 Ollama 翻译/);
  assert.equal(retry.hidden,true);
  modelRequest=()=>Promise.resolve({available:true,local:false,id:'deepseek'});
  await ctx.checkCfPolish(englishOnly);
  assert.match(text.textContent,/勾选「AI 翻译」/);
  englishOnly.polish=true;
  ctx.renderCfVideo();
  assert.match(text.textContent,/会将字幕文字发给所选服务/);
  englishOnly.video.subLangs='en';
  ctx.renderCfVideo();
  assert.equal($('cf-sub-language-note').hidden,true,'切回英文时收起翻译提示');
  englishOnly.video.subLangs='zh+en';
  englishOnly.subtitleLanguages={zh:true,en:false};
  ctx.renderCfVideo();
  assert.match(text.textContent,/缺少英文/);
  englishOnly.video.audioOnly=true;
  ctx.renderCfVideo();
  assert.equal($('cf-sub-language-note').hidden,true,'仅音频时收起字幕提示');
 }
 console.log('两版字幕查询通过：等待、确认有字幕、无字幕、失败、无效结果、迟到结果隔离及偏好保留');
})().catch(error=>{console.error(error);process.exitCode=1});
