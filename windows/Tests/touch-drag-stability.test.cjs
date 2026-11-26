const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');

const flush = () => new Promise(resolve => setImmediate(resolve));
async function setup(root, frame, dialog, scale, installer = false, pointerType = 'touch', native = false) {
  const events = {}, frames = [], positions = [], nativeCalls = [];
  let pending = null;
  const target = {
    closest: selector => selector.includes('.topbar') ? target : null,
    setPointerCapture() {}, releasePointerCapture() {},
  };
  const event = (x, y, type = 'pointermove') => ({
    type, button: 0, buttons: 0, pointerType, pointerId: 1, isPrimary: true,
    screenX: x, screenY: y, clientX: 500, clientY: 30, target,
    preventDefault() {}, stopImmediatePropagation() {},
  });
  const win = {
    isResizable: async () => true, isMaximized: async () => false, onResized() {},
    outerPosition: async () => ({ x: 100, y: 200 }), scaleFactor: async () => scale,
    setPosition(position) {
      positions.push(position);
      // 窗口移动后，浏览器在同一触点上再次派发新的事件对象。
      events.pointermove(event(520, 310));
      return pending ? pending.promise : Promise.resolve();
    },
  };
  const ctx = vm.createContext({
    document: {
      documentElement: { dataset: { windowFrame: frame }, style: {} },
      body: { classList: { contains: () => installer }, appendChild() {} }, createElement: () => ({ dataset: {}, setAttribute() {} }),
      querySelectorAll: () => [], addEventListener: (type, fn) => events[type] = fn,
    },
    window: {

      __TAURI__: { core: { invoke: async command => { nativeCalls.push(command); return true; } }, window: { getCurrentWindow: () => win }, dpi: {
        PhysicalPosition: class { constructor(x, y) { Object.assign(this, { x, y }); } },
      } }, addEventListener() {},
    },
    CSS: { supports: () => native },
    requestAnimationFrame: fn => (frames.push(fn), frames.length),
    location: { search: dialog ? '?dialog=1' : '' }, URLSearchParams,
    innerWidth: 1100, innerHeight: 720,
  });
  vm.runInContext(fs.readFileSync(root + '/windows/ui/window-frame.js', 'utf8'), ctx);
  await flush();
  return { events, event, target, frames, positions, nativeCalls, setPending(value) { pending = value; } };
}

(async () => {
  for (const root of ['.']) for (const frame of ['square', 'rounded']) for (const dialog of [false, true]) for (const installer of [false, true]) {
    const h = await setup(root, frame, dialog, 1.5, installer, 'touch', true);
    h.events.pointerdown(h.event(500, 300, 'pointerdown'));
    h.events.pointermove(h.event(550, 330)); await flush();
    assert.deepEqual(h.nativeCalls, [], '浏览器原生拖动不能再发自写拖窗命令');
    assert.equal(h.positions.length, 0, '原生触摸拖窗不能同时逐帧 setPosition');
    assert.equal(h.frames.length, 0, 'Win10 原生拖窗不依赖网页动画帧');
  }
  for (const root of ['.']) for (const frame of ['square', 'rounded']) for (const dialog of [false, true]) for (const scale of [1, 1.5, 2]) for (const installer of [false, true]) for (const pointerType of installer ? ['touch', 'mouse'] : ['touch']) {
    const h = await setup(root, frame, dialog, scale, installer, pointerType);
    h.events.pointerdown(h.event(500, 300, 'pointerdown'));
    await flush();
    let prevented = false, stopped = false;
    h.events.mousedown({ target: h.target, sourceCapabilities: { firesTouchEvents: true }, preventDefault() { prevented = true; }, stopImmediatePropagation() { stopped = true; } });
    assert.ok(prevented && stopped, '触摸合成鼠标事件不能再启动原生拖动');
    for (let i = 1; i <= 20; i++) h.events.pointermove(h.event(500 + i, 300 + i / 2));
    assert.equal(h.frames.length, 1, '一帧只处理最后一个触点');
    h.frames.shift()(); await flush();
    assert.equal(h.positions.length, 1);
    assert.equal(h.positions[0].x, 100 + 20 * scale);
    assert.equal(h.positions[0].y, 200 + 10 * scale);
    assert.equal(h.frames.length, 0, '移窗生成的同坐标事件不能形成更新循环');

    let resolve;
    h.setPending({ promise: new Promise(r => resolve = r) });
    h.events.pointermove(h.event(530, 320));
    h.frames.shift()();
    h.events.pointermove(h.event(540, 330));
    h.events.pointerup(h.event(540, 330, 'pointerup'));
    h.events.pointerdown(h.event(600, 400, 'pointerdown'));
    await flush();
    resolve(); await flush();
    assert.equal(h.frames.length, 0, '上一段手势的异步回调不能重新移动窗口');
    h.setPending(null);
    h.events.pointermove(h.event(610, 410));
    h.events.pointercancel(h.event(610, 410, 'pointercancel'));
    const count = h.positions.length;
    while (h.frames.length) h.frames.shift()();
    await flush();
    assert.equal(h.positions.length, count, '取消后已排队的一帧不能移动窗口');
  }
  console.log('触摸拖动稳定性通过：主窗口/弹窗、Win10/Win11、三档 DPI、事件反馈、按帧合并、连续手势和取消');
})().catch(error => { console.error(error); process.exitCode = 1; });
