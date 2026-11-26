const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
for(const root of ['.']){
  const source=fs.readFileSync(root+'/windows/browser-extension/hover.js','utf8');
  const start=source.indexOf('(() => {')+'(() => {'.length,end=source.indexOf('  const isXhs',start),guard=source.slice(start,end);
  for(const hostname of ['4007594602.share.123pan.cn','www.123pan.com','cloud.189.cn','yun.139.com','pan.xunlei.com','www.guangyapan.com']){
    assert.equal(vm.runInNewContext('(function(){'+guard+'return cloudPage})()',{location:{hostname}}),true,'专用网盘页面只检测播放器音频');
  }
  assert.equal(vm.runInNewContext('(function(){'+guard+'return cloudPage})()',{location:{hostname:'pan.xunlei.com.evil.test'}}),false);
  const place=source.slice(source.indexOf('  function place('),source.indexOf('  function hide() {'));
  const button={style:{}},context=vm.createContext({button,SIZE:38,GAP:4,innerWidth:1200,innerHeight:800,isX:false,isWeChatMedia:false,isXhs:false,isPixabay:true});
  vm.runInContext(place,context);context.place({tagName:'VIDEO',getBoundingClientRect:()=>({left:100,right:600,top:100,bottom:400,width:500,height:300})});
  assert.equal(button.style.left,'554px');assert.equal(button.style.top,'108px');
}
console.log('悬浮入口通过：新增五家不误挂视频按钮，普通视频默认右上角');
