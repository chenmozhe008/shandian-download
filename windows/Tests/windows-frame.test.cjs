const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
(async()=>{
for(const root of ['.']) {
 const source=fs.readFileSync(root+'/windows/ui/window-frame.js','utf8');
 const theme=fs.readFileSync(root+'/windows/src-tauri/src/theme.rs','utf8').split('pub fn frame_init()')[1];
 const init=theme.match(/format!\("([^\"]+)"/)[1];
 const head=fs.readFileSync(root+'/windows/ui/index.html','utf8').match(/<script>(.*?)<\/script>/s)[1];
 for(const frame of ['square','rounded']) {
  const handlers={},winHandlers={},calls=[],style={};let maximized=false;
  const win={isResizable:async()=>true,isMaximized:async()=>maximized,onResized:fn=>winHandlers.resize=fn,startDragging:async()=>calls.push('drag'),startResizeDragging:async d=>calls.push(d)};
  const document={body:{appendChild(){}},createElement:()=>({dataset:{},setAttribute(){}}),documentElement:null,querySelectorAll:()=>[{removeAttribute:k=>calls.push('remove:'+k)}],addEventListener:(k,fn)=>handlers[k]=fn};
  const context=vm.createContext({document,window:{__TAURI__:{window:{getCurrentWindow:()=>win}},addEventListener:(k,fn)=>winHandlers[k]=fn},URLSearchParams,location:{search:''},innerWidth:1100,innerHeight:720,Math});
  vm.runInContext(init.replace('{}',frame),context);
  assert.equal(context.window.__sdWindowFrame,frame);
  document.documentElement={dataset:{},style};
  vm.runInContext(head,context);
  assert.equal(document.documentElement.dataset.windowFrame,frame);
  vm.runInContext(source,context);
  await new Promise(r=>setImmediate(r));
  if(frame==='rounded'){assert.ok(handlers.pointerdown);assert.equal(calls.length,0);continue;}
  const target={closest:s=>s==='.topbar, .brand'};
  const event=(x,y,extra={})=>({button:0,buttons:1,clientX:x,clientY:y,target,preventDefault(){},stopImmediatePropagation(){},...extra});
  calls.length=0;
  for(const [x,y,d] of [[1,1,'NorthWest'],[500,1,'North'],[1099,1,'NorthEast'],[1,400,'West'],[1099,400,'East'],[1,719,'SouthWest'],[500,719,'South'],[1099,719,'SouthEast']]) {handlers.pointerdown(event(x,y));assert.equal(calls.at(-1),d);}
  calls.length=0;handlers.pointerdown(event(500,30));handlers.pointermove(event(502,30));assert.equal(calls.length,0,'微小移动或双击不能提前进入原生拖动');handlers.pointerup();assert.equal(calls.length,0);
  handlers.pointerdown(event(500,30));handlers.pointermove(event(505,30));assert.deepEqual(calls,['drag']);handlers.pointermove(event(520,30));assert.equal(calls.length,1);
  calls.length=0;handlers.pointerdown(event(500,30,{target:{closest:()=>true}}));handlers.pointermove(event(510,30));assert.equal(calls.length,0,'控件点击不启动拖动');
  maximized=true;winHandlers.resize();await new Promise(r=>setImmediate(r));handlers.pointerdown(event(1,1,{target:{closest:()=>false}}));assert.equal(calls.length,0,'最大化时不启动缩放');
 }
 const permissions=JSON.parse(fs.readFileSync(root+'/windows/src-tauri/capabilities/default.json')).permissions;
 assert.ok(permissions.includes('core:window:allow-start-resize-dragging'));
}
console.log('Win10 窗口回归通过：无 DOM 启动、页面接收标记、八方向缩放、拖动阈值、控件排除、最大化状态及 Win11 路径');
})().catch(e=>{console.error(e);process.exitCode=1});
