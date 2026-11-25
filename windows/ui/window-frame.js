// 无边框窗口：Win10 鼠标兜底，Win10/Win11 共用触摸拖动与缩放。
(() => {
  const frame = window.__sdWindowFrame || document.documentElement.dataset.windowFrame;
  document.documentElement.dataset.windowFrame = frame || "rounded";
  const legacy = frame === "square";
  const win = window.__TAURI__?.window?.getCurrentWindow?.();
  if (!win) return;
  const dialog = new URLSearchParams(location.search).has("dialog");
  const installer = document.body.classList?.contains("installer-window");
  const nativeCaption = typeof CSS !== "undefined" && CSS.supports("-webkit-app-region", "drag");
  if ((legacy || installer) && !dialog) document.querySelectorAll("[data-tauri-drag-region]").forEach(el => el.removeAttribute("data-tauri-drag-region"));
  let resizable = false, maximized = false, drag = null, touchEpoch = 0;
  const handles = [];
  for (const direction of ["North","South","East","West","NorthEast","NorthWest","SouthEast","SouthWest"]) {
    const handle = document.createElement("div");
    handle.className = "window-resize-handle";
    handle.dataset.windowResize = direction;
    handle.setAttribute("aria-hidden", "true");handle.hidden = true;
    document.body.appendChild(handle);handles.push(handle);
  }
  const refresh = async () => {
    [resizable, maximized] = await Promise.all([win.isResizable(), win.isMaximized()]);
    if (installer) resizable = false;
    handles.forEach(el => { el.hidden = !resizable || maximized; });
  };
  const fail = e => { if (typeof toast === "function") toast(`窗口操作失败：${e}`); };
  refresh().catch(fail);
  win.onResized?.(() => { refresh().catch(fail); });
  function edge(x, y) {
    if (!resizable || maximized) return null;
    const west = x < 6, east = x >= innerWidth - 6;
    const north = y < 6, south = y >= innerHeight - 6;
    return north ? (west ? "NorthWest" : east ? "NorthEast" : "North")
      : south ? (west ? "SouthWest" : east ? "SouthEast" : "South")
      : west ? "West" : east ? "East" : null;
  }
  const cursors = {North:"ns-resize",South:"ns-resize",West:"ew-resize",East:"ew-resize",NorthWest:"nwse-resize",SouthEast:"nwse-resize",NorthEast:"nesw-resize",SouthWest:"nesw-resize"};
  function touchMove(g) {
    if (g.cancelled || g.epoch!==touchEpoch || !g.pos || g.busy || g.frame || !g.latest) return;
    // 移窗会再次产生指针事件；按帧合并，并按坐标去重，不能按事件对象判断移动。
    if (g.lastSent && g.lastSent.x===g.latest.x && g.lastSent.y===g.latest.y) return;
    g.frame = requestAnimationFrame(() => { g.frame=0; applyTouchMove(g); });
  }
  function applyTouchMove(g) {
    if (g.cancelled || g.epoch!==touchEpoch || (drag!==g && !g.ending)) return;
    const point = g.latest;
    g.lastSent = point;
    g.busy = true;
    const P = window.__TAURI__.dpi.PhysicalPosition;
    const dx = (point.x-g.x)*g.scale, dy = (point.y-g.y)*g.scale;
    let update;
    if (g.direction) {
      const west = g.direction.includes("West"), north = g.direction.includes("North");
      const horizontal = west || g.direction.includes("East"), vertical = north || g.direction.includes("South");
      const min = dialog ? [480,240] : [880,560]; // 与 Rust 建窗口的 min_inner_size 一致。
      const width = Math.max(min[0]*g.scale, g.size.width + (horizontal ? (west ? -dx : dx) : 0));
      const height = Math.max(min[1]*g.scale, g.size.height + (vertical ? (north ? -dy : dy) : 0));
      const x = g.pos.x + (west ? g.size.width-width : 0), y = g.pos.y + (north ? g.size.height-height : 0);
      const Size = window.__TAURI__.dpi.PhysicalSize;
      update = win.setSize(new Size(Math.round(width),Math.round(height)))
        .then(() => { if (!g.cancelled && g.epoch===touchEpoch) return win.setPosition(new P(Math.round(x),Math.round(y))); });
    } else {
      update = win.setPosition(new P(Math.round(g.pos.x+dx),Math.round(g.pos.y+dy)));
    }
    update.catch(fail).finally(() => { g.busy=false; if (drag===g || g.ending) touchMove(g); });
  }
  function endDrag(e) {
    if (drag?.touch && e && e.pointerId!==drag.pointerId) return;
    if (drag?.touch) {
      drag.cancelled = !e || e.type === "pointercancel";drag.ending = !drag.cancelled;
      if (drag.ending) touchMove(drag);
      try { drag.target.releasePointerCapture?.(drag.pointerId); } catch {}
    }
    drag = null;
  }
  // 触摸合成的 mousedown 不能再进入 Tauri 的原生拖动循环。
  document.addEventListener("mousedown", e => {
    const region = e.target.closest(".topbar, .brand, .dialog-window .modal > h3, .installer-window .drag");
    const control = e.target.closest("button, input, textarea, a, select, .win-ctrls, .select-wrap");
    if (drag?.touch || (e.sourceCapabilities?.firesTouchEvents && region && !control)) {
      e.preventDefault(); e.stopImmediatePropagation();
    }
  }, true);
  document.addEventListener("pointerdown", e => {
    if (e.button !== 0 || e.isPrimary === false) return;
    const touch = e.pointerType === "touch";
    const handle = e.target.closest("[data-window-resize]");
    if (!legacy && !installer && !touch && !handle) return;
    const direction = resizable && !maximized ? (handle?.dataset?.windowResize || edge(e.clientX, e.clientY)) : null;
    // WebView2 handles these regions as real non-client captions, before DOM input.
    if (!direction && nativeCaption && e.target.closest(".topbar, .brand, .dialog-window .modal > h3, .installer-window .drag") && !e.target.closest("button, input, textarea, a, select, .win-ctrls, .select-wrap, .search")) return;
    if (direction && !touch) {
      drag = null; e.preventDefault(); e.stopImmediatePropagation();
      win.startResizeDragging(direction).catch(fail);
      return;
    }
    if ((touch || installer) && (direction || e.target.closest(".topbar, .brand, .dialog-window .modal > h3, .installer-window .drag")) && !e.target.closest("button, input, textarea, a, select, .win-ctrls, .select-wrap")) {
      e.preventDefault(); e.stopImmediatePropagation();
      const g = {touch:true,epoch:++touchEpoch,pointerId:e.pointerId,target:e.target,x:e.screenX,y:e.screenY,latest:null,pos:null,scale:1,busy:false,direction};
      drag = g;
      try { e.target.setPointerCapture?.(e.pointerId); } catch {}
      (async () => {
        if (maximized) { await win.unmaximize(); maximized=false; }
        const [pos, scale, size] = await Promise.all([win.outerPosition(), win.scaleFactor(), direction ? win.innerSize() : null]);
        if (drag!==g) return;
        g.pos=pos;g.scale=scale;g.size=size;touchMove(g);
      })().catch(fail);
      return;
    }
    if (!dialog && (e.target.closest(".topbar, .brand") || (installer && e.target.closest(".drag"))) && !e.target.closest("button, input, textarea, a, select, .win-ctrls, .select-wrap")) {
      drag = {x:e.clientX, y:e.clientY};
    }
  }, true);
  document.addEventListener("pointermove", e => {
    if (drag?.touch) {
      if (e.pointerId!==drag.pointerId) return;
      e.preventDefault();drag.latest={x:e.screenX,y:e.screenY};touchMove(drag);return;
    }
    if (!legacy || installer) return;
    document.documentElement.style.cursor = cursors[edge(e.clientX,e.clientY)] || "";
    if (!drag) return;
    if (!(e.buttons & 1)) { drag = null; return; }
    if (Math.hypot(e.clientX-drag.x,e.clientY-drag.y) < 4) return;
    drag = null; e.preventDefault(); win.startDragging().catch(fail);
  });
  document.addEventListener("pointerup", endDrag);
  document.addEventListener("pointercancel", endDrag);
  window.addEventListener("blur", () => { endDrag(); document.documentElement.style.cursor = ""; });
})();
