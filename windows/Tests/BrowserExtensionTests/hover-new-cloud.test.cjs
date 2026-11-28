const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
(async()=>{
for(const root of ['.']){
  const source=fs.readFileSync(root+'/browser-extension/hover.js','utf8');
  const start=source.indexOf('(() => {')+'(() => {'.length,end=source.indexOf('  const isXhs',start),guard=source.slice(start,end);
  for(const hostname of ['4007594602.share.123pan.cn','www.123pan.com','cloud.189.cn','yun.139.com','pan.xunlei.com','www.guangyapan.com']){
    assert.equal(vm.runInNewContext('(function(){'+guard+'return cloudPage})()',{location:{hostname}}),true,'专用网盘页面只检测播放器音频');
  }
  assert.equal(vm.runInNewContext('(function(){'+guard+'return cloudPage})()',{location:{hostname:'pan.xunlei.com.evil.test'}}),false);
  const place=source.slice(source.indexOf('  function place('),source.indexOf('  function hide() {'));
  const button={style:{}},context=vm.createContext({button,SIZE:38,GAP:4,innerWidth:1200,innerHeight:800,isX:false,isWeChatMedia:false,isXhs:false,isPixabay:true});
  vm.runInContext(place,context);context.place({tagName:'VIDEO',getBoundingClientRect:()=>({left:100,right:600,top:100,bottom:400,width:500,height:300})});
  assert.equal(button.style.left,'554px');assert.equal(button.style.top,'108px');

  const hero = 'https://mp4.vjshi.com/carousel/2025-07-11/home.mp4';
  const clip = 'https://mp4.vjshi.com/2026-10-22/clip.mp4';
  const secondClip = 'https://lmp4.vjshi.com/2026-11-11/second.mp4';
  const video = (src, width, height) => ({
    tagName: 'VIDEO', currentSrc: src,
    closest: () => null,
    getBoundingClientRect: () => ({ width, height }),
  });
  const background = video(hero, 1600, 800);
  const preview = video(clip, 160, 90), secondPreview = video(secondClip, 160, 90);
  const location = new URL('https://www.vjshi.com/?from=test');
  const card = { querySelector: selector => selector === '.video-card__title' ? { textContent: '海边假期' } : null };
  const featuredPage = new URL('/watch/42216026.html?from=index',location);
  const workPage = new URL('/watch/46062590.html?from=index',location);
  preview.closest = selector => selector === '.video-card__item' ? card : null;
  secondPreview.closest = selector => selector === 'a[href*="/watch/"]' ? featuredPage : null;
  let hits = [background];
  const picking = vm.createContext({URL, location, host: null, audioOnly: false, isX: false, isPixabay: false, MIN_W: 200, MIN_H: 120,
    audioHit: () => null, linkUrl: () => null, inlineShown: () => false,
    document: { elementsFromPoint: () => hits, elementFromPoint: () => hits[0], querySelector: () => ({textContent:'作品详情'}) },
  });
  const videoPicking = source.slice(source.indexOf('  function isPixabayVideoBanner('),source.indexOf('\n  }',source.indexOf('  function pick(x, y)'))+4);
  vm.runInContext(source.match(/  const isVjshi =[^\n]+/)[0] + '\n' + videoPicking, picking);
  assert.equal(picking.pick(0, 0), null, '光厂首页背景不显示悬浮下载按钮');
  hits = [preview, background];
  assert.equal(picking.pick(0, 0).url, clip, '较小的素材卡也按自身视频地址下载');
  assert.equal(picking.pick(0, 0).video, undefined, '不触发全页视频选取');
  assert.equal(picking.pick(0, 0).vjshi.title, '海边假期');
  hits = [secondPreview, background];
  assert.equal(picking.pick(0, 0).url, secondClip, '另一张素材卡提交另一个链接');
  assert.equal(picking.pick(0, 0).vjshi.path, '/watch/42216026.html', '精选卡绑定对应作品页');
  secondPreview.closest = selector => selector === '.video-card__item' ? card : selector === 'a[href*="/watch/"]' ? workPage : null;
  const styledWork = picking.pick(0, 0).vjshi;
  assert.equal(styledWork.title, '海边假期');
  assert.equal(styledWork.path, '/watch/46062590.html', '有标题的小卡片也保留详情页');

  const detail = new URL(styledWork.path, location.origin).href;
  const fullPreview = 'https://mp4.vjshi.com/2026-11-11/second.mp4?from=player';
  let player = fullPreview;
  let fetches = 0;
  let acceptsFetch = true;
  let canonical = detail;
  Object.assign(picking, {
    AbortSignal,
    async fetch(href) {
      assert.equal(href, detail);
      fetches++;
      return { ok: acceptsFetch, text: async () => '' };
    },
    DOMParser: class {
      parseFromString() {
        return { querySelector: selector => ({
          getAttribute: () => selector.startsWith('link') ? canonical : player,
        }) };
      }
    },
  });
  const high = await picking.resolveVjshiWork(styledWork, secondClip);
  assert.equal(high.url, fullPreview, '使用详情实际提供的更大预览地址和参数');
  assert.equal(high.path, styledWork.path);
  assert.equal(high.smallPreview, false);
  assert.equal(await picking.resolveVjshiWork(styledWork, clip).then(result => result.url), clip);
  assert.equal(fetches, 1, '主播放器地址直接使用');
  player = 'https://mp4.vjshi.com/2026-11-11/third.mp4';
  assert.equal((await picking.resolveVjshiWork(styledWork, secondClip)).url, secondClip, '不同视频路径保留小预览');
  player = fullPreview;
  canonical = featuredPage.origin + featuredPage.pathname;
  assert.equal((await picking.resolveVjshiWork(styledWork, secondClip)).smallPreview, true, '详情页不匹配时保留小预览');
  acceptsFetch = false;
  assert.equal((await picking.resolveVjshiWork(styledWork, secondClip)).smallPreview, true, '详情页请求失败时保留小预览');

  location.pathname = '/watch/46062590.html';
  hits = [video(clip, 430, 240)];
  assert.equal(picking.pick(0, 0).vjshi.title, '作品详情', '作品页直接使用当前标题');

  picking.isPixabay = true;
  vm.runInContext('const testPixabayPick = (() => { const isVjshi = false; ' + videoPicking + '; return pick; })()', picking);
  location.pathname = '/zh/videos/';
  const underPixabayBanner = selector => selector === '[class*="featuredContentCarousel--"]';
  const featured = { ...background, closest: underPixabayBanner };
  const pixabayClip = { ...video(clip, 430, 240), closest: () => null };
  hits = [featured];
  assert.equal(vm.runInContext('testPixabayPick(0, 0)', picking), null, 'Pixabay 视频首页背景不显示按钮');
  hits = [pixabayClip];
  assert.equal(vm.runInContext('testPixabayPick(0, 0)', picking).el, pixabayClip, '下方视频入口仍可识别');
  location.pathname = '/zh/videos/clip-123/';
  hits = [featured];
  assert.equal(vm.runInContext('testPixabayPick(0, 0)', picking).el, featured, '屏蔽规则仅应用于视频首页');
}
console.log('悬浮入口通过：网盘检测、Pixabay 首页背景、光厂素材命名与详情预览');
})().catch(e=>{console.error(e);process.exitCode=1;});
