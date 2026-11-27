// 前端逻辑。不引 React/Vue，直接操作 DOM —— 界面就这么多，上框架的开销大于收益。
//
// 布局对齐 Mac 版 MainView：侧栏分组 + 数量、顶部大速度卡（带速度曲线）、
// 任务行（缩略图 / 片名 / 类型 + 大小 / 进度 / 右侧速度）、底部粘贴框。
// 与 Rust 侧通过 Tauri invoke 通信（tauri.conf.json 里开了 withGlobalTauri）。

const nativeInvoke = window.__TAURI__?.core?.invoke;
const CREATE_COMMANDS = new Set(["add_task", "add_multi", "trash_redownload", "multi_task_redownload", "wechat_confirm"]);
const invoke = nativeInvoke ? (command, args) => !DIALOG && CREATE_COMMANDS.has(command) ? invokeCreatingTask(command, args) : nativeInvoke(command, args) : null;
// 新建框、确认框的独立窗口也跑这份脚本：只让用户填，投任务和排队都回主窗口做
const DIALOG = new URLSearchParams(location.search).has("dialog");
let dialogShown=false;
// 独立窗口不用跟着轮询任务、速度
const setInterval = DIALOG ? () => 0 : window.setInterval.bind(window);

const $ = (id) => document.getElementById(id);
const state = {
  view: "all",
  tasks: [],
  creatingTasks: new Map(),
  trash: [],
  engineRunning: false,
  busy: false,
  settings: null,
  query: "",
  // 速度采样点 [{t: 毫秒时间戳, v: 字节/秒}]，横轴按真时间摆位置
  speedPts: [],
  seenDone: new Set(),
  firstLoad: true,
  selId: null, // 键盘/单击选中的任务行（对齐 Mac 的 selectedID）
  // 多选：Ctrl/⌘ 点选、Shift 范围选。selId 仍表示「键盘焦点锚点」那一条
  selIds: new Set(),
  selAnchor: null,
};

// 所有来源的建任务请求先进入同一份列表，沿用现有行渲染；后端结果到达后原位接替。
let creatingSequence = 0;
function mergeCreatingTasks(tasks) {
  const real = tasks.filter(t => !t.creating);
  const ids = new Set(real.map(t => t.id));
  for (const [key, task] of state.creatingTasks) if (ids.has(task.id)) state.creatingTasks.delete(key);
  return [...state.creatingTasks.values(), ...real];
}
async function invokeCreatingTask(command, args = {}) {
  const key = `creating:${++creatingSequence}`;
  const old = state.tasks.find(t => t.id === args.id) || state.trash.find(t => t.id === args.id);
  let name = args.name || old?.name || old?.title || "新下载";
  if (!args.name && args.url) {
    try { const u = new URL(args.url); name = u.protocol === "magnet:" ? u.searchParams.get("dn") || "磁力下载" : decodeURIComponent(u.pathname.split("/").pop()) || name; } catch {}
  }
  const task = { id:key, creating:true, status:"preparing", name, kind:old?.kind || (args.url ? ndKind(args.url) : ""), created:Date.now(), progress:{downloaded:0,speed:0}, meta:{req:{url:args.url || ""},res:{name,size:0,files:[]}} };
  state.creatingTasks.set(key, task);
  state.tasks = mergeCreatingTasks(state.tasks);
  switchView("all");
  let id = null;
  try {
    const result = await nativeInvoke(command, args);
    id = typeof result === "string" && result ? result : command === "multi_task_redownload" ? args.id : null;
    if (id) task.id = id;
    return result;
  } finally {
    if (!id) state.creatingTasks.delete(key);
    state.tasks = mergeCreatingTasks(state.tasks);
    renderAll();
  }
}

// WebView2 会给文本框弹「保存的信息」（浏览器自动填充），和我们自己的下拉叠在一起。
// 后加的、动态生成的输入框也算上：获得焦点前统一关掉
const NO_AUTOFILL = 'input:not([type]), input[type="text"], input[type="search"], input[type="url"]';
document.querySelectorAll(NO_AUTOFILL).forEach((el) => el.setAttribute("autocomplete", "off"));
document.addEventListener("focusin", (e) => e.target.matches?.(NO_AUTOFILL) && e.target.setAttribute("autocomplete", "off"), true);

// ---------- 分类 / 文案 ----------

// 任务类型 → 资料库分类（Rust 侧按链接推出的 kind 标签）
const CAT_OF = {
  磁力: "movie",
  种子文件: "movie",
  ed2k: "movie",
  视频网站: "video",
  直链: "other",
};

/// 任务属于资料库哪一栏。视频网站任务后端给的 kind 是具体站点名
/// （Bilibili / YouTube / 小红书…），拿它查 CAT_OF 永远查不到，
/// 所以视频这一类按 protocol 认
// 网盘和 PikPak 的下载域名（和 Mac 版 panHosts 一样）
// 筛选里的「云盘」：国内外网盘的直链和 PikPak 云盘离线都算
const CLOUD_HOST = /baidupcs\.com|baidu\.com|quark\.cn|aliyundrive|alipan|123pan|123684|189\.cn|ctyunxs|139\.com|uc\.cn|xunlei\.com|pikpak|dropbox|drive\.google\.com|drive\.usercontent\.google\.com|1drv\.ms|onedrive|sharepoint\.com|mega\.nz/i;
function isCloud(t) {
  if (/^(?:nas:)?(?:pp:|cloud:)/.test(String(t.id))) return true;
  // 多文件任务都来自网盘
  if (String(t.meta?.req?.url || "").startsWith("shandian-multi:")) return true;
  try {
    const u = new URL(t.meta?.req?.url || "");
    return /^https?:$/.test(u.protocol) && CLOUD_HOST.test(u.hostname);
  } catch {
    return false;
  }
}

/// 从视频站页面取到的视频（地址是 CDN）由后端按来源页标上站名，也算视频
// 和后端 videos::site_name 的站名一致
const VIDEO_SITE_KINDS = new Set(["B 站", "YouTube", "抖音", "小红书", "快手", "微博", "西瓜视频", "Vimeo", "X", "新片场", "豆瓣", "微信视频号", "微信公众号"]);
function catOf(t) {
  if (t.protocol === "video" || VIDEO_SITE_KINDS.has(String(t.kind || "").replace(/^NAS · /, ""))) return "video";
  if (isCloud(t)) return "cloud";
  const c = CAT_OF[t.kind];
  if (c) return c;
  return "other";
}

const TITLE = {
  all: "下载中",
  done: "已完成",
  trash: "下载历史",
  movie: "电影",
  video: "视频",
  cloud: "云盘",
  other: "其他文件",
  settings: "设置",
};

const STATUS = {
  running: "下载中",
  ready: "等待中",
  wait: "等待中",
  pause: "已暂停",
  done: "已完成",
  error: "出错",
  closed: "已关闭",
};

const ACTIVE = new Set(["running", "wait", "ready"]);

const THUMB_ICON = { 磁力: "#i-bolt", 种子文件: "#i-doc", ed2k: "#i-cloud", 视频网站: "#i-play", 直链: "#i-link" };

// ---------- 格式化 ----------

function bytes(n) {
  if (!n) return "0 B";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let v = n, i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return (v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2)) + " " + u[i];
}

const speed = (n) => (n < 1024 ? "0 KB/s" : bytes(n) + "/s");

/// 大号速度拆成「数字 + 单位」两段，跟 Mac 版 hero 一样单位小一号
function speedParts(n) {
  if (!n || n < 1024) return ["0", "KB/s"];
  const u = ["KB", "MB", "GB", "TB"];
  let v = n / 1024, i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return [v >= 100 ? v.toFixed(0) : v.toFixed(1), u[i] + "/s"];
}

function remaining(sec) {
  if (!isFinite(sec) || sec <= 0) return "";
  const s = Math.round(sec);
  // 快下完时 (total-done)/speed 会算出 0.4 秒这种，四舍五入成「剩余 0 秒」很怪，直接不显示
  if (s < 1) return "";
  if (s < 60) return `剩余 ${s} 秒`;
  if (s < 3600) return `剩余 ${Math.floor(s / 60)} 分钟`;
  if (s >= 30 * 86400) return "遥遥无期"; // 再往上的数字没有意义
  // 一小时以上只说小时：带上分钟或天，数字跳得厉害
  return `剩余 ${Math.round(s / 3600)} 小时`;
}

/// 算剩余时间用的平滑速度：id -> { v, at }。瞬时速度一抖时间就乱跳，尤其是 BT。
/// 显示的速度仍是瞬时值，只有时间用它
const smoothSpeed = {};
function updateSmoothSpeed() {
  const now = performance.now();
  const alive = new Set();
  for (const t of state.tasks) {
    if (t.status !== "running") continue;
    alive.add(t.id);
    const v = t.progress?.speed || 0;
    const s = smoothSpeed[t.id];
    if (!s) {
      if (v > 0) smoothSpeed[t.id] = { v, at: now };
      continue;
    }
    // 时间常数 8 秒，和刷新频率无关
    s.v += (v - s.v) * (1 - Math.exp(-(now - s.at) / 8000));
    s.at = now;
  }
  for (const id of Object.keys(smoothSpeed)) if (!alive.has(id)) delete smoothSpeed[id];
}

/// 全部任务还要多久。按总速度算（空出来的带宽会让给别人，排队的也算进去）和
/// 最慢任务自己要多久（它慢可能是源少，别人下完也快不了）取较长的：宁可说久一点。
/// 大小未知或还没速度的任务算不进去，这时只能说「至少」
function etaAll(tasks) {
  let left = 0, speedSum = 0, slowest = 0, partial = false;
  for (const t of tasks) {
    const total = t.meta?.res?.size || 0;
    const rest = Math.max(0, total - (t.progress?.downloaded || 0));
    const spd = smoothSpeed[t.id]?.v || 0;
    if (!total) partial = true;
    left += rest;
    speedSum += spd;
    if (spd > 0) slowest = Math.max(slowest, rest / spd);
    else if (rest > 0 && t.status === "running") partial = true;
  }
  if (!speedSum || !left) return null;
  return { sec: Math.max(left / speedSum, slowest), partial };
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
}

let toastTimer, toastHide;
let inlineToastClose=()=>{};
window.ShFeedback?.track();
/// 把文本送进剪贴板。WebView2 里 navigator.clipboard 有时要额外授权，异步等待还可能丢掉用户手势；
/// 先用同步的 execCommand 真正写系统剪贴板，失败再退回异步 API。
function execCopy(text) {
  const active = document.activeElement;
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.setAttribute("readonly", "");
  ta.style.cssText = "position:fixed;top:0;left:0;width:1px;height:1px;padding:0;border:0;opacity:0;";
  document.body.appendChild(ta);
  let ok = false;
  try {
    ta.focus({ preventScroll: true });
    ta.select();
    ta.setSelectionRange(0, text.length);
    ok = document.execCommand("copy");
  } catch (_) {}
  ta.remove();
  // 复制完把焦点还给原来的输入框（新建框里用户可能正在改名字）
  try { active?.focus?.({ preventScroll: true }); } catch (_) {}
  return ok;
}
async function copyText(text) {
  const t = String(text ?? "");
  if (!t) return false;
  if (execCopy(t)) return true;
  try {
    await invoke("clipboard_write_text", { text:t });
    return true;
  } catch (_) {
    return false;
  }
}

/// 提示条。「已复制」这种一眼看完的很快收走，说明原因的话要留够读的时间；点一下提前关。
/// opts.action = { label, run } 在右边放一个按钮
let toastSequence = 0;
function toast(msg, opts = {}) {
  const sequence = ++toastSequence;
  const el = $("toast");
  const text = String(msg);
  inlineToastClose();inlineToastClose=()=>{};
  const feedback=window.ShFeedback,ctx=opts.anchor?feedback?.context(opts.anchor):null;
  const inlineClose=!opts.action && feedback?.inline(ctx?.anchor,text);
  if(inlineClose){
    clearTimeout(toastTimer);clearTimeout(toastHide);el.hidden=true;
    inlineToastClose=inlineClose;
    return ()=>{if(sequence===toastSequence)inlineClose()};
  }
  if(feedback)feedback.reset(el);
  el.textContent = "";
  // 长文字自动换行显示全，不再给提示条套悬停提示
  const body = document.createElement("span");
  body.className = "toast-text";
  body.textContent = text;
  el.append(body);
  if (opts.action) {
    const b = document.createElement("button");
    b.className = "toast-act";
    b.textContent = opts.action.label;
    b.onclick = () => opts.action.run();
    el.append(b);
  }
  const ms = opts.action || text.length > 40 ? 15000 : text.length > 10 ? 9000 : 3000;
  el.classList.remove("out");
  el.hidden = false;
  // 连续提示时让入场动画重新播一遍：先掐掉再强制回流
  el.style.animation = "none";
  void el.offsetWidth;
  el.style.animation = "";
  if(ctx)feedback.position(el,ctx);
  clearTimeout(toastTimer);
  clearTimeout(toastHide);
  toastTimer = setTimeout(() => {
    el.classList.add("out");
    // 等淡出动画走完再真的藏起来，不然直接消失会很生硬
    toastHide = setTimeout(() => {
      el.hidden = true;
      el.classList.remove("out");
    }, 200);
  }, ms);
  return () => {
    if (sequence !== toastSequence) return;
    clearTimeout(toastTimer); clearTimeout(toastHide);
    el.hidden = true; el.classList.remove("out");
  };
}
$("toast").addEventListener("click", () => {
  clearTimeout(toastTimer);
  clearTimeout(toastHide);
  $("toast").hidden = true;
});

// ---------- 悬浮说明 ----------

// 设置卡片的解释文字平时不占版面，收在标题旁边的小 i 里（data-tip），鼠标移上去才出。
// 文字短、只做补充说明；真正的配置值（网关地址、密钥那些）仍然留在卡片里让人看得见。
const tipEl = $("tip");
/// 页内卡片提示：钉在被说明的元素上（居中于它下方），不跟鼠标。
/// 跟着指针动的那种，鼠标从按钮左侧滑到右侧时提示会跟着跑，看着像在抖；
/// 钉在按钮上才稳，箭头也指得准。（独立 tip 窗口曾在命令线程建窗导致主界面卡死，已停用。）
/// 有下拉菜单开着时不出悬停提示：菜单已经在眼前，提示只会叠在上面挡内容
function menuOpen() {
  return !!document.querySelector(".ctx-menu:not([hidden]), .select-menu:not([hidden]), .acc-pop:not([hidden])");
}
function showTip(el) {
  const text = el.dataset.tip || "";
  if (!text || !tipEl || menuOpen()) return;
  tipEl.textContent = text;
  tipEl.hidden = false;
  tipEl.classList.remove("tip-up"); // 默认尖朝上 = 卡片在下方
  const tw = tipEl.offsetWidth;
  const th = tipEl.offsetHeight;
  const r = el.getBoundingClientRect();
  // 左右别贴边出界
  let x = r.left + r.width / 2 - tw / 2;
  x = Math.min(Math.max(6, x), window.innerWidth - tw - 6);
  // 勾选框里行距小，提示贴近一点；其他地方留 14，免得挡住下面的按钮
  let y = r.bottom + (el.closest(".grab-row") ? 8 : 14);
  if (y + th > window.innerHeight - 4) {
    y = r.top - th - 10;
    tipEl.classList.add("tip-up");
  }
  tipEl.style.left = `${Math.round(x)}px`;
  tipEl.style.top = `${Math.round(y)}px`;
  // 箭头对准元素中心（卡片被边界挤过之后，箭头得跟着修正）
  const arrowX = Math.round(r.left + r.width / 2 - x);
  tipEl.style.setProperty("--tip-arrow-x", `${arrowX}px`);
}
function hideTip() { if (tipEl) tipEl.hidden = true; }
// 点开菜单时鼠标还停在按钮上，不会再有 mouseover，已经出来的提示要在这里收掉
document.addEventListener("click", () => { if (menuOpen()) hideTip(); });
document.addEventListener("contextmenu", () => setTimeout(() => { if (menuOpen()) hideTip(); }));
document.addEventListener("mouseover", (e) => {
  const el = e.target.closest?.("[data-tip]");
  if (el) { showTip(el); return; }
  // 元素自带的 title 会由 WebView2 画成白底黑框的系统提示，和这套界面完全不是一回事。
  // 这里把 title 收编成 data-tip：先摘掉 title（不摘原生白框还会弹），再走自己的卡片提示。
  const tt = e.target.closest?.("[title]");
  if (tt) {
    // 文件名、路径说明：正文已经可读，摘掉 title 别再弹一层
    if (tt.closest(".task-name") || tt.classList.contains("set-path") || tt.classList.contains("set-hint")) {
      tt.removeAttribute("title");
      return;
    }
    // 文字没被省略就没必要再弹一层
    if (!tt.closest("button") && tt.scrollWidth <= tt.clientWidth && tt.scrollHeight <= tt.clientHeight + 1) {
      return;
    }
    const text = tt.getAttribute("title");
    if (text && text.trim()) {
      tt.removeAttribute("title");
      tt.dataset.tip = text;
      showTip(tt);
    } else {
      tt.removeAttribute("title");
    }
  }
});
document.addEventListener("mouseout", (e) => {
  if (e.target.closest?.("[data-tip]")) hideTip();
});
// 切视图、滚动、点按钮时收掉，免得提示留在原地
document.addEventListener("click", hideTip);
window.addEventListener("scroll", hideTip, true);

// 启动时把残留的 title 全部收成 data-tip：WebView2 对 title 会画系统白框，
// 和自绘 tip 同时出现（用户反馈设置页里双重提示就是这么来的）。
// 空 title、.set-path（正文已经把路径显示出来了，再弹 tip 是重复）一律只摘掉。
(function migrateTitles() {
  document.querySelectorAll("[title]").forEach((el) => {
    if (el.closest(".task-name")) { el.removeAttribute("title"); return; }
    // 路径/说明正文本身就完整可读，悬浮再弹一遍等于双重提示
    if (el.classList.contains("set-path") || el.classList.contains("set-hint")) {
      el.removeAttribute("title");
      return;
    }
    const text = el.getAttribute("title");
    if (text && text.trim() && !el.dataset.tip) {
      el.dataset.tip = text;
    }
    el.removeAttribute("title");
  });
})();

// ---------- 主题 ----------

// Mac 版用 NSColor.adaptive 跟随系统；这里多给一个手动开关（设置 › 外观），
// 三种取值里 system 时才看系统偏好，否则固定用选的那套。
// theme.js 已在首帧应用 Rust 注入的偏好。主窗口负责修改和缓存偏好。
let themePref = window.__sdTheme || "system";

function applyTheme() {
  if (DIALOG) return;
  window.__sdSetTheme(themePref);
  // 记一份到本地：下次启动 head 里的内联脚本靠它决定第一帧的深浅，不然深色用户会先白一下
  try { localStorage.setItem("sd.theme", themePref); } catch (e) {}
  document.querySelectorAll("[data-theme-set]").forEach((b) => {
    b.classList.toggle("on", b.dataset.themeSet === themePref);
  });
  // 标题栏是系统画的，得让 Tauri 一起改，否则手动选深色时会出现白标题栏
  invoke?.("set_app_theme", { theme: themePref }).catch(() => {});
}

function setTheme(v) {
  themePref = v;
  applyTheme();
}

/// 有新版时要不要弹提示（圆点 + 启动 toast）。
/// 发布源尚未接入，自动和手动检查都保持关闭。

/// 进程内启动时刻：启动头几秒任务状态还没稳，别急着显示「连接中」的转圈
const BOOT_AT = performance.now();

// ---------- 速度曲线 ----------

// 内核每 ~400ms 才刷新一次 progress（实测：0/100/200/300ms 同一个数，400ms 才动），
// 所以采样 200ms 刚好，1 秒一次会丢掉一半变化。
const SPARK_TICK = 200;
const SPARK_SPAN = 60000;   // 横轴 1 分钟
const SPARK_KEEP = 65000;   // 多留 5 秒余量给左端淡出

let sparkRaf = 0;      // 逐帧循环句柄
let sparkMaxNow = 1;   // 显示用的纵轴上限，缓慢跟随峰值
let sparkLastAt = 0;   // 上一帧时刻
let sparkIdleAt = 0;   // 所有任务停下的时刻；几秒后整条淡出
let sparkCtx = null;   // canvas 上下文
let sparkColor = "";   // --tint 的实际颜色（canvas 吃不到 CSS 变量）
let sparkTheme = "";   // 上次取色时的主题
let sparkDrawnAt = 0;  // 上一帧真正重绘的时刻（30fps 节流用）

/// 给 rgb(...) 套一个透明度
function withAlpha(color, a) {
  const m = /rgba?\(([^)]+)\)/.exec(color);
  if (!m) return color;
  const n = m[1].split(/[,\s\/]+/).filter(Boolean);
  return `rgba(${n[0]}, ${n[1]}, ${n[2]}, ${a})`;
}

/// 每帧重画。三条规矩：
///   1. 横轴是真时间轴，右端对齐「现在」，整条线连续左移，不是每秒跳一格
///   2. 画过的点数值写死不动——只有最右那个点还在往新值长，别整条一起变形
///   3. 右端 15% 以外渐隐渐细，滚出画面的旧数据自然淡掉
function drawSpark() {
  const cv = $("spark");
  if (!cv) { sparkRaf = 0; return; }
  // 30fps 足够：曲线每秒只左移 15px，30 帧下每帧 0.5px，肉眼看不出来；
  // 60fps 白烧一倍 CPU（实测曲线本身占 11% 单核，砍半后约 5%）
  const tNow = performance.now();
  if (tNow - sparkDrawnAt < 32) {
    sparkRaf = requestAnimationFrame(drawSpark);
    return;
  }
  sparkDrawnAt = tNow;
  const ctx = sparkCtx || (sparkCtx = cv.getContext("2d"));
  const pts = state.speedPts;

  let peak = 0;
  for (let i = 0; i < pts.length; i++) if (pts[i].v > peak) peak = pts[i].v;

  const now = performance.now();
  const active = state.tasks.some((t) => ACTIVE.has(t.status));
  if (active) sparkIdleAt = 0;
  else if (!sparkIdleAt) sparkIdleAt = now;
  const fade = sparkIdleAt ? Math.max(0, 1 - Math.max(0, now - sparkIdleAt - 5000) / 1000) : 1;

  if (!pts.length || peak <= 0 || fade <= 0) {
    cv.style.opacity = "0";
    drawCompactSpark(pts, now, 1, "", 0);
    sparkRaf = 0;
    if (fade <= 0 && pts.length) {
      pts.length = 0;
      sparkIdleAt = 0;
      sparkMaxNow = 1;
    }
    return;
  }
  const op = (0.95 * fade).toFixed(3);
  if (cv.dataset.op !== op) {
    cv.style.opacity = op;
    cv.dataset.op = op;
  }

  const W = Math.max(120, Math.round(cv.clientWidth || 460));
  const H = Math.max(40, Math.round(cv.clientHeight || 96));
  const dpr = Math.min(3, window.devicePixelRatio || 1);
  const pw = Math.round(W * dpr);
  const ph = Math.round(H * dpr);
  if (cv.width !== pw || cv.height !== ph) {
    cv.width = pw;
    cv.height = ph;
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, W, H);

  const dt = sparkLastAt ? Math.min(0.2, (now - sparkLastAt) / 1000) : 0.016;
  sparkLastAt = now;

  // 只有最后一个点还在动：从上一个点的高度长到新值，长完就定住
  const head = pts[pts.length - 1];
  if (head.y == null) head.y = pts.length > 1 ? pts[pts.length - 2].v : head.v;
  head.y += (head.v - head.y) * (1 - Math.exp(-9 * dt));

  // 纵轴上限：先把峰值量化到 5% 一档再跟。速度每秒都在 ±2% 地抖，
  // 上限跟着抖就等于整条曲线（连同下面那块渐变）在缩放，看着就是「细微的抖」。
  // 量化之后 5% 以内的波动完全不动它，跨档时才用慢速缓动过去
  const q = Math.max(1, peak * 0.05);
  const target = Math.ceil(peak / q) * q * 1.12;
  const k = target > sparkMaxNow ? 0.5 : 0.35;
  sparkMaxNow += (target - sparkMaxNow) * (1 - Math.exp(-k * dt));
  // 上面的缓动追不上突然的高速：上限不能低于正在画的最高点，不然圆点会冲出画布上缘。
  // 用缓动后的 y，不用目标值，所以这里跟着圆点走，不会一下子跳
  for (let i = 0; i < pts.length; i++) {
    const drawn = (pts[i].y ?? pts[i].v) * 1.03;
    if (drawn > sparkMaxNow) sparkMaxNow = drawn;
  }
  const max = Math.max(sparkMaxNow, 1);

  const drawW = W - 8;
  const xOf = (t) => drawW - ((now - t) / SPARK_SPAN) * drawW;
  const padY = 9;
  const ys = (v) => H - padY - (v / max) * (H - padY * 2);

  const themeKey = document.documentElement.dataset.theme || "";
  if (!sparkColor || sparkTheme !== themeKey) {
    sparkTheme = themeKey;
    sparkColor = getComputedStyle(cv).color;
  }
  const color = sparkColor;
  drawCompactSpark(pts, now, max, color, fade);
  if (!cv.clientWidth) { sparkRaf = requestAnimationFrame(drawSpark); return; }

  // 先攒成一条完整路径（点已全部筛掉左边界外的），再按段上色
  // 从「第一个有速度的点」往前两格起笔：下载开始前那一长串 0 会画成一条贴底的直线，
  // 又长又没意义，左下角那片空白其实都是它
  let begin = 0;
  for (let i = 0; i < pts.length; i++) {
    if (pts[i].v > 0) {
      begin = Math.max(0, i - 2);
      break;
    }
  }

  const xs = [];
  const pm = new Path2D();
  let started = false;
  let px = 0;
  let py = 0;
  let firstX = 0;
  for (let i = begin; i < pts.length; i++) {
    const x = xOf(pts[i].t);
    if (x < -6) continue;
    const y = ys(pts[i].y ?? pts[i].v);
    if (!started) {
      pm.moveTo(x, y);
      firstX = x;
      started = true;
    } else {
      const mx = (px + x) / 2;
      pm.bezierCurveTo(mx, py, mx, y, x, y);
    }
    xs.push({ x, y });
    px = x;
    py = y;
  }
  if (!started) {
    sparkRaf = requestAnimationFrame(drawSpark);
    return;
  }

  paintSpeedTrend(ctx,pm,xs,firstX,W,H,drawW,ys(head.y),color,4);

  sparkRaf = requestAnimationFrame(drawSpark);
}

// 顶栏曲线共用已有采样、纵轴上限和动画循环；隐藏时不绘制。
// Both sizes use the same fill, fade, line profile and endpoint rendering.
function paintSpeedTrend(ctx, pm, xs, firstX, W, H, drawW, headY, color, radius) {
  // 线下面的渐变面：右边界钉在画布上，只有形状在变
  const area = new Path2D(pm);
  // 铺到卡片右边缘，圆点右边没有竖着的切口
  area.lineTo(W, headY);
  area.lineTo(W, H);
  area.lineTo(Math.max(0, firstX), H);
  area.closePath();
  const grad = ctx.createLinearGradient(0, 0, 0, H);
  grad.addColorStop(0, withAlpha(color, 0.2));
  grad.addColorStop(1, withAlpha(color, 0));
  ctx.fillStyle = grad;
  ctx.fill(area);

  // 面的左边界现在是一刀切的竖线，很扎眼。这里用「擦除」把靠近起点的那段擦成透明，
  // 叠在卡片底色上就是自然淡出（直接调 fill 的渐变做不到两个方向同时渐变）
  const gx0 = Math.max(0, firstX);
  const gx1 = gx0 + Math.min(drawW * 0.4, 180);
  const cut = ctx.createLinearGradient(gx0, 0, gx1, 0);
  cut.addColorStop(0, "rgba(0,0,0,1)");
  cut.addColorStop(1, "rgba(0,0,0,0)");
  ctx.save();
  ctx.globalCompositeOperation = "destination-out";
  ctx.fillStyle = cut;
  ctx.fillRect(0, 0, drawW, H);
  ctx.restore();

  // 线：实线只留右端 15%，往左变细变淡（用户要求）。分段描边时半透明的接头会叠出一节一节，
  // 按线长算比例时一个尖峰又会让整条线变样。所以按 x 算，整条线铺成一块带状图形一次填完
  const lineLeft = Math.max(0, firstX);
  const fadeSpan = Math.max(1, (drawW - lineLeft) * 0.85);
  const fadeAt = (x) => {
    const u = Math.min(1, Math.max(0, (x - lineLeft) / fadeSpan));
    return u * u * (3 - 2 * u);
  };
  const line = xs.length ? [xs[0]] : [];
  for (let i = 1; i < xs.length; i++) {
    const a = xs[i - 1];
    const b = xs[i];
    const mx = (a.x + b.x) / 2;
    const steps = Math.min(12, Math.max(2, Math.ceil(Math.hypot(b.x - a.x, b.y - a.y) / 3)));
    for (let s = 1; s <= steps; s++) {
      const u = s / steps;
      const v = 1 - u;
      line.push({
        x: v * v * v * a.x + 3 * v * u * (v + u) * mx + u * u * u * b.x,
        y: (v * v * v + 3 * v * v * u) * a.y + (3 * v * u * u + u * u * u) * b.y,
      });
    }
  }
  if (line.length > 1) {
    const upper = [];
    const lower = [];
    for (let i = 0; i < line.length; i++) {
      const p = line[Math.max(0, i - 1)];
      const n = line[Math.min(line.length - 1, i + 1)];
      const dx = n.x - p.x;
      const dy = n.y - p.y;
      const len = Math.hypot(dx, dy) || 1;
      const t = fadeAt(line[i].x);
      const half = 0.3 + 0.8 * t * t;
      upper.push(line[i].x - (dy / len) * half, line[i].y + (dx / len) * half);
      lower.push(line[i].x + (dy / len) * half, line[i].y - (dx / len) * half);
    }
    ctx.beginPath();
    ctx.moveTo(upper[0], upper[1]);
    for (let i = 2; i < upper.length; i += 2) ctx.lineTo(upper[i], upper[i + 1]);
    for (let i = lower.length - 2; i >= 0; i -= 2) ctx.lineTo(lower[i], lower[i + 1]);
    ctx.closePath();
    const lineGrad = ctx.createLinearGradient(lineLeft, 0, lineLeft + fadeSpan, 0);
    for (let k = 0; k <= 8; k++) {
      const u = k / 8;
      lineGrad.addColorStop(u, withAlpha(color, 0.02 + 0.98 * u * u * (3 - 2 * u)));
    }
    ctx.fillStyle = lineGrad;
    ctx.fill();
  }

  // 末端圆点钉在右边缘：它是「现在」，不该跟着采样点漂
  ctx.beginPath();
  ctx.arc(drawW, headY, radius, 0, Math.PI * 2);
  ctx.fillStyle = color;
  ctx.fill();

}

function drawCompactSpark(pts, now, max, color, fade) {
  const cv = $("top-spark");
  if (!cv || !cv.clientWidth) return;
  const W = cv.clientWidth, H = cv.clientHeight;
  const dpr = Math.min(3, window.devicePixelRatio || 1);
  if (cv.width !== Math.round(W*dpr) || cv.height !== Math.round(H*dpr)) {
    cv.width = Math.round(W*dpr); cv.height = Math.round(H*dpr);
  }
  const ctx = cv.getContext("2d");
  ctx.setTransform(dpr,0,0,dpr,0,0);ctx.clearRect(0,0,W,H);
  if (!fade || !pts.length) return;
  const path=new Path2D(),points=[],drawW=W-4;
  let previous=null,firstX=0;
  for(const point of pts){
    const x=drawW-((now-point.t)/SPARK_SPAN)*drawW;
    if(x < -6)continue;
    const y=H-5-Math.min(1,(point.y ?? point.v)/max)*(H-10);
    if(previous){const mid=(previous.x+x)/2;path.bezierCurveTo(mid,previous.y,mid,y,x,y);}
    else{path.moveTo(x,y);firstX=x;}
    points.push({x,y});previous={x,y};
  }
  if(!previous)return;
  ctx.save();ctx.globalAlpha=fade;
  paintSpeedTrend(ctx,path,points,firstX,W,H,drawW,previous.y,color,2.3);
  ctx.restore();
}

/// 启动逐帧循环（已经在跑就什么都不做）
function ensureSparkLoop() {
  if (!sparkRaf) sparkRaf = requestAnimationFrame(drawSpark);
}

window.addEventListener("resize", () => ensureSparkLoop());

// ---------- 侧栏 ----------

function renderSidebar() {
  const visibleTasks = state.tasks.filter(taskVisibleAfterRemove);
  const t = visibleTasks;
  const isDone = (x) => x.status === "done";
  const cat = (c) => t.filter((x) => catOf(x) === c).length;
  $("c-all").textContent = t.filter((x) => !isDone(x)).length || "";
  $("c-done").textContent = t.filter(isDone).length || "";
  $("c-trash").textContent = state.trash.length || "";
  $("c-movie").textContent = cat("movie") || "";
  $("c-video").textContent = cat("video") || "";
  $("c-cloud").textContent = cat("cloud") || "";
  $("c-other").textContent = cat("other") || "";
}

// ---------- 任务列表 ----------

// ---------- 筛选 ----------
const FILTER_TYPES = [["video", "视频"], ["app", "安装包"], ["image", "图片"], ["audio", "音频"], ["zip", "压缩包"], ["bt", "BT 种子"], ["doc", "文档"], ["other", "其他"]];
const filterState = { sources: new Set(), types: new Set() };
try {
  const f = JSON.parse(localStorage.getItem("sd.filter") || "{}");
  filterState.sources = new Set(f.sources || []);
  filterState.types = new Set(f.types || []);
} catch (_) {}
function typeOfName(name) {
  const ext = (String(name || "").match(/\.([a-z0-9]{1,8})$/i)?.[1] || "").toLowerCase();
  if (/^(mp4|mkv|avi|mov|flv|wmv|webm|m4v|ts|m2ts|rmvb|mpg|mpeg)$/.test(ext)) return "video";
  if (/^(exe|msi|dmg|pkg|apk|deb|rpm|appimage|msix)$/.test(ext)) return "app";
  if (/^(jpg|jpeg|png|gif|webp|bmp|svg|heic|tif|tiff|exr|hdr)$/.test(ext)) return "image";
  if (/^(mp3|flac|wav|aac|m4a|ogg|oga|opus|ape|wma|aif|aiff|amr|ac3|dts|mka|mid|midi)$/.test(ext)) return "audio";
  if (/^(zip|rar|7z|tar|gz|bz2|xz|tgz|iso|cab)$/.test(ext)) return "zip";
  if (/^(pdf|docx?|xlsx?|pptx?|txt|md|epub|csv)$/.test(ext)) return "doc";
  return "other";
}
// 后端按链接标的来源名（netdisk_label），GitHub 不算网盘
const CLOUD_KINDS = new Set(["百度网盘", "夸克网盘", "迅雷云盘", "阿里云盘", "115 网盘", "天翼云盘", "移动云盘", "123 云盘", "UC 网盘", "光鸭云盘", "蓝奏云", "PikPak", "Dropbox", "Google Drive", "OneDrive", "MEGA", "网盘", "云盘离线"]);
function filterHit(id, link, name, isVideo, isTorrent, fileNames, kind) {
  const f = filterState;
  if (f.sources.size) {
    const sid = String(id);
    // 一条任务可以同时属于两个来源：NAS 上下的网盘文件既算 NAS 也算云盘
    const srcs = new Set([sid.startsWith("xl:") ? "thunder" : sid.startsWith("nas:") ? "nas" : "local"]);
    if (/^(?:nas:)?(?:pp:|cloud:)/.test(sid) || String(link || "").startsWith("shandian-multi:") || CLOUD_HOST.test(link || "") || CLOUD_KINDS.has(String(kind || "").replace(/^NAS · /, ""))) srcs.add("cloud");
    if (srcs.has("cloud") && srcs.has("local")) srcs.delete("local");
    if (![...srcs].some((x) => f.sources.has(x))) return false;
  }
  if (f.types.size) {
    if (isTorrent && f.types.has("bt")) return true;
    if (isVideo && f.types.has("video")) return true;
    const names = fileNames?.length ? fileNames : [name];
    if (!names.some((n) => f.types.has(typeOfName(n)))) return false;
  }
  return true;
}
function filterTask(t) {
  return filterHit(t.id, t.meta?.req?.url || "", t.name || t.meta?.res?.name || "", t.protocol === "video" || String(t.id).startsWith("v:"), isBt(t) || /^NAS · (磁力|种子)/.test(t.kind || ""), (t.meta?.res?.files || []).map((x) => x.name), t.kind);
}
function syncFilterBtn() {
  $("filter-btn")?.classList.toggle("on", !!(filterState.sources.size || filterState.types.size));
}
function renderFilterPop() {
  const pop = $("filter-pop");
  const sources = [["local", "本机"], ...($("nav-nas") ? [["nas", "NAS"]] : []), ["cloud", "云盘"], ...(thunderInfo.installed ? [["thunder", "迅雷"]] : [])];
  // 每一项后面标出当前页里有多少条：只勾这一项（另一组的勾选照旧）时能筛出几条
  const pool = state.view === "trash"
    ? state.trash.map((r) => () => filterHit(r.id, r.link || "", r.title || "", /^https?:/i.test(r.link || "") && !/\.[a-z0-9]{2,5}($|\?)/i.test(r.link), /^magnet:|\.torrent/i.test(r.link || ""), [], r.origin))
    : state.tasks.filter(taskVisibleAfterRemove).filter(matches).map((x) => () => filterTask(x));
  const count = (key, v) => {
    const keep = filterState[key];
    filterState[key] = v == null ? new Set() : new Set([v]);
    const n = pool.filter((hit) => hit()).length;
    filterState[key] = keep;
    return n;
  };
  const num = (n) => `<span class="filter-n">${n}</span>`;
  // 两组放在同一个 grid 里：勾选框、名称、数字三列都跨分组对齐
  const cells = (key, all, items) => `<label><input type="checkbox" data-all="${key}" ${filterState[key].size ? "" : "checked"}><span class="filter-t">${all}</span>${num(count(key, null))}</label>${items.map(([v, l]) => `<label><input type="checkbox" data-k="${key}" data-v="${v}" ${filterState[key].has(v) ? "checked" : ""}><span class="filter-t">${l}</span>${num(count(key, v))}</label>`).join("")}`;
  const group = (key, all, items) => `<div class="filter-grid">${cells(key, all, items)}</div>`;
  pop.innerHTML = `<h4>文件来源</h4>${group("sources", "全部来源", sources)}<h4>文件类型</h4>${group("types", "全部类型", FILTER_TYPES)}<div class="filter-foot"><button class="ghost" id="filter-reset">重置</button></div>`;
}
function applyFilter() {
  try { localStorage.setItem("sd.filter", JSON.stringify({ sources: [...filterState.sources], types: [...filterState.types] })); } catch (_) {}
  syncFilterBtn();
  renderFilterPop();
  $("task-list").__key = "";
  renderTasks();
}
$("filter-btn")?.addEventListener("click", (e) => {
  e.stopPropagation();
  const pop = $("filter-pop");
  if (!pop.hidden) { pop.hidden = true; return; }
  pop.hidden = false; // 先显示再渲染：量名称宽度要有布局
  renderFilterPop();
  const r = $("filter-btn").getBoundingClientRect();
  pop.style.left = `${Math.max(8, Math.min(r.right - pop.offsetWidth, innerWidth - pop.offsetWidth - 8))}px`;
  pop.style.top = `${r.bottom + 8}px`;
});
$("filter-pop")?.addEventListener("change", (e) => {
  const b = e.target;
  if (b.dataset.all) filterState[b.dataset.all].clear();
  else if (b.dataset.k) {
    if (b.checked) filterState[b.dataset.k].add(b.dataset.v);
    else filterState[b.dataset.k].delete(b.dataset.v);
  }
  applyFilter();
});
$("filter-pop")?.addEventListener("click", (e) => {
  e.stopPropagation();
  if (e.target.id === "filter-reset") {
    filterState.sources.clear();
    filterState.types.clear();
    applyFilter();
  }
});
document.addEventListener("pointerdown", (e) => {
  if (!e.target.closest?.("#filter-pop, #filter-btn") && $("filter-pop")) $("filter-pop").hidden = true;
});
setTimeout(syncFilterBtn);

function matches(t) {
  const v = state.view;
  if (v === "all") return t.status !== "done";
  if (v === "done") return t.status === "done";
  if (v === "trash") return false;
  if (v === "movie" || v === "video" || v === "cloud" || v === "other") {
    // 电影 / 视频 分类页：进行中和已完成一起列（Mac 版也是上面进行中、下面已完成）
    return catOf(t) === v;
  }
  return true;
}

/// 搜索关键字高亮：整段连续匹配（不区分大小写），全部命中处标出
function highlightName(name, q) {
  const text = String(name || "");
  const needle = String(q || "").trim();
  if (!needle) return escapeHtml(text);
  const lower = text.toLowerCase();
  const nlow = needle.toLowerCase();
  let out = "";
  let start = 0;
  let idx;
  while ((idx = lower.indexOf(nlow, start)) !== -1) {
    if (idx > start) out += escapeHtml(text.slice(start, idx));
    out += `<mark class="hl">${escapeHtml(text.slice(idx, idx + needle.length))}</mark>`;
    start = idx + needle.length;
  }
  if (start < text.length) out += escapeHtml(text.slice(start));
  return out;
}

function baiduFileProgress(t) {
  const labels = t.meta?.req?.labels;
  const total = Number(labels?.baiduFilesTotal);
  if (!Number.isSafeInteger(total) || total < 1) return "";
  const done = Math.min(total, Number(labels?.baiduFilesDone) || 0);
  return `已下载 ${done}/${total} 项`;
}

function taskStatusHtml(value, spin) {
  const text = escapeHtml(value);
  if (!spin) return text;
  const spinner = '<span class="spin" aria-hidden="true"></span>';
  return text ? spinner + "<span>" + text + "</span>" : spinner;
}

function updateTaskStatus(el, value, spin) {
  if (!el) return;
  const mode = spin ? "spin" : "text";
  // 状态没变时保留圆环，避免每次轮询重新播放动画。
  if (el.dataset.mode !== mode) {
    el.dataset.mode = mode;
    el.innerHTML = taskStatusHtml(value, spin);
  } else if (!spin && el.textContent !== value) {
    el.textContent = value;
  }
}

function taskKind(t) {
  return (t.kind || "") + (String(t.id).startsWith("xl:") ? " · 迅雷" : "");
}

function rowHtml(t) {
  const v = rowVals(t);
  const q = state.query.trim().toLowerCase();
  const err = t.shandianError || "";
  const paused = t.status === "pause";
  const isErr = t.status === "error";
  const kind = taskKind(t);
  const origin = t.originLabel || kind;
  const ownShare = isOwnBaiduShare(t);
  const actBtn = t.creating ? "" :
    t.status === "done"
      ? `<button class="circle sm" data-act="redl" title="重新下载"><svg class="ic"><use href="#i-dl-box"/></svg></button>
         <button class="circle sm" data-act="folder" title="在资源管理器里打开"><svg class="ic"><use href="#i-folder"/></svg></button>
         <button class="circle sm" data-act="del" title="删除"><svg class="ic"><use href="#i-trash"/></svg></button>`
      : `<button class="circle sm" data-act="${ownShare ? "openLink" : paused || isErr ? "resume" : "pause"}" title="${ownShare ? "打开原分享" : paused || isErr ? "继续" : "暂停"}">
           <svg class="ic"><use href="#${ownShare ? "i-link" : paused || isErr ? "i-resume" : "i-pause"}"/></svg></button>
         <button class="circle sm" data-act="folder" title="在资源管理器里打开"><svg class="ic"><use href="#i-folder"/></svg></button>
         <button class="circle sm" data-act="del" title="取消下载"><svg class="ic"><use href="#i-x"/></svg></button>`;

  // tabindex 让行能聚焦，↑↓/空格/回车/Delete 这套键盘操作才有着落（对齐 Mac）
  // 原路径找不到文件只说明位置失效，不能断定文件已删除。
  const missing = !!t.missing;
  // 文件没了，这些信息仍然有用：认得出是哪个版本，才好决定要不要重下
  const metaBody = `<span class="res"${v.resText ? "" : ' style="display:none"'}>${escapeHtml(v.resText)}</span>
        <span class="task-subtitles subtitle-controls">${renderSubtitleControls(t)}</span>
        <span class="sz">${escapeHtml(v.sizeText)}</span>
        ${v.timeText ? `<span class="tm">${escapeHtml(v.timeText)}</span>` : ""}`;

  return `<div class="task${paused ? " paused" : ""}${isErr ? " err" : ""}${missing ? " missing" : ""}${flashing === t.id ? " flash" : ""}" data-id="${escapeHtml(t.id)}" tabindex="0">
    ${taskThumbHtml(t, v.name, kind)}
    <div class="task-body">
      <div class="task-name" title="${escapeHtml(v.name)}">${highlightName(v.name, q)}</div>
      <div class="task-meta">
        ${origin ? `<span class="kind">${escapeHtml(origin)}</span>` : ""}
        ${metaBody}
        ${hasFileList(t) ? `<button class="files-chip" data-act="files" data-tip="${isBaiduFileTask(t) ? "查看文件下载进度" : "查看和选择里面的文件"}"><span>${escapeHtml(fileChipText(t))}</span><svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4.5 2.5 8 6l-3.5 3.5"/></svg></button>` : ""}
        ${t.collectionFiles ? `<span class="gone collection-missing" data-tip="在文件抽屉里查看缺失的子项"${t.collectionMissing ? "" : " hidden"}>${t.collectionMissing} 项缺失</span>` : missing ? `<span class="gone" data-tip="原路径未找到文件，可能已移动、重命名或删除。">文件不在原位置</span>` : ""}
      </div>
    </div>
    <div class="task-tail">
      <div class="task-acts">${!t.creating && canPlay(t) ? `<button class="circle sm" data-act="play" title="${t.status === "done" ? "播放" : "边下边播"}"><svg class="ic"><use href="#i-play"/></svg></button>` : ""}${actBtn}</div>
      <div class="task-right${v.mainSpin ? " spinning" : ""}">
        <div class="main-val" data-mode="${v.mainSpin ? "spin" : "text"}">${taskStatusHtml(v.mainSpin ? "" : v.mainVal, v.mainSpin)}</div>
        <div class="sub-val" data-mode="${v.subSpin ? "spin" : "text"}"${v.subTip ? ` data-tip="${escapeHtml(v.subTip)}"` : ""}>${taskStatusHtml(v.subVal, v.subSpin)}</div>
      </div>
    </div>
    ${isErr
      ? `<div class="err-line"><span class="err-text">${escapeHtml(errSummary(err))}</span><button class="err-more" data-act="errinfo">详情</button></div>`
      : `<div class="bar${v.indet ? " indet" : ""}"><i${v.indet ? "" : ` style="width:${v.pct}%"`}></i></div>`}
  </div>`;
}

/// 列表里只放一句话：friendly_error 把人话写在第一段，后面才是英文原文。
/// 没有人话的原始报错，去掉 yt-dlp 的「[站点] id:」前缀，长了由样式截断，全文在「详情」里
function errSummary(err) {
  const first = String(err || "").split("\n").map((s) => s.trim()).find(Boolean) || "下载失败";
  return first.replace(/^\[[^\]]+\]\s*[^\s:]*:\s*/, "");
}

/// 下载时间（created 是毫秒时间戳）：「M-D HH:mm」，两位补零，本地时区
function fmtTime(ms) {
  if (!ms) return "";
  const d = new Date(ms);
  const p = (n) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/// 每条任务显示过的最高进度：id -> { pct, done }
const barHigh = {};

/// 一行里会随进度变的数值。rowHtml 和就地更新共用，避免两处逻辑漂。
function subtitleState(t) {
  const note = String(t.subNote || t.shandianError || "");
  const known = t.subtitleState || "";
  if (t.status !== "done") return { label: "", tip: "", checked: false };
  if (/字幕/.test(t.stage || "")) return { label: "字幕获取中", tip: "正在检查并获取可用字幕", checked: true };
  if (known === "available") return { label: t.subs ? "" : "有字幕", tip: "已获取字幕", checked: true };
  if (!t.subs && note === "没有所选语言的字幕") return { label: "无所选语言", tip: note, checked: true };
  const none = !t.subs && (known === "none" || /^(?:这个视频)?没有(?:可用)?的?字幕[。！!]?$/u.test(note) || note === "无字幕");
  if (none) return { label: "无字幕", tip: "已检查，未找到可用字幕；可重查字幕", checked: true };
  if (known === "failed" || /字幕/.test(note)) return { label: t.subs ? "字幕未完整" : "字幕获取失败", tip: note.replace(/再点补字幕/g, "再点获取字幕") || "已检查但获取失败，请重试", checked: true };
  return { label: "", tip: t.subs ? "已获取字幕" : "", checked: !!t.subs };
}

function rowVals(t) {
  if (t.creating) return { pct:0, indet:true, name:t.name, subsText:"", mainVal:"正在准备", mainSpin:true, subSpin:false, subVal:"正在准备", subTip:"", sizeText:"", timeText:fmtTime(t.created), resText:"" };
  const total = t.meta?.res?.size || 0;
  const done = t.progress?.downloaded || 0;
  const run = t.status === "running";
  const collection = !!t.collectionFiles;
  const ratio = collection ? t.collectionProgress || 0 : total ? done / total : 0;
  // 进度只往前走：后端的总量已经是「所有轨之和」，但预计值仍可能小幅修正，
  // 不能让条子跟着来回抖。滚动条只留给「从开始到现在都不知道总量」这一种情况。
  const sizeUnknown = !total;
  const seen = barHigh[t.id];
  const checking = t.shandianVerification === "checking";
  const checkFailed = t.shandianVerification === "failed";
  const lastBit = !!lastPiece[t.id]?.waiting;
  const stall = lastBit ? null : stallOf(t);
  // 已下字节腰斩说明任务从头来过了（重试、换后缀重跑），这时才清零。校验时内核报的是核对进度，不算
  // BT 刚恢复时内核会短暂报 0（还没连上种子信息），那不是从头来过，进度条不能清空
  if (seen && !checking && done < seen.done * 0.5 && !(isBt(t) && done === 0)) delete barHigh[t.id];
  const high = collection ? 0 : barHigh[t.id]?.pct || 0;
  const waitingTransfer = run && t.meta?.req?.labels?.baiduWaitingTransfer === "true";
  const indet = run && (waitingTransfer || (sizeUnknown && !high && !collection));
  const pct = t.status === "done" ? 100 : waitingTransfer ? 0 : Math.max(high, Math.min(99, ratio * 100));
  if (!collection && t.status !== "done" && pct > 0 && !checking) barHigh[t.id] = { pct, done };
  const spd = run ? t.progress?.speed || 0 : 0;
  const etaSpd = smoothSpeed[t.id]?.v || spd;
  const eta = total && etaSpd ? remaining((total - done) / etaSpd) : "";
  const name = t.name || t.meta?.res?.name || "解析中…";
  const err = t.shandianError || "";
  const paused = t.status === "pause";
  const isErr = t.status === "error";

  let mainVal = STATUS[t.status] || t.status;
  let subVal = "";
  let subTip = "";
  if (run) {
    // 合并、处理字幕时没有速度可报，直接说在干什么（否则看着像卡死）
    mainVal = t.stage || (spd ? speed(spd) : "连接中");
    subVal = t.stage ? "" : eta || (pct ? `${Math.round(pct)}%` : "");
    if (checking) {
      mainVal = "正在校验";
      subVal = pct ? `${Math.round(pct)}%` : "";
    } else if (checkFailed) {
      mainVal = "校验失败";
      subVal = "右键可重查字幕";
    } else if (lastBit) {
      // 只差一点、进度又不动时别写「连接中」，照实说（Mac 11-01 用户反馈：卡住不动）
      mainVal = "收尾中";
      subVal = "漏了最后一小块，正在补";
    } else if (stall) {
      mainVal = stall.main;
      subVal = stall.sub;
    }
  } else if (isErr) {
    // 原因在下面的报错行，这里不再说一遍
    mainVal = "出错";
  } else if (t.status === "done") {
    mainVal = "已完成";
    // NAS 正在补字幕等后台步骤：写出来，免得点了没反应
    if (t.collectionPolishBusy) subVal = `润色字幕 ${t.collectionPolishBusy} 项`;
    else if (t.collectionPolishFailed) subVal = `润色失败 ${t.collectionPolishFailed} 项`;
    else if (t.collectionPolished) subVal = `字幕已润色 ${t.collectionPolished} 项`;
    else if (t.stage && !t.stage.includes("字幕")) subVal = t.stage;
    else
    // 文件已存在被 yt-dlp 跳过时要显示出来，否则像「秒下完成」
    if (err.includes("已跳过")) subVal = "文件已存在，已跳过";
  } else if (paused) {
    mainVal = "已暂停";
    subVal = pct ? `${Math.round(pct)}%` : "";
  } else if (t.collectionSubtitlePending) {
    subVal = `处理字幕 ${t.collectionSubtitlePending} 项`;
  } else if (total || collection) {
    subVal = `${Math.round(pct)}%`;
  }
  // 「连接中」不摆文字了，换 .spin 转圈：激活中的行（running/wait/ready）还没速度就转，
  // 非激活行照旧走上面的文字。只在速度 0↔非 0 之间来回变，所以别塞进结构 key，
  // updateRow 就地换节点就行
  // 启动头 3 秒内核还在恢复任务、速度还没测出来，这时别转圈——那几秒的「连接中」
  // 只是启动过程，用户看到的就是「一打开就在转」（旧版本刷新慢，没那么显眼）
  const settled = performance.now() - BOOT_AT > 3000;
  const mainSpin = !spd && !t.stage && !checking && !checkFailed && !lastBit && !stall && t.status !== "wait" && ACTIVE.has(t.status) && settled;
  const subSpin = t.status === "done" && isBaiduFileTask(t) && subVal === "正在校验本地文件…";
  // timeText 只在重建行时用（updateRow 不碰它）：created 不会变，放这儿不会让结构 key 抖
  // resText 是分辨率小字（契约：t.resolution，可能没有），下载中可能从空补上，updateRow 会跟着刷
  // 已完成时 done == total，写成「123 MB / 123 MB」等于同一个数报两遍，
  // 右边状态区还报第三遍——只留一个数
  // 总量不可信时只报已下：写成「284 MB / 1.21 MB」纯属给用户添乱
  const sizeText = t.status === "done"
    ? bytes(total || done)
    : `${bytes(done || (isBt(t) && barHigh[t.id]?.done) || 0)}${total && !sizeUnknown ? " / " + bytes(total) : ""}`;
  return { pct, indet, name, subsText: t.subs && subtitleState(t).label ? `${t.subs} · ${subtitleState(t).label}` : t.subs || subtitleState(t).label, mainVal, mainSpin, subSpin, subVal, subTip, sizeText, timeText: fmtTime(t.created), resText: t.resolution || "" };
}

/// 缩略图缓存：id -> data URI。
const thumbs = {};
/// 第一次要封面的记录：id -> { at, wasDone, retried, pending }。
/// 任务对象每次 list_tasks 都是新造的，记号得放这儿才不会每秒被冲掉。
/// YouTube 这类要等文件下完才截得到帧，第一次空手而归不算完，后面再补一次。
const thumbTried = {};

/// 封面画成元素自己的背景，底部压暗也是同一层：整张图只被圆角裁一次。
/// 以前是 overflow:hidden 里放 <img> 再叠一层渐变，每层各裁一次，弧线上的半透明像素
/// 叠出一圈残留。这样写外观不变（渐变的范围和深浅都和原来一样）。
function coverHtml(src) {
  return `<div class="thumb has-img" style="background-image:linear-gradient(transparent 40%,rgba(0,0,0,.55)),url('${src}')"></div>`;
}

function thumbHtml(id, name, kind, fallbackIcon) {
  const src = typeOfName(name) === "audio" ? null : thumbs[id];
  if (src) {
    // 有封面就只放封面：名字压在图上要垫一层半透明底才看得清，
    // 那层底在深色封面上一片发灰，很难看。名字在右边正文里本来就有。
    return coverHtml(src);
  }
  // 视频网站任务的 kind 是站点名（YouTube／哔哩哔哩…），不在 THUMB_ICON 里，
  // 原来一律掉到「文件」图标——明明是视频却顶着个文档。用 id 的 v: 前缀兜住。
  const icon = fallbackIcon || iconOfFile(name, kind) || THUMB_ICON[kind] || (String(id).startsWith("v:") || VIDEO_SITE_KINDS.has(kind) ? "#i-play" : "#i-doc");
  return `<div class="thumb"><svg class="ic"><use href="${icon}"/></svg><span class="thumb-text">${escapeHtml(name)}</span></div>`;
}

function taskThumbHtml(task, name, kind = task.kind) {
  const id = task.collectionFiles ? collectionCoverId(task) || task.id : task.id;
  const baiduBatch = isBaiduFileTask(task) && hasFileList(task);
  if (baiduBatch && thumbs[id]) return coverHtml(thumbs[id]);
  const isCollection = baiduBatch || task.collectionFiles || isGroup(task.id) || String(task.meta?.req?.url || "").startsWith("shandian-multi:");
  return thumbHtml(id, name, kind, isCollection ? "#i-collection" : undefined);
}

function collectionCoverId(task) {
  const covers = task.collectionFiles.filter(file => thumbs[file.taskId]);
  return (covers.find(file => !thumbIsFrame[file.taskId]) || covers[0])?.taskId;
}

function episodeCoverHtml(id) {
  const src = thumbs[id];
  return `<span class="fd-episode-cover" data-episode-cover="${escapeHtml(id || "")}">${src ? `<img src="${escapeHtml(src)}" alt="" loading="lazy">` : '<svg class="ic" aria-hidden="true"><use href="#i-play"/></svg>'}</span>`;
}

function baiduDrawerCover(id, file) {
  return isBaiduFileTask({ id }) && file.status === "done" && !file.missing && typeOfName(file.name) === "video"
    ? `${id}:${encodeURIComponent(file.name)}` : "";
}

function visibleDrawerFiles(files) {
  const scroll = $("files-scroll");
  const height = $("files-list").firstElementChild?.offsetHeight || 40;
  const start = Math.floor(scroll.scrollTop / height);
  const end = Math.ceil((scroll.scrollTop + scroll.clientHeight) / height) + 1;
  return files.slice(start, end);
}

function collectionThumbTargets() {
  const targets = new Map();
  const raw = new Map(rawTasks.map(task => [task.id, task]));
  for (const task of state.tasks) {
    if (!task.collectionFiles) {
      targets.set(task.id, task);
      if (isBaiduFileTask(task) && task.id === filesPanelId && task.status === "done") {
        for (const file of visibleDrawerFiles(filesList)) {
          const id = baiduDrawerCover(task.id, file);
          if (id) targets.set(id, { id, name: file.name, status: "done" });
        }
      }
      continue;
    }
    const files = task.collectionFiles;
    const available = files.filter(file => raw.has(file.taskId) && !file.missing && file.fileState !== "offline");
    // Keep a group's first artwork request in the same queue as individual covers.
    const cover = available.find(file => thumbs[file.taskId] && !thumbIsFrame[file.taskId])
      || available.find(file => !thumbTried[file.taskId]) || available[0];
    if (cover) targets.set(cover.taskId, raw.get(cover.taskId));
    if (task.id !== filesPanelId) continue;
    for (const file of visibleDrawerFiles(files)) {
      if (raw.has(file.taskId) && !file.missing && file.fileState !== "offline") {
        targets.set(file.taskId, raw.get(file.taskId));
      }
    }
  }
  return targets.values();
}

/// 直链按文件类型显示图标；磁力、种子、视频站保留原来的
function iconOfFile(name, kind) {
  // 文件格式优先：Pixabay、爱给等素材来源同样显示音频图标。
  if (typeOfName(name) === "audio") return "#i-music";
  if (kind && kind !== "直链" && !String(kind).endsWith("· 直链") && !/网盘|云盘|蓝奏云|PikPak|GitHub|Gitee|Dropbox|Google Drive|OneDrive|MEGA/.test(kind)) return null;
  const ext = (String(name || "").match(/\.([a-z0-9]{1,8})$/i)?.[1] || "").toLowerCase();
  if (/^(zip|rar|7z|tar|gz|bz2|xz|tgz|iso|cab)$/.test(ext)) return "#i-zip";
  if (/^(exe|msi|dmg|pkg|apk|deb|rpm|appimage|msix)$/.test(ext)) return "#i-app";
  if (/^(mp3|flac|wav|aac|m4a|ogg|opus|ape|wma)$/.test(ext)) return "#i-music";
  if (/^(jpg|jpeg|png|gif|webp|bmp|svg|heic|tif|tiff|exr|hdr)$/.test(ext)) return "#i-image";
  if (/^(mp4|mkv|avi|mov|flv|wmv|webm|m4v|ts|rmvb|mpg|mpeg)$/.test(ext)) return "#i-play";
  return "#i-doc";
}

/// 已经按片名查过海报的任务
const posterTried = new Set();

/// 显示的是截帧替补的任务：还要继续要原封面
const thumbIsFrame = {};

function showThumb(t, r) {
  thumbs[t.id] = r.src;
  thumbIsFrame[t.id] = !r.cover;
  const row = document.querySelector(`.task[data-id="${cssEscape(t.id)}"] .thumb`);
  if (row) row.outerHTML = taskThumbHtml(t, t.name || "");
  const episode = $("files-list")?.querySelector(`[data-episode-cover="${cssEscape(t.id)}"]`);
  if (episode) episode.outerHTML = episodeCoverHtml(t.id);
  for (const group of state.tasks) {
    if (!group.collectionFiles || collectionCoverId(group) !== t.id) continue;
    const parent = document.querySelector(`.task[data-id="${cssEscape(group.id)}"] .thumb`);
    if (parent) parent.outerHTML = taskThumbHtml(group, group.name || "");
  }
}

/// 后台慢慢补图：回来了再重画那一行，不整表重排。
/// 第一次没拿到的，等任务下完再补拉一次；第一次要的时候就已经「已完成」
/// （文件可能还在合并）的，等 30 秒再补一次。各补一次就认了，不每秒重试。
async function fillThumbs() {
  if (!invoke) return;
  for (const t of collectionThumbTargets()) {
    if (t.creating) continue;
    if (isBaiduFileTask(t) ? t.status !== "done" || t.missing
      : typeOfName(t.name || t.meta?.opts?.name || t.meta?.res?.name) === "audio") continue;
    // 种子下完再要一次：开始时只能查豆瓣，自带的海报要下完才拿得到
    if (thumbs[t.id] && !thumbIsFrame[t.id] && t.status === "done" && posterTried.has(t.id)) {
      posterTried.delete(t.id);
      invoke("thumb_get", { id: t.id, allowFrame: false })
        .then((r) => { if (r) showThumb(t, r); })
        .catch(() => {});
      continue;
    }
    if (thumbs[t.id] && !thumbIsFrame[t.id]) continue; // 已经有原封面了
    let tr = thumbTried[t.id];
    const fromSite = /^(v:|nas:v-)/.test(String(t.id));
    if (tr) {
      if (tr.pending) continue; // 上一次还没回来
      // 暂停后继续：yt-dlp 会重新报封面地址，回到快速重试
      if (t.status === "running" && tr.lastStatus !== "running") tr.tries = 0;
      tr.lastStatus = t.status;
      if (fromSite && t.status !== "done") {
        // 视频站的封面图是另外去抓的（几百毫秒到几秒），下载中第一次空手很正常。
        // 头半分钟每轮都要（解析页面就这么久），之后放慢，下载没结束就一直有机会出图
        if (Date.now() - tr.at < ((tr.tries || 0) < 40 ? 700 : 5000)) continue;
      } else if (fromSite) {
        // 下完了还是原封面没到（代理没开、站点抽风）：每分钟再要一次，到了就把截帧换掉
        const n = tr.upgrades || 0;
        if (n >= 10 || (n > 0 && Date.now() - tr.at < 60000)) continue;
        tr.upgrades = n + 1;
      } else if (tr.retried) {
        continue; // 非视频站，或者已经补过一轮
      } else {
        // 下载中第一次空手的，看到「已完成」就补；当时就已完成的状态不会再变，等 30 秒补
        const ready = tr.wasDone ? Date.now() - tr.at > 30000 : t.status === "done";
        if (!ready) continue;
        tr.retried = true;
      }
    } else {
      const done = t.status === "done";
      const total = t.meta?.res?.size || 0;
      const pct = total ? (t.progress?.downloaded || 0) / total : 1;
      // 下载中的文件头常还没写到，截帧大概率是黑屏或失败；下到 8% 或已完成再要
      // 视频站的封面是站点给的图，不用等文件，开始下就能要
      if (!fromSite && !done && pct < 0.08) {
        // 磁力／种子的海报按片名查，解析出资源名就能要，同样不用等文件（只查一次）
        if (["磁力", "种子文件"].includes(t.kind) && t.meta?.res?.name && !posterTried.has(t.id)) {
          posterTried.add(t.id);
          invoke("thumb_get", { id: t.id, allowFrame: false })
            .then((r) => { if (r) showThumb(t, r); })
            .catch(() => {});
        }
        continue;
      }
      tr = thumbTried[t.id] = { at: Date.now(), wasDone: done, retried: false, tries: 0 };
    }
    tr.at = Date.now();
    tr.tries = (tr.tries || 0) + 1;
    tr.pending = true;
    invoke("thumb_get", { id: t.id, allowFrame: true })
      .then((r) => {
        tr.pending = false;
        if (r) showThumb(t, r); // 这次也没拿到：靠上面的补拉规则再试一次
      })
      .catch(() => { tr.pending = false; });
  }
}

function cssEscape(s) {
  return String(s).replace(/["\\]/g, "\\$&");
}

/// 工具栏筛选按钮左侧显示匹配项数；不占用搜索框
function setSearchCount(n) {
  const el = $("search-count");
  if (!el) return;
  el.hidden = n == null;
  el.textContent = n == null ? "" : `${n} 项匹配`;
}

function downloadEmptyMarkup(watchClipboard) {
  const linkHint = watchClipboard
    ? "复制链接自动检测"
    : "复制链接后按 Ctrl+N 新建下载";
  return `<section class="download-empty" aria-labelledby="download-empty-title">
  <svg class="download-empty-outline" aria-hidden="true" focusable="false"><rect x="0.6" y="0.6" rx="21.4"/></svg>
  <svg class="download-empty-glyph" aria-hidden="true" viewBox="0 0 424 680"><use href="#i-bolt"/></svg>
  <h2 id="download-empty-title">开始下载</h2>
  <p class="download-empty-hint">网页闪电按钮 · 扩展侦测 · ${linkHint} · 拖入种子</p>
  <ul class="download-empty-sources" aria-label="下载来源与入口">
    <li>直链</li><li>磁力</li><li>种子</li>
    <li title="支持合集选集和可用字幕">哔哩哔哩</li>
    <li title="支持合集选集和可用字幕">YouTube</li>
    <li>GitHub</li><li>Hugging Face</li><li>Vimeo</li><li>Pixabay</li>
    <li title="粘贴百度网盘分享链接">百度分享</li>
    <li title="在夸克、阿里云盘、UC、123 云盘等网页用扩展选择文件">扩展选文件</li>
    <li>网页音视频</li>
    <li title="通过扩展查找网页提供的 VTT、SRT 等字幕文件">字幕文件</li>
    <li title="关联 PikPak 后可离线中转">PikPak 离线</li>
    <li>爱给</li><li>轻雀文档</li><li>飞书文档</li><li>抖音</li>
    <li>小红书</li><li>微信视频号</li><li>快手</li><li>微博</li>
    <li>新片场</li><li>ed2k</li>
  </ul>
</section>`;
}

function renderTasks() {
  if (rowDragging) return; // 拖动中别重排，松手再画
  const box = $("task-list");
  const q = state.query.trim().toLowerCase();

  // 下载历史：单独一套行（标题/类型/大小/移除时间/文件状态 + 重新下载/打开/删除记录）
  if (state.view === "trash") {
    renderTrash(box, q);
    return;
  }

  let list = state.tasks.filter(taskVisibleAfterRemove).filter(matches).filter(filterTask);
  // 过滤用的显示名要和 rowHtml / 高亮一致：t.name 可能为空，标题在 meta.res.name
  if (q) {
    list = list.filter((t) => {
      const n = (t.name || t.meta?.res?.name || "").toLowerCase();
      return n.includes(q);
    });
  }
  setSearchCount(q ? list.length : null);

  // 分类页里进行中在前、已完成在后，和 Mac 版资料库页一致
  // 进行中在前，按拖出来的顺序；已完成按顶部选的排序，默认新下完的在最上面
  const cmpDone = doneComparer();
  syncCloudSpots();
  const rank = queueRank();
  const rankOf = (t) => rank.get(t.id) ?? Infinity;
  list.sort((a, b) => {
    const da = a.status === "done", db = b.status === "done";
    if (da !== db) return Number(da) - Number(db);
    if (da) return cmpDone(a, b);
    const ra = rankOf(a), rb = rankOf(b);
    return ra === rb ? 0 : ra - rb;
  });

  $("sec-title").textContent = state.view === "done" ? "已完成" : "进行中";
  // 空状态时不显示「进行中」这个小标题，和 Mac 版一致
  $("sec-title").parentElement.hidden = list.length === 0;
  // 空状态要在速度卡下方的剩余高度里垂直居中，靠 .scroll-empty 把列表区拉满
  box.closest(".scroll")?.classList.toggle("scroll-empty", list.length === 0);
  // 大速度卡片只在正在下载页显示，其它页面在标题旁显示紧凑速度。
  const filtered = filterState.sources.size || filterState.types.size;
  const downloadEmpty = !list.length && state.view === "all" && !q && !filtered;
  $("hero").hidden = downloadEmpty || (state.view !== "all");
  syncSpeedVisibility(state.tasks.some(t=>ACTIVE.has(t.status)));

  if (!list.length) {
    if (downloadEmpty) {
      const watchClipboard = !!state.settings?.settings?.watchClipboard;
      const emptyKey = watchClipboard ? "download-empty-clipboard" : "download-empty-manual";
      if (box.__key !== emptyKey) box.innerHTML = downloadEmptyMarkup(watchClipboard);
      box.__key = emptyKey;
      syncVisibleSelection();
      return;
    }
    const text =
      state.view === "done" ? ["还没有下完的东西", "下完之后会归到这里"]
      : q ? ["没有匹配的任务", "换个关键词试试"]
      : state.view === "all" && filtered ? ["没有符合筛选条件的任务", "可在筛选菜单中调整条件"]
      : ["粘贴链接开始下载", "磁力、种子、ed2k、直链都行"];
    box.innerHTML = `<div class="empty">
      <span class="glyph"><svg viewBox="0 0 424 680"><use href="#i-bolt"/></svg></span>
      <b>${text[0]}</b><small>${text[1]}</small>
    </div>`;
    box.__key = "";
    syncVisibleSelection();
    return;
  }
  // 结构（id/状态/名字/按钮组合）没变就只更新数值，不重建 DOM。
  // 之前每秒 innerHTML 重画一遍，hover 显现的按钮和角标跟着反复重置，
  // 鼠标停在行上就一直闪（用户反馈）。
  // key 必须含搜索词：命中同一批任务时只换关键字，name 不变，不重排就改不掉高亮
  const key =
    q + "|" +
    list.map((t) => `${t.id}|${t.status}|${t.name || ""}|${canPlay(t) ? 1 : 0}|${t.missing ? 1 : 0}|${hasFileList(t) ? 1 : 0}`).join(";");
  if (box.__key === key && box.children.length === list.length) {
    list.forEach((t, i) => updateRow(box.children[i], t));
    return;
  }
  // 重建会把焦点和选中态一起冲掉，先记下焦点在哪行，画完接回去，不然键盘用着用着就失焦
  const focusId = document.activeElement?.closest?.("#task-list .task")?.dataset.id;
  box.innerHTML = list.map(rowHtml).join("");
  box.__key = key;
  syncVisibleSelection();
  if (focusId) box.querySelector(`.task[data-id="${cssEscape(focusId)}"]`)?.focus();
}

// ---------- 排序 ----------

const SORTS = [
  { key: "time", label: "时间", dir: -1 },
  { key: "name", label: "名称", dir: 1 },
  { key: "size", label: "大小", dir: -1 },
  { key: "kind", label: "类型", dir: 1 },
];
let sortPref = { key: "time", dir: -1 };
try {
  const saved = JSON.parse(localStorage.getItem("sd.sort") || "null");
  if (saved && SORTS.some((s) => s.key === saved.key) && Math.abs(saved.dir) === 1) sortPref = saved;
} catch (_) {}

function syncSortIcon() {
  $("sort-btn")?.classList.toggle("asc", sortPref.dir > 0);
}
syncSortIcon();

const textOrder = new Intl.Collator("zh-CN", { numeric: true, sensitivity: "base" });
/// 每种排序怎么比。同值时按时间新的在前，顺序才稳定
function comparer(get) {
  const { key, dir } = sortPref;
  const f = get[key];
  return (a, b) => {
    const x = f(a), y = f(b);
    const c = typeof x === "string" ? textOrder.compare(x, y) : x - y;
    return c ? c * dir : get.time(b) - get.time(a);
  };
}
function doneComparer() {
  return comparer({
    time: (t) => t.finished || t.created || 0,
    name: (t) => t.name || t.meta?.res?.name || "",
    size: (t) => t.meta?.res?.size || t.progress?.downloaded || 0,
    kind: (t) => t.kind || "",
  });
}
function sortTrash(list) {
  return list.sort(comparer({
    time: (r) => Number(new Date(r.removedAt || 0)) || 0,
    name: (r) => r.title || "",
    size: (r) => r.size || 0,
    kind: (r) => r.origin || "",
  }));
}

// 按下时菜单已经被上面的 pointerdown 收掉了，先记住按之前是不是开着，点按钮才能开关切换
let sortMenuOpen = false;
document.addEventListener("pointerdown", (e) => {
  if (e.target.closest("#sort-btn")) sortMenuOpen = !$("ctx-menu").hidden && $("ctx-menu").dataset.owner === "sort";
}, true);
$("sort-btn")?.addEventListener("click", () => {
  const menu = $("ctx-menu");
  if (sortMenuOpen) {
    sortMenuOpen = false;
    return;
  }
  menu.dataset.owner = "sort";
  menu.innerHTML = "";
  for (const s of SORTS) {
    const b = document.createElement("button");
    b.className = "ctx-item";
    b.textContent = s.label;
    if (s.key === sortPref.key) {
      const mark = document.createElement("span");
      mark.className = "sort-mark";
      mark.textContent = sortPref.dir < 0 ? "↓" : "↑";
      b.append(mark);
    }
    b.addEventListener("click", () => {
      // 再点当前项就反过来
      sortPref = s.key === sortPref.key ? { key: s.key, dir: -sortPref.dir } : { key: s.key, dir: s.dir };
      try { localStorage.setItem("sd.sort", JSON.stringify(sortPref)); } catch (_) {}
      syncSortIcon();
      closeCtx();
      $("task-list").__key = "";
      renderTasks();
    });
    menu.append(b);
  }
  menu.hidden = false;
  const r = $("sort-btn").getBoundingClientRect();
  menu.style.left = `${Math.round(Math.min(r.left, innerWidth - menu.offsetWidth - 10))}px`;
  menu.style.top = `${Math.round(r.bottom + 6)}px`;
});

// ---------- 拖动调整下载顺序 ----------
// Tauri 接管了系统拖放，HTML5 的 dragstart 在 WebView2 里收不到，所以用鼠标事件自己做。

let rowDragging = false;
const DRAG_MAX_CARDS = 10;
let queueOrder = [];
try { queueOrder = JSON.parse(localStorage.getItem("sd.queueOrder") || "[]"); } catch (_) {}
if (!Array.isArray(queueOrder)) queueOrder = [];
function queueRank() {
  return new Map(queueOrder.map((id, i) => [id, i]));
}

/// 任务换了下载方式（迅雷⇄本机）时，新任务留在原任务的位置。操作前调用，完成后调用返回的函数
function keepSpot(originId, url) {
  const before = state.tasks.filter((t) => t.status !== "done").sort((a, b) => (queueRank().get(a.id) ?? Infinity) - (queueRank().get(b.id) ?? Infinity)).map((t) => t.id);
  return async () => {
    await refreshTasks();
    const next = state.tasks.find((t) => t.id !== originId && t.status !== "done" && t.meta?.req?.url === url && !before.includes(t.id));
    if (next) commitQueueOrder(before.map((id) => (id === originId ? next.id : id)));
  };
}

/// 交给云盘的任务留在原来的位置：云端那一行接替原任务，拉回的任务再接替云端那一行
function keepCloudSpot(originId, url) {
  const spot = () => state.tasks.filter((t) => t.status !== "done").sort((a, b) => (queueRank().get(a.id) ?? Infinity) - (queueRank().get(b.id) ?? Infinity)).map((t) => t.id);
  const before = spot();
  return async (replacementId) => {
    await refreshTasks();
    const cloud = state.tasks.find((t) => t.id === replacementId || /^(?:nas:)?(?:pp:|cloud:)/.test(String(t.id)) && t.meta?.req?.url === url);
    if (cloud) commitQueueOrder(before.map((id) => (id === originId ? cloud.id : id)));
  };
}

let cloudSpotBusy = false;
async function syncCloudSpots() {
  const gone = queueOrder.filter((id) => id.startsWith("pp:") && !state.tasks.some((t) => t.id === id));
  if (!gone.length || cloudSpotBusy) return;
  cloudSpotBusy = true;
  try {
    const pulled = await invoke("pikpak_pulled");
    commitQueueOrder(queueOrder.flatMap((id) => (gone.includes(id) ? pulled[id.slice(3)] || [] : [id])));
  } catch {} finally {
    cloudSpotBusy = false;
  }
}

function commitQueueOrder(ids) {
  queueOrder = ids;
  try { localStorage.setItem("sd.queueOrder", JSON.stringify(ids)); } catch (_) {}
  $("task-list").__key = "";
  renderTasks();
  invoke?.("set_queue_order", { ids }).catch((e) => toast("调整顺序失败：" + e));
}

$("task-list").addEventListener("pointerdown", (e) => {
  if (e.button !== 0 || state.view !== "all" || state.query.trim()) return;
  if (e.ctrlKey || e.metaKey || e.shiftKey) return;
  if (e.target.closest("button, input, a, [data-act], [data-subtitle-action]")) return;
  const row = e.target.closest(".task");
  if (!row) return;
  const box = $("task-list");
  const scroller = box.closest(".scroll");
  const startY = e.clientY;
  const id = row.dataset.id;
  const rows = [...box.querySelectorAll(".task")];
  const from = rows.indexOf(row);
  if (rows.length < 2) return;
  // 按在选中的行上，选中的几条一起走，保持它们原来的先后
  const group = state.selIds.has(id) && state.selIds.size > 1 ? rows.filter((r) => state.selIds.has(r.dataset.id)) : [row];
  const rest = rows.filter((r) => !group.includes(r));
  if (!rest.length) return;
  let ghost = null;
  let target = from;
  let moved = false;
  let grabDy = 0;
  let firstCenter = 0;
  let step = 0;
  let height = 0;
  let scroll0 = 0;
  let lastY = startY;
  let scrollTimer = 0;
  let stackTop = 0; // 手指抓的那张卡片之上还有几张卡片的高度

  // 被抓的行之外，其他行滑开让出空位
  const layout = () => {
    const center = lastY - grabDy - stackTop + height / 2;
    const offset = scroller.scrollTop - scroll0;
    target = Math.max(0, Math.min(rest.length, Math.round((center - firstCenter + offset) / step)));
    // 其余的行按顺序排，在落点处空出 group.length 个位置
    rest.forEach((r, rank) => {
      const shift = ((rank < target ? rank : rank + group.length) - rows.indexOf(r)) * step;
      r.style.transform = shift ? `translateY(${shift}px)` : "";
    });
  };
  const lift = () => {
    moved = true;
    rowDragging = true;
    closeCtx();
    hideTip();
    document.body.style.userSelect = "none";
    const r = row.getBoundingClientRect();
    height = r.height;
    grabDy = startY - r.top;
    scroll0 = scroller.scrollTop;
    firstCenter = rows[0].getBoundingClientRect().top + height / 2;
    step = rows[1].getBoundingClientRect().top - rows[0].getBoundingClientRect().top;
    // 拖着的就是真实的卡片，最多叠 10 张，再多看数字
    const shown = group.slice(0, DRAG_MAX_CARDS);
    stackTop = Math.max(0, shown.indexOf(row)) * step;
    ghost = document.createElement("div");
    ghost.className = "drag-stack";
    for (const source of shown) {
      const card = source.cloneNode(true);
      card.removeAttribute("data-id");
      card.removeAttribute("tabindex");
      card.classList.remove("sel");
      card.classList.add("drag-ghost");
      card.style.marginBottom = `${step - height}px`;
      ghost.append(card);
    }
    if (group.length > DRAG_MAX_CARDS) {
      const count = document.createElement("span");
      count.className = "drag-count";
      count.textContent = group.length;
      ghost.append(count);
    }
    ghost.style.left = `${r.left}px`;
    ghost.style.width = `${r.width}px`;
    ghost.style.top = `${r.top - stackTop}px`;
    document.body.append(ghost);
    group.forEach((x) => x.classList.add("drag-source"));
    rows.forEach((x) => x.classList.add("drag-shift"));
    // 贴着列表上下边缘时自动滚动
    scrollTimer = setInterval(() => {
      const sr = scroller.getBoundingClientRect();
      if (lastY < sr.top + 40) scroller.scrollTop -= 8;
      else if (lastY > sr.bottom - 40) scroller.scrollTop += 8;
      else return;
      layout();
    }, 16);
  };
  const onMove = (ev) => {
    lastY = ev.clientY;
    if (!moved && Math.abs(lastY - startY) < 5) return;
    if (!moved) lift();
    ghost.style.top = `${lastY - grabDy - stackTop}px`;
    layout();
  };
  const finish = (commit) => {
    removeEventListener("pointermove", onMove);
    removeEventListener("pointerup", onUp);
    removeEventListener("pointercancel", onCancel);
    if (!moved) return;
    clearInterval(scrollTimer);
    // 松手后浏览器可能补一个 click，别让它把选择改掉。也可能不补，所以很快撤掉，不能吃下一次点击
    const eat = (ce) => { ce.stopPropagation(); ce.preventDefault(); };
    addEventListener("click", eat, { capture: true, once: true });
    setTimeout(() => removeEventListener("click", eat, { capture: true }), 50);
    // 落进空位，动画结束再真正换顺序，不会闪
    const dest = commit ? target : rows.indexOf(group[0]);
    const offset = scroller.scrollTop - scroll0;
    ghost.classList.add("settling");
    ghost.style.top = `${firstCenter - height / 2 + dest * step - offset}px`;
    setTimeout(() => {
      ghost.remove();
      document.body.style.userSelect = "";
      rows.forEach((x) => {
        x.classList.remove("drag-shift", "drag-source");
        x.style.transform = "";
      });
      rowDragging = false;
      if (!commit) return;
      const ids = rest.map((r) => r.dataset.id);
      ids.splice(dest, 0, ...group.map((r) => r.dataset.id));
      if (ids.join() !== rows.map((r) => r.dataset.id).join()) commitQueueOrder(ids);
    }, 150);
  };
  const onUp = () => finish(true);
  const onCancel = () => finish(false);
  addEventListener("pointermove", onMove);
  addEventListener("pointerup", onUp);
  addEventListener("pointercancel", onCancel);
});

/// 下载历史列表（标题 / 类型·大小·时间·文件状态 + 三个圆钮）
function renderTrash(box, q) {
  $("hero").hidden = true;
  syncSpeedVisibility(state.tasks.some(t=>ACTIVE.has(t.status)));
  $("sec-title").parentElement.hidden = true;
  box.closest(".scroll")?.classList.add("scroll-empty");
  let list = sortTrash(state.trash.slice()).filter((r) => filterHit(r.id, r.link || "", r.title || "", /^https?:/i.test(r.link || "") && !/\.[a-z0-9]{2,5}($|\?)/i.test(r.link), /^magnet:|\.torrent/i.test(r.link || ""), []));
  if (q) {
    list = list.filter((r) =>
      [r.title, r.origin, r.link].some((s) => String(s || "").toLowerCase().includes(q))
    );
  }
  setSearchCount(q ? list.length : null);
  if (!list.length) {
    const title = trashLoad && !trashLoaded ? "正在读取下载历史…" : trashLoadError ? "暂时无法读取下载历史" : "下载历史是空的";
    const detail = trashLoadError || (trashLoad && !trashLoaded ? "" : "归档的任务会先放这里，可以重新下载");
    box.innerHTML = `<div class="empty">
      <span class="glyph"><svg viewBox="0 0 424 680"><use href="#i-bolt"/></svg></span>
      <b>${escapeHtml(title)}</b><small>${escapeHtml(detail)}</small>
    </div>`;
    box.__key = "";
    syncVisibleSelection();
    return;
  }
  const key = list.map((r) => `${r.id}|${r.fileState}|${r.size}|${r.groupCount || 0}`).join(";");
  if (box.__key === key && box.children.length === list.length) return;
  box.innerHTML = list.map(trashRowHtml).join("");
  box.__key = key;
  syncVisibleSelection();
  loadTrashThumbs();
}

function canOpenHistory(record) {
  return !!record?.path && (record.fileState === "here" || record.fileState === "unknown" && !record.filesTrashed);
}

async function openHistoryLocation(record, openFile = false) {
  if (!canOpenHistory(record)) return toast("找不到这条记录的原保存位置");
  const stop = toast(openFile ? "正在打开文件…" : "正在打开文件夹…");
  try {
    const path = String(record.id).startsWith("nas:")
      ? await invoke("nas_history_path", { id: record.id })
      : record.path;
    await invoke(openFile ? "open_path" : "trash_open_path", { path });
  } catch (error) {
    toast(String(error));
  } finally {
    stop();
  }
}

function trashRowHtml(r) {
  const t = new Date(r.removedAt || 0);
  const when = isNaN(t) ? "" : t.toLocaleString("zh-CN", { hour12: false, month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" });
  // 当前检查结果优先于移除时记录；设备离线不能当作文件丢失。
  const fileGone = !["here", "offline"].includes(r.fileState) && (r.filesTrashed || r.fileState === "gone");
  const fileClass = fileGone ? "gone" : r.fileState === "here" ? "present" : r.fileState === "offline" ? "offline" : "unconfirmed";
  const fileNote =
    r.fileState === "here" ? "文件在原位置"
    : r.fileState === "offline" ? "存储设备未连接"
    : fileGone ? "文件不在原位置"
    : "文件位置未确认";
  const fileTip = fileGone ? "原路径未找到文件，可能已移动、重命名或删除。"
    : r.fileState === "offline" ? "连接文件所在的磁盘或网络存储后再检查。"
    : r.fileState === "here" ? ""
    : String(r.id).startsWith("nas:") ? "NAS 下载历史尚未核实文件当前位置，不代表文件已丢失。"
    : r.path ? "暂未确认文件是否仍在原路径，不代表文件已丢失。" : "这条历史未记录文件路径，无法确认文件位置。";
  const size = r.size > 0 ? bytes(r.size) : "";
  const count = r.groupCount || r.multi?.files?.length || 0;
  const name = r.title || "未命名";
  const kind = r.origin || "";
  const isCollection = r.multi || String(r.id).startsWith("hgrp:") || String(r.link || "").startsWith("shandian-multi:");
  const icon = isCollection ? "#i-collection" : iconOfFile(name, kind) || THUMB_ICON[kind] || "#i-play";
  // 缓存还在 thumbs/<taskId>.jpg：有就贴图，没有就黄底占位（和普通任务行同一套）
  const src = r.taskId ? trashThumbs[r.taskId] : null;
  const thumb = src && typeOfName(name) !== "audio"
    ? coverHtml(src)
    : `<div class="thumb"><svg class="ic"><use href="${icon}"/></svg><span class="thumb-text">${escapeHtml(name)}</span></div>`;
  return `<div class="task trash-row${fileGone ? " missing" : ""}" data-id="${escapeHtml(r.id)}" data-trash-id="${escapeHtml(r.id)}" tabindex="0">
    ${thumb}
    <div class="task-body">
      <div class="task-name" title="${escapeHtml(r.link || name)}">${escapeHtml(name)}</div>
      <div class="task-meta">
        ${kind ? `<span class="kind">${escapeHtml(kind)}</span>` : ""}
        ${r.resolution ? `<span class="res">${escapeHtml(r.resolution)}</span>` : ""}
        ${r.subs ? `<span class="subs">${escapeHtml(r.subs)}</span>` : ""}
        ${size ? `<span class="sz">${size}</span>` : ""}
        ${count ? `<span class="sz">共 ${count} 项</span>` : ""}
        ${when ? `<span class="tm">${escapeHtml(when)} 归档</span>` : ""}
      </div>
      <div class="task-file-note ${fileClass}"${fileTip ? ` data-tip="${escapeHtml(fileTip)}"` : ""}>${escapeHtml(fileNote)}</div>
    </div>
    <div class="task-tail">
      <div class="task-acts">
        ${r.link ? `<button class="circle sm" data-trash-act="redl" title="重新下载"><svg class="ic"><use href="#i-dl-box"/></svg></button>` : ""}
        ${canOpenHistory(r) ? `<button class="circle sm" data-trash-act="open" title="打开所在文件夹"><svg class="ic"><use href="#i-folder"/></svg></button>` : ""}
        <button class="circle sm" data-trash-act="del" title="删除历史记录"><svg class="ic"><use href="#i-trash"/></svg></button>
      </div>
    </div>
  </div>`;
}

/// 回收站封面缓存：taskId -> data URI。和主列表的 thumbs 分开，避免 id 冲突。
const trashThumbs = {};

async function loadTrashThumbs() {
  if (!invoke) return;
  for (const r of state.trash) {
    const tid = r.taskId;
    if (!tid || trashThumbs[tid]) continue;
    try {
      const src = await invoke("trash_thumb", { taskId: tid });
      if (src) {
        trashThumbs[tid] = src;
        // 就地换掉这一行的封面，不整表重排
        const row = document.querySelector(`.task[data-trash-id="${cssEscape(r.id)}"] .thumb`);
        if (row) {
          row.outerHTML = coverHtml(src);
        }
      }
    } catch (e) { /* 没缓存就保持黄底占位 */ }
  }
}

/// 就地刷新一行的动态数值（进度条宽度、字节数、分辨率小字、右侧状态文字），不动按钮和角标
function updateRow(row, t) {
  if (!row || !row.classList.contains("task")) return;
  const v = rowVals(t);
  const origin = row.querySelector(".task-meta .kind");
  const originText = t.originLabel || taskKind(t);
  if (origin && origin.textContent !== originText) origin.textContent = originText;
  const barBox = row.querySelector(".bar");
  const bar = row.querySelector(".bar > i");
  if (barBox) barBox.classList.toggle("indet", !!v.indet);
  if (bar) {
    // 不确定态让 CSS 动画控制宽度，别在上面写 style 打架
    if (v.indet) bar.removeAttribute("style");
    else bar.style.width = v.pct + "%";
  }
  const res = row.querySelector(".task-meta .res");
  if (res) {
    // 分辨率下载中可能从空变成 720P，就地填上不闪整行；没值就连空隙一起藏起来。
    // 老结构里没画 .res 的这里直接跳过
    res.textContent = v.resText;
    res.style.display = v.resText ? "" : "none";
  }
  const subtitles = row.querySelector(".task-subtitles");
  if (subtitles) {
    const markup = renderSubtitleControls(t);
    if (subtitles.innerHTML !== markup) {
      const focus = subtitles.contains(document.activeElement) ? document.activeElement.dataset.subtitleAction : null;
      subtitles.innerHTML = markup;
      if (focus) subtitles.querySelector(`[data-subtitle-action="${CSS.escape(focus)}"]`)?.focus({ preventScroll: true });
    }
  }
  const sz = row.querySelector(".task-meta .sz");
  if (sz) sz.textContent = v.sizeText;
  if (isBaiduFileTask(t)) {
    const chip = row.querySelector(".files-chip span");
    if (chip) chip.textContent = baiduFileProgress(t);
  }
  if (t.collectionFiles) {
    const count = row.querySelector(".files-chip span");
    const label = fileChipText(t);
    if (count && count.textContent !== label) count.textContent = label;
    const missing = row.querySelector(".collection-missing");
    if (missing) {
      missing.hidden = !t.collectionMissing;
      missing.textContent = t.collectionMissing + " 项缺失";
    }
  }
  row.querySelector(".task-right")?.classList.toggle("spinning", v.mainSpin);
  updateTaskStatus(row.querySelector(".main-val"), v.mainSpin ? "" : v.mainVal, v.mainSpin);
  const sub = row.querySelector(".sub-val");
  if (sub) {
    updateTaskStatus(sub, v.subVal, v.subSpin);
    if (v.subTip) sub.dataset.tip = v.subTip;
    else delete sub.dataset.tip;
  }
  const errEl = row.querySelector(".err-text");
  if (errEl) {
    const text = errSummary(t.shandianError);
    if (errEl.textContent !== text) errEl.textContent = text;
  }
}

function syncSpeedVisibility(hasRunning) {
  const hidden=!$("hero").hidden || !hasRunning;
  $("top-speed").hidden=hidden;
  $("top-speed-group").hidden=hidden;
}

function renderSpeed() {
  const visibleTasks = state.tasks.filter(taskVisibleAfterRemove);
  const running = visibleTasks.filter((t) => ACTIVE.has(t.status));
  const total = running.reduce((a, t) => a + (t.progress?.speed || 0), 0);
  const [num, unit] = speedParts(total);
  $("speed-num").textContent = num;
  $("speed-unit").textContent = unit;
  const compact = $("top-speed");
  if (compact) {
    syncSpeedVisibility(running.length > 0);
    if (!compact.hidden) {
      compact.textContent = `↓ ${num} ${unit}`;
      compact.dataset.tip = `所有活动任务的总下载速度：${num} ${unit}`;
    }
  }

  const failed = visibleTasks.filter((t) => t.status === "error").length;
  const stats = [];
  if (running.length) stats.push(`${running.length} 项进行中`);
  if (failed) stats.push(`${failed} 项失败`);
  const all = etaAll(running);
  const eta = all ? remaining(all.sec).replace("剩余 ", "") : "";
  if (eta) stats.push(eta === "遥遥无期" ? eta : all.partial ? `至少还要 ${eta}` : `预计 ${eta}后全部完成`);
  if (!stats.length) stats.push(state.creatingTasks.size ? "正在准备下载" : visibleTasks.length ? "没有下载任务" : "还没有任务");
  $("hero-stats").innerHTML = stats.map(escapeHtml).join("<span>·</span>");
}

function renderAll() {
  renderSidebar();
  renderTasks();
  renderSpeed();
  ensureSparkLoop();
}

// ---------- 设置 ----------

const DIR_LABEL = { movie: "磁力、种子的位置", video: "视频网站的位置", other: "其他文件的位置" };

/// 本机迅雷的状态：右键菜单和设置页都看它。没装时菜单不显示这一项，设置页写明
async function loadThunder() {
  thunderInfo = await invoke("thunder_status").catch(() => thunderInfo);
  const el = $("thunder-state");
  if (el) el.textContent = thunderInfo.installed ? "已检测到迅雷" : "未检测到迅雷";
}

function renderSettings() {
  if (!state.settings) return;
  for (const key of ["movie", "video", "other"]) {
    const d = state.settings[key];
    const el = $("dir-" + key);
    el.textContent = d.display;
    el.title = d.path;
    // 「恢复默认」只在改过之后才出现，跟 Mac 版一致
    document.querySelector(`[data-reset="${key}"]`).hidden = d.isDefault;
  }
  $("set-max").value = String(state.settings.settings.maxRunning);
  $("set-max-videos").value = String(state.settings.settings.maxVideos);
  $("set-seed-ratio").value = String(state.settings.settings.seedRatio);
  $("set-seed-time").value = String(state.settings.settings.seedTime);
  $("set-limit").value = String(state.settings.settings.downloadLimit);
  syncCustomSelects(); // 自绘下拉的按钮文字跟着值走
  if (!DIALOG) setTheme(state.settings.settings.theme || "system");
  setSwitch("sw-notify", state.settings.settings.notifyOnFinish);
  setSwitch("sw-sleep", state.settings.settings.preventSleep);
  setSwitch("sw-clip", state.settings.settings.watchClipboard);
  setSwitch("sw-ghmirror", state.settings.settings.githubMirror);
  $("set-relay").value = state.settings.settings.relay || "off";
  syncCustomSelects();
  loadThunder();
  setSwitch("sw-confirm", state.settings.settings.confirmNew);
  setSwitch("sw-polish", !!state.settings.settings.subtitlePolish);
  // 没检测到 Ollama 就不显示这一行
  invoke("ollama_models").then((models) => {
    $("polish-row").hidden = !models.length;
    $("polish-model-row").hidden = !models.length;
    $("polish-mode-row").hidden = !models.length;
    $("set-polish-mode").value = state.settings.settings.polishMode === "translate" ? "translate" : "polish";
    const sel = $("set-polish-model");
    sel.innerHTML = "";
    for (const m of models) sel.add(new Option(m, m));
    sel.value = models.includes(state.settings.settings.polishModel) ? state.settings.settings.polishModel : models[0] || "";
    syncCustomSelects();
  }).catch(() => {});
  renderCookies();
}

/// 已导入 Cookie 的站点。名单以数据目录里的 cookies-<站点>.txt 为准
async function renderCookies() {
  const el = $("ck-state");
  if (!el || !invoke) return;
  try {
    const list = await invoke("cookies_status");
    if (list && list.length) {
      el.textContent = "已导入：" + list.join("、");
      el.title = "这些站的任务会自动带上 Cookie";
    } else {
      el.textContent = "没导入（YouTube、抖音、西瓜、小红书等会报「需要 Cookie」）";
      el.title = "";
    }
  } catch (e) { /* 内核没起来时忽略 */ }
}

/// 开关画法：class + aria-checked 一起改，读屏和样式都不会落下
function setSwitch(id, on) {
  const el = $(id);
  if (!el) return;
  el.classList.toggle("on", !!on);
  el.setAttribute("aria-checked", on ? "true" : "false");
}

async function loadSettings() {
  if (!invoke) return;
  try {
    state.settings = await invoke("get_settings");
    renderSettings();
    if (state.view === "all" && $("download-empty-title")) renderTasks();
  } catch (e) {
    console.warn("取设置失败", e);
  }
}

async function saveSettings() {
  if (!invoke || !state.settings) return;
  try {
    const msg = await invoke("save_settings", { settings: state.settings.settings });
    toast(msg);
    await loadSettings();
  } catch (e) {
    toast("保存失败：" + e);
  }
}

async function pickDir(key) {
  if (!invoke) return;
  const cur = state.settings?.[key]?.path || "";
  const picked = await invoke("pick_folder", { title: DIR_LABEL[key], current: cur });
  if (!picked) return;
  state.settings.settings["dir" + key[0].toUpperCase() + key.slice(1)] = picked;
  await saveSettings();
}

async function resetDir(key) {
  state.settings.settings["dir" + key[0].toUpperCase() + key.slice(1)] = "";
  await saveSettings();
}


// ---------- 视频网站工具（yt-dlp / ffmpeg） ----------

const TOOL_NAME = { "yt-dlp": "yt-dlp", ffmpeg: "ffmpeg" };

function toolText(t) {
  if (!t.exists) return "未安装";
  const src = t.source === "system" ? "系统里已有" : "";
  const size = t.size ? bytes(t.size) : "";
  const parts = [t.version ? "v" + t.version : "已安装", src, size].filter(Boolean);
  return parts.join(" · ");
}

async function loadTools() {
  if (!invoke) return;
  try {
    const list = await invoke("tools_status");
    for (const t of list) {
      const el = $("tool-" + t.key);
      const btn = $("btn-" + t.key);
      if (!el) continue;
      el.textContent = toolText(t);
      el.title = t.path;
      if (btn) btn.textContent = t.exists ? "重新安装" : "安装";
    }
  } catch (e) {
    console.warn("取工具状态失败", e);
  }
}

async function installTool(key) {
  if (!invoke) return;
  const el = $("tool-" + key);
  const btn = $("btn-" + key);
  if (btn) btn.disabled = true;
  el.textContent = "准备下载…";
  try {
    // force：按钮写着「重新安装」就真的要重下一份。
    // 之前没传 force，后端看到本地已有自带副本就直接返回，点了什么都不发生
    const t = await invoke("install_tool", { key, force: true });
    el.textContent = toolText(t);
    toast(`${TOOL_NAME[key] || key} 已就绪`);
  } catch (e) {
    el.textContent = "安装失败：" + e;
    toast("安装失败：" + e);
  } finally {
    if (btn) btn.disabled = false;
  }
}

// 下载进度是后端事件推过来的（同一个事件里带 stage：download / extract / done / error）
// 视频号与其他来源共用主窗口里的重复提示，只在用户确认后打开下载确认框。
async function handleWeChatReady({url, opts}) {
  let handedOff = false;
  try {
    const choice = await askDuplicate(url, opts.dir);
    if (!choice) return;
    opts.wechatDuplicateChecked = true;
    opts.wechatForce = !!choice.force;
    if (opts.wechatSkipConfirm) {
      const id = await invoke("wechat_confirm", {ticket:opts.wechatTicket, name:opts.name || "", dir:opts.dir || "", remember:false, force:!!choice.force});
      switchView("all");
      await refreshTasks();
      revealTask(id);
    } else {
      handedOff = !!(await openDialog("confirm", {url, opts}));
    }
  } catch (e) { toast(String(e)); }
  finally {
    if (!handedOff) await invoke("wechat_cancel", {ticket:opts.wechatTicket}).catch(() => {});
  }
}

if (!DIALOG && window.__TAURI__?.event?.listen) {
  window.__TAURI__.event.listen("cloud-refresh-status",({payload})=>toast(payload.message));
  window.__TAURI__.event.listen("cloud-refreshed",({payload})=>{if(cloudRefresh?.id===payload.id)cloudRefresh=null;toast("新地址已自动更新到原任务，继续下载");refreshTasks();});
  window.__TAURI__.event.listen("wechat-ready", ({payload}) => handleWeChatReady(payload));
  window.__TAURI__.event.listen("reveal-history",({payload})=>revealHistoryRecord(payload.id));
  window.__TAURI__.event.listen("tool-progress", (ev) => {
    const p = ev.payload || {};
    const el = $("tool-" + p.key);
    if (!el) return;
    if (p.stage === "start") {
      // 首装提示：这几个字在设置页里看不到（用户多半在下载页），单独弹一次
      toast(p.message || "正在准备下载组件");
      return;
    }
    if (p.stage === "done") el.textContent = "已就绪";
    else if (p.stage === "extract") el.textContent = "正在解压…";
    else if (p.stage === "error") el.textContent = "失败：" + (p.message || "");
    else if (p.total) el.textContent = `${p.message} ${bytes(p.received)} / ${bytes(p.total)}`;
    else el.textContent = p.message || "下载中…";
  });
}

// ---------- 视图 ----------

/// 「清空历史」只在下载历史页、且真有记录时出现
function syncTrashClear() {
  const b = $("clear-trash-btn");
  if (b) b.hidden = state.view !== "trash" || !state.trash.length;
}

/// 设置页搜索：匹配标题、提示文字和下拉选项。标题命中就显示整张卡片，否则只留命中的行。
/// 用单独的 class 隐藏，不碰 hidden：有些行本来就按状态隐藏，清空搜索后要原样恢复
let settingsSearching = false;
function filterSettings(q) {
  const key = String(q || "").trim().toLowerCase();
  const pane = $("pane-settings");
  const text = (el) => [el.textContent, ...[...el.querySelectorAll("[data-tip]"), el].map((x) => x.dataset?.tip || ""), ...[...el.querySelectorAll("input")].map((x) => x.placeholder || "")].join(" ").toLowerCase();
  let any = false;
  pane.querySelectorAll(".set-card").forEach((card) => {
    const head = card.querySelector("h2");
    const headHit = !!key && !!head && text(head).includes(key);
    let hit = false;
    card.querySelectorAll(".set-row").forEach((row) => {
      const show = !key || headHit || text(row).includes(key);
      row.classList.toggle("search-miss", !show);
      if (show && !row.hidden) hit = true;
    });
    // 卡片里的说明文字只在没有搜索时显示
    card.querySelectorAll(":scope > .set-hint").forEach((h) => h.classList.toggle("search-miss", !!key && !headHit && !text(h).includes(key)));
    const textHit = [...card.querySelectorAll(":scope > .set-hint")].some((h) => !h.classList.contains("search-miss"));
    card.classList.toggle("search-miss", !!key && !headHit && !hit && !textHit);
    if (!card.classList.contains("search-miss")) any = true;
  });
  let empty = $("set-search-empty");
  if (!empty) {
    empty = document.createElement("div");
    empty.id = "set-search-empty";
    empty.className = "set-search-empty";
    pane.querySelector(".scroll").prepend(empty);
  }
  empty.hidden = any || !key;
  // 设置页按命中的设置行计数
  setSearchCount(key ? [...pane.querySelectorAll(".set-card:not(.search-miss) .set-row:not(.search-miss)")].filter((r) => !r.hidden).length : null);
  empty.textContent = key ? `没有找到「${q.trim()}」相关的设置` : "";
  if (key) pane.querySelector(".scroll").scrollTop = 0;
}

async function switchView(view) {
  if (state.view !== view) {
    if (filesPanelId) closeFilesPanel();
    if (ctxOpen()) closeCtx();
  }
  if(state.view==="settings" && view!=="settings")suspendCloudSettings();
  state.view = view;
  document.querySelectorAll(".nav-item[data-view]").forEach((x) => {
    x.classList.toggle("active", x.dataset.view === view);
  });
  const isSettings = view === "settings";
  // 顶栏搜索框在设置页搜设置项；进出设置页时清空，两边的关键词不混用
  if (isSettings !== settingsSearching) {
    settingsSearching = isSettings;
    $("search").value = "";
    $("search-clear").hidden = true;
    state.query = "";
    filterSettings("");
  }
  $("search").placeholder = isSettings ? "搜索设置" : "搜索";
  $("pane-downloads").hidden = isSettings;
  $("pane-settings").hidden = !isSettings;
  $("view-title").textContent = TITLE[view] || "下载中";
  renderSpeed();
  // 「清理全部失效记录」只在已完成页有意义
  const purge = $("purge-btn");
  if (purge) purge.hidden = view !== "done";
  $("start-all").hidden = view !== "all"; // 只在正在下载页
  // 排序只放在看记录的页面；正在下载页的顺序靠拖动
  const sortBtn = $("sort-btn");
  if (sortBtn) sortBtn.hidden = view === "all" || view === "settings";
  // 「清空历史」只在下载历史页出现，而且要真有记录才给按钮
  syncTrashClear();
  // 下载历史要先拉一遍记录再画
  if (view === "trash") loadTrash(false);
  // 设置页没有速度卡和粘贴框
  if (isSettings) {
    loadSettings();
    loadAbout();
    loadTools();
    loadRpcInfo(); // 引擎可能刚起来，密钥这时才有值
    refreshCloudSettings();
  }
  else renderAll();
}

// ---------- 数据 ----------

const taskRemovals = new Map();
function taskVisibleAfterRemove(task) {
  if (taskRemovals.has(task.id)) return false;
  if (!isGroup(task.id) || task.collectionFiles) return true;
  const members = groupMembers(task.id);
  return !members.length || members.some(member => !taskRemovals.has(member.id));
}
function confirmRemovedTasks() {
  if (!taskRemovals.size) return;
  const visibleIds = new Set([...rawTasks, ...state.tasks].map(task => task.id));
  for (const [id, removed] of taskRemovals) {
    if (removed && !visibleIds.has(id)) taskRemovals.delete(id);
  }
}

let refreshingTasks = null;
async function refreshTasks() {
  if (!invoke || !state.engineRunning) return;
  if (refreshingTasks) return refreshingTasks;
  refreshingTasks = Promise.resolve().then(async () => {
    try {
      state.tasks = mergeCreatingTasks(mergeGroups(await invoke("list_tasks")));
      confirmRemovedTasks();
      saveTaskSnapshot();
      refreshFileCounts();
      if (filesPanelId && state.tasks.some(task => task.id === filesPanelId && task.collectionFiles)) drawFilesPanel();
      updateSmoothSpeed();
      watchLastPiece();
      watchStall();
      playDoneSound();
      notifyCollections();
      // 封面补拉跟着任务刷新走：不管当前看哪个页面（哪怕列表是空的），补拉规则都得每秒过一遍
      fillThumbs();
      checkCollisions();
      // 看着下载历史时跟着刷文件状态：用户可能刚在资源管理器里删了文件
      if (state.view === "trash" && Date.now() - trashLoadedAt > 3000) loadTrash(false);
      if (state.view !== "settings") renderAll();
    } catch (e) {
      console.warn("取任务失败", e);
    }
  });
  try { return await refreshingTasks; } finally { refreshingTasks = null; }
}

let trashLoadedAt = 0;
let trashLoad = null;
let trashRevision = 0;
let trashLoadError = "";

/// 启动时内核要过一会儿才能列出任务。先把上次的列表画出来（速度清零），内核好了再换成真的。
/// 存的是还没下完的任务（文件列表去掉，免得太大），最多 300 条，最多 10 秒存一次
const TASK_SNAP_KEY = "sd.taskSnapshot";
let snapSavedAt = 0;
function saveTaskSnapshot() {
  if (Date.now() - snapSavedAt < 10000) return;
  snapSavedAt = Date.now();
  try {
    const keep = state.tasks.filter((t) => !t.creating && t.status !== "done").slice(0, 300).map((t) => {
      const c = { ...t, meta: t.meta ? { ...t.meta, res: t.meta.res ? { ...t.meta.res, files: [] } : t.meta.res } : t.meta };
      if (c.progress) c.progress = { ...c.progress, speed: 0 };
      c.speed = 0;
      return c;
    });
    localStorage.setItem(TASK_SNAP_KEY, JSON.stringify(keep));
  } catch (_) {}
}
function showTaskSnapshot() {
  if (state.tasks.length) return;
  try {
    const list = JSON.parse(localStorage.getItem(TASK_SNAP_KEY) || "[]");
    if (!list.length) return;
    state.tasks = list;
    renderAll();
  } catch (_) {}
}

/// 长时间没进展的任务：id -> { bytes, at, tries }。自动暂停再继续一次，常常就恢复了，
/// 不用用户动手；重试间隔逐渐拉长（10、30、60 分钟），不会一直打扰内核。
/// 下完最后一点的情况归 lastPiece 管
const stallSeen = {};
const STALL_META_MS = 5 * 60000; // 磁力拿不到文件信息
const STALL_MS = 10 * 60000; // 已经开始下，但进度不动
function stallOf(t) {
  const s = stallSeen[t.id];
  // 迅雷的任务进度是从迅雷界面读的，读数一时没更新不代表卡住，不做这项判断
  if (!s || t.status !== "running" || t.stage || t.shandianVerification === "checking" || String(t.id).startsWith("xl:")) return null;
  const idle = Date.now() - s.at;
  const bt = isBt(t);
  const retried = s.tries ? `，已自动重试 ${s.tries} 次` : "";
  if (bt && !t.meta?.res?.size && idle > STALL_META_MS) return { main: "找不到资源", sub: "可能已经没人分享" };
  if (idle <= STALL_MS && !s.tries) return null;
  return bt ? { main: "没有进度", sub: `暂时没有可用来源${retried}` } : { main: "没有响应", sub: "10 分钟没进度，可以暂停后重试" };
}
function watchStall() {
  const now = Date.now();
  const alive = new Set();
  for (const t of state.tasks) {
    // 迅雷的任务不归这里自动暂停重试
    if (t.status !== "running" || String(t.id).startsWith("xl:")) continue;
    alive.add(t.id);
    const bytes = t.progress?.downloaded || 0;
    const s = stallSeen[t.id];
    // 只有真的多下了才算恢复。重试后内核可能短暂报 0，不能当成有进展
    if (!s || bytes > s.bytes) stallSeen[t.id] = { bytes, at: now, tries: 0 };
    else if (autoRetryable(t) && now - s.at > stallWait(s.tries)) autoRetry(t, s);
  }
  for (const id of Object.keys(stallSeen)) if (!alive.has(id) && !stallRetrying.has(id)) delete stallSeen[id];
}

const stallRetrying = new Set();
const stallWait = (tries) => [10, 30, 60][Math.min(tries, 2)] * 60000;
const autoRetryable = (t) => !/^(?:nas:)?(?:v:|v-|pp:|cloud:|xl:|grp:)/.test(String(t.id)) && !t.stage && t.shandianVerification !== "checking" && !lastPiece[t.id]?.waiting;
async function autoRetry(t, s) {
  if (stallRetrying.has(t.id)) return;
  stallRetrying.add(t.id);
  s.tries += 1;
  s.at = Date.now();
  try {
    await invoke("pause_task", { id: t.id });
    await new Promise((r) => setTimeout(r, 1500));
    await invoke("resume_task", { id: t.id });
  } catch {
    // 下一轮再试
  } finally {
    stallRetrying.delete(t.id);
  }
}

/// 卡在最后一点的 BT 任务：id -> { bytes, at, tries, retryAt, waiting }
/// 末块常和没选中的小文件共用一个分片，或者网上的源很少。30 秒不动就说明情况；
/// 干等超过 1 分钟让内核重新校验（Mac 实测几秒内就能接着下），最多 3 次、间隔 90 秒
const lastPiece = {};
function watchLastPiece() {
  const now = Date.now();
  const stuck = new Set();
  for (const t of state.tasks) {
    const total = t.meta?.res?.size || 0;
    const done = t.progress?.downloaded || 0;
    if (t.status !== "running" || !isBt(t) || t.shandianVerification === "checking" || !total || done >= total || done / total <= 0.995) continue;
    stuck.add(t.id);
    const p = lastPiece[t.id];
    if (!p || p.bytes !== done) {
      lastPiece[t.id] = { bytes: done, at: now, tries: 0, retryAt: 0, waiting: false };
      continue;
    }
    p.waiting = now - p.at > 30000;
    if (now - p.at > 60000 && !t.progress?.speed && p.tries < 3 && now - p.retryAt > 90000) {
      p.tries += 1;
      p.retryAt = now;
      invoke("verify_task", { id: t.id }).catch((e) => console.warn("重新校验失败", e));
    }
  }
  for (const id of Object.keys(lastPiece)) if (!stuck.has(id)) delete lastPiece[id];
}

function isBt(t) {
  return t.kind === "磁力" || t.kind === "种子文件";
}

/// 目录里已有同名文件、下载被跳过的任务（列表里查不到的那种，比如文件是以前下的）：
/// 问一次要不要再下一份，每条只问一次
const collisionAsked = new Set();
async function confirmBaiduCopy(task) {
  const yes = await askYes(
    "保存位置已有同名文件",
    `「${task.name || "这个文件"}」已有一份。要把这次下载的内容另存一份吗？`,
    "另存一份",
  );
  if (!yes) return;
  try {
    await invoke("save_baidu_copy", { id: task.id });
    refreshTasks();
  } catch (error) {
    toast("保存失败：" + error);
  }
}
async function checkCollisions() {
  for (const t of [...state.tasks, ...rawTasks.filter(task => task.collided && collectionForEpisode(task.id))]) {
    if (!t.collided || collisionAsked.has(t.id)) continue;
    collisionAsked.add(t.id);
    if (String(t.id).startsWith("bd:")) {
      await confirmBaiduCopy(t);
      continue;
    }
    const downloaded = String(t.shandianError || "").includes("已保留下载结果");
    const yes = await askYes(
      "目录里已有同名文件",
      downloaded
        ? `「${t.name || ""}」的成品已经下载。另存一份时，视频和字幕会使用同一个编号。`
        : `「${t.name || ""}」已经在保存目录里，这次没有重复下载。\n如果想要另一种清晰度或字幕，可以再下一份，文件名会自动加 (1)、(2)。`,
      downloaded ? "另存一份" : "再下一份",
    );
    try {
      await invoke(yes ? "video_copy_anyway" : "video_dismiss_collision", { id: t.id });
    } catch (e) {
      toast("操作失败：" + e);
    }
    refreshTasks();
  }
}

/// 刚下完的任务放一声「叮—咚」（Resources/done.wav，和 Mac 版同一个文件）
const doneAudio = new Audio("assets/done.wav");
const observedCollections = new Map();

function notifyCollections() {
  for (const task of state.tasks) {
    if (!task.collectionFiles) continue;
    const previous = observedCollections.get(task.id);
    const downloading = ["running", "wait"].includes(task.status);
    const polishing = !!task.collectionPolishBusy;
    if (task.status === "done" && previous?.downloading && state.settings?.settings.notifyOnFinish) {
      invoke("notify_collection", { id: task.id, title: task.name }).catch(() => {});
    }
    if (previous?.polishing && !polishing && task.status === "done" && task.collectionFiles.every(file => file.taskId)) {
      const heading = task.collectionPolishFailed ? `字幕润色结束，${task.collectionPolishFailed} 项失败` : "合集字幕已润色";
      invoke("notify_collection", { id: task.id, title: task.name, heading }).catch(() => {});
    }
    observedCollections.set(task.id, { downloading: downloading || (!!previous?.downloading && task.status === "pause"), polishing });
  }
}
function playDoneSound() {
  // 跳过下载的（文件本来就在）不算刚下完，别响
  const now = state.tasks.filter((t) => t.status === "done" && !t.collided).map((t) => t.id);
  // 又跑起来的任务（换后缀再下一份）下完要再响一次
  for (const t of state.tasks) if (t.status === "running") state.seenDone.delete(t.id);
  const fresh = now.filter((id) => !state.seenDone.has(id));
  now.forEach((id) => state.seenDone.add(id));
  // 首次载入时把历史任务都记成「已见过」，不然一开 App 就响一串
  if (state.firstLoad) {
    state.firstLoad = false;
    return;
  }
  if (fresh.length) {
    doneAudio.currentTime = 0;
    doneAudio.play().catch(() => {});
  }
}

/// 「关于」里的内核信息：运行状态、端口、数据目录
async function loadAbout() {
  if (!invoke) return;
  try {
    const s = await invoke("engine_status");
    state.engine = s;
    $("about-engine").textContent = s.running ? `运行中 · 127.0.0.1:${s.port}` : "未启动";
    $("about-engine").title = s.exe;
  } catch (e) {
    console.warn("取内核信息失败", e);
  }
}

let refreshingEngine = null;
async function refreshEngine() {
  if (!invoke) return;
  if (refreshingEngine) return refreshingEngine;
  refreshingEngine = Promise.resolve().then(async () => {
    try {
      const s = await invoke("engine_status");
      const changed = s.running !== state.engineRunning;
      state.engineRunning = s.running;
      $("engine-dot").className = "dot" + (s.running ? " on" : state.busy ? " busy" : "");
      // 正常时这一行整体隐藏（CSS 里 .engine 默认 display:none），只有没起来才冒出来提醒
      document.body.classList.toggle("engine-down", !s.running && !state.busy);
      $("engine-text").textContent = s.running ? "内核运行中" : state.busy ? "正在启动…" : "内核没在运行，任务不会开始";
      $("engine-btn").textContent = s.running ? "停止" : "启动";
      if (changed && s.running) refreshTasks();
    } catch (e) {
      console.warn("取内核状态失败", e);
    }
  });
  try { return await refreshingEngine; } finally { refreshingEngine = null; }
}

async function toggleEngine() {
  if (!invoke || state.busy) return;
  if (state.engineRunning) {
    await invoke("engine_stop");
    state.engineRunning = false;
    state.tasks = [];
    state.seenDone.clear();
    renderAll();
    refreshEngine();
    return;
  }
  state.busy = true;
  $("engine-text").textContent = "启动中…";
  $("engine-dot").className = "dot busy";
  try {
    const ver = await invoke("engine_start");
    state.engineRunning = true;
    console.log("内核版本", ver);
  } catch (e) {
    toast("内核启动失败：" + e);
  } finally {
    state.busy = false;
    refreshEngine();
    refreshTasks();
  }
}

/// 新建弹窗一次贴多条时的排队：一条处理完（直投完／确认完／取消掉）再上下一条，
/// 不然几条挤着同时弹确认框。格式 { url, opts }
let pendingLinks = [];

/// 开着的独立窗口：label → 类型。还在这里就关了的，算用户取消
const openDialogs = new Map();

async function openDialog(kind, payload) {
  try {
    const label = await invoke("open_dialog", { kind, payload });
    openDialogs.set(label, kind);
    if (pushOpening && kind === "confirm" && !payload.opts?.collection) {
      pushDialog = label;
      pushOpening = false;
    }
    return label;
  } catch (e) {
    toast(String(e));
    if (kind === "new") pendingLinks = [];
    else nextPending();
  }
}

/// 独立窗口：取回主窗口交代的内容并显示
let confirmFitFrame=0, confirmAutoHeight=0, confirmMonitor=null;
async function keepConfirmOnScreen(me,height) {
  const bounds=confirmMonitor?.workArea || confirmMonitor;
  if (!bounds || !me.outerPosition || !me.setPosition) return;
  const position=await me.outerPosition().catch(()=>null);
  if (!position) return;
  const scale=confirmMonitor.scaleFactor || 1;
  const y=Math.max(bounds.position.y+8*scale,Math.min(position.y,bounds.position.y+bounds.size.height-(height+8)*scale));
  if (Math.abs(y-position.y)>scale) await me.setPosition(new window.__TAURI__.dpi.PhysicalPosition(position.x,Math.round(y))).catch(()=>{});
}
function fitConfirmDialog() {
  if (!DIALOG || !dialogShown || $("confirm").hidden || confirmFitFrame) return;
  confirmFitFrame=requestAnimationFrame(async()=>{
    confirmFitFrame=0;
    // Once the user resizes the window, keep their size and use the body scrollbar.
    if (confirmAutoHeight && Math.abs(window.innerHeight-confirmAutoHeight)>2) return;
    const box=$("confirm").querySelector(".modal"), body=box.querySelector(".modal-body");
    const extra=box.offsetHeight-body.clientHeight;
    const available=confirmMonitor?.workArea ? confirmMonitor.workArea.size.height/confirmMonitor.scaleFactor : window.screen.availHeight;
    const height=Math.min(Math.max(240,available-80),Math.max(240,$("cf-fields").scrollHeight+extra));
    if (Math.abs(window.innerHeight-height)<2) return;
    confirmAutoHeight=height;
    const me=window.__TAURI__.window.getCurrentWindow();
    await me.setSize(new window.__TAURI__.dpi.LogicalSize(window.innerWidth,height)).catch(()=>{});
    await keepConfirmOnScreen(me,height);
  });
}
async function startDialogWindow() {
  document.body.classList.add("dialog-window");
  const job = await invoke("dialog_payload");
  if (!job) return closeDialogWindow();
  window.__sdSetTheme(job.theme || window.__sdTheme);
  const me = window.__TAURI__.window.getCurrentWindow();
  enableDialogDrag(me);
  if (job.kind === "substyle") {
    await showSubStyleDialog(me, job.payload);
    return;
  }
  if (job.kind === "new") {
    openNewSheet(job.payload?.text);
  } else {
    const loading = dialogProgress("新建下载", "正在读取下载选项…");
    await me.show();
    me.setFocus?.().catch(() => {});
    try {
      await loadSettings();
      if (window.__TAURI__.window.currentMonitor) confirmMonitor = await window.__TAURI__.window.currentMonitor().catch(() => null);
      await askConfirm(job.payload.url, job.payload.opts);
      if (!cf) {
        setProbeState(loading.querySelector("[role=status]"), "无法读取这条任务的下载选项", false);
        return;
      }
      $("confirm").hidden = false;
      me.setTitle($("cf-title").textContent);
      loading.remove();
    } catch (error) {
      setProbeState(loading.querySelector("[role=status]"), "读取下载选项失败：" + (error?.message || String(error)), false);
      return;
    }
  }
  // 窗口高度跟内容走，不留大片空白；太长才在框内滚动
  const box = (job.kind === "new" ? $("new-sheet") : $("confirm")).querySelector(".modal");
  const body = box.querySelector(".modal-body");
  const extra = box.offsetHeight - body.clientHeight;
  const h = Math.min(job.kind === "new" ? 520 : Math.max(240,window.screen.availHeight-80), Math.max(job.kind === "new" ? 280 : 240, (job.kind === "new" ? body.scrollHeight : $("cf-fields").scrollHeight) + extra));
  await me.setSize(new window.__TAURI__.dpi.LogicalSize(720, h)).catch(() => {});
  if(job.kind!=="new"){confirmAutoHeight=h;await keepConfirmOnScreen(me,h);}
  await me.show();
  dialogShown=true;me.setFocus?.();
  if(job.kind!=="new")requestAnimationFrame(()=>beginConfirmLookups());
}

/// 独立窗口（新建、确认、字幕样式）统一的拖动：按在任何不是控件的地方都能拖窗口，
/// 不用再给每个元素单独标 data-tauri-drag-region（那个只对元素本身生效，子元素、留白处都拖不动）。
/// 双击、点在滚动条上不算
function enableDialogDrag(me) {
  // #cf-url 是 span，不在控件列表里时 mousedown 会启动原生窗口拖动，把后面的 click 整个吞掉：
  // 「点击复制链接」的提示能看到、点了却什么都没发生，就是这么来的。
  const CONTROLS = "button, input, textarea, select, a, label, .select-wrap, .select-menu, .seg, .ss-range, .switch, [contenteditable], .acc-pop, .info, #cf-url";
  document.addEventListener("mousedown", (e) => {
    if (e.button !== 0 || e.detail > 1) return;
    const el = e.target;
    if (!(el instanceof Element) || el.closest(CONTROLS)) return;
    // 点在滚动条上：留给滚动
    if (el.scrollHeight > el.clientHeight && e.offsetX > el.clientWidth) return;
    // 开始拖之前先把开着的下拉、右键菜单收起来（拖动会吞掉后面的 click，靠 click 收的菜单就收不了）
    document.querySelectorAll(".ctx-menu:not([hidden]), .select-menu:not([hidden]), .acc-pop:not([hidden])").forEach((m) => { m.hidden = true; });
    me.startDragging?.().catch?.(() => {});
  });
}

async function submitDialog(result) {
  const me = window.__TAURI__.window.getCurrentWindow();
  await window.__TAURI__.event.emitTo("main", "dialog-submit", { label: me.label, result });
  await closeDialogWindow();
}

function closeDialogWindow() {
  return window.__TAURI__.window.getCurrentWindow().close();
}

if (!DIALOG && window.__TAURI__?.event?.listen) {
  window.__TAURI__.event.listen("reveal-task",({payload})=>{refreshTasks().then(()=>revealTask(payload.id));});
  window.__TAURI__.event.listen("dialog-submit", ({ payload: { label, result } }) => {
    openDialogs.delete(label);
    if ("feishuId" in result) {
      switchView("all");
      refreshTasks().then(() => { if (result.feishuId) revealTask(result.feishuId); });
      nextPending();
      return;
    }
    if (result.baiduId) {
      showBaiduTask(result.baiduId);
      return;
    }
    if (result.baiduHandled) {
      nextPending();
      return;
    }
    if (result.wechatId) {
      if (result.remember && state.settings) {
        state.settings.settings.confirmNew = false;
        setSwitch("sw-confirm", false);
      }
      switchView("all");
      refreshTasks();
      revealTask(result.wechatId);
      return;
    }
    if (result.substyle) {
      const { ok, fail, err } = result.substyle;
      toast(fail ? `已更新 ${ok} 个视频的字幕样式，${fail} 个没改成：${err}` : ok > 1 ? `已更新 ${ok} 个视频的字幕样式` : "字幕样式已更新");
      return;
    }
    if (result.links) {
      pendingLinks = [...new Set(result.links)].map((url) => ({ url, opts: {} }));
      nextPending();
    } else {
      finishConfirm({ ...result, selections: new Map(result.selections) });
    }
  });
  window.__TAURI__.event.listen("dialog-closed", ({ payload: label }) => {
    const kind = openDialogs.get(label);
    if (!openDialogs.delete(label)) return;
    if (kind === "substyle") return;
    if (kind === "new") pendingLinks = [];
    else nextPending();
  });
}

/// 投排队里的下一条；队列空了就是无事可做
let extBuffer = [];
let extTimer;
/// 批量投来的文件名和鉴权头。合成一批后要靠它们下载，丢了网盘直链就 403
const extInfo = new Map();

/// 正在等确认的那个推送弹窗。助手「全部推送」是一条条发的，间隔可能很长，后来的都并进这个框
let pushDialog = null;
let pushOpening = false;

let cloudRefresh = null;
const cloudHomes = {"阿里云盘":"https://www.alipan.com/drive/file/all","夸克网盘":"https://pan.quark.cn/list","UC 网盘":"https://drive.uc.cn/","百度网盘":"https://pan.baidu.com/disk/main","123 云盘":"https://yun.123pan.cn/","天翼云盘":"https://cloud.189.cn/web/main/","移动云盘":"https://yun.139.com/w/","迅雷云盘":"https://pan.xunlei.com/","光鸭云盘":"https://www.guangyapan.com/"};
const cloudKind = t => String(t.kind || "").replace(/^NAS · /, "");
const canCloudRefresh = t => !String(t.id).startsWith("bd:") && !/^(?:nas:)?cloud:/.test(String(t.id)) && !String(t.id).startsWith("nas:") && (/^(https?:|shandian-multi:)/.test(String(t.meta?.req?.url || ""))) && !!cloudHomes[cloudKind(t)];
const needsCloudRefresh = t => t.status === "error" && canCloudRefresh(t)
  && /(?:\b(?:http(?:\s+status)?(?:\s+code)?|status|code)[=: ]+)(?:401|403|410)\b|(?:链接|下载地址|签名)(?:已)?(?:过期|失效)/i.test(t.shandianError || "");
const cloudResumeLinkProviders = new Set(["夸克网盘","UC 网盘","移动云盘","123 云盘"]);
const refreshName = name => String(name || "").replace(/\\/g,"/");
async function beginCloudRefresh(t, fromContinue = false) {
  const home = cloudHomes[String(t.kind || "").replace(/^NAS · /,"")];
  if (!home || String(t.id).startsWith("nas:")) return toast("此来源暂不支持换链续传");
  const contextual=fromContinue && needsCloudRefresh(t);
  if(!contextual){
    const v=await askChoice({title:"继续原下载",desc:"打开网盘后，闪电会尝试接着原进度下载。需要时请在网页登录。",buttons:[{label:"取消",value:false},{label:"打开网盘",value:true,primary:true}]});
    if(!v)return;
  }
  toast("正在核对原任务文件与进度…");
  if (ACTIVE.has(t.status)) await invoke("pause_task",{id:t.id});
  const single=!String(t.meta?.req?.url || "").startsWith("shandian-multi:");
  const info=single ? {files:[{name:t.meta?.res?.files?.[0]?.name}]} : await invoke("multi_task_info",{id:t.id});
  const stats=single ? [{index:0,selected:true,status:"error"}] : await invoke("task_file_list",{id:t.id});
  const all=new Set((info.files || []).filter(f=>f.name).map(f=>refreshName(f.name)));
  const wanted=new Set(stats.filter(f=>f.selected && f.status!=="done").map(f=>refreshName(info.files?.[f.index]?.name || f.name)).filter(Boolean));
  if (!wanted.size) return toast("没有需要更新地址的文件");
  const token=await invoke("multi_refresh_prepare",{id:t.id});
  cloudRefresh={id:t.id,kind:t.kind,all,wanted,files:new Map(),at:Date.now()};
  if(contextual && cloudResumeLinkProviders.has(cloudKind(t))){
    try {
      await invoke("cloud_resume_account_link",{token});
      toast("已打开网盘，登录后继续原下载");
      return;
    } catch { /* The same browser can still provide a fresh link. */ }
  }
  const url=new URL(home);url.searchParams.set("shandian-refresh",token);
  try { await invoke("open_url",{url:url.href}); }
  catch(e) { cloudRefresh=null;throw e; }
  toast("正在自动获取新地址；未找到时请进入网盘原文件夹");
}
async function acceptCloudRefresh(items) {
  const job=cloudRefresh;
  if(!job)return items;
  if(Date.now()-job.at>10*60000){cloudRefresh=null;toast("等待新地址已超时，请重新选择换链续传");return items;}
  const matching=items.filter(x=>job.all.has(refreshName(x.name)));
  if(!matching.length)return items;
  const remaining=items.filter(x=>!job.all.has(refreshName(x.name)));
  for(const x of matching)if(job.wanted.has(refreshName(x.name))) job.files.set(refreshName(x.name),{url:x.url,name:refreshName(x.name),headers:x.headers || []});
  if(job.files.size<job.wanted.size){toast(`已收到 ${job.files.size}/${job.wanted.size} 个新地址`);return remaining;}
  if(job.applying)return remaining;
  job.applying=true;
  try {
    await invoke("multi_refresh_links",{id:job.id,files:[...job.files.values()]});
    cloudRefresh=null;toast("已更新原任务地址，继续下载");await refreshTasks();
  }catch(e){job.files.clear();toast(`未能完成换链续传，原任务和进度保留：${e}`);}
  finally {job.applying=false;}
  return remaining;
}

async function flushExtBuffer() {
  // 上一批的框还在打开的路上（比如正在问重复），等它开好再并进去
  if (pushOpening) {
    extTimer = setTimeout(flushExtBuffer, 400);
    return;
  }
  const seen = new Set();
  let items = extBuffer.filter((x) => !seen.has(x.url) && seen.add(x.url));
  extBuffer = [];
  items = await acceptCloudRefresh(items);
  if (!items.length) return;
  for (const x of items) {
    extInfo.set(x.url, x);
    if (x.name) grabNames.set(x.url, x.name);
  }
  if (pushDialog && openDialogs.has(pushDialog)) {
    await window.__TAURI__.event.emitTo(pushDialog, "dialog-append", { items: items.map(({ url, name, size, sizeLabel }) => ({ url, name, size, sizeLabel })) });
    return;
  }
  pushOpening = true;
  // 超时兜底：用户取消了重复询问时不会有框打开
  setTimeout(() => { pushOpening = false; }, 8000);
  if (items.length === 1) return submitLink(items[0].url, { name: items[0].name, headers: items[0].headers, size: items[0].size, sizeLabel: items[0].sizeLabel });
  const infos = await Promise.all(items.map((x) => invoke("target_info", { link: x.url }).catch(() => null)));
  if (infos.some((info) => info?.collection || info?.baiduShare)) {
    pendingLinks.push(...items.map(({ url, name, headers, size, sizeLabel }) => ({ url, opts: { name, headers, size, sizeLabel } })));
    return nextPending();
  }
  await submitBatch(items.map((x) => x.url), [...new Set(infos.filter(Boolean).map((i) => i.kind))]);
}

async function nextPending() {
  if (!pendingLinks.length) return;
  const head = pendingLinks.shift();
  if (BAIDU_SHARE.test(head.url) || isFeishuDocument(head.url)) return submitLink(head.url, head.opts || {});
  const plain = (x) => !x.opts || !Object.keys(x.opts).length;
  // 带文件名、鉴权头的（扩展投来的）要单独走，合并了这些信息就丢了
  if (!plain(head) || !pendingLinks.some(plain)) return submitLink(head.url, head.opts || {});
  const rest = pendingLinks.filter((item) => plain(item) && !BAIDU_SHARE.test(item.url) && !isFeishuDocument(item.url));
  if (!rest.length) return submitLink(head.url, head.opts || {});
  const infos = await Promise.all(
    [head, ...rest].map((x) => invoke("target_info", { link: x.url }).catch(() => null))
  );
  const key = (i) => i && `${i.dir}|${!!i.videoOptions}|${!!i.supportsSubs}`;
  if (!infos[0] || infos[0].collection) return submitLink(head.url, {}); // 合集各自勾选
  const same = rest.filter((_, n) => !infos[n + 1]?.collection && key(infos[n + 1]) === key(infos[0]));
  if (!same.length) return submitLink(head.url, {});
  pendingLinks = pendingLinks.filter((x) => !same.includes(x));
  const group = [infos[0], ...infos.slice(1).filter((i, n) => same.includes(rest[n]))];
  await submitBatch([head.url, ...same.map((x) => x.url)], [...new Set(group.map((i) => i.kind))]);
}

/// 一组保存位置和选项都相同的链接：重复的汇总问一次，确认框也只弹一个
async function submitBatch(urls, kinds, forceConfirm, metadata = {}) {
  if (urls.some((url) => BAIDU_SHARE.test(url) || isFeishuDocument(url))) {
    pendingLinks.unshift(...urls.map((url) => ({ url, opts: { confirm: true } })));
    return nextPending();
  }
  if (!state.engineRunning) {
    pendingLinks = [];
    toast("内核还没起来，稍等一下再试（或用侧栏的「启动」）");
    return;
  }
  if (!state.settings) await loadSettings();
  const dups = await Promise.all(urls.map((url) => invoke("find_duplicate", { url, dir: null }).catch(() => null)));
  let items = urls.map((url, n) => ({ url, dup: dups[n] }));
  const dupNames = items.filter((x) => x.dup).map((x) => x.dup.name);
  if (dupNames.length) {
    const shown = dupNames.slice(0, 3).map((n) => `「${n}」`).join("\n");
    const more = dupNames.length > 3 ? `\n等 ${dupNames.length} 个` : "";
    const all = dupNames.length === items.length;
    const c = await askChoice({
      title: all ? `这 ${items.length} 个链接都下载过` : `${items.length} 个链接里有 ${dupNames.length} 个下载过`,
      desc: `${shown}${more}\n再下一份时，文件名会自动加 (1)、(2)。`,
      buttons: [
        { label: "取消", value: null },
        ...(all ? [] : [{ label: "跳过这些", value: "skip" }]),
        { label: all ? "全部再下一份" : "也再下一份", value: "copy", primary: true },
      ],
    });
    if (!c) return nextPending();
    if (c === "skip") items = items.filter((x) => !x.dup);
  }
  // 文件丢了的旧条目，新的投成功后拿掉（和单条一样）
  // 确认框是独立窗口，看不到这边的 extInfo：名字要跟着带过去，不然只能显示链接里的乱码
  items = items.map((x) => ({ url: x.url, name: extInfo.get(x.url)?.name || grabNames.get(x.url) || "", size: extInfo.get(x.url)?.size, sizeLabel: extInfo.get(x.url)?.sizeLabel, replaceId: x.dup?.missing && x.dup.id ? x.dup.id : null }));
  if (items.length === 1 && !isHuggingFaceFilePage(metadata.sourceUrl)) return submitLink(items[0].url, { force: true, replaceId: items[0].replaceId, name: extInfo.get(items[0].url)?.name || "", headers: extInfo.get(items[0].url)?.headers || null, size: items[0].size, sizeLabel: items[0].sizeLabel });
  if (forceConfirm || state.settings?.settings.confirmNew) return askConfirm(items[0].url, { ...metadata, batch: items, kinds });
  await runBatch(items, "", null);
  nextPending();
}

/// 重复已经问过了（force）。同名文件也直接按 (1) 顺延，不再一条条弹提示
// ---------- 合并任务 ----------
// 网盘助手推一个文件夹是一个文件一条直链，内核里只能是多条。列表上合成一个任务：
// 进度、速度、大小相加，暂停、继续、删除对全部成员生效。记录存在本机界面（localStorage）
const GROUP_KEY = "sd.taskGroups";
let taskGroups = (() => {
  try { return JSON.parse(localStorage.getItem(GROUP_KEY) || "[]"); } catch { return []; }
})();
let rawTasks = [];
/// 迅雷转本机时撞了同名：链接 → 等用户选完要去掉的迅雷任务
const xlWaiting = new Map();
const isGroup = (id) => String(id).startsWith("grp:");
let lastCollectionMembers = "";
function syncCollectionMembers() {
  if (typeof DIALOG !== "undefined" && DIALOG) return;
  const ids = taskGroups.filter(group => group.collection).flatMap(group => group.ids);
  const fingerprint = JSON.stringify(ids);
  if (fingerprint === lastCollectionMembers) return;
  lastCollectionMembers = fingerprint;
  nativeInvoke("sync_collection_members", { ids }).catch(() => { lastCollectionMembers = ""; });
}
const saveGroups = () => {
  localStorage.setItem(GROUP_KEY, JSON.stringify(taskGroups));
  syncCollectionMembers();
};
async function openGroupFolder(members) {
  const available = members.filter(task => !task.missing && task.fileState !== "offline");
  const member = available.find(task => task.status === "done") || available[0];
  if (!member) return toast("找不到合集的保存位置");
  try {
    await invoke("open_task_folder", { id: member.id });
  } catch (error) {
    toast("打开文件夹失败：" + error);
  }
}

const groupMembers = (id) => {
  const g = taskGroups.find((x) => x.id === id);
  if (!g) return [];
  const ids = new Set(g.collection ? g.collection.items.map(item => item.id) : g.ids);
  return rawTasks.filter(task => ids.has(task.id));
};

/// 手动一条条点时没事，一口气建十几条就大量 EOF。所以像手动那样隔开建：链接先排在 g.queue，
/// 隔 GROUP_GAP 建一条，同时最多 GROUP_SLOTS 条。不要暂停刚建的任务，会打断它取文件信息
const GROUP_SLOTS = 3;
const GROUP_GAP = 2000;
const groupRetries = new Map();
const collectionSubmissions = new Map();

async function existingCollectionMember(group, entry) {
  const owned = new Set(taskGroups.flatMap(item => item.ids));
  const key = path => String(path).replace(/\\/g, "/").replace(/\/+$/, "");
  const sameFolder = path => group.dir.startsWith("/")
    ? key(path) === key(group.dir) : key(path).toLowerCase() === key(group.dir).toLowerCase();
  const candidates = rawTasks.filter(task => task.meta?.req?.url === entry.url && !task.missing && !owned.has(task.id));
  for (const task of candidates) {
    if (sameFolder(await invoke("task_folder_path", { id: task.id }).catch(() => ""))) return task.id;
  }
  return null;
}

const collectionForEpisode = id => id && taskGroups.find(group => group.collection?.items.some(item => item.id === id));
const restartingEpisodes = new WeakSet();

function collectionRedownloadDescription(ids) {
  const nas = ids.some(id => String(id).startsWith("nas:"));
  const local = ids.some(id => !String(id).startsWith("nas:"));
  return [
    "下载到原来的目录，合集沿用每集保存的下载设置。",
    local && "本机旧文件会移到系统回收站。",
    nas && "NAS 合集会保留已有文件，重名时自动编号。",
  ].filter(Boolean).join("");
}

async function openCollectionRedownload(group) {
  return askConfirm(group.collection.url, {
    confirm: true,
    collection: true,
    redownloadGroup: group.id,
    collectionItems: group.collection.items.map(({ url, name }) => ({ url, name })),
    folderName: group.name,
    dir: group.collection.parent,
    nasRedownload: group.dir.startsWith("/"),
  });
}

async function redownloadCollectionEpisode(group, oldId, record) {
  const item = group.collection.items.find(episode => episode.id === oldId);
  if (!item || restartingEpisodes.has(item)) throw new Error("这一集正在重新下载或已更新");
  if (!group.dir) throw new Error("找不到原来的合集目录");
  restartingEpisodes.add(item);
  try {
    const nas = String(oldId).startsWith("nas:");
    const video = item.video || (!record && !nas ? await invoke("video_task_options", { id: oldId }) : null);
    let id;
    if (record) {
      id = await nativeInvoke("trash_redownload", { id: record.id, dir: group.dir, video, rememberVideo: false });
    } else {
      // NAS file deletion returns before removing files. Keep the old file
      // and let the gateway choose an unused name instead of racing deletion.
      if (!nas) await invoke("recycle_task_file", { id: oldId });
      id = await nativeInvoke("add_task", {
        url: item.url, dir: group.dir, video, forceNew: nas, rememberVideo: false,
      });
    }
    if (!id || id === oldId) throw new Error("没有创建新的下载任务");

    if (group.collection.closed) {
      group.collection.items = group.collection.items.filter(episode => episode.id);
      group.collection.closed = false;
      group.paused = false;
    }
    item.id = id;
    item.video = video;
    delete item.error;
    group.ids.push(id);
    saveGroups();

    if (!record) {
      try { await invoke("forget_task", { id: oldId }); }
      catch { return "新任务已添加，暂时无法移除旧任务记录"; }
    }
    group.ids = group.ids.filter(value => value !== oldId);
    saveGroups();
  } finally {
    restartingEpisodes.delete(item);
  }
}

function scheduleCollection(group) {
  if (group.paused || group.collection.closed || collectionSubmissions.has(group.id)) return;
  const waiting = group.collection.items.filter(item => !item.id && !item.error);
  if (!waiting.length) return;
  const submission = Promise.resolve().then(async () => {
    let cleanupFailures = 0;
    // Register every selected video with the existing video queue. Its local/NAS
    // concurrency limit controls downloads after the dialog or app has closed.
    for (const item of waiting) {
      if (group.paused || group.collection.closed) break;
      try {
        if (item.replaceId && !item.recycled) {
          const old = rawTasks.find(task => task.id === item.replaceId);
          if (!old?.missing) await invoke("recycle_task_file", { id: item.replaceId });
          item.recycled = true;
          saveGroups();
        }
        item.id = await nativeInvoke("add_task", {
          url: item.url, dir: group.dir, rememberDir: group.collection.parent,
          video: item.video ? { ...item.video, collectionMember: true, confirmCollision: !!item.confirmCollision } : null,
          forceNew: !!item.force, rememberVideo: false, collectionId: group.id,
        });
        if (!item.id) throw new Error("任务未创建");
        if (!group.ids.includes(item.id)) group.ids.push(item.id);
        saveGroups();
        if (group.paused && !group.archived) {
          try { await invoke("pause_task", { id: item.id }); }
          catch (error) { item.pauseError = error?.message || String(error); }
        }
        if (item.replaceId && item.replaceId !== item.id) {
          try {
            await invoke("forget_task", { id: item.replaceId });
            group.historyMembers = [...new Set([...(group.historyMembers || []), item.replaceId])];
          } catch {
            cleanupFailures += 1;
          }
          group.ids = group.ids.filter(id => id !== item.replaceId);
        }
        delete item.replaceId;
        delete item.recycled;
      } catch (error) {
        const message = error?.message || String(error);
        item.id = !item.force && /已经在下载列表|NAS 上已有这个任务/.test(message) ? await existingCollectionMember(group, item) : null;
        if (!item.id) item.error = message;
      }
      if (item.id && !group.ids.includes(item.id)) group.ids.push(item.id);
      saveGroups();
    }
    if (cleanupFailures) toast(`已创建新任务，${cleanupFailures} 集的旧任务暂时未归档`);
  }).finally(() => {
    collectionSubmissions.delete(group.id);
    refreshTasks();
  });
  collectionSubmissions.set(group.id, submission);
}

function collectionTask(group, members) {
  const ids = new Set(group.collection.items.map(item => item.id));
  members = members.filter(task => ids.has(task.id));
  const byId = new Map(members.map(task => [task.id, task]));
  const removedIds = new Set(historyRecs(group).map(record => historyMember(group, record)));
  const files = group.collection.items.map((item, index) => {
    const task = byId.get(item.id);
    return {
      index, taskId: task?.id, name: task?.name || item.name, url: item.url, selected: true,
      status: task?.status || (item.error ? "error" : item.id ? removedIds.has(item.id) ? "removed" : "unavailable" : "wait"),
      stage: task?.stage,
      mediaReady: task?.status === "done" || /^(处理字幕|下载字幕)$/.test(task?.stage || ""),
      error: task?.shandianError || item.error,
      fileState: task?.fileState,
      missing: !!task?.missing && task.fileState !== "offline",
      size: task?.meta?.res?.size || 0, downloaded: task?.progress?.downloaded || 0,
    };
  });
  const done = files.filter(file => file.status === "done").length;
  const failed = files.filter(file => file.status === "error").length;
  const collectionMissing = files.filter(file => file.missing || file.status === "removed").length;
  const withSubs = members.filter(canSubStyle);
  const subtitleLabels = [...new Set(withSubs.map(task => task.subs))];
  // Queued episodes have no size yet. Give each selected episode one share
  // so completed transfers never make an unfinished collection look full.
  const fraction = files.reduce((sum, file) => {
    if (file.status === "done") return sum + 1;
    return sum + (file.size > 0 ? Math.min(0.99, file.downloaded / file.size) : 0);
  }, 0) / (files.length || 1);
  const total = files.every(file => file.size > 0) ? files.reduce((sum, file) => sum + file.size, 0) : 0;
  const active = files.some(file => file.taskId && ACTIVE.has(file.status));
  const status = done === files.length ? "done" : group.paused && !active ? "pause"
    : files.some(file => file.status === "running") ? "running"
    : files.some(file => ACTIVE.has(file.status)) ? "wait" : failed ? "error" : "pause";
  return {
    id: group.id, name: group.name, created: group.at, status, protocol: "video",
    kind: members[0]?.kind || "视频合集",
    subs: subtitleLabels.length > 1 ? "多种字幕" : subtitleLabels[0],
    collectionSubtitleCount: withSubs.length,
    collectionSubtitleLabels: subtitleLabels,
    collectionFiles: files,
    collectionSubtitlePending: files.filter(file => file.status === "running" && file.mediaReady).length,
    collectionPolishBusy: members.filter(isPolishing).length,
    collectionPolishFailed: members.filter(task => /润色失败/.test(task.polish || "")).length,
    collectionPolished: members.filter(task => task.polished).length,
    collectionProgress: fraction,
    collectionMissing,
    missing: status === "done" && files.every(file => file.missing),
    progress: { downloaded: files.reduce((sum, file) => sum + file.downloaded, 0), speed: members.reduce((sum, file) => sum + (file.progress?.speed || 0), 0) },
    meta: { req: { url: group.collection.url }, res: { name: group.name, size: total, files } },
    shandianError: status === "error" ? `已完成 ${done}/${files.length} 项，${failed} 项失败，可点继续重试` : "",
  };
}

async function changeCollectionPlayback(group, pause) {
  group.paused = pause;
  for (const item of group.collection.items) {
    delete item.pauseError;
    if (!pause && !item.id) delete item.error;
  }
  saveGroups();
  const snapshot = new Map(rawTasks.map(task => [task.id, task]));
  const sent = new Set();
  const apply = item => {
    if (!item.id || sent.has(item.id)) return;
    sent.add(item.id);
    const status = snapshot.get(item.id)?.status;
    if (pause && (!status || ACTIVE.has(status))) return invoke("pause_task", { id: item.id });
    if (!pause && (!status || ["pause", "error"].includes(status))) return invoke("resume_task", { id: item.id });
  };
  const results = await Promise.allSettled(group.collection.items.map(apply));
  if (pause) {
    await collectionSubmissions.get(group.id);
    results.push(...await Promise.allSettled(group.collection.items.filter(item => item.pauseError).map(item => {
      sent.delete(item.id);
      return apply(item);
    })));
  }
  const failed = results.find(result => result.status === "rejected");
  if (failed) throw failed.reason;
}

async function collectionEmptyFolder(group, members) {
  if (!group.collection || !group.dir) return "";
  const normalize = path => String(path).replaceAll("\\", "/").replace(/\/+$/, "").toLowerCase();
  const wanted = normalize(group.dir);
  for (const id of members) {
    const remote = String(id).startsWith("nas:");
    const actual = remote
      ? await invoke("item_path", { id }).then(path => String(path).replace(/[\\/][^\\/]*$/, "")).catch(() => "")
      : await invoke("task_folder_path", { id }).catch(() => "");
    if (actual && (remote ? normalize(actual).endsWith(wanted) : normalize(actual) === wanted)) return actual;
  }
  return "";
}

async function removeCollection(group, alsoFiles) {
  group.paused = true;
  group.archived = true;
  saveGroups();
  await collectionSubmissions.get(group.id);
  const ids = group.collection
    ? group.collection.items.map(item => item.id).filter(Boolean)
    : groupMembers(group.id).map(task => task.id);
  const remaining = new Set(ids);
  group.historyMembers = [...new Set([...(group.historyMembers || []), ...ids])];
  const directory = alsoFiles ? await collectionEmptyFolder(group, remaining) : "";
  group.queue = [];
  let error;
  for (const id of remaining) {
    try {
      await invoke("delete_task", { id, alsoFiles });
      remaining.delete(id);
    } catch (failure) {
      error ||= failure;
    }
  }
  if (group.collection) {
    if (!remaining.size) group.collection.closed = true;
    else group.collection.items = group.collection.items.filter(item => remaining.has(item.id));
  }
  saveGroups();
  if (error) throw error;
  if (directory) {
    try {
      await invoke("remove_empty_collection_dir", { path: directory, waitForFiles: ids.some(id => String(id).startsWith("nas:")) });
    } catch (failure) {
      throw new Error("任务已删除，但无法清理空文件夹：" + failure);
    }
  }
}

function scheduleGroup(g, ms) {
  // 成员都被删了：用户不要这组了，排队的也不下
  if (g.ids.length && !ms.length) g.queue = [];
  if (g.paused) return;
  let free = GROUP_SLOTS - ms.filter((t) => ACTIVE.has(t.status)).length;
  for (const t of ms) {
    if (free <= 0) break;
    if (t.status === "pause") {
      free -= 1;
      invoke("resume_task", { id: t.id }).catch(() => {});
    } else if (t.status === "error") {
      const r = groupRetries.get(t.id) || { n: 0, at: Date.now() };
      groupRetries.set(t.id, r);
      if (r.n >= 3 || Date.now() - r.at < 15000 * (r.n + 1)) continue;
      r.n += 1;
      r.at = Date.now();
      free -= 1;
      invoke("resume_task", { id: t.id }).catch(() => {});
    }
  }
  if (free > 0 && g.queue?.length && !g.adding && Date.now() - (g.lastAdd || 0) > GROUP_GAP) {
    const x = g.queue.shift();
    g.adding = true;
    g.lastAdd = Date.now();
    const submission = invoke("add_task", { url: x.url, name: x.name || null, dir: g.dir || null, video: null, headers: x.headers || null, forceNew: true, selectFiles: null })
      .then((id) => {
        if (id) g.ids.push(id, `nas:${id}`);
      })
      .catch((e) => toast(`「${x.name || x.url.slice(0, 40)}」添加失败：${e}`))
      .finally(() => {
        g.adding = false;
        collectionSubmissions.delete(g.id);
        saveGroups();
      });
    collectionSubmissions.set(g.id, submission);
  }
}

/// 下载历史也合成一条：记录靠 taskId 认回所属的组
let rawTrash = [];
let trashLoaded = false;
const historyId = (g) => `h${g.id}`;
function historyMember(group, record) {
  if (!record.taskId) return null;
  const ids = [...group.ids, ...(group.historyMembers || [])];
  if (ids.includes(record.taskId)) return record.taskId;
  // Older Windows video history stored the native job ID without the UI's "v:" prefix.
  const legacy = "v:" + record.taskId;
  return ids.includes(legacy) ? legacy : null;
}
const historyRecs = group => rawTrash.filter(record => historyMember(group, record));
const historyOwner = record => taskGroups.find(group => historyMember(group, record));
const groupTitle = (group, count) => group.collection || count === 1 ? group.name : `${group.name} 等 ${count} 个文件`;
const historyGroup = (id) => taskGroups.find((g) => historyId(g) === id);
function mergeTrash(list) {
  rawTrash = list;
  trashLoaded = true;
  const inGroup = new Set();
  const out = [];
  for (const g of taskGroups) {
    const recs = historyRecs(g);
    if (!recs.length) continue;
    recs.forEach((r) => inGroup.add(r.id));
    const sample = recs.find(record => record.fileState === "here") || recs[0];
    out.push({
      ...sample,
      id: historyId(g),
      title: groupTitle(g, g.total || recs.length),
      groupCount: recs.length,
      size: recs.reduce((n, r) => n + (r.size || 0), 0),
      taskId: sample.taskId,
      removedAt: Math.max(...recs.map(record => record.removedAt || 0)),
    });
  }
  return [...out, ...list.filter((r) => !inGroup.has(r.id))];
}

function recoverNasCollections(list) {
  const videos = list.filter(task => String(task.id).startsWith("nas:v-"));
  if (!videos.length) return;
  const owned = new Set(taskGroups.flatMap(group => group.ids));
  const collections = taskGroups.filter(group => group.collection && !group.collection.closed && group.dir?.startsWith("/"));
  const matchesFolder = (task, group) => sameDownloadDirectory(task.meta?.opts?.path || "", group.dir);
  const linkOf = task => task.meta?.req?.url;
  const labelOf = task => task.meta?.req?.labels?.["shandian.collection"];
  let changed = false;
  for (const group of collections) {
    // NAS confirms this ownership even if the App closed before receiving the add response.
    for (const item of group.collection.items) {
      if (item.id) continue;
      const matches = videos.filter(task => !owned.has(task.id) && labelOf(task) === group.id
        && matchesFolder(task, group) && linkOf(task) === item.url);
      if (matches.length !== 1) continue;
      item.id = matches[0].id;
      delete item.error;
      group.ids.push(item.id);
      owned.add(item.id);
      changed = true;
    }

    // Old gateways did not store an owner. Restore only a second complete,
    // uniquely matched batch launched together in the same collection folder.
    const items = group.collection.items;
    const urls = new Set(items.map(item => item.url));
    if (items.length < 2 || urls.size !== items.length || items.some(item => !item.id)) continue;
    const matches = videos.filter(task => !labelOf(task) && !owned.has(task.id)
      && task.status === "done" && matchesFolder(task, group) && urls.has(linkOf(task)));
    if (matches.length !== items.length || new Set(matches.map(linkOf)).size !== urls.size) continue;
    const at = Math.min(...matches.map(task => task.created));
    const last = Math.max(...matches.map(task => task.created));
    if (!Number.isFinite(at) || at <= 0 || last - at > 10000) continue;
    const byUrl = new Map(matches.map(task => [linkOf(task), task]));
    const restored = items.map(item => ({ url: item.url, name: item.name, id: byUrl.get(item.url).id, video: item.video }));
    const ids = restored.map(item => item.id);
    ids.forEach(id => owned.add(id));
    taskGroups.push({
      id: `grp:nas-history-${ids[0].slice(4)}`, name: group.name, dir: group.dir, at, ids,
      collection: { url: group.collection.url, parent: group.collection.parent, items: restored },
    });
    changed = true;
  }
  if (changed) saveGroups();
}

function mergeGroups(list) {
  rawTasks = list;
  recoverNasCollections(list);
  syncCollectionMembers();
  const inGroup = new Set();
  const out = [];
  const before = taskGroups.length;
  taskGroups = taskGroups.filter((g) => g.archived || g.collection && !g.collection.closed || list.some((t) => g.ids.includes(t.id)) || g.queue?.length || !trashLoaded || historyRecs(g).length || Date.now() - g.at < 60000);
  if (taskGroups.length !== before) saveGroups();
  for (const g of taskGroups) {
    const ms = list.filter((t) => g.ids.includes(t.id));
    if (g.collection) {
      ms.forEach((t) => inGroup.add(t.id));
      if (!g.collection.closed) {
        scheduleCollection(g);
        out.push(collectionTask(g, ms));
      }
      continue;
    }
    scheduleGroup(g, ms);
    if (!ms.length) continue;
    ms.forEach((t) => inGroup.add(t.id));
    const has = (s) => ms.some((t) => t.status === s);
    const status = g.queue?.length && !g.paused && !has("running") ? "running" : ms.every((t) => t.status === "done") && !g.queue?.length ? "done" : has("running") ? "running" : has("wait") ? "wait" : has("error") ? "error" : "pause";
    const sum = (f) => ms.reduce((n, t) => n + (f(t) || 0), 0);
    const count = g.total || ms.length;
    const left = groupTitle(g, count);
    const failed = ms.filter((t) => t.status === "error");
    const finished = ms.filter((t) => t.status === "done").length;
    out.push({
      ...ms[0],
      id: g.id,
      name: left,
      status,
      kind: ms[0].kind,
      stage: undefined,
      missing: ms.every((t) => t.missing),
      progress: { ...ms[0].progress, downloaded: sum((t) => t.progress?.downloaded), speed: sum((t) => t.progress?.speed) },
      meta: { ...ms[0].meta, req: { url: "" }, res: { name: left, size: sum((t) => t.meta?.res?.size), files: [] } },
      // 不显示原始报错：里面是带令牌的完整链接，一百个文件也看不过来
      shandianError: status === "error" ? `已完成 ${finished}/${count}，${failed.length} 个文件失败，可点继续重试` : "",
    });
  }
  return [...out, ...list.filter((t) => !inGroup.has(t.id))];
}

function multiCommonHead(names) {
  const first = String(names[0] || "").split("/")[0];
  return names.length && names.every((n) => String(n).startsWith(first + "/")) ? first : "";
}

/// 文件夹名：取文件名的共同开头（如「第一季 E01」「第一季 E02」→「第一季」），太短就用第一个文件名
function multiFileStem(name) {
  const file = String(name || "").split(/[\\/]/).pop();
  return file.replace(/(?:\.tar\.(?:gz|bz2|xz|zst)|\.(?=[a-z0-9]{1,15}$)[0-9]*[a-z][a-z0-9]*)$/i, "");
}

function multiFolderName(names) {
  // 选的是网盘里的一个文件夹：名字都以它开头（Houdini tools/a.zip），直接用它
  const head = multiCommonHead(names);
  if (head) return head;
  const stems = names.map(multiFileStem);
  let p = stems[0] || "";
  for (const s of stems) while (p && !s.startsWith(p)) p = p.slice(0, -1);
  p = p.replace(/[\s._\-–—(（\[【第]+$/, "");
  return p.length >= 2 ? p : stems[0] || "多文件下载";
}

/// 文件抽屉：点任务行上的「n 项」打开。勾选马上生效（隔一小会儿一起提交）
let filesPanelId = null;
let filesTimer = null;
let filesList = [];
let filesSaving = null;
const drawerFileSelection = new Set();
const drawerFileKey = file => file.taskId || file.index;
let drawerAnchor = null;
function drawerSelectionEnabled(task = state.tasks.find(item => item.id === filesPanelId)) {
  return !!task && (task.status === "done" || !!task.collectionFiles || isBaiduFileTask(task));
}
function applyDrawerSelection(ids) {
  if (ids.size === drawerFileSelection.size && [...ids].every(id => drawerFileSelection.has(id))) return;
  drawerFileSelection.clear();
  ids.forEach(id => drawerFileSelection.add(id));
  drawFilesRows();
}
function drawerFileAt(element) {
  const index = Number(element.closest(".fd-row")?.querySelector("[data-i]")?.dataset.i);
  const file = filesList[index];
  return file?.index === index ? file : filesList.find(file => file.index === index);
}

function drawerFilesFor(file) {
  if (file && !drawerFileSelection.has(drawerFileKey(file))) return [file];
  return filesList.filter(item => drawerFileSelection.has(drawerFileKey(item)));
}

function drawerSubtitleTasks(files, members = groupMembers(filesPanelId)) {
  const ids = new Set(files.map(file => file.taskId));
  return members.filter(task => ids.has(task.id));
}
const DRAWER_AUDIO_MODES = new Map([
  ["single", ["单曲播放", "i-music"]],
  ["repeat-one", ["单曲循环", "i-audio-repeat-one"]],
  ["order", ["顺序播放", "i-audio-order"]],
  ["repeat-list", ["列表循环", "i-audio-repeat"]],
  ["shuffle", ["随机播放", "i-audio-shuffle"]],
]);
const drawerAudioPlayed = new Set();
let drawerAudioMode = "single";
try {
  const saved = localStorage.getItem("sd.drawerAudioMode");
  if (DRAWER_AUDIO_MODES.has(saved)) drawerAudioMode = saved;
} catch {}
const drawerAudio = createDrawerAudioPlayer($("files-audio"), syncDrawerAudioButtons, advanceDrawerAudio);
setDrawerAudioMode(drawerAudioMode);

function canPlayDrawerAudio(file) {
  return file?.selected && file.status === "done" && !file.missing && file.fileState !== "offline" &&
    typeOfName(file.name) === "audio";
}

function setDrawerAudioMode(mode) {
  drawerAudioMode = mode;
  drawerAudioPlayed.clear();
  const current = drawerAudio.current();
  if (mode === "shuffle" && current !== null) drawerAudioPlayed.add(current);
  drawerAudio.setLoop(mode === "repeat-one");
  const [name, icon] = DRAWER_AUDIO_MODES.get(mode);
  const button = $("files-audio-mode");
  const label = `播放模式：${name}`;
  button.setAttribute("aria-label", label);
  button.dataset.tip = label;
  button.querySelector("use").setAttribute("href", "#" + icon);
  try { localStorage.setItem("sd.drawerAudioMode", mode); } catch {}
}

function playDrawerAudio(index) {
  const file = filesList.find(item => item.index === index);
  const id = filesPanelId;
  if (!id || !canPlayDrawerAudio(file)) return;
  if (drawerAudioMode === "shuffle") drawerAudioPlayed.add(index);
  return drawerAudio.toggle(index, file.name, async () => {
    const source = await invoke("task_drawer_audio", { id, index });
    return { url: window.__TAURI__.core.convertFileSrc(source.path), size: source.size };
  });
}

function advanceDrawerAudio(index) {
  if (!filesPanelId || drawerAudioMode === "single" || drawerAudioMode === "repeat-one") return;
  const tracks = filesList.filter(canPlayDrawerAudio);
  const position = tracks.findIndex(file => file.index === index);
  if (position < 0) return;
  let next = tracks[position + 1];
  if (drawerAudioMode === "repeat-list" && !next) next = tracks[0];
  if (drawerAudioMode === "shuffle") {
    const others = tracks.filter(file => file.index !== index);
    let available = others.filter(file => !drawerAudioPlayed.has(file.index));
    if (!available.length) {
      drawerAudioPlayed.clear();
      drawerAudioPlayed.add(index);
      available = others.length ? others : tracks;
    }
    next = available[Math.floor(Math.random() * available.length)];
  }
  if (next) playDrawerAudio(next.index);
}

function syncDrawerAudioButtons() {
  const current = drawerAudio.current();
  const playing = drawerAudio.playing();
  for (const button of $("files-list").querySelectorAll("[data-file-audio]")) {
    const isCurrent = Number(button.dataset.fileAudio) === current;
    button.closest(".fd-row").setAttribute("aria-current", String(isCurrent));
    button.dataset.playing = String(isCurrent && playing);
    const label = isCurrent && playing ? "暂停音频" : "播放音频";
    button.setAttribute("aria-label", label);
    button.querySelector("use").setAttribute("href", isCurrent && playing ? "#i-pause" : "#i-media-play");
  }
}
function setDrawerSelection(on) {
  drawerFileSelection.clear();
  if (on) {
    for (const f of filesList) drawerFileSelection.add(drawerFileKey(f));
  }
  drawFilesRows();
}
function closeFilesPanel() {
  drawerAudio.close();
  drawerAudioPlayed.clear();
  if ($("ctx-menu")?.dataset.owner === "drawer-files") closeCtx();
  if (String(filesPanelId).startsWith("xl:")) invoke("thunder_panel_close").catch(() => {});
  filesPanelId = null;
  clearTimeout(filesTimer);
  // 收回去的动画和左侧侧边栏一致，播完再隐藏
  const box = $("files-panel");
  box.classList.add("closed");
  setTimeout(() => { if (!filesPanelId) box.hidden = true; }, 260);
}
/// 任务行上的按钮写「已下载 2/5 项」。计数隔几秒查一次，只查没下完的多文件任务
const fileCounts = new Map();
function isBaiduFileTask(task) {
  return String(task?.id || "").startsWith("bd:");
}

function fileChipText(t) {
  if (isBaiduFileTask(t)) return baiduFileProgress(t);
  if (t.collectionFiles) {
    const downloaded = t.collectionFiles.filter(file => file.mediaReady).length;
    return `已下载 ${downloaded}/${t.collectionFiles.length} 项`;
  }
  if (t.xlItems) return `已下载 ${t.xlItems[0]}/${t.xlItems[1]} 项`;
  const n = t.meta?.res?.files?.length || t.meta?.req?.extra?.files?.length || 0;
  const c = fileCounts.get(t.id);
  const sel = t.meta?.opts?.selectFiles?.length || n;
  if (!c && t.status === "done") return `已下载 ${c?.total ?? sel}/${c?.total ?? sel} 项`;
  return c ? `已下载 ${c.done}/${c.total} 项` : `已下载 0/${sel} 项`;
}
let fileCountAt = 0;
async function refreshFileCounts() {
  if (Date.now() - fileCountAt < 3000) return;
  fileCountAt = Date.now();
  for (const t of state.tasks.filter((x) => hasFileList(x) && !x.collectionFiles && !isBaiduFileTask(x) && x.status !== "done" && !String(x.id).startsWith("xl:"))) {
    const list = await invoke("task_file_list", { id: t.id }).catch(() => null);
    if (!list) continue;
    const on = list.filter((f) => f.selected);
    fileCounts.set(t.id, { done: on.filter((f) => f.status === "done").length, total: on.length });
    const chip = document.querySelector(`.task[data-id="${CSS.escape(t.id)}"] .files-chip`);
    if (chip) chip.querySelector("span").textContent = fileChipText(t);
  }
}
function hasFileList(t) {
  if (t?.collectionFiles) return true;
  if (isBaiduFileTask(t)) return !!baiduFileProgress(t);
  // 迅雷任务：文件在迅雷的详情面板里，下完的迅雷任务行已经不在「下载中」，不给入口
  if (String(t?.id).startsWith("xl:")) return t.status !== "done" && t.xlItems?.[1] > 1;
  if (!t || /^(v:|xl:|pp:|nas:|grp:)/.test(String(t.id))) return false;
  return String(t.meta?.req?.url || "").startsWith("shandian-multi:") || (t.meta?.res?.files || []).length > 1;
}
function fileRetryTip(f) {
  const reason = f.error || f.lastError || "";
  const count = f.tries ? `已自动重试 ${f.tries} 次。` : "";
  if (/(?:code:\s*|status=)(401|403|404|410)\b/.test(reason)) return `${count}网盘拒绝了当前链接，请确认账号权限并回到网盘重新获取下载地址。`;
  if (f.status === "retrying") return `${count}正在等待重试，仅处理此文件。${reason}`;
  return `${count}${reason}`;
}

function drawFilesRows() {
  const box = $("files-list");
  const scroll = $("files-scroll");
  const top = scroll.scrollTop;
  const t = state.tasks.find((x) => x.id === filesPanelId);
  const collection = !!t?.collectionFiles;
  const locked = drawerSelectionEnabled(t);
  const indices = new Set(filesList.map(drawerFileKey));
  for (const index of drawerFileSelection) {
    if (!indices.has(index)) drawerFileSelection.delete(index);
  }
  if (!indices.has(drawerAnchor)) drawerAnchor = null;
  const members = collection ? new Map(groupMembers(filesPanelId).map(task => [task.id, task])) : null;
  const batch = $("files-fill-subtitles");
  const subtitleAction = members && collectionSubtitleAction([...members.values()]);
  batch.hidden = !subtitleAction;
  if (subtitleAction) {
    batch.setAttribute("aria-disabled", String(!subtitleAction.targets.length));
    batch.textContent = subtitleAction.label;
  }
  const focused = box.contains(document.activeElement) ? document.activeElement.dataset : null;
  const panel = $("files-panel");
  panel.classList.toggle("collection-files", collection);
  panel.classList.toggle("baidu-video-files", filesList.some(file => baiduDrawerCover(filesPanelId, file)));
  const mediaFiles = !collection && filesList.some(file => canPlayDrawerAudio(file) ||
    (file.status === "done" && file.selected && typeOfName(file.name) === "video"));
  panel.classList.toggle("media-files", mediaFiles);
  const canOpenFiles = !String(filesPanelId).startsWith("xl:");
  const rows = filesList.map((f) => {
    const collectionStatus = t?.collectionFiles && ({
      wait: "等待中", ready: "等待中", pause: "已暂停", done: "100%",
      running: f.stage || (!f.size ? "下载中" : f.downloaded >= f.size ? "收尾中" : ""),
    })[f.status];
    const unavailable = f.missing ? "文件缺失" : collection && (f.fileState === "offline" ? "存储离线"
      : f.status === "removed" ? "已移除"
      : f.status === "unavailable" ? "暂不可用" : "");
    const pct = unavailable || collectionStatus || (f.status === "retrying" ? `重试 ${f.tries || 0}/${f.retryLimit || 3}` : f.status === "error" ? "失败" : !f.selected ? "—" : f.percent ? f.percent : f.size ? `${Math.min(100, Math.floor((f.downloaded / f.size) * 100))}%` : f.status === "done" ? "100%" : "—");
    const canOpen = canOpenFiles && f.selected && f.status === "done" && !unavailable && !f.missing;
    const open = canOpen ? ` data-open-file="${Number(f.index)}" tabindex="0"` : "";
    const tip = canOpen ? `${f.name} · 双击打开` : unavailable ? f.name + " · " + unavailable : f.name;
    const task = members?.get(f.taskId);
    const subtitleControls = collection && task?.status === "done" ? renderSubtitleControls(task) : "";
    const subtitleStatus = subtitleControls ? `<span class="fd-file-note task-meta subtitle-controls">${subtitleControls}</span>` : "";
    const selected = drawerFileSelection.has(drawerFileKey(f));
    const checked = locked ? selected : f.selected;
    const type = typeOfName(f.name);
    const playAudio = canOpen && !collection && canPlayDrawerAudio(f);
    const playVideo = canOpen && (type === "video" || (collection && f.taskId));
    const playLabel = playAudio ? "播放音频" : "播放视频";
    const play = playAudio || playVideo
      ? `<button type="button" class="circle sm fd-play-control" data-file-${playAudio ? "audio" : "video"}="${f.index}" aria-label="${playLabel}"><svg class="ic"><use href="#i-media-play"/></svg></button>` : "";
    const actions = collection || mediaFiles ? `<span class="fd-file-actions">${play}</span>` : "";
    const coverId = collection ? f.taskId : baiduDrawerCover(filesPanelId, f);
    return `<div class="fd-row${f.selected ? "" : " off"}${selected ? " fd-selected" : ""}"${open}><${collection ? "div" : "label"} class="fd-file-entry" data-tip="${escapeHtml(tip)}"><input type="checkbox" aria-label="${escapeHtml(f.name)}" data-i="${f.index}"${checked ? " checked" : ""}>${coverId ? episodeCoverHtml(coverId) : ""}<span class="fd-file-info"><span class="n">${escapeHtml(f.name)}</span>${subtitleStatus}</span></${collection ? "div" : "label"}>${actions}<span class="p ${f.status}${f.missing ? " file-missing" : ""}"${f.missing ? ' data-tip="原路径未找到文件，可能已移动、重命名或删除。"' : (f.error || f.lastError || f.tries) ? ` data-tip="${escapeHtml(fileRetryTip(f))}"` : ""}>${escapeHtml(pct)}</span><span class="s">${f.size ? bytes(f.size) : "—"}</span></div>`;
  });
  const previous = box.__fileRows;
  if (previous?.length !== rows.length || box.children.length !== rows.length) {
    box.innerHTML = rows.join("");
  } else {
    rows.forEach((markup, index) => {
      if (previous[index] === markup) return;
      const row = box.children[index];
      row.insertAdjacentHTML("afterend", markup);
      row.remove();
    });
  }
  box.__fileRows = rows;
  syncDrawerAudioButtons();
  scroll.scrollTop = top;
  if (focused?.fileAudio !== undefined) box.querySelector(`[data-file-audio="${CSS.escape(focused.fileAudio)}"]`)?.focus({ preventScroll: true });
  else if (focused?.fileVideo !== undefined) box.querySelector(`[data-file-video="${CSS.escape(focused.fileVideo)}"]`)?.focus({ preventScroll: true });
  else if (focused?.i !== undefined) box.querySelector(`[data-i="${CSS.escape(focused.i)}"]`)?.focus({ preventScroll: true });
  else if (focused?.subtitleAction) {
    box.querySelector(`[data-subtitle-action="${CSS.escape(focused.subtitleAction)}"][data-task-id="${CSS.escape(focused.taskId)}"]`)?.focus({ preventScroll: true });
  }
  const on = filesList.filter((f) => f.selected);
  const all = $("files-all");
  const selectedCount = drawerFileSelection.size;
  const count = locked ? selectedCount : on.length;
  all.checked = !!filesList.length && count === filesList.length;
  all.indeterminate = count > 0 && count < filesList.length;
  $("files-all-label").hidden = !filesList.length;
  const totalSize = bytes(on.reduce((n, f) => n + (f.size || 0), 0));
  const progress = collection || isBaiduFileTask(t) ? fileChipText(t) : `已下载 ${on.filter(file => file.status === "done").length}/${on.length} 项，${totalSize}`;
  $("files-count").textContent = locked
    ? progress + (count ? ` · 已选 ${count}` : "")
    : `下载 ${on.length} / ${filesList.length} 项，${totalSize}` + (selectedCount ? ` · 已选 ${selectedCount}` : "");
}
function drawerListStatus(text, loading = false) {
  const status = $("files-list-status");
  status.hidden = !text;
  setProbeState(status, text, loading);
}

async function drawFilesPanel() {
  const id = filesPanelId;
  if (!id) return;
  const task = state.tasks.find(item => item.id === id);
  if (task?.collectionFiles) {
    filesList = task.collectionFiles;
    drawerListStatus("");
    drawFilesRows();
    return;
  }
  const isXl = String(id).startsWith("xl:");
  try {
    const list = await invoke("task_file_list", { id });
    if (filesPanelId !== id) return;
    if (!list?.length) {
      drawerListStatus("这条任务没有可显示的文件记录");
      return;
    }
    if (!filesSaving) {
      filesList = list;
      const selected = list.filter(file => file.selected);
      fileCounts.set(id, { done: selected.filter(file => file.status === "done").length, total: selected.length });
      drawerListStatus("");
      drawFilesRows();
      if (isBaiduFileTask({ id })) fillThumbs();
    }
    if (task?.status === "done" && list.every(file => !file.selected || file.status === "done")) return;
    filesTimer = setTimeout(drawFilesPanel, isXl ? 3000 : 1000);
  } catch (error) {
    if (filesPanelId !== id) return;
    const message = "读取文件列表失败：" + (error?.message || String(error));
    drawerListStatus(message);
    const retry = document.createElement("button");
    retry.type = "button";
    retry.className = "ghost sm";
    retry.textContent = "重试";
    retry.onclick = () => {
      drawerListStatus("正在读取文件…", true);
      drawFilesPanel();
    };
    $("files-list-status").appendChild(retry);
  }
}

function saveFileSelection() {
  clearTimeout(filesSaving);
  const id = filesPanelId;
  const select = filesList.filter(file => file.selected).map(file => file.index);
  filesSaving = setTimeout(async () => {
    await invoke("task_select_files", { id, select }).catch((e) => toast(String(e)));
    filesSaving = null;
    refreshTasks();
  }, 400);
}
function openFilesPanel(id) {
  // 再点同一条的按钮：收起
  if (filesPanelId === id) return closeFilesPanel();
  const t = state.tasks.find((x) => x.id === id);
  clearTimeout(filesTimer);
  $("files-title").textContent = t?.name || "文件";
  $("files-fill-subtitles").hidden = true;
  if ($("ctx-menu")?.dataset.owner === "drawer-files") closeCtx();
  drawerAudio.close();
  drawerAudioPlayed.clear();
  const list = $("files-list");
  list.replaceChildren();
  list.__fileRows = null;
  $("files-count").textContent = "";
  $("files-all-label").hidden = true;
  drawerListStatus("正在读取文件…", true);
  drawerFileSelection.clear();
  drawerAnchor = null;
  filesList = [];
  const box = $("files-panel");
  if (box.hidden) {
    box.classList.add("closed");
    box.hidden = false;
    void box.offsetWidth; // 先定在屏幕外，下一步才有滑入动画
  }
  box.classList.remove("closed");
  filesPanelId = id;
  drawFilesPanel();
  if (t?.collectionFiles) fillThumbs();
}
$("files-x").addEventListener("click", closeFilesPanel);
$("files-fill-subtitles").addEventListener("click", () => {
  if (!filesPanelId) return;
  const action = collectionSubtitleAction(groupMembers(filesPanelId));
  if (action?.targets.length) fetchSubtitlesBatch(action.targets, action.mode);
});
// 点抽屉外面任何地方都收起。点别的任务的按钮是切换，由按钮自己处理；弹窗和菜单里的操作不算
document.addEventListener("pointerdown", (e) => {
  if (!filesPanelId) return;
  if (e.target.closest?.("#files-panel, .files-chip, .modal-mask, .ctx-menu, .toast")) return;
  closeFilesPanel();
});
// 拖左边缘调宽度，记住
{
  const box = $("files-panel");
  const saved = Number(localStorage.getItem("sd.filesWidth"));
  if (saved) box.style.width = `${saved}px`;
  const grip = $("files-grip");
  grip.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    grip.setPointerCapture(e.pointerId);
    box.classList.add("resizing");
    const move = (m) => {
      const w = Math.max(220, Math.min(innerWidth - 240, innerWidth - m.clientX));
      box.style.width = `${w}px`;
    };
    const up = () => {
      grip.removeEventListener("pointermove", move);
      box.classList.remove("resizing");
      localStorage.setItem("sd.filesWidth", String(box.offsetWidth));
    };
    grip.addEventListener("pointermove", move);
    grip.addEventListener("pointerup", up, { once: true });
    grip.addEventListener("pointercancel", up, { once: true });
  });
}
$("files-scroll").addEventListener("click", (event) => {
  if (!filesPanelId || event.target.closest(".fd-row, button, a, input, select, label")) return;
  drawerAnchor = null;
  setDrawerSelection(false);
});
const drawerFiles = $("files-list");
drawerFiles.addEventListener("click", (event) => {
  const audio = event.target.closest("[data-file-audio]");
  if (audio) return playDrawerAudio(Number(audio.dataset.fileAudio));
  const video = event.target.closest("[data-file-video]");
  if (video && filesPanelId) {
    drawerAudio.close();
    return openDownloadedDrawerFile(Number(video.dataset.fileVideo));
  }
  const file = drawerFileAt(event.target);
  if (file && !event.target.closest("button,a,[data-subtitle-action]")) {
    const add = event.ctrlKey || event.metaKey;
    if (!event.target.closest("input") || add || event.shiftKey) {
      event.preventDefault();
      const key = drawerFileKey(file);
      applyDrawerSelection(selectIds(drawerFileSelection, filesList.map(drawerFileKey), drawerAnchor, key,
        { add, range: event.shiftKey }));
      if (!event.shiftKey || drawerAnchor === null) drawerAnchor = key;
    }
    return;
  }
  if (event.target.closest("[data-open-file] label") && !event.target.closest("input")) event.preventDefault();
});
function openDrawerFile(event) {
  if (event.target.closest("input, button")) return;
  const row = event.target.closest("[data-open-file]");
  if (!filesPanelId || !row) return;
  event.preventDefault();
  openDownloadedDrawerFile(Number(row.dataset.openFile));
}
function openDownloadedDrawerFile(index) {
  const file = filesList.find(item => item.index === index);
  if (!file || file.status !== "done" || !file.selected || file.missing || file.fileState === "offline") return;
  const command = file?.taskId
    ? invoke("item_open", { id: file.taskId })
    : invoke("task_open_downloaded_file", { id: filesPanelId, index });
  command
    .catch((error) => toast(String(error)));
}
drawerFiles.addEventListener("dblclick", openDrawerFile);
drawerFiles.addEventListener("keydown", (event) => {
  if (event.key === "Enter" && !event.repeat) openDrawerFile(event);
});

drawerFiles.addEventListener("contextmenu", (event) => {
  const row = event.target.closest(".fd-row");
  if (!filesPanelId || !row) return;
  const file = drawerFileAt(row);
  if (!file) return;
  event.preventDefault();
  event.stopPropagation();
  openDrawerFileMenu(drawerFilesFor(file), {
    left: event.clientX, right: event.clientX, top: event.clientY, bottom: event.clientY,
  }, file);
});

function openDrawerFileMenu(files, anchor, clickedFile = null) {
  if (!files.length) return;
  const tasks = drawerSubtitleTasks(files);
  const styled = tasks.filter(task => canSubStyle(task) && !isFetchingTaskSubtitles(task));
  const action = selectedSubtitleAction(tasks);
  const polishable = tasks.filter(canPolishTask);
  const restorable = polishable.filter(task => task.polished);
  const available = files.filter(file => file.status === "done" && file.selected && !file.missing && file.fileState !== "offline");
  const canReveal = files.length === 1 && available.includes(clickedFile);
  const links = files.map(file => file.url || tasks.find(task => task.id === file.taskId)?.meta?.req?.url).filter(Boolean);
  const drawerId = filesPanelId;

  closeCtx();
  const menu = ctxEl();
  const add = (title, icon, run, disabled = false) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "ctx-item";
    button.disabled = disabled;
    setMenuLabel(button, title, icon);
    button.onclick = () => {
      closeCtx();
      run();
    };
    menu.append(button);
  };
  menu.replaceChildren();
  menu.dataset.owner = "drawer-files";
  if (styled.length) {
    const label = files.length > 1 ? `字幕样式…（${styled.length}）` : "字幕样式…";
    add(label, "i-doc", () => openSubStyle(styled.map(task => task.id)));
  }
  if (action) {
    const single = files.length === 1;
    const label = !action.targets.length ? "正在获取…"
      : single ? (styled.length ? "重新获取字幕" : "获取字幕") : action.label;
    add(label, "i-doc", () => {
      if (single) fetchTaskSubtitles(action.targets[0].id);
      else fetchSubtitlesBatch(action.targets, action.mode);
    }, !action.targets.length);
  }
  if (polishable.length) {
    const count = files.length > 1 ? `（${polishable.length}）` : "";
    add((polishable.every(task => task.polished) ? "重新润色字幕" : "润色字幕") + count, "i-doc", () => polishCollection(polishable));
  }
  if (restorable.length) {
    const count = files.length > 1 ? `（${restorable.length}）` : "";
    add("恢复原字幕" + count, "i-history", () => polishCollection(restorable, true));
  }
  if (canReveal) {
    const id = filesPanelId;
    add("在资源管理器中选中", "i-folder", async () => {
      try {
        if (clickedFile.taskId) {
          const path = await invoke("item_path", { id: clickedFile.taskId });
          await invoke("reveal_path", { path });
        } else {
          await invoke("task_open_downloaded_file", { id, index: clickedFile.index, reveal: true });
        }
      } catch (error) {
        toast(String(error));
      }
    });
  }
  const count = size => size > 1 ? "（" + size + " 项）" : "";
  const copyItems = async values => {
    const items = values.filter(Boolean);
    toast(items.length && await copyText(items.join("\n")) ? "已复制" + count(items.length) : "没有可复制的内容或复制失败");
  };
  add("复制名称" + count(files.length), "i-clipboard", () => copyItems(files.map(file => file.name)));
  if (links.length) add("复制链接" + count(links.length), "i-link", () => copyItems(links));
  if (available.length) add("复制完整路径" + count(available.length), "i-folder", async () => {
    const paths = [];
    for (const file of available) {
      try {
        paths.push(await (file.taskId
          ? invoke("item_path", { id: file.taskId })
          : invoke("task_downloaded_file_path", { id: drawerId, index: file.index })));
      } catch {}
    }
    await copyItems(paths);
  });
  showDrawerMenu(menu, anchor, false);
}
function showDrawerMenu(menu, anchor, alignRight) {
  menu.hidden = false;
  const rect = menu.getBoundingClientRect();
  const x = alignRight ? anchor.right - rect.width : anchor.left;
  menu.style.left = Math.max(0, Math.min(x, window.innerWidth - rect.width)) + "px";
  menu.style.top = (anchor.bottom + rect.height <= window.innerHeight ? anchor.bottom : Math.max(0, anchor.top - rect.height)) + "px";
  menu.querySelector("button:not(:disabled)")?.focus({ preventScroll: true });
}
$("files-audio-mode").addEventListener("click", (event) => {
  event.stopPropagation();
  const control = event.currentTarget;
  if (control.getAttribute("aria-expanded") === "true") return closeCtx();
  closeCtx();
  const menu = ctxEl();
  menu.replaceChildren();
  menu.dataset.owner = "drawer-files";
  control.setAttribute("aria-expanded", "true");
  control.setAttribute("aria-controls", menu.id);
  for (const [mode, [label, icon]] of DRAWER_AUDIO_MODES) {
    const option = document.createElement("button");
    option.type = "button";
    option.className = "ctx-item";
    option.setAttribute("aria-pressed", String(mode === drawerAudioMode));
    setMenuLabel(option, label, icon);
    if (mode === drawerAudioMode) {
      const mark = document.createElement("span");
      mark.className = "sort-mark";
      mark.textContent = "✓";
      mark.setAttribute("aria-hidden", "true");
      option.append(mark);
    }
    option.onclick = () => {
      setDrawerAudioMode(mode);
      closeCtx();
      control.focus({ preventScroll: true });
    };
    menu.append(option);
  }
  showDrawerMenu(menu, control.getBoundingClientRect(), true);
  menu.querySelector('[aria-pressed="true"]')?.focus({ preventScroll: true });
});
drawerFiles.addEventListener("change", (e) => {
  const f = filesList.find((x) => x.index === Number(e.target.dataset.i));
  if (!f) return;
  const task = state.tasks.find(item => item.id === filesPanelId);
  if (task?.collectionFiles || task?.status === "done" || isBaiduFileTask(task)) {
    drawerAnchor = drawerFileKey(f);
    if (e.target.checked) drawerFileSelection.add(drawerFileKey(f));
    else drawerFileSelection.delete(drawerFileKey(f));
    drawFilesRows();
    return;
  }
  // 至少留一个
  if (!e.target.checked && filesList.filter((x) => x.selected).length === 1) {
    e.target.checked = true;
    return toast("至少要选一个文件");
  }
  f.selected = e.target.checked;
  drawFilesRows();
  saveFileSelection();
});
$("files-all").addEventListener("change", (e) => {
  const task = state.tasks.find(item => item.id === filesPanelId);
  if (task?.collectionFiles || task?.status === "done" || isBaiduFileTask(task)) return setDrawerSelection(e.target.checked);
  if (!e.target.checked) {
    // 全不选没有意义：只留第一个
    filesList.forEach((f, i) => { f.selected = i === 0; });
  } else filesList.forEach((f) => { f.selected = true; });
  drawFilesRows();
  saveFileSelection();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && filesPanelId && $("ctx-menu")?.dataset.owner !== "drawer-files") closeFilesPanel();
});

/// 多文件任务重新下载：走新建下载框，能改位置和文件夹名
function openMultiConfirm(info, extra) {
  const files = (info.files || []).map((f) => ({ url: f.url, name: f.name, headers: Object.entries(f.header || {}) }));
  if (!files.length) return toast("这条记录没有保存文件列表，请到网盘重新推送");
  for (const f of files) {
    extInfo.set(f.url, f);
    grabNames.set(f.url, f.name);
  }
  return askConfirm(files[0].url, { batch: files.map((f) => ({ url: f.url, name: f.name })), kinds: ["网盘"], dir: info.dir, folderName: info.folder, ...extra });
}
/// 新任务建好后，去掉被替换的旧任务或历史记录。撞了重名时等用户选完
let multiPendingSettle = null;
async function settleMulti(extra) {
  if (extra?.replaceMulti) await invoke("forget_task", { id: extra.replaceMulti }).catch(() => {});
  if (extra?.trashMulti) {
    await invoke("trash_delete", { id: extra.trashMulti }).catch(() => {});
    await loadTrash();
  }
}

function isHuggingFaceFilePage(value) {
  return /^https:\/\/huggingface\.co\/[^/]+\/[^/]+\/tree\//i.test(value || "");
}

async function runBatch(items, dir, video, selections, folder, extra) {
  const keepTree = isHuggingFaceFilePage(extra?.sourceUrl);
  // 推送来的一批是一条多文件任务。NAS 内核还没有这个能力，存到 NAS 时仍走下面的合并显示
  if ((items.length > 1 || keepTree) && items.every((x) => extInfo.has(x.url)) && !String(dir || "").startsWith("/")) {
    const files = items.filter((x) => selections?.get(x.url)?.length !== 0).map((x) => ({ url: x.url, name: extInfo.get(x.url)?.name || nameFromLink(x.url), headers: extInfo.get(x.url)?.headers || [] }));
    // 共同的最外层已经是任务文件夹，里面不再套一层
    const head = keepTree ? "" : multiCommonHead(files.map((f) => f.name));
    if (head) files.forEach((f) => { f.name = f.name.slice(head.length + 1); });
    const id = await invoke("add_multi", { name: folder || head || multiFolderName(files.map((f) => f.name)), dir: dir || null, files }).catch((e) => (toast(String(e)), null));
    if (id === null) return;
    if (id) await settleMulti(extra);
    else multiPendingSettle = extra || null;
    switchView("all");
    refreshTasks();
    if (id) toast(`已加入 ${files.length} 个文件`);
    return;
  }
  // 推送来的一批合成一个任务，由 scheduleGroup 隔开逐条建
  if (items.length > 1 && items.every((x) => extInfo.has(x.url))) {
    const queue = items.filter((x) => selections?.get(x.url)?.length !== 0).map((x) => ({ url: x.url, name: extInfo.get(x.url)?.name || "", headers: extInfo.get(x.url)?.headers || null }));
    taskGroups.push({ id: `grp:${Date.now().toString(36)}`, name: folder || multiFolderName(queue.map(item => item.name || nameFromLink(item.url))), ids: [], queue, total: queue.length, dir, at: Date.now() });
    saveGroups();
    switchView("all");
    refreshTasks();
    return toast(`已加入 ${queue.length} 个文件`);
  }
  const newIds = [];
  for (const x of items) {
    const newId = await doAdd(x.url, extInfo.get(x.url)?.name || "", dir, video, extInfo.get(x.url)?.headers || null, { force: true, replaceId: x.replaceId, select: selections?.get(x.url) || null });
    if (newId) newIds.push(newId);
  }
  toast(`已加入 ${items.length} 个下载`);
}

/// 投一条链接的公共流程：粘贴框回车和浏览器扩展（ext-add）都走这里。
/// opts.name 是扩展解析出的文件名，opts.headers 是网盘直链的鉴权头（[ [k,v], ... ]），
/// 要一路带进 add_task，不然网盘直链下了个 403 回来
const WECHAT_SHARE = /^https?:\/\/weixin\.qq\.com\/sph\/[^/?#]+/i;
// Default browser first, then other supported browsers; only prompt after all probes fail.
const wechatGuidePending = new Map();
async function wechatGuide(url) {
  if (wechatGuidePending.has(url)) return wechatGuidePending.get(url);
  const pending = Promise.resolve().then(async () => {
    const finishNotice = toast("正在连接浏览器扩展…");
    try {
      const result = await invoke("wechat_open_preview", { share: url });
      finishNotice();
      if (result === "ready") return;
      const choice = await askChoice({
        title: result === "missing" ? "未找到已启用的视频号解析扩展" : "浏览器扩展暂未响应",
        desc: result === "missing"
          ? "已检查 Chrome 和 Edge，请在其中一个浏览器安装并启用闪电扩展；解析时还需要登录腾讯元宝。"
          : "请在扩展管理页重新加载闪电扩展后重试，并确认浏览器没有被系统阻止启动。",
        buttons: [{ label: "取消", value: null }, { label: "配置浏览器扩展", value: "setup", primary: true }],
      });
      if (choice === "setup") {
        await switchView("settings");
        $("btn-ext-folder")?.closest(".set-card")?.scrollIntoView({ block: "center" });
      }
    } catch (e) { finishNotice(); toast(String(e)); }
  });
  wechatGuidePending.set(url, pending);
  try { return await pending; } finally { wechatGuidePending.delete(url); }
}

// 网盘分享页是网页，直接下只会得到一个 HTML 文件
const PAN_SHARE = /^https?:\/\/(drive\.uc\.cn\/(s\/|share\/)|pan\.baidu\.com\/(s\/|share\/)|pan\.quark\.cn\/s\/|(www\.)?(aliyundrive|alipan)\.com\/s\/|((www|yun|share)\.|[a-z0-9-]+\.share\.)?(123(pan|684|865|952|912|957)\.com|123pan\.cn)\/(s|123pan)\/|cloud\.189\.cn\/(t\/|web\/share)|pan\.xunlei\.com\/(s|share)\/|(www\.)?guangyapan\.com\/(s|share)\/|(yun|caiyun)\.139\.com\/(link|shareweb)(\/|\?))/i;
const isFeishuDocument = url => /^https:\/\/(?:[a-z0-9-]+\.)*feishu\.cn\/(?:wiki|docx)\/[a-z0-9]+\/?(?:[?#].*)?$/i.test(url);
const BAIDU_SHARE = /^https?:\/\/(?:pan|yun)\.baidu\.com\/(?:s\/|share\/(?:init|link)\/?(?:\?|$))/i;

function baiduShareWithCode(link, code) {
  const url = new URL(link);
  if (code) url.searchParams.set("pwd", code);
  else url.searchParams.delete("pwd");
  return url.href;
}

function linksWithBaiduCode(text) {
  text = text.replace(/\[[^\r\n\]]*\]\((https?:\/\/(?:pan|yun)\.baidu\.com\/[^\s)]+)\)/ig, " $1 ");
  text = text.replace(/([:：])(?=https?:\/\/(?:pan|yun)\.baidu\.com\/)/ig, "$1 ");
  const matches = [...text.matchAll(/(?:^|[\s（(「])((?:magnet:\?|ed2k:\/\/|https?:\/\/)\S+)/gi)];
  return matches.map((match, i) => {
    const link = match[1];
    if (!BAIDU_SHARE.test(link)) return link;
    const start = match.index + match[0].length;
    let tail = text.slice(start, matches[i + 1]?.index);
    const inline = link.match(/(?:提取码|提取密码|访问码|密码)(?:\s*[:：=]|$)/);
    if (inline) tail = link.slice(inline.index) + tail;
    const cleaned = (inline ? link.slice(0, inline.index) : link)
      .replace(/[，,。；;、）)】\]」』]+$/, "").replace(/\\&/g, "&");
    const code = tail.match(/^[\s，,。；;、）)】\]」』]*(?:提取码|提取密码|访问码|密码)\s*[:：=]?\s*[`“‘"']?([a-z0-9]{4})(?![a-z0-9])/i)?.[1];
    try {
      const parsed = new URL(cleaned);
      return code && !/^[a-z0-9]{4}$/i.test(parsed.searchParams.get("pwd") || "")
        ? baiduShareWithCode(cleaned, code) : cleaned;
    } catch { return cleaned; }
  });
}

async function panGuide(url) {
  const go = await askChoice({
    title: "这是网盘分享页，不能直接下载",
    desc: "在浏览器打开分享页，选择文件后点击「闪电下载」获取实际下载地址。若网盘要求先保存到自己的网盘，请按页面提示处理。",
    buttons: [{ label: "取消", value: null }, { label: "在浏览器打开", value: "go", primary: true }],
  });
  if (go) invoke("open_url", { url }).catch((e) => toast(String(e)));
}

async function submitLink(url, opts = {}) {
  url = String(url || "").trim();
  if (!url || !invoke) return;
  if (WECHAT_SHARE.test(url)) return wechatGuide(url);
  if (isFeishuDocument(url)) return askConfirm(url, opts);
  if (BAIDU_SHARE.test(url) || /baidu\.com\//i.test(url)) {
    const share = linksWithBaiduCode(url).find((link) => BAIDU_SHARE.test(link));
    if (share) {
      const info = await invoke("target_info", { link: share }).catch(() => null);
      if (info?.baiduShare) {
        return askConfirm(share, {
          baiduShare: true, dir: opts.dir, name: opts.name, force: opts.force, cleanupRemote: opts.cleanupRemote,
          baiduCodeRequired: opts.baiduCodeRequired, baiduRedownloadId: opts.baiduRedownloadId,
          baiduSelection: opts.baiduSelection,
        }, info);
      }
      toast("百度网盘分享链接无效，请重新复制分享链接");
      return nextPending();
    }
  }
  if (PAN_SHARE.test(url)) return panGuide(url);
  if (!state.engineRunning) {
    pendingLinks = []; // 内核没起，排队的几条也投不进去，别把队列留成暗雷
    toast("内核还没起来，稍等一下再试（或用侧栏的「启动」）");
    return;
  }
  // 设置还没取回来就先等一下：state.settings 为空时 confirmNew 是 undefined，
  // 会跳过确认框直接下（扩展点进来那一下最容易踩到）
  if (!state.settings) await loadSettings();
  const info = await invoke("target_info", { link: url, prepare: true, headers: opts.headers || null }).catch(() => null);
  if (opts.collection || info?.collection) return askConfirm(url, { ...opts, collection: true }, info);
  // Show the editable panel first; duplicate and metadata checks run visibly inside it.
  if(state.settings?.settings.confirmNew || opts.confirm){
    await askConfirm(url,{...opts,checkDuplicate:!opts.force},info);return;
  }
  // 下过的链接一贴进来就问，不等到确认框点完才说
  if (!opts.force) {
    const answer = await askDuplicate(url, "");
    if (!answer) {
      nextPending(); // 这条不下了，接下一条
      return;
    }
    if (answer.force) opts = { ...opts, ...answer };
  }
  await doAdd(url, opts.name || "", opts.dir || "", null, opts.headers || null, opts);
  nextPending(); // 直投完就算处理完，接下一条
}

/// 真正投任务。name / dir 为空就走 Rust 侧的默认策略（按类型分目录）；
/// video 是视频站选项（画质／字幕／仅音频），不传就用 Rust 侧记住的；
/// headers 是扩展带过来的请求头，直链鉴权用；
/// extra.force 表示用户已经明确要再下一份（重新下载按钮），不用再问；
/// extra.replaceId 是投成功后要拿掉的旧记录（文件已丢失的那条）
function sameDownloadDirectory(a, b) {
  const normalize = path => String(path || "").replaceAll("\\", "/").replace(/\/+$/, "");
  const first = normalize(a);
  const second = normalize(b);
  return first.startsWith("/") && !first.startsWith("//")
    ? first === second : first.toLowerCase() === second.toLowerCase();
}

function chooseRedownloadMode(name) {
  return askChoice({
    title: "原位置已有下载",
    desc: `「${name}」已在这个目录。可以保留原文件再下一份，或者将旧文件移到回收站后重新下载。`,
    buttons: [{ label: "取消", value: null }, { label: "再下一份", value: "copy" }, { label: "替换旧文件", value: "replace", primary: true }],
  });
}

async function doAdd(url, name, dir, video, headers, extra = {}) {
  if (BAIDU_SHARE.test(url)) {
    throw new Error("请在「新建下载」中确认百度网盘分享下载");
  }
  let force = !!extra.force;
  // 贴链接时查过一次了；确认框里可能换了保存目录，那个目录里也可能已有这个文件
  if (!force && !(extra.duplicateChecked && extra.duplicateUrl === url && extra.duplicateDir === dir)) {
    const answer = await askDuplicate(url, dir);
    if (!answer) return;
    force = !!answer.force;
  }
  if (extra.nasRedownload && extra.originDir && extra.originId) {
    if (sameDownloadDirectory(extra.originDir, dir) && !extra.originalMissing) {
      const mode = await chooseRedownloadMode(extra.originName);
      if (!mode) return;
      if (mode === "replace") {
        extra.recycleId = extra.originId;
        extra.replaceId = extra.originId;
      } else {
        extra.allowRenaming = true;
      }
    } else if (extra.originalMissing) {
      extra.replaceId = extra.originId;
    }
  }
  let newId = null;
  try {
    if (extra.nasRedownload && extra.recycleId) await invoke("recycle_task_file", { id: extra.recycleId });
    else if (extra.recycleId) await invoke("recycle_task_file", { id: extra.recycleId }).catch((e) => toast(String(e)));
    // 从下载历史重下：后端在这一步才把旧文件送回收站，并拿掉那条记录
    if (extra.trashId) {
      newId = await invoke("trash_redownload", { id: extra.trashId, dir: dir || null, video: video || null });
      await loadTrash();
    } else {
      if (extra.btName) await invoke("name_hint", { url, name: extra.btName }).catch(() => {});
      const options = video && extra.nasRedownload ? { ...video, confirmCollision: !extra.allowRenaming } : video;
      newId = await invoke("add_task", { url, name: name || null, dir: dir || null, video: options || null, headers: headers || null, forceNew: force, selectFiles: extra.select || null });
    }
    // 空 id：保存目录里有同名，后端没建任务，正在问用户
    if (!newId) {
      refreshTasks();
      return;
    }
    if (extra.btName && newId) invoke("name_precheck", { id: newId, name: extra.btName }).catch(() => {});
    if (extra.replaceId) await invoke("forget_task", { id: extra.replaceId }).catch(() => {});
    // 点了开始下载就切到「正在下载」看进度（用户要求，不管之前在哪个页）
    switchView("all");
    refreshTasks();
    focusNewTask(newId);
    return newId;
  } catch (e) {
    // 用 toast 不用 alert：WebView2 里 alert 有时直接弹不出来，用户看到的就是「点了没反应」
    const inList = String(e).includes("下载中") || String(e).includes("下载列表");
    if (!inList) toast("投任务失败：" + e);
    else {
      const dup = await invoke("find_duplicate", { url, dir: dir || null }).catch(() => null);
      toast(String(e), dup?.id ? { action: { label: "定位任务", run: () => revealTask(dup.id) } } : {});
    }
  }
}

function chooseDuplicate(dup, {
  allowCopy = true,
  copyHint = "如果想要另一种清晰度或字幕，可以再下一份，文件名会自动加 (1)、(2)。",
} = {}) {
  const where = dup.missing
    ? "以前下载过，记录里找不到文件"
    : dup.status === "done" ? "已经下载完成"
    : dup.status === "history" ? "以前下载过，文件还在"
    : dup.status === "file" ? `已经在 ${dup.folder || "保存目录"} 里`
    : "已经在下载列表里";
  return askChoice({
    title: "这个链接已经下载过",
    desc: dup.missing
      ? `「${dup.name}」${where}。`
      : `「${dup.name}」${where}。${allowCopy ? "\n" + copyHint : ""}`,
    buttons: [
      ...(dup.id ? [{ label: "查看", value: "show", primary: !allowCopy }] : []),
      ...(dup.historyId ? [{ label: "查看", value: "history" }] : []),
      // 文件在目录里、列表里没有这条任务：直接打开文件夹看
      ...(dup.folder ? [{ label: "打开文件夹", value: "folder" }] : []),
      ...(allowCopy ? [{ label: dup.missing ? "重新下载" : "再下一份", value: "copy", primary: true }] : []),
    ],
  });
}

async function viewDuplicateTask(id) {
  if (DIALOG) await window.__TAURI__.event.emitTo("main", "reveal-task", { id });
  else await revealTask(id);
}

/// 这个链接下过的话问用户怎么办：可能就是要换分辨率、补字幕，我们判断不了。
/// 没下过返回 {}；要再下返回 { force, replaceId }；不下了返回 null
async function askDuplicate(url, dir) {
  const dup = await invoke("find_duplicate", { url, dir: dir || null }).catch(() => null);
  if (!dup) return {};
  const c = await chooseDuplicate(dup);
  if (c === "show") await viewDuplicateTask(dup.id);
  if (c === "history") {
    if(DIALOG)await window.__TAURI__.event.emitTo("main","reveal-history",{id:dup.historyId});
    else await revealHistoryRecord(dup.historyId);
  }
  if (c === "folder") await invoke("open_dir", { path: dup.folder }).catch((e) => toast(String(e)));
  if (c !== "copy") return null;
  // 文件丢了的旧条目没用了，新的投成功后拿掉
  return { force: true, replaceId: dup.missing && dup.id ? dup.id : null };
}

async function revealHistoryRecord(id) {
    if (state.query) {
      state.query = "";
      $("search").value = "";
    }
    await switchView("trash");
    const row = document.querySelector(`.task[data-trash-id="${cssEscape(id)}"]`);
    row?.scrollIntoView({ block: "center" });
    if (row) selectRow(id);
}

/// 切到这条任务所在的页面，滚过去选中并闪两下。搜索词可能把它筛掉了，先清掉
let flashing = null;
let flashTimer;
/// 新任务要等列表刷新才出现（NAS 还要等一轮轮询），出现了就滚过去并高亮
async function focusNewTask(id) {
  if (typeof id !== "string" || !id) return;
  for (let i = 0; i < 12; i++) {
    await new Promise((r) => setTimeout(r, 500));
    if (state.view !== "all") return; // 用户已经去别的页面了
    await refreshTasks();
    if (state.tasks.some((t) => t.id === id)) return revealTask(id);
  }
}

async function revealTask(id) {
  const t = state.tasks.find((x) => x.id === id);
  if (state.query) {
    state.query = "";
    $("search").value = "";
  }
  await switchView(t?.status === "done" ? "done" : "all");
  // 列表每秒可能重建，记在变量里，rowHtml 重画时也带上
  flashing = id;
  clearTimeout(flashTimer);
  flashTimer = setTimeout(() => { flashing = null; }, 2000);
  renderTasks();
  selectRow(id);
  const row = document.querySelector(`#task-list .task[data-id="${cssEscape(id)}"]`);
  if (!row) return;
  row.classList.remove("flash");
  void row.offsetWidth;
  row.classList.add("flash");
  row.addEventListener("animationend", () => row.classList.remove("flash"), { once: true });
  row.scrollIntoView({ block: "center" });
}

// ---------- 扩展抓取的一批链接 ----------

let grabLinks = [];

const grabNames = new Map();
/// 抓到的链接直接进新建框，在那里勾选要下的文件，不用弹两层
async function openGrab(links, metadata = {}) {
  if (!links.length) return;
  links.forEach((l) => { if (l.name) grabNames.set(l.url, l.name); extInfo.set(l.url,{url:l.url,name:l.name||"",headers:l.headers||null, size: Number.isSafeInteger(l.sizeBytes) && l.sizeBytes >= 0 ? l.sizeBytes : undefined}); });
  const sourceUrl = typeof metadata.sourceUrl === "string" ? metadata.sourceUrl : "";
  const folderName = typeof metadata.folderName === "string" && metadata.folderName ? collectionFolderName(metadata.folderName) : "";
  if (links.length === 1 && !isHuggingFaceFilePage(sourceUrl)) {
    const { url, name, headers, size } = extInfo.get(links[0].url);
    return submitLink(url, { confirm: true, name, headers, size, sourceUrl });
  }
  const infos = await Promise.all(links.map((l) => invoke("target_info", { link: l.url }).catch(() => null)));
  if (infos.some((info) => info?.collection || info?.baiduShare)) {
    pendingLinks.push(...links.map((l) => ({ url: l.url, opts: { confirm: true, name: l.name, headers: l.headers } })));
    return nextPending();
  }
  submitBatch(links.map((l) => l.url), [...new Set(links.map((l) => l.kind))], true, { folderName, sourceUrl });
}

// ---------- 通用确认框 ----------
// WebView2 里 confirm()/alert() 经常弹不出来还直接返回 false，用户看到的就是「点了没反应」。
// 所有确认都走这个自绘的。同时只显示一个，后来的排队。

let choiceTail = Promise.resolve();
let closeChoice = null;

/// buttons: [{ label, value, primary }]。关掉（Esc、点遮罩）返回 null
/// checks: [{ label, checked, foot }] 显示成勾选框，关掉时把勾选结果写回 checked；foot 的放在按钮行左边（「不再询问」）
function askChoice({ title, desc, buttons, checks }) {
  const run = () =>
    new Promise((resolve) => {
      $("choice-title").textContent = title;
      $("choice-desc").textContent = desc || "";
      let box = $("choice-checks");
      if (!box) {
        box = document.createElement("div");
        box.id = "choice-checks";
        $("choice-desc").after(box);
      }
      box.innerHTML = "";
      const inputs = (checks || []).map((c) => {
        const label = document.createElement("label");
        label.className = "modal-note";
        const input = document.createElement("input");
        input.type = "checkbox";
        input.checked = !!c.checked;
        label.append(input, " " + c.label);
        box.appendChild(label);
        return input;
      });
      const done = resolve;
      resolve = (v) => {
        (checks || []).forEach((c, i) => { c.checked = inputs[i].checked; });
        done(v);
      };
      const foot = $("choice-foot");
      foot.innerHTML = "";
      closeChoice = (v) => {
        closeChoice = null;
        $("choice").hidden = true;
        resolve(v);
      };
      let main = null;
      for (const b of buttons) {
        const el = document.createElement("button");
        el.className = b.primary ? "primary" : "ghost";
        el.textContent = b.label;
        el.addEventListener("click", () => closeChoice?.(b.value));
        foot.appendChild(el);
        if (b.primary) main = el;
      }
      (checks || []).forEach((c, i) => { if (c.foot) foot.prepend(inputs[i].parentElement); });
      $("choice").hidden = false;
      main?.focus();
    });
  const p = choiceTail.then(run);
  choiceTail = p.catch(() => {});
  return p;
}

/// 是/否。确定返回 true
async function askYes(title, desc, okLabel = "确定") {
  const v = await askChoice({
    title,
    desc,
    buttons: [
      { label: "取消", value: false },
      { label: okLabel, value: true, primary: true },
    ],
  });
  return v === true;
}

$("choice")?.addEventListener("click", (e) => {
  if (e.target.id === "choice") closeChoice?.(null);
  if (e.target.closest?.("#choice-x")) closeChoice?.(null);
});

// ---------- 新建下载确认框 ----------

let cf = { url: "", dir: "", video: null, name: "", headers: null, extra: {} };

/// 老配置里的正则归一成按钮上的语义标签（zh / en / zh+en / all）
function normSubLangs(s) {
  const t = String(s || "zh").trim();
  if (t === "all" || t === ".*") return "all";
  if (t === "zh+en" || (t.includes("zh") && t.includes("en"))) return "zh+en";
  if (t === "en" || (t.includes("en") && !t.includes("zh"))) return "en";
  if (t === "zh" || t.includes("zh")) return "zh";
  return "zh";
}

function syncCfSubtitleDisclosures() {
  const visible=!$("cf-subtitles").hidden;
  for (const [id,body,enabled,label] of [
    ["cf-subsettings-toggle","cf-subsettings-body",visible,"字幕设置"],
    ["cf-substyle-toggle","cf-substyle-body",visible && !$("cf-subcolor-row").hidden,"字幕样式选项"],
  ]) {
    const button=$(id), expanded=button.getAttribute("aria-expanded")!=="false";
    button.hidden=!enabled;
    $(body).hidden=!enabled || !expanded;
    button.setAttribute("aria-label",(expanded?"折叠":"展开")+label);
  }
  if($("cf-subsettings-body").hidden)$("cf-subpreset")._customSelect?.close();
}
for (const id of ["cf-subsettings-toggle","cf-substyle-toggle"]) $(id).addEventListener("click",()=>{
  const button=$(id);
  button.setAttribute("aria-expanded",String(button.getAttribute("aria-expanded")==="false"));
  renderCfVideo();
});

function renderCfSize(draft) {
  const { extra, video, probedSize } = draft;
  let label = "";
  if (!extra?.batch && !extra?.collection) {
    const currentVideoSize = probedSize && video
      && probedSize.maxHeight === (video.maxHeight || 0) && probedSize.audioOnly === !!video.audioOnly
      ? probedSize.size : undefined;
    const size = extra?.size ?? currentVideoSize;
    label = extra?.sizeLabel || (size != null ? bytes(size) : "");
  }
  $("cf-size").textContent = label;
  $("cf-size").hidden = !label;
}

/// 视频选项控件 ←→ cf.video 同步。仅音频勾上时画质和字幕没意义，一起灰掉（Mac 同款）
function renderCfVideo() {
  const row = $("cf-video-row");
  if (!row) return;
  const v = cf.video;
  renderCfSize(cf);
  renderCfNameState();
  const collection = !!cf.extra?.collection;
  row.hidden = !v || collection;
  $("cf-opt-row").hidden = !v || (collection && !cf.extra.batch);
  if (!v) {
    // 字幕那几行要一起藏：不然上一次打开视频站留下的语言、格式会出现在磁力、种子的确认框里
    for (const id of ["cf-sub-row", "cf-subfmt-row", "cf-submode-row", "cf-subcolor-row", "cf-subsize-row", "cf-subfx-row", "cf-subpreset-wrap", "cf-subpreset-empty", "cf-sub-hint", "cf-polish-wrap"]) {
      const r = $(id);
      if (r) r.hidden = true;
    }
    $("cf-subpreset")._customSelect?.close();
    $("cf-subtitles").hidden=true;
    syncCfSubtitleDisclosures();
    fitConfirmDialog();
    return;
  }
  // 这个站没有字幕：不给选，画质和仅音频还在。只影响显示，用户记住的字幕偏好不动
  const noSubs = cf.subtitleAvailability !== "available";
  $("cf-subtitles").hidden=noSubs || !v.subtitles || !!v.audioOnly;
  $("cf-subs").closest("label").hidden = noSubs;
  // 记住的清晰度这个视频可能没有：下载时取不超过它的最高一档，这里标同一档
  const qBtns = [...$("cf-quality").querySelectorAll("button")];
  const fit = v.maxHeight ? Math.max(0, ...qBtns.map((b) => Number(b.dataset.h)).filter((h) => h && h <= v.maxHeight)) : 0;
  qBtns.forEach((b) => {
    b.classList.toggle("on", Number(b.dataset.h) === fit);
    b.disabled = !!v.audioOnly;
  });
  const subs = $("cf-subs");
  subs.checked = !!v.subtitles && !noSubs;
  subs.disabled = !!v.audioOnly;
  // 字幕语言那行：勾了字幕才出来，并把当前值标上
  const subRow = $("cf-sub-row");
  const subFmtRow = $("cf-subfmt-row");
  if (subRow) {
    subRow.hidden = noSubs || !v.subtitles || !!v.audioOnly;
    if (subFmtRow) subFmtRow.hidden = subRow.hidden;
    // 老配置可能是正则（zh.*,en.* 等），归一成按钮上的语义标签再标 on
    const langs = normSubLangs(v.subLangs);
    document.querySelectorAll("#cf-sublangs button").forEach((b) => {
      b.classList.toggle("on", b.dataset.langs === langs);
    });
    const fmt = v.subFormat || "vtt";
    document.querySelectorAll("#cf-subfmt button").forEach((b) => {
      b.classList.toggle("on", b.dataset.fmt === fmt);
    });
    // 着色只在「勾了下载字幕 + 格式选 ASS」时才有意义（srt/vtt 没颜色）。
    // 色点跟着上面的字幕语言变：中文只出「中文」，英文只出「英文」，
    // 中英出两个，「全部」只出一个（用中文那个颜色，两行统一）
    const colorRow = $("cf-subcolor-row");
    if (colorRow) {
      colorRow.hidden = noSubs || !v.subtitles || v.audioOnly || fmt !== "ass";
      const langNow = normSubLangs(v.subLangs);
      const zhBtn = document.querySelector('#cf-subcolor [data-lang="zh"]');
      const enBtn = document.querySelector('#cf-subcolor [data-lang="en"]');
      const allBtn = document.querySelector('#cf-subcolor [data-lang="all"]');
      if (zhBtn) zhBtn.hidden = langNow === "en" || langNow === "all";
      if (enBtn) enBtn.hidden = langNow === "zh" || langNow === "all";
      if (allBtn) allBtn.hidden = langNow !== "all";
      const zh = v.subColorZh || "#00FFAC";
      const en = v.subColorEn || "#516EFE";
      if ($("sub-dot-zh")) $("sub-dot-zh").style.background = zh;
      if ($("sub-dot-en")) $("sub-dot-en").style.background = en;
      // 「全部」两行同色，就只画一个色点——一绿一蓝各半那种对角点看着像「亮色圆点」，
      // 分不清是哪一行的颜色
      if ($("sub-dot-all")) $("sub-dot-all").style.background = zh;
      const off = document.querySelector('#cf-subcolor [data-color="off"]');
      if (off) off.classList.toggle("on", !v.subColorize);
      [zhBtn, enBtn, allBtn].forEach((b) => b && b.classList.toggle("on", !!v.subColorize));
    }
    const sizeRow = $("cf-subsize-row");
    if (sizeRow) sizeRow.hidden = subRow.hidden || fmt !== "ass";
    const sc = String(v.subScale ?? 1);
    document.querySelectorAll("#cf-subsize button").forEach((b) => {
      b.classList.toggle("on", b.dataset.scale === sc);
    });
    const fxRow = $("cf-subfx-row");
    if (fxRow) fxRow.hidden = subRow.hidden || fmt !== "ass";
    syncSubFx();
    syncSubMode(!subRow.hidden && fmt === "ass");
  }
  $("cf-audio").checked = !!v.audioOnly;
  $("cf-polish-wrap").hidden = !cf.hasOllama || noSubs || !v.subtitles || !!v.audioOnly;
  $("cf-polish").checked = !!cf.polish;
  syncCfSubtitleDisclosures();
  fitConfirmDialog();
}

async function askConfirm(url, opts = {}, info) {
  if (info === undefined) info = await invoke("target_info", { link: url, prepare: true, headers: opts.headers || null }).catch(() => null);
  if (!info) {
    toast("这个链接不认识。支持磁力、种子文件、ed2k、直链");
    nextPending(); // 这条认不出来也算过了一条，别把排队的堵死
    return;
  }
  const collection = !!(opts.collection || info.collection);
  const baiduShare = !!info.baiduShare && !opts.batch;
  const feishu = !!info.feishuDocument && !opts.batch;
  opts = { ...opts, baiduShare, feishu };
  if (!DIALOG && invoke) {
    openDialog("confirm", { url, opts: { ...opts, collection, localDefaultDir: info.dir, dir: opts.dir || info.dir } }); // 目录这边算好带过去
    return;
  }
  const batch = opts.batch || null;
  $("cf-title").textContent = batch
    ? `新建下载（${batch.length} 个）`
    : opts.baiduRetryId ? (opts.baiduCodeRequired ? "填写提取码" : "继续下载")
    : opts.recycleId || opts.trashHere ? "重新下载（旧文件会移到回收站）" : "新建下载";
  // 一个名字没法给多条用
  $("cf-name").closest(".set-row").hidden = !!batch || collection;
  $("cf-files").hidden = true;
  cf = {
    url,
    supportsSubs: !!info.supportsSubs,
    subtitleAvailability: info.supportsSubs ? "checking" : "unsupported",
    nameLoading: !collection && !opts.wechatTicket && !!info.videoOptions && !batch,
    hasOllama: false,
    polish: !!state.settings?.settings.subtitlePolish,
    // 重新下载已经知道该放哪，就别让后端再按类型算一遍
    dir: opts.dir || info.dir,
    localDefaultDir: opts.localDefaultDir || info.dir,
    video: !opts.wechatTicket && info.videoOptions ? { ...info.videoOptions } : null,
    name: baiduShare && opts.baiduSelection?.length > 1 ? multiFileStem(opts.name) : opts.name || "",
    headers: opts.headers || null,
    extra: { ...opts, collection },
  };
  $("cf-kind").textContent = opts.wechatTicket ? "微信视频号（已解析）" : batch ? opts.kinds.join("、") : info.kind;
  // 目录现在是可编辑输入框：用户能直接粘贴路径，也能从「最近」里挑
  $("cf-dir").value = cf.dir;
  $("cf-dir").readOnly = !!opts.baiduRetryId;
  $("cf-pick").disabled = !!opts.baiduRetryId;
  $("cf-url").textContent = opts.sourceUrl || (batch ? `${batch.length} 个：${url} 等` : url);
  $("cf-url").dataset.tip = opts.sourceUrl ? (isHuggingFaceFilePage(opts.sourceUrl) ? "点击复制模型文件页链接" : "点击复制文档链接") : opts.wechatTicket ? "来源分享链接；视频播放地址已在后台解析，点击复制来源" : "点击复制链接";
  $("cf-url").closest(".set-row").querySelector(".set-label").textContent = opts.sourceUrl ? (isHuggingFaceFilePage(opts.sourceUrl) ? "来源链接" : "文档链接") : opts.wechatTicket ? "来源链接" : collection ? "合集链接" : "链接";
  // 扩展送来的名字先填上，用户可以改；粘贴进来的留空走链接里的名字
  $("cf-name").value = cf.name;
  delete $("cf-name").dataset.userEdited;
  setCfDefaultName(opts.folderName || cf.name || (cf.video ? "" : nameFromLink(url)),
    cf.video && !batch ? "正在获取名称…" : "留空就用链接里的名字");
  cfFolderRow(batch);
  if (feishu) $("cf-name").closest(".set-row").hidden = true;
  $("cf-baidu-code-row").hidden = !baiduShare;
  $("cf-baidu-destination-tip").hidden = !baiduShare;
  $("cf-baidu-cleanup-row").hidden = !baiduShare;
  setSwitch("cf-baidu-cleanup", opts.cleanupRemote !== false);
  $("cf-baidu-cleanup").disabled = !!opts.baiduRetryId;
  $("cf-baidu-setup").hidden = true;
  $("cf-baidu-method-row").hidden = true;
  $("cf-baidu-status-row").hidden = true;
  $("cf-ok").hidden = false;
  $("cf-ok").textContent = opts.baiduRetryId ? "继续下载" : "开始下载";
  $("cf-baidu-qr").hidden = true;
  $("cf-baidu-qr").removeAttribute("src");
  if (baiduShare) {
    const path = "我的应用数据 / bdpan / Shandian";
    const scope = opts.baiduSelection?.length ? "所选项目" : "整个分享";
    $("cf-baidu-destination-tip").dataset.tip =
      `先转存到百度网盘的 ${path}，再将${scope}下载到所选位置。`
      + (opts.force ? "重新下载会保留原文件。" : "");
  }
  $("cf-baidu-code").value = baiduShare ? new URL(url).searchParams.get("pwd") || opts.baiduSavedCode || "" : "";
  $("cf-baidu-code").placeholder = opts.baiduCodeRequired ? "输入 4 位提取码" : "无需提取码可留空";
  $("cf-name").readOnly = !!opts.baiduRetryId;
  if (baiduShare) {
    $("cf-name").closest(".set-row").hidden = false;
    setCfDefaultName(cf.name || "", "留空就用分享文件名");
    $("cf-url").textContent = baiduShareWithCode(url, "");
    cfFiles = null;
    cfClosedFolders.clear();
    renderBaiduPreviewFallback(cf);

  }
  // 重新下载：默认用原来的文件夹名
  if (opts.folderName) $("cf-name").value = opts.folderName;
  $("cf-remember").checked = false;
  $("cf-remember").closest("label").hidden = collection || baiduShare || feishu;
  $("cf-subs-state").hidden = true;
  $("cf-subsettings-toggle").setAttribute("aria-expanded", "false");
  renderCfVideo();
  $("confirm").hidden = false;
  const checking=!!opts.checkDuplicate && !opts.force && !batch && !opts.wechatDuplicateChecked;
  $("cf-ok").disabled=collection||feishu||checking;
  $("cf-check-state").hidden=collection||!checking;
  $("cf-collection-state").hidden=true;
  if (feishu) setProbeState($("cf-collection-state"), "正在读取飞书原始音视频…", true);
  else if(collection)showCollectionLoading();
  else if(checking)setProbeState($("cf-check-state"),"正在检查已有下载记录…",true);
  if (opts.baiduCodeRequired) $("cf-baidu-code").focus();
  else if (baiduShare) $("cf-dir").focus();
  else if (!batch && !collection) $("cf-name").focus();
  if(!DIALOG || dialogShown)beginConfirmLookups();
}

// ---------- 新建框里选 BT 文件 ----------
const VIDEO_FILE = /\.(mp4|mkv|avi|mov|flv|wmv|webm|m4v|ts|m2ts|rmvb|mpg|mpeg|iso)$/i;
let cfFiles = null; // [{ url, name, on, state: loading|ok|fail, files: [{name,size,on}] }]
let cfFilesGen = 0;
const cfClosedFolders = new Set();
let cfFileRows = [];

/// 网盘推来的一批（每条都有文件名）是一条任务：名称框改成文件夹名，给默认值，可以改。
/// 粘贴的多条链接各是各的任务，没有这一项
function cfFolderRow(batch) {
  const row = $("cf-name").closest(".set-row");
  const isFolder = !!batch && batch.every((b) => b.name || grabNames.get(b.url));
  $("cf-name-label").textContent = isFolder ? "文件夹" : "名称";
  row.hidden = !!batch && !isFolder;
  if (!isFolder) return;
  const def = cf.extra.folderName || multiFolderName(batch.map((b) => b.name || grabNames.get(b.url)));
  const input = $("cf-name");
  // 用户改过就不动；还是自动值（或单条时的文件名）才跟着更新
  if (!input.dataset.userEdited && !cf?.extra?.folderName) input.value = def;
  setCfDefaultName(def);
}
$("cf-name")?.addEventListener("input", () => { $("cf-name").dataset.userEdited = "1"; renderCfNameState(); });

/// 确认框开着时又推来新链接：单条变批量，加进同一个框
if (DIALOG) {
  window.__TAURI__?.event?.listen("dialog-append", ({ payload: { items } }) => {
    if (cf.extra?.collection || cf.extra?.baiduShare || cf.extra?.feishu) return;
    const old = cf.extra?.batch || [{ url: cf.url, name: $("cf-name").value.trim() || cf.name, size: cf.extra.size, sizeLabel: cf.extra.sizeLabel }];
    const previous = new Map((cfFiles || []).map((g) => [g.url, g]));
    const batch = [...old];
    for (const x of items) {
      if (!batch.some((b) => b.url === x.url)) batch.push(x);
      if (x.name) grabNames.set(x.url, x.name);
    }
    for (const b of batch) if (b.name) grabNames.set(b.url, b.name);
    cf.extra = { ...cf.extra, batch, kinds: cf.extra?.kinds || [$("cf-kind").textContent] };
    $("cf-title").textContent = `新建下载（${batch.length} 个）`;
    cfFolderRow(batch);
    renderCfSize(cf);
    $("cf-url").textContent = `${batch.length} 个：${cf.url} 等`;
    loadCfFiles(batch.map((x) => ({ url: x.url, name: x.name || grabNames.get(x.url), size: x.size, sizeLabel: x.sizeLabel })), previous);
  });
}

function btLink(u) { return /^magnet:|\.torrent(\?|$)/i.test(u); }
function nameFromLink(u) {
  try {
    if (/^magnet:/i.test(u)) return new URLSearchParams(u.slice(u.indexOf("?") + 1)).get("dn") || "";
    return decodeURIComponent(new URL(u).pathname.split("/").filter(Boolean).pop() || "");
  } catch { return ""; }
}

/// urls 里的磁力、种子去取文件列表。同时最多 3 个，元数据慢的不挡别的
function loadCfFiles(items, previous) {
  const confirm = cf;
  const gen = ++cfFilesGen;
  if (!previous) cfClosedFolders.clear();
  cfFiles = items.map((x) => {
    const old = previous?.get(x.url);
    const torrent = btLink(x.url);
    if (torrent && old?.state === "ok") return old;
    return { url: x.url, name: x.name || nameFromLink(x.url) || x.url, size: x.size, sizeLabel: x.sizeLabel,
      path: confirm.extra?.collection ? "" : x.name || "", icon: confirm.extra?.collection ? "#i-play" : null,
      on: old?.on ?? true, state: torrent ? "loading" : "ok", files: [] };
  });
  const box = $("cf-files");
  box.hidden = !confirm.extra?.collection && !confirm.extra?.feishu && !isHuggingFaceFilePage(confirm.extra?.sourceUrl) && cfFiles.length < 2 && !btLink(cfFiles[0]?.url || "");
  $("confirm").querySelector(".modal").classList.toggle("wide", !box.hidden);
  renderCfFiles();
  if (box.hidden) return;
  const queue = cfFiles.filter((g) => g.state === "loading");
  const worker = async () => {
    while (queue.length) {
      const g = queue.shift();
      const apply = (r) => {
        if (gen !== cfFilesGen || cf !== confirm) return;
        const res = r?.res || {};
        const files = res.files || [];
        const hasVideo = files.some((f) => VIDEO_FILE.test(f.name || ""));
        g.files = files.map((f, i) => ({ i, name: [f.path, f.name].filter(Boolean).join("/"), size: f.size || 0, on: g.on && (!hasVideo || VIDEO_FILE.test(f.name || "")) }));
        if (g.files.length) g.on = true;
        if (res.name) g.name = res.name;
        g.state = "ok";
        if (cfFiles.length === 1 && !cf.extra?.batch && res.name) setCfDefaultName(res.name);
        renderCfFiles();
      };
      // 45 秒没结果先提示并让出名额给后面的，这条还在后台找，找到了再补上
      const job = invoke("bt_files", { url: g.url }).then(apply);
      const late = await Promise.race([job.then(() => false, () => false), new Promise((ok) => setTimeout(() => ok(true), 45000))]);
      if (gen !== cfFilesGen || cf !== confirm) return;
      if (g.state === "loading") g.state = "fail";
      if (late) job.catch(() => {});
      renderCfFiles();
    }
  };
  // 批量时多开几路：缓存命中很快，不让慢的那几条挡住后面
  for (let n = Math.min(6, queue.length); n > 0; n--) worker();
}

function cfFileTree(files = cfFiles) {
  const root = { children: [], folders: new Map() };
  function add(parent, path, entry, prefix) {
    const parts = path.split(/[\\/]+/).filter(Boolean);
    const name = parts.pop() || entry.name;
    if (entry.directory) parts.push(name);
    for (const part of parts) {
      prefix = [...prefix, part];
      let folder = parent.folders.get(part);
      if (!folder) {
        folder = { name: part, key: JSON.stringify(prefix), children: [], folders: new Map(), entries: [] };
        parent.folders.set(part, folder);
        parent.children.push(folder);
      }
      if (!entry.directory) folder.entries.push(entry);
      parent = folder;
    }
    if (entry.directory) {
      parent.pick = entry;
      return;
    }
    parent.children.push({ name, path: entry.name, entries: [entry], key: JSON.stringify([...prefix, entry.i ?? entry.url]), size: entry.size, sizeLabel: entry.sizeLabel, icon: entry.icon });
  }
  files.forEach((g, index) => {
    if (!btLink(g.url)) return add(root, g.path, g, ["batch"]);
    const torrent = { name: g.name, key: JSON.stringify([index]), state: g.state, children: [], folders: new Map(), entries: g.files.length ? g.files : [g] };
    root.children.push(torrent);
    for (const f of g.files) add(torrent, f.name, f, [index]);
  });
  return root.children;
}

function renderCfFiles() {
  renderCfNameState();
  fitConfirmDialog();
  const box = $("cf-files");
  const preview = cf.extra?.baiduShare;
  const files = preview ? cf.baiduFiles : cfFiles;
  if (!files || box.hidden) return;
  const baiduSelectable = canPickBaiduFiles(cf);
  const scrollTop = box.querySelector(".cf-files-list")?.scrollTop || 0;
  const all = files.flatMap((g) => g.files);
  const on = files.filter((g) => g.on).flatMap((g) => g.files.filter((f) => f.on));
  const model = isHuggingFaceFilePage(cf.extra?.sourceUrl);
  const selectedFiles = files.filter((file) => file.on);
  const size = (preview || model ? selectedFiles : on).reduce((n, f) => n + (f.size || 0), 0);
  const showTotal = model ? selectedFiles.length && selectedFiles.every((file) => file.size != null) : size;
  const vids = all.filter((f) => VIDEO_FILE.test(f.name));
  const others = all.filter((f) => !VIDEO_FILE.test(f.name));
  const wait = files.filter((g) => g.state === "loading").length;
  const checked = (list) => list.length && list.every((f) => f.on) ? "checked" : "";
  // 直链没有文件列表，一条链接就是一个文件
  const plainOn = files.filter((g) => g.on && !g.directory && !btLink(g.url)).length;
  const plainAll = files.filter((g) => !g.directory && !btLink(g.url)).length;
  const collection = cf.extra?.collection;
  const selectable = collection || cf.extra?.feishu || baiduSelectable || model;
  if (selectable && !preview) $("cf-ok").disabled = plainOn === 0;
  if (cf.extra?.feishu) $("cf-name").closest(".set-row").hidden = plainOn <= 1;
  const active = document.activeElement;
  const activeKey = box.contains(active) ? active.dataset.focus : null;
  const mixed = [];
  cfFileRows = [];
  function row(node, depth) {
    const index = cfFileRows.push(node) - 1;
    const expanded = !!node.children?.length && !cfClosedFolders.has(node.key);
    const selection = checked(node.entries);
    const toggle = !preview || (baiduSelectable && node.entries.length);
    if (toggle && !selection && node.entries.some((entry) => entry.on)) mixed.push(index);
    const waitText = node.state === "loading" ? '<span class="wait"><span class="wait-spin"></span>获取文件列表…</span>'
      : node.state === "fail" ? '<span class="wait">暂时没有节点提供文件列表，将下载全部文件</span>' : "";
    const key = escapeHtml(node.key);
    const name = escapeHtml(node.name);
    const icon = node.children ? "#i-folder" : node.icon || iconOfFile(node.name);
    const tag = toggle ? "label" : "div";
    const sizeLabel = node.sizeLabel || (node.size != null ? bytes(node.size) : "");
    return `<div class="cf-tree-row" style="--cf-depth:${Math.min(depth, 12)}">
      ${node.children?.length ? `<button type="button" class="cf-fold-toggle" data-folder="${index}" data-focus="folder${key}" aria-expanded="${expanded}" aria-label="${expanded ? "折叠" : "展开"} ${name}"></button>` : '<span class="cf-tree-spacer"></span>'}
      <${tag} class="${node.children ? "cf-group" : "cf-file"}">${toggle ? `<input type="checkbox" data-pick="${index}" data-focus="check${key}" ${selection}>` : ""}
        <svg class="ic" aria-hidden="true"><use href="${icon}"/></svg><span class="n" title="${escapeHtml(node.path || node.name)}">${name}</span>
        ${sizeLabel ? `<span class="s">${escapeHtml(sizeLabel)}</span>` : ""}${waitText}</${tag}></div>
      ${expanded ? node.children.map((child) => row(child, depth + 1)).join("") : ""}`;
  }
  box.innerHTML = `<div class="cf-files-head">
      <span class="sum">已选 ${preview && !baiduSelectable ? plainAll : `${on.length + plainOn}/${all.length + plainAll}`} 个${collection ? "视频" : "文件"}${showTotal ? `，共 ${bytes(size)}` : ""}${wait ? `，还有 ${wait} 个在获取文件列表，不等也能下载` : ""}${collection && cf.extra.unavailable ? `，${cf.extra.unavailable} 个视频不可用` : ""}</span>
      ${selectable ? `<label><input type="checkbox" data-cat="all" data-focus="all" ${plainAll && plainOn === plainAll ? "checked" : ""}> 全选</label>` : all.length ? `<label><input type="checkbox" data-cat="v" ${checked(vids)}> 视频</label>
      <label><input type="checkbox" data-cat="o" ${checked(others)}> 其他</label>` : ""}
    </div>
    <div class="cf-files-list">${cfFileTree(files).map((node) => row(node, 0)).join("")}</div>`;
  const list = box.querySelector(".cf-files-list");
  if (list) list.scrollTop = scrollTop;
  mixed.forEach((index) => { box.querySelector(`[data-pick="${index}"]`).indeterminate = true; });
  if (selectable) box.querySelector('[data-cat="all"]').indeterminate = plainOn > 0 && plainOn < plainAll;
  if (preview) renderBaiduDownloadButton(cf);
  if (activeKey) [...box.querySelectorAll("[data-focus]")].find((element) => element.dataset.focus === activeKey)?.focus({ preventScroll: true });
}

$("cf-files")?.addEventListener("click", (e) => {
  const button = e.target.closest("[data-folder]");
  if (!button) return;
  const key = cfFileRows[button.dataset.folder].key;
  if (!cfClosedFolders.delete(key)) cfClosedFolders.add(key);
  renderCfFiles();
});

function setCfCollectionSelection(on) {
  const baidu = canPickBaiduFiles(cf);
  for (const item of baidu ? cf.baiduFiles : cfFiles) if (!item.directory) item.on = on;
  if (baidu) rememberBaiduPicks(cf);
  renderCfFiles();
}

$("cf-files")?.addEventListener("change", (e) => {
  const b = e.target;
  if (b.dataset.cat === "all") return setCfCollectionSelection(b.checked);
  if (b.dataset.cat) {
    const isV = b.dataset.cat === "v";
    cfFiles.forEach((g) => g.files.forEach((f) => { if (VIDEO_FILE.test(f.name) === isV) f.on = b.checked; }));
  } else if (b.dataset.pick !== undefined) {
    for (const entry of cfFileRows[b.dataset.pick].entries) entry.on = b.checked;
    if (canPickBaiduFiles(cf)) rememberBaiduPicks(cf);
  }
  renderCfFiles();
});

function isSelectAllShortcut(e) {
  return !e.defaultPrevented && !e.isComposing && !e.shiftKey && !e.altKey &&
    (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a" &&
    !e.target.closest?.("textarea, select, input:not([type=checkbox]):not([type=radio]), [contenteditable]:not([contenteditable=false])");
}

document.addEventListener("keydown", (e) => {
  if (!isSelectAllShortcut(e)) return;
  const baidu = canPickBaiduFiles(cf) && !cf.baiduAuth && !cf.baiduPreparing && !cf.baiduSubmitting;
  if ((!baidu && !(cf.extra?.collection || cf.extra?.feishu || isHuggingFaceFilePage(cf.extra?.sourceUrl))) || $("confirm").hidden || $("cf-files").hidden) return;
  if (!$("choice").hidden || !$("rm-confirm").hidden) return;
  const items = baidu ? cf.baiduFiles : cfFiles;
  if (!items?.length) return;
  e.preventDefault();
  if (items.some(item => !item.directory && !item.on)) setCfCollectionSelection(true);
});

/// 这个链接要下哪些文件。null = 全部；空数组 = 这条不下
function cfSelection(url) {
  const g = cfFiles?.find((x) => x.url === url);
  if (!g) return null;
  if (!g.on) return [];
  if (!g.files.length) return null;
  const on = g.files.filter((f) => f.on);
  return on.length === g.files.length ? null : on.map((f) => f.i);
}

function heightLabel(h) {
  return { 4320: "8K", 2160: "4K", 1440: "2K" }[h] || `${h}P`;
}

/// 清晰度按钮：0 是「最高」，其余最多放 5 档，再多一行放不下
function setQualityButtons(heights) {
  const box = $("cf-quality");
  box.innerHTML = "";
  for (const h of [0, ...heights.slice(0, 5)]) {
    const b = document.createElement("button");
    b.type = "button";
    b.dataset.h = h;
    b.textContent = h ? heightLabel(h) : "最高";
    box.append(b);
  }
}

/// 框先开，后台查这个视频的字幕和清晰度：有结果再换成实际的选项。
/// 查询中的能力不能当作查询结果：字幕控件只在确认有字幕后显示。
async function beginConfirmLookups() {
  const cur=cf;if(cur.lookupsStarted)return;cur.lookupsStarted=true;
  if (cur.extra?.baiduShare) return probeBaiduFileList(cur);
  if (cur.extra?.feishu) return probeCfFeishu(cur);
  if (cur.extra?.collection) return probeCfCollection(cur);
  const batch=cur.extra?.batch;
  loadCfFiles(batch ? batch.map(x=>({url:x.url,name:x.name||grabNames.get(x.url),size:x.size,sizeLabel:x.sizeLabel})) : [{url:cur.url,name:""}]);
  probeCfSubs();
  if(cur.extra?.checkDuplicate && !cur.extra.force && !batch && !cur.extra.wechatDuplicateChecked){
    const answer=await askDuplicate(cur.url,cur.dir);
    if(cf!==cur)return;
    if(!answer){closeConfirm();return;}
    cur.extra={...cur.extra,...answer,duplicateChecked:true,duplicateUrl:cur.url,duplicateDir:cur.dir};$("cf-check-state").hidden=true;$("cf-ok").disabled=false;
  }
}

function showCollectionLoading() {
  const feedback = $("cf-collection-state");
  if (feedback.hidden || !feedback.querySelector(".spin")) {
    setProbeState(feedback, "正在读取合集…", true);
  }
  return feedback;
}

function showFeishuIssue(draft, error) {
  if (draft !== cf || $("confirm").hidden) return;
  const feedback = $("cf-collection-state");
  const needsAuthorization = error?.needsAuthorization === true;
  setProbeState(feedback, error?.message || String(error), false);
  if (error?.permissionUrl) {
    const guide = document.createElement("div");
    guide.className = "feishu-permission-guide";
    const message = document.createElement("p");
    message.textContent = error.message;
    const instructions = document.createElement("p");
    instructions.textContent = "打开权限管理，选「用户身份权限」，勾选查看新版文档、下载云文档中的图片和附件、查看知识空间节点信息，确认开通。按飞书提示发布后返回重试。";
    guide.append(message, instructions);
    if (error.policyBlocked) {
      const note = document.createElement("p");
      note.textContent = "三项已开通并发布后仍出现 21001，请联系飞书核查当前账号或应用的数据安全策略。";
      guide.append(note);
    }
    const actions = document.createElement("div");
    actions.className = "feishu-permission-actions";
    const open = document.createElement("button");
    open.type = "button";
    open.className = "primary";
    open.textContent = "打开飞书权限管理";
    open.onclick = () => invoke("open_url", { url: error.permissionUrl }).catch(e => toast(String(e)));
    const retry = document.createElement("button");
    retry.type = "button";
    retry.className = "ghost";
    retry.textContent = "重新检查";
    retry.onclick = () => probeCfFeishu(draft);
    actions.append(open, retry);
    if (error.policyBlocked) {
      const documentButton = document.createElement("button");
      documentButton.type = "button";
      documentButton.className = "ghost";
      documentButton.textContent = "打开飞书文档";
      documentButton.onclick = () => invoke("open_url", { url: draft.url }).catch(e => toast(String(e)));
      actions.append(documentButton);
    }
    guide.append(actions);
    feedback.replaceChildren(guide);
    fitConfirmDialog();
    return;
  }
  const action = document.createElement("button");
  action.type = "button";
  action.className = needsAuthorization ? "primary" : "ghost";
  if (error?.policyBlocked) {
    action.textContent = "打开飞书文档";
    action.onclick = () => invoke("open_url", { url: draft.url }).catch(error => toast(String(error)));
  } else {
    action.textContent = needsAuthorization ? "授权飞书" : "重试";
    action.onclick = () => needsAuthorization ? authorizeCfFeishu(draft) : probeCfFeishu(draft);
  }
  feedback.append(action);
  fitConfirmDialog();
}

async function authorizeCfFeishu(draft) {
  if (draft !== cf || draft.feishuAuthorizing || $("confirm").hidden) return;
  draft.feishuAuthorizing = true;
  setProbeState($("cf-collection-state"), "正在准备飞书授权…", true);
  fitConfirmDialog();
  try {
    await invoke("authorize_feishu");
  } catch (error) {
    showFeishuIssue(draft, typeof error === "object" && error !== null
      ? error : { message: String(error), needsAuthorization: true });
    return;
  } finally {
    draft.feishuAuthorizing = false;
  }
  if (draft === cf && !$("confirm").hidden) await probeCfFeishu(draft);
}

window.__TAURI__?.event?.listen("feishu-authorization-progress", ({ payload }) => {
  if (cf?.feishuAuthorizing && !$("confirm").hidden) {
    setProbeState($("cf-collection-state"), payload, true);
    fitConfirmDialog();
  }
});

async function probeCfFeishu(cur) {
  const feedback = $("cf-collection-state");
  setProbeState(feedback, "正在读取飞书原始音视频…", true);
  fitConfirmDialog();
  try {
    const result = await invoke("probe_feishu_document", { link: cur.url });
    if (cur !== cf || $("confirm").hidden) return;
    cur.extra.feishuTicket = result.ticket;
    cur.extra.folderName = result.title;
    $("cf-name-label").textContent = "文件夹";
    $("cf-name").closest(".set-row").hidden = false;
    $("cf-name").value = result.title;
    setCfDefaultName(result.title);
    feedback.hidden = true;
    loadCfFiles(result.files.map((file, index) => ({ ...file, url: "feishu:" + index })));
  } catch (error) {
    showFeishuIssue(cur, error);
  }
}

async function probeCfCollection(cur) {
  const feedback = showCollectionLoading();
  fitConfirmDialog();
  try {
    const result = cur.extra.collectionItems
      ? { title: cur.extra.folderName, items: cur.extra.collectionItems }
      : await invoke("probe_collection", { url: cur.url });
    if (cur !== cf || $("confirm").hidden) return;
    if (!result.items.length) throw new Error("合集里没有可下载的视频");
    const folderName = collectionFolderName(result.title);
    cur.extra = { ...cur.extra, batch: result.items, folderName, unavailable: result.unavailable };
    $("cf-name-label").textContent = "合集文件夹";
    $("cf-name").closest(".set-row").hidden = false;
    $("cf-name").value = folderName;
    setCfDefaultName(folderName);
    $("cf-title").textContent = `${cur.extra.redownloadGroup ? "重新下载" : "新建下载"}（${result.items.length} 个视频）`;
    feedback.hidden = true;
    loadCfFiles(result.items);
    cur.subtitleAvailability = cur.supportsSubs ? "available" : "unsupported";
    renderCfVideo();
    checkCfPolish(cur);
  } catch (error) {
    if (cur !== cf || $("confirm").hidden) return;
    setProbeState(feedback, error?.message || String(error), false);
    const retry = document.createElement("button");
    retry.type = "button";
    retry.className = "ghost";
    retry.textContent = "重试";
    retry.addEventListener("click", () => probeCfCollection(cur), { once: true });
    feedback.append(retry);
    fitConfirmDialog();
  }
}

function collectionFolderName(title) {
  const name = String(title || "").replace(/[<>:"/\\|?*\u0000-\u001f]/g, " ").trim().slice(0, 120).replace(/[ .]+$/, "");
  if (!name) return "视频合集";
  return /^(CON|CONIN\$|CONOUT\$|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])(?:\.|$)/i.test(name) ? "_" + name : name;
}

function setProbeState(el, text, loading) {
  el.replaceChildren();
  if(loading){const spin=document.createElement("span");spin.className="spin";spin.setAttribute("aria-hidden","true");el.append(spin);}
  el.append(document.createTextNode(text));el.hidden=false;
}
function setCfDefaultName(name, fallback = "") {
  cf.defaultName = String(name || "").trim();
  $("cf-name").placeholder = cf.defaultName || fallback;
  renderCfNameState();
}
function renderCfNameState() {
  const name=$("cf-name"), loading=$("cf-name-state");
  const hint=$("cf-name-hint");
  hint.hidden=!(cf?.defaultName && !name.value);
  if (hint.hidden) name.removeAttribute("aria-describedby");
  else name.setAttribute("aria-describedby",hint.id);
  const pending=!!cf?.nameLoading || (!cf?.video && cfFiles?.length===1 && cfFiles[0].url===cf.url && cfFiles[0].state==="loading");
  const busy=pending && !String(name.value || "").trim();
  const wrap=name.closest?.(".clear-wrap");
  if(wrap){
    if(loading.parentElement!==wrap)wrap.appendChild(loading);
    wrap.classList.toggle("has-leading-status",busy);
  }
  loading.hidden=!busy;
  name.setAttribute("aria-busy",String(busy));
}
function checkCfPolish(cur) {
  if (!cur.video) return;
  invoke("ollama_models").then(models => {
    if (cf === cur && !$("confirm").hidden) {
      cur.hasOllama = !!models.length;
      renderCfVideo();
    }
  }).catch(() => {});
}

async function probeCfSubs() {
  const cur=cf, note=$("cf-subs-state"), quality=$("cf-quality-state");
  cur.subtitleAvailability=cur.supportsSubs ? "checking" : "unsupported";
  note.hidden=true;quality.hidden=true;$("cf-quality").setAttribute("aria-busy","false");
  setQualityButtons([1080,720]);renderCfVideo();
  checkCfPolish(cur);
  if(!cur.video || cur.extra?.batch)return;
  setProbeState(quality,"正在查询画质…",true);$("cf-quality").setAttribute("aria-busy","true");
  if(cur.supportsSubs)setProbeState(note,"正在查询字幕…",true);
  const video = { maxHeight: cur.video.maxHeight || 0, audioOnly: !!cur.video.audioOnly };
  const r=await invoke("probe_video",{url:cur.url,video}).catch(()=>null);
  if(cf!==cur)return;
  $("cf-quality").setAttribute("aria-busy","false");quality.hidden=true;note.hidden=true;
  cur.nameLoading=false;
  if (!cur.extra?.batch) setCfDefaultName(cur.extra?.folderName || r?.title || cur.name,"留空就用视频标题");
  if(!r){cur.subtitleAvailability=cur.supportsSubs ? "failed" : "unsupported";setProbeState(quality,"画质查询未完成，保留默认选项",false);if(cur.supportsSubs)setProbeState(note,"字幕查询失败，暂不提供字幕选项",false);renderCfVideo();return;}
  if (Number.isSafeInteger(r.size) && r.size > 0) cur.probedSize = { ...video, size: r.size };
  if(r.heights?.length)setQualityButtons(r.heights);
  if(cur.supportsSubs){
    cur.subtitleAvailability=r.subs===true ? "available" : r.subs===false ? "none" : "failed";
    if(cur.subtitleAvailability==="none")setProbeState(note,"这个视频没有字幕",false);
    if(cur.subtitleAvailability==="failed")setProbeState(note,"字幕查询失败，暂不提供字幕选项",false);
  }
  renderCfVideo();
}

/// 关掉确认框。drain=false 用在 #cf-ok：确认那条路要等 doAdd 做完再接排队的下一条，
/// 在这儿接会抢跑；取消（按钮/遮罩/Esc）才顺手在这儿接
function closeConfirm(drain = true) {
  $("cf-dir-menu").hidden = true;
  if (cf?.extra?.baiduShare) {
    cf.baiduCancelled = true;
    cf.baiduAuth = null;
    invoke?.("cancel_baidu_prepare").catch(() => {});
  }
  if (cf?.feishuAuthorizing) {
    cf.feishuAuthorizing = false;
    invoke?.("cancel_feishu_authorization").catch(() => {});
  }
  if (DIALOG) return closeDialogWindow();
  $("confirm").hidden = true;
  if (drain) nextPending();
}

// 点「保存到」输入框列出用过的目录。没历史就什么都不弹——
// 一个空下拉比没有下拉更碍事；「默认位置」和「选择文件夹」由旁边那个「更改…」和清空输入框承担
function positionDirMenu(menu) {
  const input = $("cf-dir").getBoundingClientRect();
  const margin = 8, gap = 6;
  const width = Math.max(0, Math.min(560, input.width, window.innerWidth - margin * 2));
  menu.style.minWidth = "0";
  menu.style.width = `${width}px`;
  menu.style.maxHeight = "";
  const desired = Math.min(280, menu.scrollHeight + menu.offsetHeight - menu.clientHeight);
  const below = window.innerHeight - input.bottom - gap - margin;
  const above = input.top - gap - margin;
  const up = below < desired && above > below;
  menu.style.maxHeight = `${Math.max(0, Math.floor(up ? above : below))}px`;
  const left = Math.max(margin, Math.min(input.left, window.innerWidth - menu.offsetWidth - margin));
  menu.style.left = `${Math.round(left)}px`;
  menu.style.top = `${up ? Math.floor(input.top - gap - menu.offsetHeight) : Math.ceil(input.bottom + gap)}px`;
}

function renderDirMenu() {
  const menu = $("cf-dir-menu");
  const recents = state.settings?.settings?.recentDirs || [];
  if (!recents.length) {
    menu.hidden = true;
    return;
  }
  const scrollTop = menu.hidden ? 0 : menu.scrollTop;
  menu.innerHTML = "";
  for (const d of recents) {
    // 一行 = 路径 + 悬停才出现的 ×：删掉这条记忆，不动目录本身
    const row = document.createElement("div");
    row.className = "cf-row";
    const b = document.createElement("button");
    b.className = "ctx-item";
    b.textContent = d;
    b.title = d;
    b.addEventListener("click", () => {
      $("cf-dir").value = d;
      cf.dir = d;
      menu.hidden = true;
    });
    const del = document.createElement("button");
    del.className = "cf-del";
    del.title = "不再记住这个目录";
    del.innerHTML = '<svg class="ic"><use href="#i-x"/></svg>';
    del.addEventListener("click", async (ev) => {
      ev.stopPropagation();
      state.settings.settings.recentDirs = recents.filter((x) => x !== d);
      // 菜单留着，可能还要接着删别的
      renderDirMenu();
      await invoke("save_settings", { settings: state.settings.settings }).catch((e) => toast("保存失败：" + e));
    });
    row.appendChild(b);
    row.appendChild(del);
    menu.appendChild(row);
  }
  menu.hidden = false;
  positionDirMenu(menu);
  menu.scrollTop = scrollTop;
}
$("cf-dir")?.addEventListener("click", () => {
  if ($("cf-dir").readOnly) return;
  const menu = $("cf-dir-menu");
  if (!menu) return;
  if (!menu.hidden) {
    menu.hidden = true;
    return;
  }
  renderDirMenu();
});
$("confirm")?.querySelector(".modal-body")?.addEventListener("scroll", () => {
  $("cf-dir-menu").hidden = true;
}, { passive: true });
window.addEventListener("resize", () => {
  const menu = $("cf-dir-menu");
  if (menu && !menu.hidden) positionDirMenu(menu);
});
// 按下就收（不等 click）：独立窗口里按在空白处会开始拖窗口，click 不会再来
document.addEventListener("pointerdown", (e) => {
  const menu = $("cf-dir-menu");
  if (!menu || menu.hidden) return;
  if (e.target.closest("#cf-dir-menu") || e.target.closest("#cf-dir")) return;
  menu.hidden = true;
});

$("cf-pick").addEventListener("click", async () => {
  if (!invoke) return;
  const picked = await invoke("pick_folder", { title: "保存到", current: cf.dir }).catch(() => null);
  if (!picked) return;
  cf.dir = picked;
  $("cf-dir").value = picked;
});

$("cf-cancel").addEventListener("click", () => closeConfirm());
// 点遮罩空白处也关掉，和系统弹窗的手感一致
$("confirm").addEventListener("click", (e) => {
  // 点外面不关：选了半天的文件不能一碰就没了，要关用取消或 Esc
  void e;
});

$("cf-name").addEventListener("keydown", (e) => {
  const input=e.currentTarget;
  if (e.key === "Tab" && !e.repeat && !e.isComposing && !e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey && !input.value && cf.defaultName) {
    e.preventDefault();
    input.value=cf.defaultName;
    input.setSelectionRange(input.value.length,input.value.length);
    input.dispatchEvent(new Event("input",{bubbles:true}));
    return;
  }
  if (e.key === "Enter") $("cf-ok").click();
});

// 视频选项：画质分段、下载字幕、仅音频（勾了仅音频，画质和字幕灰掉）
$("cf-quality").addEventListener("click", (e) => {
  const b = e.target.closest("button[data-h]");
  if (!b || !cf.video || b.disabled) return;
  cf.video.maxHeight = Number(b.dataset.h);
  renderCfVideo();
});
$("cf-polish").addEventListener("change", () => {
  cf.polish = $("cf-polish").checked;
});
$("cf-subs").addEventListener("change", () => {
  if (cf.video) cf.video.subtitles = $("cf-subs").checked;
  renderCfVideo();
});
document.querySelectorAll("#cf-sublangs button").forEach((b) => {
  b.addEventListener("click", () => {
    if (!cf.video) return;
    cf.video.subLangs = b.dataset.langs;
    // 以前这里只改了按钮的 on 类，下面着色行那几个色点不跟着变——
    // 于是选了中文还挂着「中/英」两个点。统一走 renderCfVideo 重算
    renderCfVideo();
  });
});
document.querySelectorAll("#cf-subfmt button").forEach((b) => {
  b.addEventListener("click", () => {
    if (!cf.video) return;
    cf.video.subFormat = b.dataset.fmt;
    renderCfVideo();
  });
});
document.querySelectorAll("#cf-subsize button").forEach((b) => {
  b.addEventListener("click", () => {
    if (!cf.video) return;
    cf.video.subScale = Number(b.dataset.scale);
    document.querySelectorAll("#cf-subsize button").forEach((x) =>
      x.classList.toggle("on", x === b)
    );
  });
});
// ---------- 字幕样式：改已下载视频的字幕外观，带预览 ----------
// 和新建下载一样开独立窗口（不受主窗口大小限制，能拖大小）。多选时一次改多个视频的字幕。
// 改的是 ASS 字幕文件本身（字号、字体、颜色、描边、阴影、黑底、位置），改完重新嵌进视频；
// NAS 的字幕取到电脑上改完再写回。样式可以存成预设、设一个为默认（新下载按它生成），
// 也能导出成文件（含全部预设和当前设置）发给别人，别人导入这个文件
const SUB_LOOK_DEFAULT = { scale: 1, scaleEn: 0.86, colorize: false, colorZh: "#00FFAC", colorEn: "#516EFE", shadow: true, shadowSize: 0.8, shadowAlpha: 50, boxed: false, boxAlpha: 62, outline: 1.5, margin: 3, font: "Microsoft YaHei" };
const SUB_LOOK_KEY = "sd.subLookNas"; // NAS 任务读不到原来的样式，记住上次用的
const SUB_PRESETS_KEY = "sd.subPresets"; // [{ name, look }]
const SUB_DEFAULT_KEY = "sd.subPresetDefault"; // 默认预设的名字
const SUB_SIZES = [["小", 0.8], ["标准", 1], ["大", 1.25]];
const SUB_FILE_TAG = "shandian-subtitle-styles";

const pendingPolish = new Set();
function isPolishing(task) {
  return !!task && (pendingPolish.has(task.id) || /润色字幕中/.test(task.polish || ""));
}

function canPolishTask(task) {
  return canSubStyle(task) && !task.collectionFiles && !isPolishing(task)
    && !isFetchingTaskSubtitles(task) && (!String(task.id).startsWith("nas:") || window.__nasOllama);
}

async function polishCollection(tasks, restore = false) {
  const targets = tasks.filter(task => canPolishTask(task) && (!restore || task.polished));
  if (!targets.length) return;
  const cmd = restore ? "polish_restore" : "polish_task";
  targets.forEach(task => pendingPolish.add(task.id));
  if (filesPanelId) drawFilesRows();
  toast(restore ? "正在恢复原字幕…" : `已开始处理 ${targets.length} 项字幕`);
  const results = [];
  try {
    for (const task of targets) {
      try {
        await invoke(cmd, { id: task.id });
        results.push(true);
      } catch {
        results.push(false);
      }
    }
  } finally {
    targets.forEach(task => pendingPolish.delete(task.id));
    refreshTasks();
  }
  const failed = results.filter(result => !result).length;
  if (failed) toast(`${failed} 项字幕操作未开始，请查看单项状态`);
  if (restore && failed < targets.length) toast(`已恢复 ${targets.length - failed} 项原字幕`);
}

function renderSubtitleControls(task) {
  if (!task) return "";
  const info = subtitleState(task);
  const finishedVideo = task.protocol === "video" && task.status === "done";
  if (!finishedVideo && !task.collectionFiles && !task.subs && !task.subsFetchable && !info.label) return "";
  const fetching = isFetchingTaskSubtitles(task);
  const working = isPolishing(task);
  const accessible = task.status === "done" && !task.missing && task.fileState !== "offline";
  const english = task.subs === "英文字幕";
  const waiting = task.polish === "等待润色";
  const button = (label, action, hint) =>
    `<button type="button" class="subs clickable" data-subtitle-action="${action}" data-task-id="${escapeHtml(task.id)}" data-tip="${escapeHtml(hint)}">${escapeHtml(label)}</button>`;
  const members = task.collectionFiles && accessible ? groupMembers(task.id) : [];
  const missing = missingSubtitleTasks(members);
  const count = task.collectionSubtitleCount;
  let label = task.subs || info.label || "未获取字幕";
  if (task.collectionFiles) {
    if (!accessible && !task.subs) return "";
    if (count && count < task.collectionFiles.length) label = `字幕 ${count}/${task.collectionFiles.length}`;
  }
  if (!task.subs && fetching) label = "获取字幕中";
  const status = task.subs && canSubStyle(task) ? button(label, "style", subStyleHint(task))
    : `<span class="subs" data-tip="${escapeHtml(info.tip || label)}">${escapeHtml(label)}</span>`;
  let detail = "";
  if (fetching) detail = task.subs ? "正在获取…" : "";
  else if (working || waiting) {
    const phase = pendingPolish.has(task.id) ? "润色字幕中…" : task.polish || "润色字幕中…";
    detail = english ? phase.replace("等待润色", "等待翻译").replace("润色字幕中", "翻译中") : phase;
  } else if (task.polish) {
    detail = /失败/.test(task.polish) ? (english ? "翻译失败" : "润色失败") : task.polish;
  }
  const detailTip = task.polish?.trim();
  const tooltip = detailTip && detailTip !== detail.trim() ? ` data-tip="${escapeHtml(detailTip)}"` : "";
  const progress = detail ? `<span class="subtitle-detail"${tooltip}>${escapeHtml(detail)}</span>` : "";
  let action = "";
  if (accessible && !fetching && !working) {
    if (task.collectionFiles) {
      if (missing.some(candidate => candidate.subs !== "英文字幕")) action = button("补字幕", "fill", "补齐合集里缺少的字幕");
      if (members.some(candidate => candidate.subs === "英文字幕" && !isPolishing(candidate))) {
        action += button("翻译", "translate", "用本机 Ollama 翻译合集内只有英文字幕的项目");
      }
    } else if (english) action = button("翻译", "translate", "用本机 Ollama 将英文字幕翻译成中文");
    else if (canFetchTaskSubtitles(task) && (!task.subs || info.label === "字幕未完整")) {
      action = button(task.subs ? "补字幕" : info.checked ? "重查" : "获取", "fetch", "从网站获取字幕");
    }
  }
  return status + action + progress;
}

async function translateTaskSubtitles(task) {
  if (task.collectionFiles) {
    const members = groupMembers(task.id).filter(candidate => candidate.subs === "英文字幕" && !isPolishing(candidate) && !isFetchingTaskSubtitles(candidate));
    if (!members.length) return;
    if (members.some(candidate => String(candidate.id).startsWith("nas:")) && !window.__nasOllama) {
      toast("翻译中文字幕需要先启动本机 Ollama 并选择模型");
      return;
    }
    return polishCollection(members);
  }
  if (isPolishing(task) || isFetchingTaskSubtitles(task)) return;
  pendingPolish.add(task.id);
  if (filesPanelId) drawFilesRows();
  try {
    await invoke("polish_task", { id: task.id });
    toast("已开始翻译中文字幕");
  } catch (error) {
    toast(error?.message || String(error));
  } finally {
    pendingPolish.delete(task.id);
    refreshTasks();
  }
}

function canSubStyle(t) {
  if (t?.collectionFiles) return t.status === "done" && t.collectionSubtitleCount > 0;
  return !!t && t.status === "done" && !t.missing && t.fileState !== "offline" && !!t.subs && /^(v|nas):/.test(String(t.id));
}

function subStyleHint(task) {
  if (!task.collectionFiles) return "调整字幕样式";
  const labels = task.collectionSubtitleLabels.join("、");
  return "调整合集里 " + task.collectionSubtitleCount + " 项的字幕样式（" + labels + "）";
}

function findSubtitleTask(id) {
  return state.tasks.find(task => task.id === id) || rawTasks.find(task => task.id === id);
}

function canFetchTaskSubtitles(task) {
  return !!task && task.status === "done" && !task.missing && task.fileState !== "offline" && (task.subsFetchable || canSubStyle(task));
}

const subtitleFetches = new Set();
function isFetchingTaskSubtitles(task) {
  return task?.status === "done" && (subtitleFetches.has(task.id) || /字幕/.test(task.stage || ""));
}

function missingSubtitleTasks(tasks) {
  return tasks.filter(task => canFetchTaskSubtitles(task) && (subtitleState(task).label === "字幕未完整" || (task.subsFetchable && !canSubStyle(task))));
}

function selectedSubtitleAction(tasks) {
  const fetchable = tasks.filter(canFetchTaskSubtitles);
  const busy = tasks.some(isFetchingTaskSubtitles);
  if (!fetchable.length && !busy) return null;
  return { targets: busy ? [] : fetchable, mode: "fetch", label: busy ? "正在获取…" : `获取字幕 ${fetchable.length}` };
}

function collectionSubtitleAction(tasks) {
  if (drawerFileSelection.size) return selectedSubtitleAction(drawerSubtitleTasks(drawerFilesFor(), tasks));
  const missing = missingSubtitleTasks(tasks);
  if (missing.length || tasks.some(task => isFetchingTaskSubtitles(task) && !canSubStyle(task))) {
    const targets = missing.filter(task => !isFetchingTaskSubtitles(task));
    return { targets, mode: "fill", label: targets.length ? `补齐字幕 ${targets.length}` : "正在补字幕…" };
  }
  const existing = tasks.filter(canSubStyle);
  if (!existing.length) return null;
  const busy = tasks.some(isFetchingTaskSubtitles);
  return { targets: busy ? [] : existing, mode: "refresh", label: busy ? "正在获取…" : "重新获取" };
}

async function fetchTaskSubtitles(id) {
  const task = findSubtitleTask(id);
  if (!canFetchTaskSubtitles(task) || isFetchingTaskSubtitles(task)) return;
  subtitleFetches.add(id);
  if (filesPanelId && filesList.some(file => file.taskId === id)) drawFilesRows();
  const close = toast("正在获取字幕…");
  try {
    const got = await invoke("subtitle_fetch", { id });
    close();
    toast(got === "queued" ? "NAS 正在获取字幕，好了会自动显示" : got ? "字幕已获取" : "没有找到可下载的所选语言字幕");
  } catch (error) {
    close();
    toast("获取字幕失败：" + (error?.message || String(error)));
  } finally {
    subtitleFetches.delete(id);
    refreshTasks();
  }
}
async function fetchSubtitlesBatch(tasks, mode = "fill") {
  const targets = tasks.filter(task => !isFetchingTaskSubtitles(task));
  if (!targets.length) return;
  targets.forEach(task => subtitleFetches.add(task.id));
  if (filesPanelId && filesList.some(file => subtitleFetches.has(file.taskId))) drawFilesRows();
  const verb = { fill: "补字幕", refresh: "重新获取字幕", fetch: "获取字幕" }[mode];
  const close = toast(`正在${verb}（${targets.length} 集）…`);
  let done = 0;
  let queued = 0;
  let unavailable = 0;
  let failures = 0;
  try {
    // Limit this batch to one subtitle fetch at a time on the client.
    for (const task of targets) {
      try {
        const result = await invoke("subtitle_fetch", { id: task.id });
        if (result === "queued") queued += 1;
        else if (result) done += 1;
        else unavailable += 1;
      } catch {
        failures += 1;
      }
    }
  } finally {
    targets.forEach(task => subtitleFetches.delete(task.id));
    close();
    refreshTasks();
  }
  toast([
    done && `已${verb} ${done} 集`,
    queued && `已交给 NAS ${verb} ${queued} 集`,
    unavailable && `${unavailable} 集没有所选语言的字幕`,
    failures && `${failures} 集获取失败，可稍后重试`,
  ].filter(Boolean).join("，"));
}
function subPresets() {
  try { return JSON.parse(localStorage.getItem(SUB_PRESETS_KEY) || "[]"); } catch (_) { return []; }
}
function saveSubPresets(list) {
  try { localStorage.setItem(SUB_PRESETS_KEY, JSON.stringify(list)); } catch (_) {}
}
function defaultSubLook() {
  const name = localStorage.getItem(SUB_DEFAULT_KEY);
  const p = name && subPresets().find((x) => x.name === name);
  return p ? { ...SUB_LOOK_DEFAULT, ...p.look } : null;
}
/// 新建下载：有默认预设时，按它填字幕的各项样式（框里的字号、阴影、黑底开关仍可再改）
function withSubDefault(v) {
  const d = defaultSubLook();
  if (!d) return v;
  return { ...v, subScale: d.scale, subScaleEn: d.scaleEn, subColorize: d.colorize, subColorZh: d.colorZh, subColorEn: d.colorEn, subShadow: d.shadow, subShadowSize: d.shadowSize, subShadowAlpha: d.shadowAlpha, subBox: d.boxed, subBoxAlpha: d.boxAlpha, subOutline: d.outline, subMargin: d.margin, subFont: d.font };
}

/// 主窗口：开字幕样式窗口。ids 是要改的任务（多选时多个）
function openSubStyle(ids) {
  const members = (Array.isArray(ids) ? ids : [ids]).flatMap(id => isGroup(id) ? groupMembers(id) : [findSubtitleTask(id)]);
  const tasks = [...new Map(members.filter(canSubStyle).map(task => [task.id, task])).values()];
  if (!tasks.length) return toast("选中的任务里没有可以改样式的字幕");
  const list = tasks.map(task => task.id);
  const names = tasks.map((t) => t?.name || t?.meta?.res?.name || "");
  // 预览按视频画面比例画（如「1080 x 1920」的竖屏）；读不到就按 16:9
  const m = /(\d+)\s*[x×]\s*(\d+)/.exec(tasks[0]?.resolution || "");
  const ratio = m && +m[1] > 0 && +m[2] > 0 ? +m[1] / +m[2] : 0;
  // 预览画面用这个视频的封面；比例没记下时按封面图本身的比例
  openDialog("substyle", { ids: list, names, ratio, cover: thumbs[list[0]] || "" });
}

function dialogProgress(title, message, subtitle = false) {
  const loading = document.createElement("div");
  if (subtitle) loading.id = "sub-style";
  else loading.className = "modal-mask";
  loading.innerHTML = `<div class="modal${subtitle ? " sub-style-modal" : ""}" role="dialog" aria-labelledby="dialog-progress-title">
    <button type="button" class="modal-x" aria-label="关闭"><svg class="ic"><use href="#i-x"/></svg></button>
    <h3 id="dialog-progress-title">${escapeHtml(title)}</h3>
    <div class="probe-state" role="status"></div>
  </div>`;
  loading.querySelector("button").addEventListener("click", () => closeDialogWindow());
  setProbeState(loading.querySelector("[role=status]"), message, true);
  document.body.appendChild(loading);
  return loading;
}

async function showSubStyleDialog(me, { ids, names, ratio, cover }) {
  document.body.classList.add("substyle-window");
  const loading = dialogProgress("字幕样式", "正在读取字幕样式…", true);
  const status = loading.querySelector("[role=status]");
  await me.show();
  me.setFocus?.().catch(() => {});
  try {
    await buildSubStyle(ids, names, ratio, cover, loading);
  } catch (error) {
    const message = "打开字幕样式失败：" + (error?.message || String(error));
    if (loading.isConnected) setProbeState(status, message, false);
    else toast(message);
  }
}

async function buildSubStyle(ids, names, ratio = 0, cover = "", loading) {
  const readCoverRatio = !ratio && !!cover;
  ratio ||= 16 / 9;
  const one = ids.length === 1;
  const isNas = (id) => String(id).startsWith("nas:");
  // 先读单个本机视频现有的设置，封面比例和字体清单独立加载。
  let look = null;
  if (one && !isNas(ids[0])) {
    let timer;
    try {
      look = await Promise.race([
        invoke("subtitle_look", { id: ids[0] }).catch(() => null),
        new Promise(resolve => { timer = setTimeout(() => resolve(null), 2000); }),
      ]);
    } finally {
      clearTimeout(timer);
    }
  }
  if (!look) {
    try { look = JSON.parse(localStorage.getItem(SUB_LOOK_KEY) || "null"); } catch (_) {}
  }
  look = { ...SUB_LOOK_DEFAULT, ...(look || {}) };
  const box = document.createElement("div");
  box.id = "sub-style";
  const range = (key, min, max, step) => `<input type="range" class="ss-range" data-k="${key}" min="${min}" max="${max}" step="${step}"><span class="ss-val" data-v="${key}"></span>`;
  const title = one ? "字幕样式" : `字幕样式（${ids.length} 个视频）`;
  box.innerHTML = `<div class="modal sub-style-modal" role="dialog">
    <button class="modal-x" data-ss="close" aria-label="关闭"><svg class="ic"><use href="#i-x"/></svg></button>
    <h3>${title}</h3>
    <div class="ss-target"></div>
    <div class="ss-presets">
      <div class="select-wrap ss-preset-wrap"><select class="select" id="ss-preset"></select></div>
      <button type="button" class="ghost sm" data-ss="save" data-tip="将当前设置存为预设">存为预设</button>
      <button type="button" class="ghost sm" data-ss="default" data-tip="新下载的视频按当前设置生成字幕">设为默认</button>
      <button type="button" class="ghost sm" data-ss="delete" data-tip="删除预设">删除</button>
      <button type="button" class="ghost sm" data-ss="export" data-tip="导出预设文件">导出</button>
      <button type="button" class="ghost sm" data-ss="import" data-tip="导入预设文件">导入</button>
    </div>
    <div class="ss-ask" id="ss-ask" hidden><input class="field" id="ss-ask-input" placeholder="预设名字，比如「电影」「教程」"><button type="button" class="primary sm" data-ss="ask-ok">确定</button><button type="button" class="ghost sm" data-ss="ask-cancel">取消</button></div>
    <div class="ss-preview" id="ss-preview"><div class="ss-frame" id="ss-frame">
      <div class="ss-cue" id="ss-cue"><div class="ss-zh">风从山那边吹过来，带着雨的味道。</div><div class="ss-en">The wind came over the hills, smelling of rain.</div></div>
    </div></div>
    <div class="ss-body">
      <div class="set-row"><span class="set-label">字体</span><div class="select-wrap ss-font-wrap"><select class="select" id="ss-font"></select></div></div>
      <div class="set-row"><span class="set-label">字号</span><div class="seg" id="ss-size">${SUB_SIZES.map(([l, s]) => `<button type="button" data-size="${s}">${l}</button>`).join("")}</div>
        <label class="modal-note ss-split-label"><input type="checkbox" id="ss-split"> 分别调整</label></div>
      <div class="set-row ss-split-row"><span class="set-label">主字幕字号</span>${range("scale", 0.5, 2, 0.05)}</div>
      <div class="set-row ss-split-row"><span class="set-label">副字幕字号</span>${range("scaleEn", 0.4, 2, 0.05)}</div>
      <div class="set-row"><span class="set-label">着色</span><div class="seg" id="ss-color">
        <button type="button" data-c="off">不着色</button>
        <button type="button" data-c="zh" data-tip="主字幕（第一行）的颜色">主字幕<i class="sub-dot" id="ss-dot-zh"></i></button>
        <button type="button" data-c="en" data-tip="副字幕（第二行）的颜色">副字幕<i class="sub-dot" id="ss-dot-en"></i></button></div></div>
      <div class="set-row"><span class="set-label">背景</span><div class="seg" id="ss-fx">
        <button type="button" data-fx="none">无</button><button type="button" data-fx="shadow">阴影</button><button type="button" data-fx="box">黑底</button></div></div>
      <div class="set-row ss-fx-shadow"><span class="set-label">阴影大小</span>${range("shadowSize", 0.3, 4, 0.1)}</div>
      <div class="set-row ss-fx-shadow"><span class="set-label">阴影浓度</span>${range("shadowAlpha", 10, 100, 5)}</div>
      <div class="set-row ss-fx-box"><span class="set-label">黑底浓度</span>${range("boxAlpha", 10, 100, 5)}</div>
      <div class="set-row ss-fx-line"><span class="set-label">描边粗细</span>${range("outline", 0, 4, 0.1)}</div>
      <div class="set-row"><span class="set-label">离底部</span>${range("margin", 0, 20, 0.5)}</div>
    </div>
    <p class="ss-hint">预览为大致效果，以播放器为准；仅 ASS 字幕可改，字体需电脑已安装。</p>
    <div class="modal-foot"><button class="ghost" data-ss="close">取消</button><button class="primary" data-ss="ok">应用</button></div>
  </div>`;
  if (loading) loading.replaceWith(box);
  else document.body.appendChild(box);
  if (cover) {
    const f = $("ss-frame");
    f.style.backgroundImage = `url("${cover.replace(/"/g, "%22")}")`;
    f.classList.add("has-cover");
  }
  box.querySelector(".ss-target").textContent = one ? names?.[0] || "" : `${(names || []).slice(0, 3).join("、")}${ids.length > 3 ? " 等" : ""}`;
  const fontSelect = $("ss-font");
  fontSelect.add(new Option(look.font, look.font));
  let hoverFont = null;
  initCustomSelect(fontSelect, { fontItems: true, cache: true, onHover: (font) => { hoverFont = font; draw(); } });
  initCustomSelect($("ss-preset"));

  const cur = { ...look };
  // 中英文分开调默认关；字号按原字幕显示（不在三档里时三个按钮都不亮）
  let split = false;
  let ask = null;
  let fitted = false;
  const drawPresets = (pick) => {
    const list = subPresets();
    const def = localStorage.getItem(SUB_DEFAULT_KEY);
    $("ss-preset").innerHTML = `<option value="">${one && !isNas(ids[0]) ? "当前视频样式" : "上次用的样式"}</option>` + list.map((p) => `<option value="${escapeHtml(p.name)}">${escapeHtml(p.name)}${p.name === def ? "（默认）" : ""}</option>`).join("");
    $("ss-preset").value = pick || "";
    const has = !!$("ss-preset").value;
    box.querySelectorAll('[data-ss="default"], [data-ss="delete"]').forEach((b) => { b.disabled = !has; });
    syncCustomSelects();
  };
  const draw = () => {
    $("ss-font").value = cur.font;
    box.querySelectorAll(".ss-range").forEach((r) => {
      r.value = cur[r.dataset.k];
      r.style.setProperty("--fill", `${((r.value - r.min) / (r.max - r.min)) * 100}%`);
    });
    box.querySelectorAll(".ss-val").forEach((s) => {
      const k = s.dataset.v, v = Number(cur[k]);
      s.textContent = k === "scale" || k === "scaleEn" ? `${Math.round(v * 100)}%` : k.endsWith("Alpha") ? `${Math.round(v)}%` : k === "margin" ? `${v}%` : v.toFixed(1);
    });
    $("ss-split").checked = split;
    // 行一直都在，不适用的变灰（以前是藏起来，一切换下面的行就上下跳）
    const dim = (sel, off) => box.querySelectorAll(sel).forEach((r) => r.classList.toggle("ss-off", off));
    dim(".ss-split-row", !split);
    box.querySelectorAll("#ss-size button").forEach((b) => b.classList.toggle("on", Math.abs(Number(b.dataset.size) - cur.scale) < 0.01 && Math.abs(cur.scaleEn - cur.scale * 0.86) < 0.02));
    box.querySelector('#ss-color [data-c="off"]').classList.toggle("on", !cur.colorize);
    box.querySelectorAll('#ss-color [data-c="zh"], #ss-color [data-c="en"]').forEach((b) => b.classList.toggle("on", !!cur.colorize));
    $("ss-dot-zh").style.background = cur.colorZh;
    $("ss-dot-en").style.background = cur.colorEn;
    const fx = cur.boxed ? "box" : cur.shadow ? "shadow" : "none";
    box.querySelectorAll("#ss-fx button").forEach((b) => b.classList.toggle("on", b.dataset.fx === fx));
    dim(".ss-fx-shadow", fx !== "shadow");
    dim(".ss-fx-box", fx !== "box");
    syncCustomSelects();
    // 预览：按视频比例在预览区里等比放一个「画面」（和播放器一样整个画面放得下），
    // 字号 = 画面高度 × 4.2% × 倍率，和生成 ASS 时同一个算法（ASS 的字号就是按画面高度算的）
    const pv = $("ss-preview"), frame = $("ss-frame");
    const W = pv.clientWidth || 400, H = pv.clientHeight || 225;
    const fw = Math.min(W, H * ratio), fh = fw / ratio;
    frame.style.width = `${fw}px`;
    frame.style.height = `${fh}px`;
    // 独立窗口第一次画好时，把窗口宽度收到刚好放下预览画面（左右不留空条）。
    // 只收一次，之后用户自己拖窗口大小不再干预
    if (DIALOG && !fitted && pv.clientWidth && W - fw > 4) {
      fitted = true;
      const win = window.__TAURI__.window.getCurrentWindow();
      Promise.all([win.innerSize(), win.scaleFactor(), win.outerPosition()]).then(async ([s, k, p]) => {
        const w = Math.max(480, s.width / k - (W - fw));
        await win.setSize(new window.__TAURI__.dpi.LogicalSize(w, s.height / k));
        // 收窄后挪回原来的中线
        await win.setPosition(new window.__TAURI__.dpi.PhysicalPosition(Math.round(p.x + (s.width - w * k) / 2), p.y));
      }).catch(() => {});
    } else if (pv.clientWidth) fitted = true;
    const cue = $("ss-cue");
    const h = fh;
    const zh = cue.querySelector(".ss-zh"), en = cue.querySelector(".ss-en");
    const px = (s) => `${Math.max(4, h * 0.042 * s)}px`;
    zh.style.fontSize = px(cur.scale);
    en.style.fontSize = px(cur.scaleEn);
    cue.style.fontFamily = `"${hoverFont || cur.font}", "Microsoft YaHei", sans-serif`;
    cue.style.bottom = `${cur.margin}%`;
    zh.style.color = cur.colorize ? cur.colorZh : "#fff";
    en.style.color = cur.colorize ? cur.colorEn : "#fff";
    zh.style.fontWeight = cur.colorize ? "700" : "400";
    const o = Number(cur.outline) * 0.8;
    const outline = o > 0 ? `-${o}px -${o}px 0 #000, ${o}px -${o}px 0 #000, -${o}px ${o}px 0 #000, ${o}px ${o}px 0 #000` : "";
    const d = Math.max(0, cur.shadowSize) * 2;
    const shadow = cur.shadow && !cur.boxed ? `${d}px ${d}px ${Math.max(1, d / 2)}px rgba(0,0,0,${cur.shadowAlpha / 100})` : "";
    cue.classList.toggle("boxed", !!cur.boxed);
    cue.style.setProperty("--ss-box", `rgba(0,0,0,${cur.boxAlpha / 100})`);
    // 黑底时文字照样有描边（底框是单独一层）；阴影只在不用黑底时有
    [zh, en].forEach((el) => { el.style.textShadow = [outline, cur.boxed ? "" : shadow].filter(Boolean).join(", ") || "none"; });
  };
  const closeAsk = () => { ask = null; $("ss-ask").hidden = true; };
  const finishAsk = () => {
    const name = $("ss-ask-input").value.trim();
    if (ask === "save" && name) {
      const list = subPresets().filter((p) => p.name !== name);
      list.push({ name, look: { ...cur } });
      saveSubPresets(list);
      drawPresets(name);
      toast(`已存为预设：${name}`);
    }
    closeAsk();
  };
  /// 导入文件：合并里面的预设（同名的覆盖），并套用文件里的当前设置
  const importText = (text) => {
    let data;
    try { data = JSON.parse(text); } catch (_) { data = null; }
    if (!data || data.type !== SUB_FILE_TAG) return toast("无法识别的预设文件");
    const incoming = (data.presets || []).filter((p) => p && p.name && typeof p.look === "object");
    const list = subPresets().filter((p) => !incoming.some((x) => x.name === p.name));
    list.push(...incoming.map((p) => ({ name: String(p.name).slice(0, 40), look: { ...SUB_LOOK_DEFAULT, ...p.look } })));
    saveSubPresets(list);
    if (data.current) Object.assign(cur, SUB_LOOK_DEFAULT, data.current);
    drawPresets("");
    draw();
    toast(`已导入 ${incoming.length} 个预设`);
  };
  box.addEventListener("input", (e) => {
    if (e.target.classList.contains("ss-range")) cur[e.target.dataset.k] = Number(e.target.value);
    if (e.target.id === "ss-split") {
      split = e.target.checked;
      if (!split) cur.scaleEn = Math.round(cur.scale * 0.86 * 100) / 100;
    }
    if (e.target.id !== "ss-ask-input") draw();
  });
  box.addEventListener("change", (e) => {
    if (e.target.id === "ss-font") { cur.font = e.target.value; draw(); }
    if (e.target.id !== "ss-preset") return;
    const p = subPresets().find((x) => x.name === e.target.value);
    Object.assign(cur, p ? { ...SUB_LOOK_DEFAULT, ...p.look } : look);
    drawPresets(e.target.value);
    draw();
  });
  box.addEventListener("keydown", (e) => {
    if (e.target.id === "ss-ask-input" && e.key === "Enter") finishAsk();
    if (e.target.id === "ss-ask-input" && e.key === "Escape") { e.stopPropagation(); closeAsk(); }
  });
  box.addEventListener("click", async (e) => {
    const b = e.target.closest("button");
    if (!b || b.closest(".select-wrap")) return;
    const act = b.dataset.ss;
    const presetName = $("ss-preset").value;
    if (act === "close") return closeDialogWindow();
    if (act === "save") {
      ask = "save";
      $("ss-ask").hidden = false;
      $("ss-ask-input").value = presetName;
      return $("ss-ask-input").focus();
    }
    if (act === "ask-ok") return finishAsk();
    if (act === "ask-cancel") return closeAsk();
    if (act === "default" && presetName) {
      localStorage.setItem(SUB_DEFAULT_KEY, presetName);
      drawPresets(presetName);
      return toast(`已设为默认：${presetName}`);
    }
    if (act === "delete" && presetName) {
      saveSubPresets(subPresets().filter((p) => p.name !== presetName));
      if (localStorage.getItem(SUB_DEFAULT_KEY) === presetName) localStorage.removeItem(SUB_DEFAULT_KEY);
      drawPresets("");
      return toast(`已删除预设：${presetName}`);
    }
    if (act === "export") {
      const data = { type: SUB_FILE_TAG, version: 1, current: cur, presets: subPresets(), default: localStorage.getItem(SUB_DEFAULT_KEY) || "" };
      const ok = await invoke("export_sub_styles", { text: JSON.stringify(data, null, 2) }).catch((err) => { toast(String(err)); return false; });
      if (ok) toast("预设文件已导出");
      return;
    }
    if (act === "import") {
      const text = await invoke("import_sub_styles").catch((err) => { toast(String(err)); return null; });
      if (text) importText(text);
      return;
    }
    if (b.dataset.size) {
      split = false;
      cur.scale = Number(b.dataset.size);
      cur.scaleEn = Math.round(cur.scale * 0.86 * 100) / 100;
    }
    if (b.dataset.c === "off") cur.colorize = false;
    if (b.dataset.c === "zh" || b.dataset.c === "en") {
      const key = b.dataset.c === "zh" ? "colorZh" : "colorEn";
      const wasOn = cur.colorize;
      cur.colorize = true;
      draw();
      // 已经在着色时再点才改颜色，第一次点只是打开着色
      if (wasOn) openPicker(b, cur[key], (hex) => { cur[key] = hex; draw(); });
      return;
    }
    if (b.dataset.fx) {
      cur.boxed = b.dataset.fx === "box";
      cur.shadow = b.dataset.fx === "shadow";
    }
    if (act === "ok") {
      b.disabled = true;
      let ok = 0;
      const errs = [];
      for (const id of ids) {
        try {
          await invoke(isNas(id) ? "nas_subtitle_restyle" : "subtitle_restyle", { id, look: cur });
          ok++;
        } catch (err) {
          errs.push(String(err));
        }
      }
      try { localStorage.setItem(SUB_LOOK_KEY, JSON.stringify(cur)); } catch (_) {}
      b.disabled = false;
      if (!ok) return toast(errs[0] || "没改成");
      return submitDialog({ substyle: { ok, fail: errs.length, err: errs[0] || "" } });
    }
    draw();
  });
  new ResizeObserver(() => draw()).observe($("ss-preview"));
  drawPresets("");
  draw();
  if (readCoverRatio) {
    const image = new Image();
    image.onload = () => {
      if (!box.isConnected || !image.naturalHeight) return;
      ratio = image.naturalWidth / image.naturalHeight;
      draw();
    };
    image.src = cover;
  }
  invoke("system_fonts").then(fonts => {
    if (!box.isConnected || !Array.isArray(fonts) || !fonts.length) return;
    const choices = [...new Set([...(fonts.includes(cur.font) ? [] : [cur.font]), ...fonts])];
    fontSelect.replaceChildren(...choices.map(font => new Option(font, font)));
    draw();
    const warm = () => fontSelect.parentElement?.dispatchEvent(new CustomEvent("warm"));
    if (window.requestIdleCallback) window.requestIdleCallback(warm, { timeout: 1500 });
    else setTimeout(warm, 300);
  }).catch(() => {});
}

document.addEventListener("click", (event) => {
  const control = event.target.closest?.("[data-subtitle-action]");
  if (!control) return;
  event.stopPropagation();
  const task = findSubtitleTask(control.dataset.taskId);
  if (!task) return;
  const action = control.dataset.subtitleAction;
  if (action === "style") {
    if (!canSubStyle(task)) return;
    if (task.collectionFiles || control.closest("#files-list")) return openSubStyle(task.id);
    return openSubStyle(multiFor(task.id).map(candidate => candidate.id));
  }
  if (action === "translate") return translateTaskSubtitles(task);
  if (action === "fill") {
    return fetchSubtitlesBatch(missingSubtitleTasks(groupMembers(task.id)).filter(candidate => candidate.subs !== "英文字幕"), "fill");
  }
  return fetchTaskSubtitles(task.id);
}, true);

// ---------- 新建下载里的字幕样式：简易 / 预设 ----------
// 有默认预设（字幕样式窗口里「设为默认」）时，新建下载默认选「预设」并选中它；没有就是「简易」。
// 选预设时把预设的各项写进这次的视频选项；切回简易时恢复简易那几项原来的值
const SUB_FIELDS = ["subScale", "subScaleEn", "subColorize", "subColorZh", "subColorEn", "subShadow", "subShadowSize", "subShadowAlpha", "subBox", "subBoxAlpha", "subOutline", "subMargin", "subFont"];
function lookToVideo(d) {
  return { subScale: d.scale, subScaleEn: d.scaleEn, subColorize: d.colorize, subColorZh: d.colorZh, subColorEn: d.colorEn, subShadow: d.shadow, subShadowSize: d.shadowSize, subShadowAlpha: d.shadowAlpha, subBox: d.boxed, subBoxAlpha: d.boxAlpha, subOutline: d.outline, subMargin: d.margin, subFont: d.font };
}
function pickSubFields(v) {
  return Object.fromEntries(SUB_FIELDS.filter((k) => k in (v || {})).map((k) => [k, v[k]]));
}
function syncSubMode(ass) {
  const row = $("cf-submode-row");
  if (!row) return;
  row.hidden = !ass;
  $("cf-sub-hint").hidden = !ass;
  $("cf-subpreset-wrap").hidden = true;
  $("cf-subpreset-empty").hidden = true;
  if (!ass) $("cf-subpreset")._customSelect?.close();
  if (!ass || !cf.video) { syncCfSubtitleDisclosures(); return; }
  // 第一次显示这组时定下模式：有默认预设就用它
  if (cf.subMode == null) {
    const def = localStorage.getItem(SUB_DEFAULT_KEY);
    const has = def && subPresets().some((p) => p.name === def);
    cf.simpleSub = pickSubFields(cf.video);
    cf.subMode = has ? "preset" : "simple";
    cf.subPreset = has ? def : "";
    if (has) applySubPreset(def);
  }
  const preset = cf.subMode === "preset";
  const list = subPresets();
  row.hidden = !ass;
  $("cf-sub-hint").hidden = !ass;
  document.querySelectorAll("#cf-submode button").forEach((b) => b.classList.toggle("on", b.dataset.mode === cf.subMode));
  ["cf-subcolor-row", "cf-subsize-row", "cf-subfx-row"].forEach((id) => { if (ass) $(id).hidden = preset; });
  $("cf-subpreset-wrap").hidden = !ass || !preset || !list.length;
  $("cf-subpreset-empty").hidden = !ass || !preset || !!list.length;
  if (preset && list.length) {
    const def = localStorage.getItem(SUB_DEFAULT_KEY);
    $("cf-subpreset").innerHTML = list.map((p) => `<option value="${escapeHtml(p.name)}">${escapeHtml(p.name)}${p.name === def ? "（默认）" : ""}</option>`).join("");
    if (!list.some((p) => p.name === cf.subPreset)) {
      cf.subPreset = list[0].name;
      applySubPreset(cf.subPreset);
    }
    $("cf-subpreset").value = cf.subPreset;
    initCustomSelect($("cf-subpreset"));
    syncCustomSelects();
  }
  syncCfSubtitleDisclosures();
}
function applySubPreset(name) {
  const p = subPresets().find((x) => x.name === name);
  if (p && cf.video) Object.assign(cf.video, lookToVideo({ ...SUB_LOOK_DEFAULT, ...p.look }));
}
document.querySelectorAll("#cf-submode button").forEach((b) => {
  b.addEventListener("click", () => {
    if (!cf.video || cf.subMode === b.dataset.mode) return;
    if (b.dataset.mode === "preset") {
      cf.simpleSub = pickSubFields(cf.video);
      cf.subMode = "preset";
      if (cf.subPreset) applySubPreset(cf.subPreset);
    } else {
      cf.subMode = "simple";
      Object.assign(cf.video, cf.simpleSub || {});
      renderCfVideo();
    }
    syncSubMode(true);
  });
});
$("cf-subpreset")?.addEventListener("change", (e) => {
  cf.subPreset = e.target.value;
  applySubPreset(cf.subPreset);
});

// 字幕效果：阴影、黑底各自开关（默认有阴影、没黑底）
function syncSubFx() {
  const v = cf.video || {};
  document.querySelectorAll("#cf-subfx button").forEach((b) => {
    b.classList.toggle("on", b.dataset.fx === "shadow" ? v.subShadow !== false : !!v.subBox);
  });
}
document.querySelectorAll("#cf-subfx button").forEach((b) => {
  b.addEventListener("click", () => {
    if (!cf.video) return;
    if (b.dataset.fx === "shadow") cf.video.subShadow = cf.video.subShadow === false;
    else cf.video.subBox = !cf.video.subBox;
    syncSubFx();
  });
});
// 字幕着色：点「不着色」关掉；点「中文」「英文」「全部」开启着色，
// 并用同一个取色面板改颜色（「全部」两行一起改）
$("cf-subcolor")?.addEventListener("click", (e) => {
  if (!cf.video) return;
  const off = e.target.closest('[data-color="off"]');
  if (off) {
    cf.video.subColorize = false;
    closePicker();
    renderCfVideo();
    return;
  }
  const langBtn = e.target.closest("[data-lang]");
  if (!langBtn) return;
  const lang = langBtn.dataset.lang;
  // 第一次从「不着色」切过来只是把着色打开，不弹色板——
  // 每次点一下就从旁边冒出个取色面板太打断。已经在着色状态里再点才调色
  const wasOn = !!cf.video.subColorize;
  cf.video.subColorize = true;
  if (!wasOn) {
    closePicker();
    renderCfVideo();
    return;
  }
  const cur =
    lang === "en"
      ? cf.video.subColorEn || "#516EFE"
      : cf.video.subColorZh || "#00FFAC";
  const dot = lang === "en" ? $("sub-dot-en") : lang === "zh" ? $("sub-dot-zh") : $("sub-dot-all");
  openPicker(dot || langBtn, cur, (hex) => {
    if (lang === "all") {
      cf.video.subColorZh = hex;
      cf.video.subColorEn = hex;
    } else if (lang === "en") {
      cf.video.subColorEn = hex;
    } else {
      cf.video.subColorZh = hex;
    }
    renderCfVideo();
  });
  renderCfVideo();
});

$("cf-audio").addEventListener("change", () => {
  if (cf.video) cf.video.audioOnly = $("cf-audio").checked;
  renderCfVideo();
});

function showBaiduProgress(text, loading, qr) {
  $("cf-baidu-setup").hidden = false;
  $("cf-baidu-status-row").hidden = !text;
  setProbeState($("cf-baidu-state"), text, loading);
  const image = $("cf-baidu-qr");
  image.hidden = !qr;
  if (qr) image.src = qr;
  else image.removeAttribute("src");
  fitConfirmDialog();
}

function baiduDraftActive(draft) {
  return cf === draft && !draft.baiduCancelled && !$("confirm").hidden;
}

function canPickBaiduFiles(draft) {
  return !!draft?.extra?.baiduShare && !draft.extra.baiduRetryId &&
    !!draft.baiduFiles?.length && draft.baiduFiles.every((file) => file.directory || /^[1-9]\d*$/.test(file.id || ""));
}

function rememberBaiduPicks(draft) {
  const files = draft.baiduFiles.filter(file => !file.directory);
  const on = files.filter(file => file.on);
  if (on.length === files.length) {
    draft.baiduCheckedIds = draft.baiduPicks = null;
    return;
  }
  draft.baiduCheckedIds = new Set(on.map(file => file.id));
  const picks = [];
  function add(node) {
    const item = node.children ? node.pick : node.entries[0];
    if (item?.id && node.entries.length && node.entries.every(entry => entry.on)) {
      picks.push({ id: item.id, name: node.name });
    } else {
      node.children?.forEach(add);
    }
  }
  cfFileTree(draft.baiduFiles).forEach(add);
  draft.baiduPicks = picks;
}

function renderBaiduPreviewFallback(draft, text = "", loading = false) {
  const selected = draft.baiduPicks ?? draft.extra.baiduSelection ?? [];
  const box = $("cf-files");
  box.hidden = false;
  const title = selected.length || draft.baiduPicks ? `已选 ${selected.length} 项` : "下载整个分享";
  box.innerHTML = `<div class="cf-files-head"><span class="sum">${title}</span>${text ? `<span class="probe-state">${loading ? '<span class="spin" aria-hidden="true"></span>' : ""}${escapeHtml(text)}</span>` : ""}</div>`
    + (selected.length ? `<div class="cf-files-list">${selected.map(file =>
      `<div class="cf-file"><span class="n" title="${escapeHtml(file.name)}">${escapeHtml(file.name)}</span></div>`).join("")}</div>` : "");
  renderBaiduDownloadButton(draft);
}

async function probeBaiduFileList(draft) {
  if (!invoke || !baiduDraftActive(draft)) return;
  const code = $("cf-baidu-code").value.trim();
  const active = () => baiduDraftActive(draft) && code === $("cf-baidu-code").value.trim();
  if ((code || draft.extra.baiduCodeRequired) && !/^[a-z0-9]{4}$/i.test(code)) {
    renderBaiduPreviewFallback(draft, "填好提取码后可查看文件");
    return;
  }
  renderBaiduPreviewFallback(draft, "正在读取文件列表…", true);
  fitConfirmDialog();
  try {
    const files = await invoke("preview_baidu_share", {
      link: baiduShareWithCode(draft.url, code),
      selection: draft.extra.baiduSelection || null,
    });
    if (!active()) return;
    draft.baiduFiles = files.map((file, index) => ({
      ...file, url: `baidu-preview:${index}`, name: file.path, files: [],
      on: !file.directory && (!draft.baiduCheckedIds || draft.baiduCheckedIds.has(file.id)),
    }));
    const roots = files.filter(file => !/[\\/]/.test(file.path));
    if (roots.length === 1 && roots[0].directory) {
      $("cf-name-label").textContent = "文件夹";
      setCfDefaultName(draft.name || roots[0].path);
    }
    renderCfFiles();
  } catch (error) {
    if (active()) renderBaiduPreviewFallback(draft, String(error));
  } finally {
    if (active()) fitConfirmDialog();
  }
}

function renderBaiduDownloadButton(draft) {
  const button = $("cf-ok");
  const busy = !!draft.baiduAuth || !!draft.baiduPreparing || !!draft.baiduSubmitting;
  button.hidden = !!draft.baiduAuth;
  button.disabled = busy || draft.baiduPicks?.length === 0;
  $("cf-files").querySelectorAll("input[type=checkbox]").forEach(input => { input.disabled = busy; });
  button.textContent = draft.extra.baiduRetryId ? "继续下载" : "开始下载";
}

async function submitBaiduDraft(draft) {
  if (draft.baiduSubmitting || draft.baiduPicks?.length === 0) return;
  const code = $("cf-baidu-code").value.trim();
  if (code && !/^[a-z0-9]{4}$/i.test(code)) {
    $("cf-baidu-code").focus();
    throw new Error("提取码是 4 位字母或数字");
  }
  const request = {
    link: baiduShareWithCode(draft.url, code),
    dir: $("cf-dir").value.trim() || draft.dir,
    force: !!draft.extra.force,
    cleanupRemote: $("cf-baidu-cleanup").getAttribute("aria-checked") === "true",
    retryId: draft.extra.baiduRetryId || null,
    selection: draft.baiduPicks ?? draft.extra.baiduSelection ?? null,
    name: $("cf-name").dataset.userEdited ? $("cf-name").value.trim() : null,
    suggestedName: draft.name || null,
  };
  draft.baiduSubmitting = true;
  $("cf-baidu-method-row").hidden = true;
  renderBaiduDownloadButton(draft);
  try {
    showBaiduProgress("正在开始下载…", true);
    let result = await invoke("add_baidu_share", request);
    while (baiduDraftActive(draft) && result.status !== "started") {
      showBaiduProgress("", false);
      const completed = result.status === "alreadyDownloaded";
      const choice = await chooseDuplicate(
        { id: result.id, name: result.name, status: completed ? "done" : "running" },
        { allowCopy: completed, copyHint: "再下一份时，同名文件会自动加 (1)、(2)。" },
      );
      if (!baiduDraftActive(draft) || !choice) return;
      if (choice === "show") {
        await viewDuplicateTask(result.id);
        if (DIALOG) await submitDialog({ baiduHandled: true });
        else { closeConfirm(false); nextPending(); }
        return;
      }
      showBaiduProgress("正在开始下载…", true);
      result = await invoke("add_baidu_share", { ...request, force: true, allowDuplicate: true });
    }
    if (!baiduDraftActive(draft)) return;
    if (DIALOG) await submitDialog({ baiduId: result.id });
    else {
      closeConfirm(false);
      showBaiduTask(result.id);
    }
  } finally {
    draft.baiduSubmitting = false;
    if (draft.baiduAuth?.done) draft.baiduAuth = null;
    if (baiduDraftActive(draft)) renderBaiduDownloadButton(draft);
  }
}

async function beginBaiduDownload(draft) {
  if (draft.baiduPreparing || draft.baiduPicks?.length === 0) return;
  const code = $("cf-baidu-code").value.trim();
  if ((code || draft.extra.baiduCodeRequired) && !/^[a-z0-9]{4}$/i.test(code)) {
    $("cf-baidu-code").focus();
    toast("提取码是 4 位字母或数字");
    return;
  }
  draft.baiduPreparing = true;
  renderBaiduDownloadButton(draft);
  showBaiduProgress("正在检查百度网盘授权…", true);
  try {
    const authorized = await invoke("prepare_baidu_share");
    if (!baiduDraftActive(draft)) return;
    if (authorized) return await submitBaiduDraft(draft);
    draft.baiduAuth = { pending: null, switching: false, done: false };
    $("cf-baidu-method-row").hidden = false;
    for (const button of $("cf-baidu-methods").querySelectorAll("button")) {
      button.classList.remove("on");
      button.setAttribute("aria-pressed", "false");
    }
    renderBaiduDownloadButton(draft);
    await chooseBaiduAuthorization("web");
  } catch (error) {
    if (baiduDraftActive(draft)) showBaiduProgress(error.message || String(error), false);
  } finally {
    draft.baiduPreparing = false;
    if (baiduDraftActive(draft)) renderBaiduDownloadButton(draft);
  }
}

async function chooseBaiduAuthorization(method) {
  const draft = cf;
  const auth = draft?.baiduAuth;
  if (!auth || auth.switching || auth.done) return;
  if (auth.pending) {
    if (auth.method === method) return;
    auth.switching = true;
    try {
      await invoke("cancel_baidu_prepare");
      await auth.pending?.catch(() => {});
    } catch (error) {
      if (baiduDraftActive(draft)) toast(String(error));
      return;
    } finally {
      auth.switching = false;
    }
    if (!baiduDraftActive(draft) || auth.done) return;
  }
  auth.method = method;
  for (const button of $("cf-baidu-methods").querySelectorAll("button")) {
    const selected = button.value === method;
    button.classList.toggle("on", selected);
    button.setAttribute("aria-pressed", String(selected));
  }
  const waiting = method === "web" ? "正在打开百度授权网页…" : "正在获取授权二维码…";
  showBaiduProgress(waiting, true);
  const pending = invoke("authorize_baidu_share", { method });
  auth.pending = pending;
  renderBaiduDownloadButton(draft);
  try {
    await pending;
    if (!baiduDraftActive(draft)) return;
    auth.done = true;
    await submitBaiduDraft(draft);
  } catch (error) {
    if (!baiduDraftActive(draft) || auth.switching) return;
    showBaiduProgress(error.message || String(error), false);
    if (auth.done) {
      draft.baiduAuth = null;
      renderBaiduDownloadButton(draft);
    }
  } finally {
    if (auth.pending === pending) {
      auth.pending = null;
      if (baiduDraftActive(draft)) renderBaiduDownloadButton(draft);
    }
  }
}

$("cf-baidu-cleanup").addEventListener("click", event => {
  setSwitch("cf-baidu-cleanup", event.currentTarget.getAttribute("aria-checked") !== "true");
});
$("cf-baidu-code").addEventListener("keydown", event => {
  if (event.key === "Enter" && !event.isComposing) {
    event.preventDefault();
    $("cf-ok").click();
  }
});
$("cf-baidu-code").addEventListener("input", () => {
  const draft = cf;
  if (!draft.extra?.baiduShare) return;
  clearTimeout(draft.baiduListTimer);
  draft.baiduFiles = null;
  cfClosedFolders.clear();
  $("cf-name-label").textContent = "名称";
  renderBaiduPreviewFallback(draft);
  draft.baiduListTimer = setTimeout(() => probeBaiduFileList(draft), 300);
});
$("cf-baidu-methods").addEventListener("click", event => {
  const button = event.target.closest("button");
  if (button) chooseBaiduAuthorization(button.value);
});
window.__TAURI__?.event?.listen("baidu-prepare-progress", ({ payload }) => {
  if (cf?.extra?.baiduShare && baiduDraftActive(cf) && !cf.baiduAuth?.switching) {
    showBaiduProgress(payload.text, !payload.qr, payload.qr);
  }
});

$("cf-ok").addEventListener("click", async () => {
  const startButton = $("cf-ok");
  if (startButton.disabled) return;
  if (cf?.baiduAuth) return;
  const label = startButton.textContent;
  startButton.disabled = true;
  startButton.textContent = "正在准备…";
  try {
  if (cf.extra?.baiduShare) {
    await beginBaiduDownload(cf);
    return;
  }
  if (cf.extra?.feishu) {
    const draft = cf;
    const selected = (cfFiles || []).flatMap((file, index) => file.on ? [index] : []);
    if (!selected.length || !draft.extra.feishuTicket) return;
    let id;
    try {
      id = await invoke("add_feishu_document", {
        ticket: draft.extra.feishuTicket,
        selected,
        name: $("cf-name").value.trim() || draft.extra.folderName,
        dir: $("cf-dir").value.trim() || draft.dir,
      });
    } catch (error) {
      showFeishuIssue(draft, error);
      return;
    }
    if (DIALOG) await submitDialog({ feishuId: id });
    else {
      closeConfirm(false);
      switchView("all");
      await refreshTasks();
      if (id) revealTask(id);
      nextPending();
    }
    return;
  }
  if (cf.extra?.wechatTicket) {
    const button = $("cf-ok");
    button.disabled = true;
    const remember = $("cf-remember").checked;
    try {
      const choice = cf.extra.wechatDuplicateChecked ? { force:cf.extra.wechatForce } : await askDuplicate(cf.url, $("cf-dir").value.trim());
      if (!choice) return;
      const id = await invoke("wechat_confirm", { ticket: cf.extra.wechatTicket, name: $("cf-name").value.trim(), dir: $("cf-dir").value.trim(), remember, force: !!choice.force });
      await submitDialog({ wechatId: id, remember });
    } catch (e) { toast(String(e)); }
    finally { button.disabled = false; }
    return;
  }
  const { url, video, headers } = cf;
  // 确认框已查到的真名带给 doAdd，投完立刻查重名（磁力的 dn 常常和真名不一样）
  const known = cfFiles.find((g) => g.url === url && g.state === "ok" && btLink(g.url))?.name;
  const extra = known ? { ...cf.extra, btName: known } : cf.extra;
  // 框里看得到才算用户选过；看不到（没字幕、没 Ollama）就跟设置走
  if (video) video.polish = $("cf-polish-wrap").hidden ? null : cf.polish;
  // 以输入框里最后的值为准（用户可能手改过）
  const dir = ($("cf-dir").value || "").trim();
  const name = $("cf-name").value.trim();
  if ((extra?.collection || isHuggingFaceFilePage(extra?.sourceUrl)) && !cfFiles?.some((entry) => entry.on)) return;
  const remember = $("cf-remember").checked;
  // 关框前记下每条要下哪些文件
  const selections = new Map((extra?.batch ? extra.batch.map((x) => x.url) : [url]).map((u) => [u, cfSelection(u)]));
  if (DIALOG) {
    await submitDialog({ url, name, dir: extra?.collection ? dir || cf.dir : dir, video, headers, extra, remember, selections: [...selections] });
    return;
  }
  await finishConfirm({ url, name, dir: extra?.collection ? dir || cf.dir : dir, video, headers, extra, remember, selections });

  } catch (error) { toast(String(error)); }
  finally {
    if (cf?.extra?.baiduShare) renderBaiduDownloadButton(cf);
    else { startButton.disabled = false; startButton.textContent = label; }
  }
});

/// 确认框点了下载：独立窗口填完交回主窗口，从这里接着投
async function finishConfirm({ url, name, dir, video, headers, extra, remember, selections }) {
  closeConfirm(false); // 下一条等 doAdd 做完再接，见 closeConfirm 的注释
  if (remember && state.settings) {
    state.settings.settings.confirmNew = false;
    setSwitch("sw-confirm", false);
    await saveSettings();
    toast("以后不再询问。想恢复可到「设置 › 新建前确认」打开");
  }
  if (extra?.collection) await submitCollection(extra.batch.filter((x) => selections.has(x.url) && selections.get(x.url)?.length !== 0), dir, name || extra.folderName, url, video, extra);
  else if (extra?.batch) await runBatch(extra.batch.filter((x) => selections.get(x.url)?.length !== 0), dir, video, selections, name, extra);
  else if (selections.get(url)?.length === 0) toast("没有选中要下载的文件");
  else await doAdd(url, name, dir, video, headers, { ...extra, select: selections.get(url) });
  nextPending(); // 这条投完，接排队的下一条
}

async function showBaiduTask(id) {
  switchView("all");
  await refreshTasks();
  revealTask(id);
  nextPending();
}

async function submitCollection(items, parent, name, url, video, extra = {}) {
  if (!items.length) return;
  const separator = parent.startsWith("/") ? "/" : "\\";
  const folder = collectionFolderName(name);
  const dir = parent.replace(/[\\/]+$/, "") + separator + folder;
  const original = taskGroups.find(group => group.id === extra.redownloadGroup);
  const replacing = original && sameDownloadDirectory(original.dir, dir);
  const mode = replacing ? await chooseRedownloadMode(original.name) : "copy";
  if (!mode) return;
  const matchesCollection = item => !original && item.collection?.url === url && !item.collection.closed && item.dir === dir;
  const queued = new Set((taskGroups.find(matchesCollection)?.collection.items || []).map(item => item.url));
  if (video && items.some(item => !queued.has(item.url))) {
    await invoke("remember_video_options", { video });
  }
  let group = mode === "replace" ? original : taskGroups.find(matchesCollection);
  if (!group) {
    group = { id: `grp:${crypto.randomUUID()}`, name: folder, ids: [], dir, at: Date.now(), collection: { url, parent, items: [] } };
    taskGroups.push(group);
  }
  const known = new Map(group.collection.items.map(item => [item.url, item]));
  const added = [];
  group.paused = false;
  for (const item of items) {
    const existing = known.get(item.url);
    if (existing && mode !== "replace") continue;
    const episode = existing || { url: item.url, name: item.name };
    episode.video = video ? { ...video } : null;
    if (mode === "replace") {
      episode.replaceId = episode.id;
      episode.id = null;
      delete episode.error;
    }
    if (original) {
      episode.force = true;
      episode.confirmCollision = !replacing || mode === "replace";
    }
    if (!existing) group.collection.items.push(episode);
    known.set(item.url, episode);
    added.push(episode);
  }
  saveGroups();
  switchView("all");
  toast(added.length ? `已加入合集下载 · ${added.length} 个视频` : "这些视频已在合集任务中");
  await refreshTasks();
}

// 按住标题拖动面板。每次打开回到中间
function makeDraggable(maskId) {
  if (DIALOG) return; // 独立窗口拖的是窗口本身
  const mask = $(maskId), modal = mask?.querySelector(".modal"), handle = modal?.querySelector("h3");
  if (!handle) return;
  handle.style.cursor = "move";
  handle.style.userSelect = "none";
  let dx = 0, dy = 0;
  handle.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    const x0 = e.clientX - dx, y0 = e.clientY - dy, box = modal.getBoundingClientRect();
    const left0 = box.left - dx, top0 = box.top - dy;
    handle.setPointerCapture(e.pointerId);
    const move = (ev) => {
      // 标题不能拖出窗口，否则就抓不回来了
      dx = Math.min(innerWidth - left0 - 80, Math.max(80 - left0 - box.width, ev.clientX - x0));
      dy = Math.min(innerHeight - top0 - 40, Math.max(-top0, ev.clientY - y0));
      modal.style.transform = `translate(${dx}px, ${dy}px)`;
    };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
  });
  new MutationObserver(() => {
    if (mask.hidden) {
      dx = dy = 0;
      modal.style.transform = "";
    }
  }).observe(mask, { attributes: true, attributeFilter: ["hidden"] });
}
makeDraggable("confirm");
makeDraggable("new-sheet");
// 右上角的关闭按钮
for (const [id, close] of [["confirm", () => closeConfirm()], ["new-sheet", () => closeNewSheet()]]) {
  const modal = $(id)?.querySelector(".modal");
  if (!modal) continue;
  const x = document.createElement("button");
  x.className = "modal-x";
  x.type = "button";
  x.setAttribute("aria-label", "关闭");
  x.innerHTML = '<svg class="ic"><use href="#i-x"/></svg>';
  x.addEventListener("click", close);
  modal.append(x);
}
// 点链接就复制（批量时把所有链接一行一个一起复制）
const copyCfUrl = async () => {
  const text = cf?.extra?.sourceUrl || (cf?.extra?.batch && !cf.extra.collection ? cf.extra.batch.map((x) => x.url).join("\n") : cf?.url);
  if (text) toast((await copyText(text)) ? "已复制链接" : "复制失败");
};
$("cf-url")?.addEventListener("click", copyCfUrl);
$("cf-url")?.addEventListener("keydown", (e) => {
  if (e.key === "Enter" || e.key === " ") { e.preventDefault(); copyCfUrl(); }
});

// ---------- 新建下载弹窗（#new-sheet） ----------

/// 点「新建下载」开居中弹窗贴链接（老做法是把焦点甩到底部粘贴框，从顶上跳到很底下，很怪）。
/// 开了就顺手看一眼剪贴板：里头有链接就填进去，省一步粘贴
function openNewSheet(prefilled) {
  const initialText = typeof prefilled === "string" ? prefilled : "";
  if (!DIALOG && invoke) {
    openDialog("new", { text: initialText });
    return;
  }
  const sheet = $("new-sheet");
  if (!sheet) return;
  sheet.hidden = false;
  const text = $("nd-text");
  if (text) {
    text.value = initialText;
    text.focus();
  }
  renderNdList();
  if (initialText) return;
  Promise.resolve(invoke?.("clipboard_read_text"))
    .then((s) => {
      if (!s || !/magnet:\?|ed2k:\/\/|https?:\/\//i.test(s)) return;
      const el = $("nd-text");
      if (!el) return;
      el.value = s;
      renderNdList();
    })
    .catch(() => {}); // 读不到剪贴板就算了，用户自己粘
}

/// 关弹窗。drain=false 是点「下载」那条路：刚排好的队列要留着逐条投；
/// 取消/点遮罩才把没投的一起清掉
function closeNewSheet(drain = true) {
  if (DIALOG) return closeDialogWindow();
  const sheet = $("new-sheet");
  if (!sheet) return;
  sheet.hidden = true;
  if (drain) pendingLinks = [];
}

/// 弹窗文本拆链接：按空白/换行切，认 magnet: / ed2k:// / http(s):// 开头的（不严格校验 URL）
function parseNdLinks() {
  const v = $("nd-text")?.value || "";
  return linksWithBaiduCode(v);
}

/// 后端不可用时的协议标签，正常预览与确认框共用 target_info 的来源识别。
function ndKind(url) {
  if (/^magnet:\?/i.test(url)) return "磁力";
  if (/^ed2k:\/\//i.test(url)) return "ed2k";
  if (/\.torrent$/i.test(url)) return "种子文件";
  return "直链";
}

/// 实时预览解析结果：最多 5 行，多的收成「等 N 个链接」；「下载」按钮跟着链接数走（0 条禁用）
let ndListGeneration = 0;
async function renderNdList() {
  const links = parseNdLinks();
  const box = $("nd-list");
  const generation = ++ndListGeneration;
  const shown = links.slice(0, 5);
  const draw = (labels) => {
    if (!box) return;
    let html = shown
      .map((u, i) => `<div class="nd-row"><span class="nd-kind">${escapeHtml(labels[i])}</span><span class="nd-name">${escapeHtml(u)}</span></div>`)
      .join("");
    if (links.length > 5) html += `<div class="nd-more">等 ${links.length - 5} 个链接</div>`;
    box.innerHTML = html;
  };
  draw(shown.map(() => "识别中…"));
  const ok = $("nd-ok");
  if (ok) ok.disabled = links.length === 0;
  if (!box || !shown.length) return;
  const infos = await Promise.all(shown.map((link) => invoke("target_info", { link }).catch(() => null)));
  if (generation !== ndListGeneration) return;
  draw(shown.map((link, i) => infos[i]?.kind || ndKind(link)));
}

$("nd-text")?.addEventListener("input", renderNdList);
$("nd-cancel")?.addEventListener("click", () => closeNewSheet());
// 点遮罩空白处关掉，和确认框一个手感
$("new-sheet")?.addEventListener("click", (e) => {
  void e; // 点外面不关，要关用取消或 Esc
});
$("nd-ok")?.addEventListener("click", () => {
  const links = parseNdLinks();
  if (!links.length) return;
  if (DIALOG) {
    submitDialog({ links });
    return;
  }
  closeNewSheet(false); // 队列留着，别被清掉
  // 排队后由 nextPending 合并：保存位置和选项相同的只弹一个框
  pendingLinks = [...new Set(links)].map((url) => ({ url, opts: {} }));
  nextPending();
});

async function pauseAll() {
  const running = state.tasks.filter((t) => t.status === "running" || t.status === "wait");
  if (!running.length) return toast("没有正在下载的任务");
  await bulkAction("pause", running);
}

async function startAll() {
  const stopped = state.tasks.filter((t) => t.status === "pause" || t.status === "error");
  if (!stopped.length) return toast("没有可以开始的任务");
  await bulkAction("resume", stopped);
}

/// 能不能边下边播：只对走内核的任务（磁力／种子／直链）出手，
/// 视频网站的任务是 yt-dlp 在写文件，没有内核那套分块，播不了。
const PLAY_EXT = /\.(mp4|mkv|avi|mov|flv|wmv|webm|m4v|ts|rmvb|mpg|mpeg)$/i;
function canPlay(t) {
  if (t?.collectionFiles) return false;
  if (!t || isGroup(t.id)) return false;
  // 视频网站下的：下完了就是本地视频，用系统播放器放；没下完不支持边下边播
  // NAS 上视频网站下的也一样：名字是标题、不带扩展名，不能按名字判断
  if (/^(v:|nas:v-)/.test(String(t.id))) return t.status === "done" && !t.missing;
  if (/^(?:nas:)?(?:xl:|cloud:)/.test(String(t.id)) && t.status !== "done") return false; // 文件在迅雷手里，内核播不了
  const name = t.name || t.meta?.res?.name || "";
  const files = t.meta?.res?.files || [];
  const isVideo = PLAY_EXT.test(name) || files.some((f) => PLAY_EXT.test(f.name || ""));
  // 下完了就是本地文件，用系统播放器放（音乐种子也一样）
  if (t.status === "done") return !t.missing && (isVideo || t.kind === "磁力" || t.kind === "种子文件");
  if (t.status !== "running" && t.status !== "pause") return false;
  // 边下边播只有 BT 支持。磁力刚投、还不知道里面有什么时先给按钮
  if (t.kind !== "磁力" && t.kind !== "种子文件") return false;
  return isVideo || !files.length;
}

async function playTask(id) {
  try {
    const t = state.tasks.find((x) => x.id === id);
    if (t?.status === "done") {
      const stop = toast("正在打开播放器…");
      try { await invoke("item_open", { id }); } finally { stop(); }
      return;
    }
    playerStatus("正在连接…");
    $("player").hidden = false;
    const url = await invoke("play_url", { id });
    openPlayer(url);
  } catch (e) {
    $("player").hidden = true;
    toast(String(e));
  }
}

function playerStatus(text) {
  const el = $("player-status");
  if (el) el.textContent = text;
}

// 标题栏说清楚现在是什么状态，黑屏时才知道是在缓冲还是坏了
{
  const v = $("player-video");
  v.addEventListener("waiting", () => playerStatus("正在缓冲…"));
  v.addEventListener("loadstart", () => { if (v.getAttribute("src")) playerStatus("正在缓冲…"); });
  v.addEventListener("playing", () => playerStatus("正在播放"));
  v.addEventListener("pause", () => { if (!v.ended && v.getAttribute("src")) playerStatus("已暂停"); });
  v.addEventListener("error", () => {
    if (!v.getAttribute("src")) return; // 关闭播放器时清地址也会触发
    closePlayer();
    toast("内置播放器放不了这个格式（常见于 MKV、HEVC）。下载完成后点「播放」用系统播放器打开");
  });
}

function openPlayer(url) {
  const box = $("player");
  const v = $("player-video");
  v.src = url;
  box.hidden = false;
  v.play().catch(() => {});   // 自动播放被拦就当用户会自己按播放
}

function closePlayer() {
  const box = $("player");
  const v = $("player-video");
  v.pause();
  v.removeAttribute("src");
  v.load();                    // 断开连接，别让内核一直挂着一个播放会话
  box.hidden = true;
}

function visibleTaskIds() {
  const preparing = new Set(state.tasks.filter(task => task.creating).map(task => task.id));
  return [...document.querySelectorAll("#task-list .task[data-id]:not([hidden])")]
    .map(row => row.dataset.id).filter(id => id && !preparing.has(id));
}

function syncVisibleSelection() {
  const available = new Set(visibleTaskIds());
  state.selIds = new Set([...state.selIds].filter(id => available.has(id)));
  const first = state.selIds.values().next().value ?? null;
  if (!available.has(state.selId)) state.selId = first;
  if (!available.has(state.selAnchor)) state.selAnchor = first;
  syncSelUi();
}

function selectAllVisibleTasks() {
  const ids = visibleTaskIds();
  if (!ids.length) return false;
  state.selIds = new Set(ids);
  const focused = document.activeElement?.closest?.("#task-list .task")?.dataset.id;
  const current = state.selIds.has(focused) ? focused : state.selId;
  state.selId = state.selAnchor = state.selIds.has(current) ? current : ids[0];
  syncSelUi();
  return true;
}

/// 选中一行。opts.add=Ctrl/⌘ 点选（切换），opts.range=Shift 范围选。
/// 点列表空白处调 selectRow(null) 取消全部选中。
function selectIds(current, ordered, anchor, id, { add, range }) {
  if (range) {
    const a = ordered.indexOf(anchor), b = ordered.indexOf(id);
    if (a >= 0 && b >= 0) {
      const span = ordered.slice(Math.min(a, b), Math.max(a, b) + 1);
      return new Set(add ? [...current, ...span] : span);
    }
  }
  if (add) {
    const next = new Set(current);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    return next;
  }
  return new Set([id]);
}

function selectRow(id, opts = {}) {
  state.selIds = id ? selectIds(state.selIds, visibleTaskIds(), state.selAnchor, id, opts) : new Set();
  state.selId = id;
  if (!id || !opts.range || !state.selAnchor) state.selAnchor = id;
  syncSelUi();
}

/// 选中状态的显示：行高亮、标题旁的数量、按钮文字。列表重画后也要调一次
const MULTI_TITLES = { pause: "暂停所选", resume: "继续所选", redl: "重新下载所选", del: "移除所选", play: null, folder: null, retrySubs: null };
function syncSelUi() {
  const n = state.selIds.size;
  const count = $("sel-count");
  if (count) {
    count.hidden = n < 2;
    count.textContent = `已选择 ${n} 项`;
  }
  document.querySelectorAll("#task-list .task").forEach((r) => {
    const on = state.selIds.has(r.dataset.id);
    r.classList.toggle("sel", on);
    r.querySelectorAll("[data-act], [data-trash-act]").forEach((b) => {
      // 提示可能已被收编进 data-tip
      const cur = b.getAttribute("title") || b.dataset.tip || "";
      if (b.dataset.oneTitle === undefined) b.dataset.oneTitle = cur;
      const act = b.dataset.act || b.dataset.trashAct;
      const multi = on && n > 1 ? MULTI_TITLES[act] : null;
      const text = multi ? `${multi}（${n} 项）` : b.dataset.oneTitle;
      if (b.hasAttribute("title")) b.setAttribute("title", text);
      else b.dataset.tip = text;
    });
  });
}

/// 框选：从列表空白处按下拖动（已完成、下载历史页从行上开始也可以）。按住 Ctrl 是追加
function bindRubberBand(scroller, { selector, keyOf, selection, apply, allowed, onCommit, layer = "" }) {
  if (!scroller) return;
  const ignored = "button, a, input, select, .hero, [data-act], [data-trash-act], [data-subtitle-action]";
  let mouseBand = null, touchGesture = null;

  function beginBand(x0, y0, append) {
    const previous = new Set(selection());
    const base = append ? previous : new Set();
    const top0 = scroller.scrollTop;
    const previousUserSelect = document.body.style.userSelect;
    let rect = null, lastX = x0, lastY = y0;
    const paint = () => {
      const y0Now = y0 - (scroller.scrollTop - top0);
      const l = Math.min(x0, lastX), t = Math.min(y0Now, lastY), w = Math.abs(lastX - x0), h = Math.abs(lastY - y0Now);
      Object.assign(rect.style, { left: `${l}px`, top: `${t}px`, width: `${w}px`, height: `${h}px` });
      const picked = new Set(base);
      scroller.querySelectorAll(selector).forEach((r) => {
        const b = r.getBoundingClientRect();
        const key = keyOf(r);
        if (key != null && b.left < l + w && b.right > l && b.top < t + h && b.bottom > t) picked.add(key);
      });
      apply(picked);
    };
    const timer = setInterval(() => {
      if (!rect) return;
      const b = scroller.getBoundingClientRect();
      if (lastY < b.top + 30) scroller.scrollTop -= 12;
      else if (lastY > b.bottom - 30) scroller.scrollTop += 12;
      else return;
      paint();
    }, 30);
    const move = (x, y) => {
      lastX = x; lastY = y;
      if (!rect && Math.hypot(lastX - x0, lastY - y0) < 5) return;
      if (!rect) {
        rect = document.createElement("div");
        rect.className = "sel-rect " + layer;
        document.body.append(rect);
        document.body.style.userSelect = "none";
      }
      paint();
    };
    const finish = (commit) => {
      clearInterval(timer);
      if (!rect) return;
      rect.remove();
      document.body.style.userSelect = previousUserSelect;
      if (!commit) {
        apply(previous);
        return;
      }
      onCommit([...selection()].at(-1) ?? null);
      // 松手后的 click 不能把刚框好的选择清掉
      const eat = (ce) => { ce.stopPropagation(); ce.preventDefault(); };
      addEventListener("click", eat, { capture: true, once: true });
      setTimeout(() => removeEventListener("click", eat, { capture: true }), 50);
    };
    return { move, finish };
  }

  scroller.addEventListener("pointerdown", (e) => {
    if (e.pointerType === "touch" || e.button !== 0 || e.shiftKey || e.isPrimary === false || mouseBand || touchGesture) return;
    if (e.target.closest(ignored) || !allowed(e)) return;
    if (e.clientX > scroller.getBoundingClientRect().right - 12) return;
    const pointer = e.pointerId;
    mouseBand = beginBand(e.clientX, e.clientY, e.ctrlKey || e.metaKey);
    const move = (ev) => { if (ev.pointerId === pointer) mouseBand.move(ev.clientX, ev.clientY); };
    const finish = (commit) => {
      removeEventListener("pointermove", move);
      removeEventListener("pointerup", up);
      removeEventListener("pointercancel", cancel);
      removeEventListener("blur", blur);
      mouseBand.finish(commit);
      mouseBand = null;
    };
    const up = (ev) => { if (ev.pointerId === pointer) finish(true); };
    const cancel = (ev) => { if (ev.pointerId === pointer) finish(false); };
    const blur = () => finish(false);
    addEventListener("pointermove", move);
    addEventListener("pointerup", up);
    addEventListener("pointercancel", cancel);
    addEventListener("blur", blur);
  });

  const finishTouch = (commit) => {
    touchGesture?.band?.finish(commit);
    touchGesture = null;
  };
  scroller.addEventListener("touchstart", (e) => {
    if (e.touches.length !== 1) { finishTouch(false); return; }
    if (mouseBand || e.target.closest(ignored) || e.target.closest(selector) || !allowed(e)) return;
    const point = e.changedTouches[0];
    touchGesture = { id: point.identifier, x: point.clientX, y: point.clientY, start: performance.now(), band: null };
  }, { passive: true });
  scroller.addEventListener("touchmove", (e) => {
    const gesture = touchGesture;
    if (!gesture) return;
    const point = [...e.changedTouches].find((p) => p.identifier === gesture.id);
    if (!point) return;
    if (!gesture.band) {
      if (Math.hypot(point.clientX - gesture.x, point.clientY - gesture.y) < 5) return;
      if (performance.now() - gesture.start < 350 || !e.cancelable) { finishTouch(false); return; }
      gesture.band = beginBand(gesture.x, gesture.y, false);
    }
    if (!e.cancelable) { finishTouch(false); return; }
    e.preventDefault();
    gesture.band.move(point.clientX, point.clientY);
  }, { passive: false });
  for (const event of ["touchend", "touchcancel"]) {
    scroller.addEventListener(event, (e) => {
      if ([...e.changedTouches].some((p) => p.identifier === touchGesture?.id)) finishTouch(event === "touchend");
    });
  }
  addEventListener("blur", () => finishTouch(false));
}
bindRubberBand(document.querySelector("#pane-downloads .scroll"), {
  selector: "#task-list .task", keyOf: row => row.dataset.id,
  selection: () => state.selIds, apply: ids => { state.selIds = ids; syncSelUi(); },
  allowed: event => state.view !== "all" || !event.target.closest(".task"),
  onCommit: last => { state.selId = state.selAnchor = last; },
});
bindRubberBand($("files-scroll"), {
  selector: ".fd-row", keyOf: row => {
    const file = drawerFileAt(row);
    return file ? drawerFileKey(file) : null;
  },
  selection: () => drawerFileSelection, apply: applyDrawerSelection,
  allowed: () => !!filesPanelId && filesList.length > 0,
  onCommit: last => { drawerAnchor = last; }, layer: "sel-rect-drawer",
});

/// 当前选中的任务对象列表（多选时按列表顺序）
function selectedTasks() {
  const order = state.tasks.filter((t) => !t.creating && state.selIds.has(t.id)).flatMap((t) => (isGroup(t.id) ? groupMembers(t.id) : [t]));
  if (order.length) return order;
  if (state.selId) {
    const t = state.tasks.find((x) => x.id === state.selId);
    if (t && !t.creating) return [t];
  }
  return [];
}

/// 行内/菜单上的 del：若这一条在多选集合里，对全部选中项生效；否则只处理这一条
function multiFor(id) {
  // 先看多选：合并任务和别的任务一起选中时，要作用于全部，不能只算这一组
  if (state.selIds.has(id) && state.selIds.size > 1) return selectedTasks();
  if (isGroup(id)) return groupMembers(id);
  if (state.selIds.has(id) && state.selIds.size > 1) return selectedTasks();
  const t = state.tasks.find((x) => x.id === id);
  return t ? [t] : [];
}

/// 双击 / 空格 / 回车的主操作（对齐 Mac 的 primaryAction）：
/// 下完的打开文件，失败的重试，下载中的暂停或继续。都走 taskAction 那套，不另起炉灶
function primaryAction(id) {
  const t = state.tasks.find((x) => x.id === id);
  if (!t) return;
  if (t.missing) return taskAction(id, "reveal"); // 文件已不在原位置，打开没意义，定位到文件夹
  if (t.status === "done") return taskAction(id, "open");
  if (t.status === "error") return taskAction(id, "resume"); // 重试就是再 resume 一次，和行内按钮同一套
  if (t.status === "pause") return taskAction(id, "resume");
  if (ACTIVE.has(t.status)) return taskAction(id, "pause");
  return taskAction(id, "open"); // 其它非激活状态（已关闭之类）跟 Mac 一样当打开
}

/// 多选时的暂停、继续、重新下载：只动适用的那几条（选中里混着不同状态很常见）。
async function bulkAction(act, targets) {
  if (act === "redl") {
    const group = targets.length && collectionForEpisode(targets[0].id);
    if (group && targets.every(task => collectionForEpisode(task.id) === group)) {
      return openCollectionRedownload(group);
    }
  }
  let list = [];
  if (act === "pause") list = targets.filter((t) => ACTIVE.has(t.status));
  else if (act === "resume") list = targets.filter((t) => t.status === "pause" || t.status === "error");
  else list = targets.filter((t) => t.status === "done" && !String(t.id).startsWith("bd:") && t.meta?.req?.url);
  if (!list.length) return toast("选中的任务都不适用这个操作");
  let ok = 0;
  let firstErr = "";
  let cleanupNote = "";
  let collectionId = "";
  const owners = new Map(act === "redl" ? list.map(task => [task.id, collectionForEpisode(task.id)]) : []);
  if (act === "redl") {
    const collection = [...owners.values()].some(Boolean);
    const desc = collection ? collectionRedownloadDescription(list.map(task => task.id)) : "旧文件会移到系统回收站，再下载新的一份到原来的目录。";
    const yes = await askYes(`重新下载 ${list.length} 个任务？`, desc, "重新下载");
    if (!yes) return;
    if (collection) toast("正在重新下载…");
  }
  for (const t of list) {
    try {
      if (t.collectionFiles && (act === "pause" || act === "resume")) {
        const group = taskGroups.find(group => group.id === t.id);
        await changeCollectionPlayback(group, act === "pause");
      }
      else if (act === "pause") await invoke("pause_task", { id: t.id });
      else if (act === "resume") {
        if (needsCloudRefresh(t)) { await beginCloudRefresh(t,true); continue; }
        await invoke("resume_task", { id: t.id });
      }
      else {
        if (String(t.meta?.req?.url || "").startsWith("shandian-multi:")) {
          await invoke("multi_task_redownload", { id: t.id });
          ok += 1;
          continue;
        }
        const group = owners.get(t.id);
        if (group && t.fileState === "offline") throw new Error("原保存位置未连接");
        if (group) {
          cleanupNote = (await redownloadCollectionEpisode(group, t.id)) || cleanupNote;
          collectionId ||= group.id;
        } else {
          const dir = await invoke("task_folder_path", { id: t.id }).catch(() => "");
          const id = await doAdd(t.meta.req.url, "", dir, null, null, { force: true, replaceId: t.id, recycleId: t.missing ? null : t.id });
          if (!id) throw new Error("没有创建新的下载任务");
        }
      }
      ok += 1;
    } catch (e) {
      firstErr = firstErr || String(e);
    }
  }
  if (collectionId) {
    switchView("all");
    focusNewTask(collectionId);
  }
  const verb = { pause: "暂停", resume: "继续", redl: "重新下载" }[act];
  toast(firstErr ? `已${verb} ${ok} 项，${list.length - ok} 项失败：${firstErr}` : cleanupNote || `已${verb} ${ok} 项`);
  refreshTasks();
}

let thunderInfo = { installed: false, saveDir: "" };
function thunderDuplicateNotice(message) {
  toast(message, { action: {
    label: "打开迅雷查看",
    run: async () => {
      try { await nativeInvoke("thunder_open"); }
      catch (error) { toast(error?.message || String(error)); }
    },
  } });
}
const canThunder = (t) => !/^(v:|pp:|cloud:|xl:|nas:)/.test(String(t.id)) && t.status !== "done" && (t.kind === "种子文件" || /^(magnet:|ed2k:)/i.test(t.meta?.req?.url || ""));

function isOwnBaiduShare(task) {
  return task?.status === "error" && String(task.id).startsWith("bd:") && /自己的分享/.test(task.shandianError || "");
}

function baiduSelectionOf(task) {
  try { return JSON.parse(task.meta?.req?.labels?.baiduSelection || "null"); }
  catch (_) { return null; }
}

function openBaiduRetry(task, codeRequired = false) {
  const link = task.meta?.req?.url;
  if (!link) return toast("找不到原分享链接");
  return askConfirm(link, {
    baiduRetryId: task.id,
    baiduSelection: baiduSelectionOf(task),
    baiduCodeRequired: codeRequired,
    dir: task.meta?.opts?.path,
    cleanupRemote: task.meta?.req?.labels?.baiduCleanup === "true",
  });
}

function copyTaskRoots(id) {
  const ids = state.selIds.has(id) ? state.selIds : new Set([id]);
  return state.tasks.filter(task => !task.creating && ids.has(task.id));
}

function taskSourceLink(task) {
  const group = taskGroups.find(entry => entry.id === task.id);
  const input = group ? group.collection?.url : task.meta?.req?.url;
  if (!input || input.startsWith("shandian-multi:")) return "";
  if (!group?.collection) return input;
  try {
    const url = new URL(input);
    if (["youtube.com", "www.youtube.com", "m.youtube.com", "youtu.be"].includes(url.hostname) && url.searchParams.get("list")) {
      return "https://www.youtube.com/playlist?list=" + encodeURIComponent(url.searchParams.get("list"));
    }
    if (["bilibili.com", "www.bilibili.com", "m.bilibili.com"].includes(url.hostname) && url.pathname.startsWith("/video/")) {
      url.searchParams.delete("p");
      return url.href;
    }
  } catch {}
  return input;
}

async function copyTaskPath(task) {
  const group = taskGroups.find(entry => entry.id === task.id);
  if (!group) return invoke("item_path", { id: task.id }).catch(() => "");
  if (group.dir && !group.dir.startsWith("/")) return group.dir.replaceAll("/", "\\");
  const members = groupMembers(group.id);
  const member = members.find(item => item.status === "done" && !item.missing) || members[0];
  if (!member) return group.dir || "";
  return invoke("task_folder_path", { id: member.id }).catch(() => group.dir || "");
}

async function taskAction(id, act) {
  const selectedTask = state.tasks.find(task => task.id === id);
  if (selectedTask?.creating) return;
  if (selectedTask && String(id).startsWith("bd:")) {
    if (act === "resume" && selectedTask.collided) return confirmBaiduCopy(selectedTask);
    if (isOwnBaiduShare(selectedTask) && act === "resume") act = "openLink";
    const needsCode = /提取码/.test(selectedTask.shandianError || "");
    if (act === "baiduCode" || (act === "resume" && needsCode)) return openBaiduRetry(selectedTask, true);
    if (act === "resume" && /百度网盘授权/.test(selectedTask.shandianError || "")) return openBaiduRetry(selectedTask);
  }
  if (["pause", "resume", "del", "purge"].includes(act)) {
    const roots = state.selIds.has(id) && state.selIds.size > 1
      ? state.tasks.filter(task => state.selIds.has(task.id) && !task.creating)
      : state.tasks.filter(task => task.id === id);
    if (["del","purge"].includes(act) && roots.some(task => isGroup(task.id))) {
      return askRemove(roots, act === "purge");
    }
    if (roots.some(task => task.collectionFiles)) {
      const targets = roots.flatMap(task => !task.collectionFiles && isGroup(task.id) ? groupMembers(task.id) : [task]);
      return bulkAction(act, targets);
    }
  }
  if (isGroup(id)) {
    const ms = groupMembers(id);
    const g = taskGroups.find((x) => x.id === id);
    if (g?.collection && (act === "redl" || act === "redlopt")) return openCollectionRedownload(g);
    if (act === "pause" || act === "resume") {
      g.paused = act === "pause";
      saveGroups();
      if (act === "pause") return bulkAction("pause", ms);
      // 手动继续：失败的立刻重试，重新有 3 次机会
      ms.forEach((m) => groupRetries.set(m.id, { n: 0, at: 0 }));
      return refreshTasks();
    }
    if (["open", "reveal", "folder"].includes(act)) return openGroupFolder(ms);
    if (act === "errinfo") return toast(state.tasks.find((x) => x.id === id)?.shandianError || "");
  }
  const t = state.tasks.find((x) => x.id === id);
  if (!t) return toast("任务列表刚刷新，请重新选中这条任务");
  if (act === "thunder") {
    const targets = multiFor(id).filter(canThunder);
    let ok = 0;
    let firstErr = "";
    let duplicate = false;
    // 一条一条交：迅雷的确认窗口同一时间只有一个
    for (const x of targets) {
      try {
        // 先于迅雷查重名：同名的通常是这条任务自己下了一半的文件，迅雷用不上
        const same = await invoke("thunder_conflict", { id: x.id });
        let mode = null;
        if (same) {
          mode = await askChoice({
            title: same.own ? "这条任务已下载了一部分" : "保存位置已有同名内容",
            desc: same.own
              ? `迅雷会在「迅雷下载」文件夹从头下载「${same.name}」。确认迅雷开始后，再处理原任务和文件。`
              : `「${same.name}」已存在。迅雷会存到单独的「迅雷下载」文件夹。`,
            buttons: same.own
              ? [{ label: "两边都下", value: "both", primary: true }, { label: "移到回收站", value: "trash" }]
              : [{ label: "继续交给迅雷", value: "replace", primary: true }],
          });
          if (!mode) continue;
        }
        const spot = keepSpot(x.id, x.meta?.req?.url || "");
        if (mode) toast("正在等待迅雷接收…");
        await invoke("thunder_handoff", { id: x.id, mode });
        await spot();
        ok += 1;
      } catch (e) {
        duplicate ||= e?.code === "duplicate";
        firstErr = firstErr || e?.message || String(e);
      }
    }
    const message = firstErr && targets.length === 1 ? firstErr
      : firstErr ? `已交给迅雷 ${ok} 项，${targets.length - ok} 项失败：${firstErr}` : `已交给迅雷 ${ok} 项`;
    if (duplicate) thunderDuplicateNotice(message);
    else toast(message);
    return refreshTasks();
  }
  if (act === "pause" || act === "resume" || act === "redl") {
    const targets = multiFor(id);
    if (targets.length > 1) return bulkAction(act, targets);
  }
  try {
    if (act === "play") return playTask(id);
    if (act === "polish") {
      const refined = await invoke("polish_task", { id });
      toast(refined ? "已开始润色字幕" : "没检测到 Ollama，只整理重复和标点");
      return refreshTasks();
    }
    if (act === "subStyle") return openSubStyle(id);
    if (act === "fetchSubs") return fetchTaskSubtitles(id);
    if (act === "polishUndo") {
      await invoke("polish_restore", { id });
      toast("已恢复原字幕");
      return refreshTasks();
    }
    if (act === "verify") {
      await invoke("verify_task", { id });
      return toast("正在重新检查已下载的文件");
    }
    if (act === "xlToLocal") {
      const url = t.meta?.req?.url || "";
      if (!url) return toast("这条任务没有原始链接");
      const spot = keepSpot(id, url);
      // 原来那条还在就直接恢复，已下的进度不丢
      if (await invoke("thunder_to_local", { id })) return spot();
      const info = await invoke("target_info", { link: url });
      const newId = await invoke("add_task", { url, name: null, dir: info?.dir || null, video: null, headers: null, forceNew: false, selectFiles: null });
      // 空 id：本机保存位置有同名，正在问用户，选完（没取消）再去掉迅雷这条
      if (!newId) {
        xlWaiting.set(url, { id, spot });
        return refreshTasks();
      }
      await invoke("forget_task", { id }).catch(() => {});
      return spot();
    }
    if (act === "pikpak") return await openCloudChooser(t);
    if (act === "refreshLinks") return await beginCloudRefresh(t);
    // 已失败的网盘任务直接更新链接，不再先重试旧地址。
    if (act === "resume" && needsCloudRefresh(t)) return await beginCloudRefresh(t,true);
    if (act === "open") await invoke("item_open", { id });
    else if (act === "openLink") {
      const url = taskSourceLink(t);
      if (!url) return toast("这条任务没有原始链接");
      await invoke("open_url", { url });
    }
    else if (act === "reveal") await invoke("item_reveal", { id });
    else if (act === "redl" || act === "redlopt") {
      const url = t.meta?.req?.url || "";
      if (!url) return toast("这条任务没有原始链接");
      if (String(t.id).startsWith("bd:")) {
        return submitLink(url, {
          force: true,
          dir: t.meta?.opts?.path || "",
          baiduRedownloadId: t.id,
          baiduSelection: baiduSelectionOf(t),
          baiduCodeRequired: t.meta?.req?.labels?.baiduHasCode === "true",
        });
      }
      // 多文件任务用存着的文件列表重下；链接过期时任务上会提示失败
      if (url.startsWith("shandian-multi:")) {
        if (act === "redlopt" || state.settings?.settings.confirmNew) return openMultiConfirm(await invoke("multi_task_info", { id }), { replaceMulti: id });
        if (!(await askYes("重新下载这个任务？", "已有的文件会移到系统回收站，再下载到原来的位置。网盘链接有时效，过期会提示失败。"))) return;
        await invoke("multi_task_redownload", { id });
        switchView("all");
        return refreshTasks();
      }
      const dir = await invoke("task_folder_path", { id }).catch(() => "");
      // 没开新建确认框时不会有下一步询问，这里必须先问：文件要被送进回收站
      // redlopt 是右键的「选择选项」：这一次照样弹确认框，那里已经写明了
      if (act === "redl" && !t.missing && !state.settings?.settings.confirmNew) {
        const yes = await askYes("重新下载？", "旧文件会移到系统回收站，再下载新的一份到原来的目录。", "重新下载");
        if (!yes) return;
      }
      // 这里什么文件都不动。用户确认后才在 doAdd 里把旧文件送回收站，旧记录也是投成功才拿掉
      const options = { force: true, replaceId: id, dir, recycleId: t.missing ? null : id };
      if (act === "redlopt" || state.settings?.settings.confirmNew) return askConfirm(url, options);
      await submitLink(url, options);
      return;
    }
    else if (act === "errinfo") {
      const full = t.shandianError || "下载失败";
      const c = await askChoice({
        title: "下载失败",
        desc: full,
        buttons: [
          { label: "复制", value: "copy" },
          { label: "关闭", value: null, primary: true },
        ],
      });
      if (c === "copy") toast((await copyText(full)) ? "已复制" : "复制失败");
      return;
    }
    else if (act === "pause") await invoke("pause_task", { id });
    else if (act === "resume") {
      collisionAsked.delete(id);
      try {
        await invoke("resume_task", { id });
      } catch (error) {
        if (id.startsWith("bd:") && /提取码/.test(String(error))) return openBaiduRetry(t, true);
        throw error;
      }
    }
    else if (act === "folder") {
      const stop = toast("正在打开文件夹…");
      try { await invoke("open_task_folder", { id }); } finally { stop(); }
    }
    else if (act === "del") {
      // 移除：多选时对所有选中项生效；先弹确认（可选同时删文件）
      const targets = multiFor(id);
      // 失效条目（文件早就不在原位置）没什么可确认的：删的只是这条记录，还进下载历史，
      // 想再下还能从历史里重新下载。选中的全是这种就直接办，不再弹窗
      if (targets.length && targets.every((x) => x.missing)) {
        return removeTaskCards(targets.map(task => task.id), false);
      }
      await askRemove(targets);
      return; // askRemove 里会 refresh
    } else if (act === "purge") {
      // 连文件一起删：走同一套确认（勾上删文件），文件进系统回收站
      await askRemove(multiFor(id), true);
      return;
    } else if (act === "files") {
      return openFilesPanel(id);
    } else if (act === "copyLink") {
      // 多文件任务的 shandian-multi: 不是真实链接，不能拿去用
      const links = [...new Set(copyTaskRoots(id).map(taskSourceLink).filter(Boolean))];
      toast(links.length && (await copyText(links.join("\n"))) ? (links.length > 1 ? `已复制 ${links.length} 个链接` : "已复制链接") : "复制失败");
      return;
    } else if (act === "copyName") {
      // 有实际文件就复制完整文件名（带后缀），没下完的只有显示名
      const names = (await Promise.all(copyTaskRoots(id).map(async (x) => {
        if (isGroup(x.id)) return taskGroups.find(group => group.id === x.id)?.name || x.name;
        const path = x.status === "done" ? String((await invoke("item_path", { id: x.id }).catch(() => "")) || "") : "";
        return path.split(/[\\/]/).pop() || x.name || x.meta?.res?.name || "";
      }))).filter(Boolean);
      toast((await copyText(names.join("\n"))) ? (names.length > 1 ? `已复制 ${names.length} 个名称` : "已复制名称") : "复制失败");
      return;
    } else if (act === "copyPath") {
      // 复制完整路径：剪贴板 API 不给用就退回隐藏 textarea 那套老办法
      const paths = (await Promise.all(copyTaskRoots(id).map(copyTaskPath))).filter(Boolean);
      if (!paths.length) return toast("这些任务还没有文件路径");
      toast((await copyText(paths.join("\n"))) ? (paths.length > 1 ? `已复制 ${paths.length} 个路径` : "已复制路径") : "复制失败");
    }
  } catch (e) {
    toast("操作失败：" + e);
  }
  refreshTasks();
}

/// 移除确认弹窗（Mac RemovalConfirm 同款）：标题 + 说明 + 可选「同时删除文件」。
/// targets 是任务数组，支持多选批量。
let rmPending = null;
function askRemove(targets, defaultTrash = false) {
  const list = Array.isArray(targets) ? targets : [targets];
  if (!list.length) return Promise.resolve();
  return new Promise((resolve) => {
    rmPending = { ids: list.map((t) => t.id), resolve };
    const n = list.length;
    const name = n === 1
      ? (list[0].name || list[0].meta?.res?.name || "这条任务")
      : `${n} 个任务`;
    const anyActive = list.some((t) => ["running", "wait", "ready"].includes(t.status));
    const anyFile = list.some((t) => !t.missing && (t.status === "done" || (t.progress?.downloaded || 0) > 0));
    if (defaultTrash) {
      $("rm-title").textContent = n === 1 ? `删除「${name}」和它的文件？` : `删除 ${n} 个任务和它们的文件？`;
      $("rm-desc").textContent = "任务记录移入下载历史，只有这条任务下载的文件会移到系统回收站，可以还原。";
    } else {
      $("rm-title").textContent = n === 1
        ? (anyActive ? `取消下载「${name}」？` : `删除「${name}」？`)
        : `删除 ${n} 个任务？`;
      $("rm-desc").textContent = anyActive
        ? "下载将停止。任务记录移入下载历史，可以重新下载。"
        : anyFile
          ? "任务记录移入下载历史，可以重新下载。文件保留在原位置。"
          : "文件不在原位置，只移除任务记录。";
    }
    $("rm-files-row").hidden = !anyFile && !defaultTrash;
    const cb = $("rm-files");
    $("rm-files-label").textContent = anyActive
      ? "同时把已下载的部分移到系统回收站"
      : "同时把文件移到系统回收站";
    // 右键「删除任务和文件」默认勾上；普通移除默认不勾
    cb.checked = defaultTrash;
    $("rm-confirm").hidden = false;
    $("rm-ok").textContent = "删除";
    $("rm-ok").focus();
  });
}

function closeRemove(ok) {
  const p = rmPending;
  rmPending = null;
  $("rm-confirm").hidden = true;
  if (!p) return;
  if (!ok) {
    p.resolve();
    return;
  }
  const alsoFiles = !$("rm-files-row").hidden && $("rm-files").checked;
  removeTaskCards(p.ids, alsoFiles).finally(p.resolve);
}

async function removeTaskCards(ids, alsoFiles) {
  const pending = [...new Set(ids)].filter(id => !taskRemovals.has(id));
  if (!pending.length) return;
  for (const id of pending) taskRemovals.set(id, false);
  selectRow(null);
  renderAll();

  let removed = 0;
  let failure = "";
  let restore = false;
  for (const id of pending) {
    try {
      const group = taskGroups.find(group => group.id === id);
      if (group) await removeCollection(group, alsoFiles);
      else await invoke("delete_task", { id, alsoFiles });
      taskRemovals.set(id, true);
      removed += 1;
    } catch (error) {
      failure = String(error);
      if (failure.startsWith("任务已删除")) {
        taskRemovals.set(id, true);
      } else {
        taskRemovals.delete(id);
        restore = true;
      }
    }
  }
  if (removed || pending.some(id => taskRemovals.get(id))) await loadTrash();
  if (restore) renderAll();
  if (removed && failure) toast(`已删除 ${removed} 项，${failure}`);
  else if (removed > 1) toast(alsoFiles ? `已删除 ${removed} 项，文件在系统回收站` : `已删除 ${removed} 项，记录在下载历史`);
  else if (removed) toast(alsoFiles ? "已删除，文件在系统回收站" : "已删除，记录在下载历史");
  else if (failure) toast(failure.startsWith("任务已删除") ? failure : "删除失败：" + failure);
  refreshTasks();
}

$("rm-ok")?.addEventListener("click", () => closeRemove(true));
$("rm-cancel")?.addEventListener("click", () => closeRemove(false));
$("rm-confirm")?.addEventListener("click", (e) => {
  if (e.target.id === "rm-confirm") closeRemove(false);
});

/// 删除下载记录前问一下：文件还在时可以勾「同时把文件移到系统回收站」。
/// 选了「不再询问」以后直接删、只删记录；设置里「删除记录前确认」可以重新打开。
/// 返回 null = 取消；否则 { alsoFiles }
const SKIP_HISTORY_DELETE = "sd.skipHistoryDeleteConfirm";
async function confirmHistoryDelete(recs) {
  if (localStorage.getItem(SKIP_HISTORY_DELETE) === "1") return { alsoFiles: false };
  const here = recs.filter((r) => r.fileState === "here").length;
  const files = { label: here === recs.length ? "同时把文件移到系统回收站" : `同时把文件移到系统回收站（${here} 项还在）`, checked: false };
  const never = { label: "不再询问（可在设置中打开）", checked: false, foot: true };
  const c = await askChoice({
    title: recs.length > 1 ? `删除 ${recs.length} 条下载记录？` : "删除这条下载记录？",
    desc: here ? "" : "只删除记录。",
    checks: here ? [files, never] : [never],
    buttons: [{ label: "删除", value: "yes", primary: true }],
  });
  if (!c) return null;
  if (never.checked) {
    localStorage.setItem(SKIP_HISTORY_DELETE, "1");
    setSwitch("sw-history-confirm", false);
  }
  return { alsoFiles: !!here && files.checked };
}

/// 下载历史行上的操作
async function redownloadHistory(recs) {
  const owners = new Map(recs.map(record => [record.id, historyOwner(record)]));
  const desc = [...owners.values()].some(group => group?.collection)
    ? collectionRedownloadDescription(recs.map(record => record.taskId || record.id))
    : "将这组任务重新下载到原来的保存位置。";
  if (!(await askYes(`重新下载 ${recs.length} 项？`, desc, "重新下载"))) return;
  toast("正在重新下载…");
  let ok = 0;
  let firstErr = "";
  let collectionId = "";
  for (const record of recs) {
    try {
      const group = owners.get(record.id);
      if (group && record.fileState === "offline") throw new Error("原保存位置未连接");
      if (group?.collection) {
        await redownloadCollectionEpisode(group, historyMember(group, record), record);
      } else {
        const id = await invoke("trash_redownload", { id: record.id, dir: group?.dir || null, video: null });
        if (!id) throw new Error("没有创建新的下载任务");
        if (group) {
          if (!group.ids.includes(id)) group.ids.push(id);
          group.paused = false;
          saveGroups();
        }
      }
      if (group) collectionId ||= group.id;
      ok += 1;
    } catch (error) {
      firstErr ||= error?.message || String(error);
    }
  }
  await loadTrash();
  if (ok) {
    switchView("all");
    if (collectionId) focusNewTask(collectionId);
  } else if (state.view === "trash") renderTasks();
  refreshTasks();
  syncTrashClear();
  toast(firstErr ? `已重新下载 ${ok} 项，${recs.length - ok} 项失败：${firstErr}` : `已重新下载 ${ok} 项`);
}

async function trashAction(id, act) {
  if (!id || !invoke) return;
  const hg = historyGroup(id);
  if (hg && !(state.selIds.has(id) && state.selIds.size > 1)) {
    const recs = historyRecs(hg);
    if (act === "redl") return redownloadHistory(recs);
    if (act === "open") {
      const record = recs.find(item => item.fileState === "here" && canOpenHistory(item)) || recs.find(canOpenHistory);
      return openHistoryLocation(record);
    }
    if (act === "del") {
      const ok = await confirmHistoryDelete(recs);
      if (!ok) return;
      for (const r of recs) await invoke("trash_delete", { id: r.id, alsoFiles: ok.alsoFiles }).catch((e) => toast(String(e)));
      toast("已删除历史记录");
      await loadTrash();
      if (state.view === "trash") renderTasks();
      syncTrashClear();
      return;
    }
  }
  // 多文件任务重新下载：链接可能过期，照样试，失败了任务上会提示
  const multiRec = state.trash.find((r) => r.id === id);
  if (act === "redl" && multiRec?.link?.startsWith("shandian-multi:") && !(state.selIds.has(id) && state.selIds.size > 1)) {
    if (multiRec.multi && state.settings?.settings.confirmNew) return openMultiConfirm(multiRec.multi, { trashMulti: id });
    if (!(await askYes("重新下载这个任务？", "已有的文件会移到系统回收站。网盘链接有时效，过期会提示失败。"))) return;
    await invoke("trash_redownload", { id, dir: null, video: null }).catch((e) => toast(String(e)));
    await loadTrash();
    switchView("all");
    return refreshTasks();
  }
  // 多选时对所有选中的记录生效
  if (state.selIds.has(id) && state.selIds.size > 1 && (act === "del" || act === "redl")) {
    // 选中里有合并的那条就展开成各个记录
    const recs = state.trash.filter((r) => state.selIds.has(r.id)).flatMap((r) => (historyGroup(r.id) ? historyRecs(historyGroup(r.id)) : [r]));
    if (act === "redl" && recs.some(historyOwner)) return redownloadHistory(recs);
    // 多选只问一次
    const delOk = act === "del" ? await confirmHistoryDelete(recs) : null;
    if (act === "del" && !delOk) return;
    if (act === "redl") {
      const yes = await askYes(`重新下载 ${recs.length} 项？`, "旧文件会移到系统回收站，画质和字幕用记住的选项。");
      if (!yes) return;
    }
    let ok = 0;
    for (const r of recs) {
      try {
        if (act === "del") await invoke("trash_delete", { id: r.id, alsoFiles: delOk.alsoFiles });
        else if (r.link?.startsWith("shandian-multi:")) await invoke("trash_redownload", { id: r.id, dir: null, video: null });
        else if (r.link) {
          const isBt = /^magnet:|\.torrent(\?|$)/i.test(r.link);
          await doAdd(r.link, "", !isBt && r.path ? r.path.replace(/[\\/][^\\/]*$/, "") : "", null, null, { force: true, trashId: r.id, trashHere: r.fileState === "here" });
        }
        ok += 1;
      } catch (e) {
        toast("操作失败：" + e);
      }
    }
    toast(act === "del" ? `已删除 ${ok} 条记录` : `已重新下载 ${ok} 项`);
    await loadTrash();
    if (state.view === "trash") renderTasks();
    syncTrashClear();
    return;
  }
  try {
    if (act === "redl") {
      const rec = state.trash.find((x) => x.id === id);
      if (!rec?.link) return toast("这条记录没有保存链接");
      if (historyOwner(rec)) return redownloadHistory([rec]);
      const here = rec.fileState === "here";
      // 开着新建确认时这里不问，确认框标题已经写明；关着才要先问一句
      if (here && !state.settings?.settings.confirmNew) {
        const yes = await askYes("重新下载？", "旧文件会移到系统回收站，再下载新的一份到原来的目录。", "重新下载");
        if (!yes) return;
      }
      const isBt = /^magnet:|\.torrent(\?|$)/i.test(rec.link);
      const dir = !isBt && rec.path ? rec.path.replace(/[\\/][^\\/]*$/, "") : "";
      const options = { force: true, trashId: id, trashHere: here, dir };
      if (String(id).startsWith("nas:") || state.settings?.settings.confirmNew) return askConfirm(rec.link, options);
      await submitLink(rec.link, options);
      return;
    } else if (act === "open") {
      return openHistoryLocation(state.trash.find(record => record.id === id));
    } else if (act === "del") {
      const rec = state.trash.find((x) => x.id === id);
      const ok = await confirmHistoryDelete(rec ? [rec] : []);
      if (!ok) return;
      await invoke("trash_delete", { id, alsoFiles: ok.alsoFiles });
      toast("已删除历史记录");
    }
  } catch (e) {
    toast("操作失败：" + e);
  }
  await loadTrash();
  if (state.view === "trash") renderTasks();
  syncTrashClear();
  renderSidebar();
}

function loadTrash(fresh = true) {
  if (!invoke) return Promise.resolve();
  if (trashLoad && !fresh) return trashLoad;
  const revision = ++trashRevision;
  trashLoadedAt = Date.now();
  const renderHistory = () => {
    syncTrashClear();
    if (state.view === "trash") renderAll();
  };
  trashLoad = Promise.resolve().then(async () => {
    // Show local records while checking media locations or waiting for the NAS.
    if (!trashLoaded) {
      try {
        const snapshot = await window.__TAURI__.core.invoke("trash_list", { checkFiles: false });
        if (revision !== trashRevision) return;
        state.trash = mergeTrash(snapshot);
        renderHistory();
      } catch (error) {
        if (revision !== trashRevision) return;
        trashLoadError = String(error);
      }
    }
    try {
      const records = await invoke("trash_list");
      if (revision !== trashRevision) return;
      state.trash = mergeTrash(records || []);
      trashLoadError = "";
    } catch (error) {
      if (revision === trashRevision) trashLoadError = String(error);
    }
  }).finally(() => {
    if (revision !== trashRevision) return;
    trashLoad = null;
    renderHistory();
  });
  return trashLoad;
}


// ---------- 自绘下拉框 ----------
// 原生 <select> 的弹出列表是系统矩形（方角、白底），和界面风格不搭；
// 这里把原生件藏起来，用「按钮 + 圆角菜单」接管，值仍然存在 select 上，
// 原来的 change 监听一行都不用改。

/// 外部改了 select.value 之后调它，按钮文字才不会停在旧值
function syncCustomSelects() {
  document.querySelectorAll("select.select").forEach(sel => sel._customSelect?.sync());
}
function closeCustomSelects(except) {
  let closed=false;
  document.querySelectorAll("select.select").forEach(sel => {
    const control=sel._customSelect;
    if (sel!==except && control && !control.menu.hidden) { control.close(); closed=true; }
  });
  return closed;
}

/// opts.onHover(value)：鼠标在某一项上时回调（离开菜单时回调 null），字体这类可以实时预览；
/// opts.fontItems：每一项用它自己的字体显示
function initCustomSelect(sel, opts = {}) {
  if (sel._customSelect) {
    Object.assign(sel._customSelect.opts, opts);
    sel._customSelect.sync();
    return;
  }
  opts = { ...opts };
  const wrap = sel.closest(".select-wrap");
  if (!wrap) return;
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "select-btn";
  btn.innerHTML = `<span class="label"></span><svg class="ic"><use href="#i-chev"/></svg>`;
  wrap.insertBefore(btn, sel);
  const menu = document.createElement("div");
  menu.className = "select-menu";
  menu.hidden = true;
  wrap.appendChild(menu);
  const label = btn.querySelector(".label");

  const syncLabel = () => {
    const opt = sel.options[sel.selectedIndex];
    label.textContent = opt ? opt.textContent : "";
    btn.disabled = sel.disabled;
    if (sel.disabled) close();
  };
  sel._customSelect = { opts, sync: syncLabel, close, menu };

  // 选项没变就不重画（字体列表几百项、每项用自己的字体，重画一次要卡一下）
  let builtFor = "";
  function rebuildMenu() {
    const sig = [...sel.options].map((o) => o.value).join("|");
    if (opts.cache && sig === builtFor && menu.childElementCount) {
      [...menu.children].forEach((item, i) => item.classList.toggle("on", i === sel.selectedIndex));
      return;
    }
    builtFor = sig;
    menu.innerHTML = "";
    [...sel.options].forEach((o) => {
      const item = document.createElement("div");
      item.className = "select-item" + (o.selected ? " on" : "");
      item.textContent = o.textContent;
      if (opts.fontItems) item.style.fontFamily = `"${o.value}", sans-serif`;
      item.addEventListener("mouseenter", () => opts.onHover?.(o.value));
      item.setAttribute("role", "option");
      item.setAttribute("aria-disabled", String(o.disabled));
      item.addEventListener("click", () => {
        if (sel.disabled || o.disabled) return;
        if (sel.value !== o.value) {
          sel.value = o.value;
          syncLabel(); // 先把按钮文字换掉，别等保存
          // 冒泡：外层容器上的监听也收得到（原有直接挂在 select 上的监听不受影响）
          sel.dispatchEvent(new Event("change", { bubbles: true }));
        }
        close();
      });
      menu.appendChild(item);
    });
  }

  function open() {
    if (sel.disabled) return;
    closeCustomSelects(sel);
    rebuildMenu();
    menu.hidden = false;
    if (typeof btn.getBoundingClientRect === "function") {
      const bounds=btn.getBoundingClientRect(), clip=wrap.closest(".modal-body, .scroll, .cloud-body")?.getBoundingClientRect();
      const above=bounds.top-Math.max(4,clip?.top ?? 4)-4;
      const below=Math.min(window.innerHeight-4,clip?.bottom ?? window.innerHeight-4)-bounds.bottom-4;
      const up=below<Math.min(menu.scrollHeight,224) && above>below;
      menu.classList.toggle("above",up);
      menu.style.maxHeight=Math.max(32,Math.min(224,up?above:below))+"px";
    }
    menu.querySelector(".select-item.on")?.scrollIntoView({ block: "nearest" });
  }
  function close() {
    if (!menu.hidden) opts.onHover?.(null);
    menu.hidden = true;
  }
  menu.addEventListener("mouseleave", () => opts.onHover?.(null));
  // 预热：先在看不见的地方把菜单画一遍（字体也就载入了），真正点开时只是显示出来
  wrap.addEventListener("warm", () => {
    if (!menu.hidden) return;
    rebuildMenu();
    menu.style.visibility = "hidden";
    menu.hidden = false;
    void menu.scrollHeight;
    menu.hidden = true;
    menu.style.visibility = "";
  });

  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    menu.hidden ? open() : close();
  });
  // 点外面收起；滚动设置页也收起，免得菜单悬在半空
  if (opts.documentDismiss !== false) document.addEventListener("pointerdown", (e) => {
    if (!wrap.contains(e.target)) close();
  });
  btn.addEventListener("keydown", (event) => {
    if (!opts.keyboard) return;
    if (sel.disabled) return;
    if (event.key === "Escape") { close(); event.preventDefault(); event.stopPropagation(); return; }
    if (!["ArrowDown","ArrowUp","Enter"," "].includes(event.key)) return;
    event.preventDefault();
    if (event.key === "Enter" || event.key === " ") { menu.hidden ? open() : close(); return; }
    const options=[...sel.options].filter(option => !option.disabled);
    if(!options.length)return;
    const index=options.findIndex(option => option.value===sel.value), delta=event.key==="ArrowDown"?1:-1;
    const option=options[Math.max(0,Math.min(options.length-1,index<0?0:index+delta))];
    sel.value=option.value; syncLabel(); sel.dispatchEvent(new Event("change",{bubbles:true}));
  });
  sel.closest(".scroll")?.addEventListener("scroll", close, { passive: true });

  syncLabel();
}

document.querySelectorAll("select.select").forEach(sel => initCustomSelect(sel));

// ---------- 任务右键菜单 ----------

/// 右键菜单的图标，都用界面现有的图标集。没配的留空位，文字依然对齐
const MENU_ICONS = {
  baiduCode: "i-link",
  refreshLinks:"i-link", open: "i-external", openLink: "i-link", reveal: "i-folder", folder: "i-folder", redl: "i-dl-box", redlopt: "i-dl-box",
  resume: "i-resume", pause: "i-pause", play: "i-media-play", verify: "i-check", pikpak: "i-cloud", toNas: "i-to-nas",
  toLocal: "i-to-local", xlToLocal: "i-to-local", thunder: "i-thunder", nasThunder: "i-thunder", polish: "i-doc", polishUndo: "i-history", subStyle: "i-doc", fetchSubs: "i-doc",
  files: "i-doc", copyName: "i-clipboard", copyLink: "i-link", copyPath: "i-folder", del: "i-trash", purge: "i-trash",
};
const HISTORY_MENU_ICONS = { 重新下载: "i-dl-box", 打开所在文件夹: "i-folder", 打开链接: "i-external", 复制链接: "i-link", 复制文件路径: "i-clipboard", 删除记录: "i-trash" };
function setMenuLabel(b, label, icon) {
  b.classList.add("has-ic");
  b.innerHTML = `<span class="ctx-ic">${icon ? `<svg class="ic"><use href="#${icon}"/></svg>` : ""}</span><span class="ctx-txt"></span>`;
  b.querySelector(".ctx-txt").textContent = label;
}

/// #ctx-menu 是 index.html 里的容器（CSS 已备好 .ctx-item / .ctx-sep），
/// 兜一层：万一 DOM 还没就位就自己补一个同名壳，菜单功能不至于哑掉
function ctxEl() {
  let menu = $("ctx-menu");
  if (!menu) {
    menu = document.createElement("div");
    menu.id = "ctx-menu";
    menu.className = "ctx-menu";
    document.body.appendChild(menu);
  }
  return menu;
}

function ctxOpen() {
  const menu = $("ctx-menu");
  return !!menu && !menu.hidden;
}

function closeCtx() {
  const menu = $("ctx-menu");
  if (!menu) return;
  if (menu.dataset.owner === "drawer-files") $("files-audio-mode").setAttribute("aria-expanded", "false");
  menu.hidden = true;
  if(cloudContextMenu?.menu===menu)cloudContextMenu=null;
  delete menu.dataset.owner;
}

/// 设置页右键：只有一个「复制」，把选中的文字送进剪贴板。
/// 用 execCommand 而不是 navigator.clipboard：后者在 WebView 里要额外授权，
/// 这一段是用户主动点的，execCommand 够用也稳。
function openCopyCtx(x, y) {
  const menu = ctxEl();
  const sel = (window.getSelection()?.toString() || "").trim();
  menu.innerHTML = "";
  const b = document.createElement("button");
  b.className = "ctx-item";
  b.textContent = "复制";
  b.disabled = !sel;
  b.addEventListener("click", () => {
    closeCtx();
    try { document.execCommand("copy"); } catch (_) {}
  });
  menu.appendChild(b);
  menu.hidden = false;
  menu.style.left = x + "px";
  menu.style.top = y + "px";
  const r = menu.getBoundingClientRect();
  if (r.right > window.innerWidth) menu.style.left = Math.max(0, x - r.width) + "px";
  if (r.bottom > window.innerHeight) menu.style.top = Math.max(0, y - r.height) + "px";
}

/// 任务行右键：按状态出菜单项（对齐 Mac 的 ItemMenu），钉在鼠标位置。
function openCtx(x, y, id) {
  const menu = ctxEl();
  const t = state.tasks.find((q) => q.id === id);
  if (!t) return;
  menu.innerHTML = "";
  const item = (act, label, danger) => {
    const b = document.createElement("button");
    b.className = "ctx-item" + (danger ? " danger" : "");
    if (act === "fetchSubs" || act === "retrySubs") b.classList.add("warning");
    setMenuLabel(b, label, MENU_ICONS[act]);
    b.addEventListener("click", () => {
      closeCtx();
      taskAction(id, act);
    });
    menu.appendChild(b);
    return b;
  };
  const sep = () => {
    const d = document.createElement("div");
    d.className = "ctx-sep";
    menu.appendChild(d);
  };

  const picked0 = multiFor(t.id);
  if (isGroup(t.id)) {
    if (groupMembers(t.id).some(task => !task.missing && task.fileState !== "offline")) {
      item("folder", "打开文件夹");
    }
  } else {
    if (t.status === "done" && !t.missing) item("open", "打开文件");
    if (picked0.length > 1) item("reveal", "打开所在文件夹");
  }
  // 原始链接：http(s) 直接开浏览器，磁力交给装了关联的 BT 客户端
  const baiduJob = String(t.id).startsWith("bd:");
  if (taskSourceLink(t)) item("openLink", baiduJob ? "打开原分享" : "打开链接");
  if (baiduJob && !isOwnBaiduShare(t) && ["pause", "error"].includes(t.status)) {
    item("baiduCode", "填写提取码");
  }
  if (t.status!=="done" && canCloudRefresh(t)) item("refreshLinks", "重新获取地址并续传");
  // 多选时菜单作用于所有选中项：看里面有哪些状态，文字带上适用的数量
  const picked = multiFor(t.id);
  const many = picked.length > 1;
  const nDone = picked.filter((x) => x.status === "done" && !String(x.id).startsWith("xl:")
    && (!many || !String(x.id).startsWith("bd:")) && x.meta?.req?.url).length;
  const nStopped = picked.filter((x) => x.status === "pause" || x.status === "error").length;
  const nActive = picked.filter((x) => ACTIVE.has(x.status)).length;
  if (nDone) {
    if (many) item("redl", `重新下载（${nDone} 项）`);
    // 关了新建确认就没地方换清晰度、保存位置了，这里给个单次入口
    if (!many && !t.collectionFiles && !state.settings?.settings.confirmNew) item("redlopt", "重新下载（选择选项）");
  }
  if (many) {
    if (nStopped) item("resume", `继续（${nStopped} 项）`);
    if (nActive) item("pause", `暂停（${nActive} 项）`);
  }
  // 换内核后进度对不上、或卡在最后一点时用。内核只有运行中的 BT 任务才能校验
  if (t.status === "running" && isBt(t) && !String(t.id).startsWith("xl:") && t.shandianVerification !== "checking") item("verify", "重新检查已下载文件");
  const thunderTargets = picked.filter(canThunder);
  if (thunderInfo.installed && thunderTargets.length) {
    const label = thunderTargets.every(isBt) ? "交给迅雷（推荐）" : "交给迅雷";
    item("thunder", many ? `${label} · ${thunderTargets.length} 项` : label);
  }
  if (canCloudOffline(t)) attachCloudSubmenu(menu,t);
  // 迅雷任务转回闪电本机。配了 NAS 时上面的互转已经给了这一项
  if (!many && String(t.id).startsWith("xl:") && t.status !== "done" && !window.__nasUsable?.()) item("xlToLocal", "转到本机");
  sep();
  // 有字幕文件才给润色；没下到或字幕丢了，只给「获取字幕」
  if (!many && t.status === "done" && !t.missing && String(t.id).startsWith("v:") && t.subs) {
    item("polish", t.polished ? "重新润色字幕" : "润色字幕");
    if (t.polished) item("polishUndo", "恢复原字幕");
  }
  if (!many && t.subsFetchable) item("fetchSubs", subtitleState(t).checked ? "重查字幕" : "获取字幕");
  // 多选时一次改所有带字幕的视频
  const polishTargets = t.collectionFiles ? picked.filter(canPolishTask) : [];
  if (polishTargets.length) {
    const polish = item("polish", `润色字幕（${polishTargets.length}）`);
    polish.replaceWith(polish.cloneNode(true));
    menu.lastElementChild.addEventListener("click", () => { closeCtx(); polishCollection(polishTargets); });
    const restoreTargets = polishTargets.filter(task => task.polished);
    if (restoreTargets.length) {
      const restore = item("polishUndo", `恢复原字幕（${restoreTargets.length}）`);
      restore.replaceWith(restore.cloneNode(true));
      menu.lastElementChild.addEventListener("click", () => { closeCtx(); polishCollection(restoreTargets, true); });
    }
  }
  const styleIds = picked.filter(canSubStyle).map((x) => x.id);
  const collectionSelection = t.collectionFiles || picked.some(task => collectionForEpisode(task.id));
  if (styleIds.length) {
    const b = item("subStyle", collectionSelection ? `字幕样式…（${styleIds.length} 集有字幕）` : styleIds.length > 1 ? `字幕样式…（${styleIds.length} 项）` : "字幕样式…");
    b.replaceWith(b.cloneNode(true));
    menu.lastElementChild.addEventListener("click", () => { closeCtx(); openSubStyle(styleIds); });
  }
  if (collectionSelection) {
    const missing = missingSubtitleTasks(picked).filter(task => !isFetchingTaskSubtitles(task));
    if (missing.length) {
      const b = item("fetchSubs", `补字幕（${missing.length} 集缺字幕）`);
      b.replaceWith(b.cloneNode(true));
      menu.lastElementChild.addEventListener("click", () => { closeCtx(); fetchSubtitlesBatch(missing); });
    }
  }
  const roots = copyTaskRoots(id);
  const count = size => size > 1 ? "（" + size + " 项）" : "";
  const urls = new Set(roots.map(taskSourceLink).filter(Boolean));
  item("copyName", "复制名称" + count(roots.length));
  if (urls.size) item("copyLink", "复制链接" + count(urls.size));
  item("copyPath", "复制完整路径" + count(roots.length));
  // 文案按状态区分：下载中 = 取消下载；已完成 = 删除（进下载历史）
  const countText = many ? `（${picked.length} 项）` : "";
  if (many && nActive && nActive + nStopped < picked.length) item("del", `移除${countText}`);
  else if (many && t.status === "done") item("del", `删除${countText}`);
  else if (many) item("del", `取消下载${countText}`);
  item("purge", `删除任务和文件${countText}`, true); // 文件进系统回收站，danger 红

  menu.hidden = false;
  menu.style.left = x + "px";
  menu.style.top = y + "px";
  // 贴着屏幕右缘/下缘弹的时候别被裁出去
  const r = menu.getBoundingClientRect();
  if (r.right > window.innerWidth) menu.style.left = Math.max(0, x - r.width) + "px";
  if (r.bottom > window.innerHeight) menu.style.top = Math.max(0, y - r.height) + "px";


}

// ---------- 事件绑定 ----------

document.querySelectorAll("[data-view]").forEach((b) => {
  b.addEventListener("click", () => switchView(b.dataset.view));
});

document.querySelectorAll("[data-pick]").forEach((b) => {
  b.addEventListener("click", () => pickDir(b.dataset.pick));
});

document.querySelectorAll("[data-reset]").forEach((b) => {
  b.addEventListener("click", () => resetDir(b.dataset.reset));
});

document.querySelectorAll("[data-open]").forEach((b) => {
  b.addEventListener("click", async () => {
    const p = state.settings?.[b.dataset.open]?.path;
    if (!p) return;
    try { await invoke("open_dir", { path: p }); } catch (e) { toast("打不开：" + e); }
  });
});

document.querySelectorAll("[data-theme-set]").forEach((b) => {
  b.addEventListener("click", () => {
    const v = b.dataset.themeSet;
    setTheme(v); // 先变，别等磁盘写完才刷新
    if (!state.settings) return;
    state.settings.settings.theme = v;
    saveSettings();
  });
});

document.querySelectorAll("[data-install]").forEach((b) => {
  b.addEventListener("click", () => installTool(b.dataset.install));
});

$("set-max").addEventListener("change", async (e) => {
  state.settings.settings.maxRunning = Number(e.target.value);
  await saveSettings();
  startWaiting();
});

/// 内核只在有任务结束时才从等待队列里取，调大同时下载数不会触发，所以这里按空位补上。
/// 一条一条来：没有空位时再「继续」，内核会暂停正在下载的任务来腾位置
async function startWaiting() {
  const mine = state.tasks.filter((t) => !/^(v:|nas:)/.test(String(t.id)));
  let free = state.settings.settings.maxRunning - mine.filter((t) => t.status === "running").length;
  for (const t of mine.filter((x) => x.status === "wait")) {
    if (free-- <= 0) break;
    await invoke("resume_task", { id: t.id }).catch(() => {});
  }
  refreshTasks();
}

$("set-max-videos").addEventListener("change", (e) => {
  state.settings.settings.maxVideos = Number(e.target.value);
  saveSettings();
});
$("set-seed-ratio").addEventListener("change", (e) => {
  state.settings.settings.seedRatio = Number(e.target.value);
  saveSettings();
});
$("set-seed-time").addEventListener("change", (e) => {
  state.settings.settings.seedTime = Number(e.target.value);
  saveSettings();
});

$("set-limit").addEventListener("change", (e) => {
  state.settings.settings.downloadLimit = Number(e.target.value);
  saveSettings();
});

// 系统卡片：每点一下立刻存盘，不等用户去点「保存」
function bindSwitch(id, key) {
  $(id)?.addEventListener("click", () => {
    if (!state.settings) return;
    const on = !state.settings.settings[key];
    state.settings.settings[key] = on;
    setSwitch(id, on);
    saveSettings();
  });
}

bindSwitch("sw-notify", "notifyOnFinish");
bindSwitch("sw-sleep", "preventSleep");
bindSwitch("sw-clip", "watchClipboard");
bindSwitch("sw-ghmirror", "githubMirror");
$("set-relay")?.addEventListener("change", (e) => {
  if (!state.settings) return;
  state.settings.settings.relay = e.target.value;
  saveSettings();
});
bindSwitch("sw-confirm", "confirmNew");
// 删除下载记录前确认：存在本机界面里，和删除时的「不再询问」是同一个开关
setSwitch("sw-history-confirm", localStorage.getItem(SKIP_HISTORY_DELETE) !== "1");
$("sw-history-confirm")?.addEventListener("click", () => {
  const on = localStorage.getItem(SKIP_HISTORY_DELETE) === "1";
  if (on) localStorage.removeItem(SKIP_HISTORY_DELETE);
  else localStorage.setItem(SKIP_HISTORY_DELETE, "1");
  setSwitch("sw-history-confirm", on);
});
bindSwitch("sw-polish", "subtitlePolish");
$("set-polish-mode")?.addEventListener("change", (e) => {
  if (!state.settings) return;
  state.settings.settings.polishMode = e.target.value;
  saveSettings();
});
$("set-polish-model")?.addEventListener("change", (e) => {
  if (!state.settings) return;
  state.settings.settings.polishModel = e.target.value;
  saveSettings();
});

// 开机启动不在 settings.json 里，状态以注册表为准，所以单独问后端
/// 设置页的「浏览器扩展」：扩展有没有连上来、扩展文件夹在哪。
/// 扩展每 10 分钟 ping 一次，所以这里 10 秒刷一次就够（进设置页时立刻刷一次）。
async function refreshExt() {
  if (!invoke) return;
  try {
    const st = await invoke("extension_status");
    const box = $("ext-state");
    if (!box) return;
    if (st.connected) {
      box.textContent = `已连接：${st.browser || "浏览器"}`;
      box.classList.add("ok");
    } else if (st.lastSeen) {
      // 扩展是被动上报的：它只在浏览器发起下载时才联系本程序，
      // 所以「装好了但没用过」也会显示成未连接，把这件事写在状态里免得误判
      box.textContent = `上次活动：${new Date(st.lastSeen).toLocaleString("zh-CN", { hour12: false })}`;
      box.classList.remove("ok");
    } else {
      box.textContent = "未激活（用浏览器下载一次即生效）";
      box.classList.remove("ok");
    }
  } catch (_) {}
  try {
    const dir = await invoke("extension_dir");
    const el = $("ext-dir");
    if (el) {
      el.textContent = dir || "—";
      el.title = dir || "";
    }
  } catch (_) {}
}

async function loadAutostart() {
  if (!invoke) return;
  try { setSwitch("sw-autostart", await invoke("autostart_status")); } catch (e) { /* 读不到就当没开 */ }
}

$("sw-autostart")?.addEventListener("click", async () => {
  const el = $("sw-autostart");
  const on = !el.classList.contains("on");
  try {
    await invoke("set_autostart", { enabled: on });
    setSwitch("sw-autostart", on);
    toast(on ? "已设置为开机自动启动" : "已取消开机自动启动");
  } catch (e) {
    toast("设置失败：" + e);
  }
});

// 剪贴板里出现能下的链接：直接填进输入框，用户按回车就投
if (!DIALOG && window.__TAURI__?.event?.listen) {
  let conflictQueue = Promise.resolve();
  // 创建任务前查到同名：任务还没建，选完才建。关闭 = 不下
  // 右下角卡片点了重新登录
  window.__TAURI__.event.listen("collection-notice-open", ({ payload }) => {
    if (!state.tasks.some(task => task.id === payload.id)) return;
    if (payload.action === "open") {
      revealTask(payload.id);
      openFilesPanel(payload.id);
    } else {
      openGroupFolder(groupMembers(payload.id));
    }
  });
  window.__TAURI__.event.listen("cloud-account-relink", ({payload}) => openCloudAccounts(payload.target==="nas"?"nas:":"", null, payload.provider));
  // 多文件任务：整组只问一次
  window.__TAURI__.event.listen("multi-conflict", ({ payload: { token, name, dir, total, existing } }) => {
    conflictQueue = conflictQueue.then(async () => {
      const action = await askChoice({
        title: "保存位置已有部分文件",
        desc: `「${name}」的 ${total} 个文件里，有 ${existing} 个已在 ${dir} 里。`,
        buttons: [
          { label: "加存下载", value: "saveas" },
          { label: "重新下载", value: "recycle" },
          { label: existing === total ? "全部跳过" : "跳过已有文件", value: "skip", primary: true },
        ],
      });
      const settle = multiPendingSettle;
      multiPendingSettle = null;
      await invoke("multi_conflict", { token, action: action || "cancel" }).catch((e) => toast(String(e)));
      if (action === "recycle" || action === "saveas") await settleMulti(settle);
      refreshTasks();
    });
  });
  window.__TAURI__.event.listen("name-conflict-new", ({ payload: { token, name, dir, bt, complete, url } }) => {
    conflictQueue = conflictQueue.then(async () => {
      const action = await askChoice({
        title: complete ? "这个种子已经下载完成" : "保存位置已有同名内容",
        desc: `「${name}」已在 ${dir} 里。`,
        buttons: complete ? [
          { label: "加存下载", value: "saveas" },
          { label: "重新下载", value: "recycle" },
          { label: "打开文件夹", value: "folder", primary: true },
        ] : bt ? [
          { label: "加存下载", value: "saveas" },
          { label: "重新下载", value: "recycle" },
          { label: "继续下载", value: "verify", primary: true },
        ] : [
          { label: "重新下载", value: "recycle" },
          { label: "加存下载", value: "saveas", primary: true },
        ],
      });
      const xl = xlWaiting.get(url);
      xlWaiting.delete(url);
      const moved = xl && action && action !== "cancel" && action !== "folder";
      if (moved) await invoke("forget_task", { id: xl.id }).catch(() => {});
      await invoke("name_conflict_new", { token, action: action || "cancel" }).catch((e) => toast(String(e)));
      if (moved) await xl.spot();
      refreshTasks();
      loadTrash();
    });
  });
  // BT 任务元数据到了、发现目录里有同名内容：后端已暂停任务，一条条问
  window.__TAURI__.event.listen("name-conflict", ({ payload: { id, name, dir, bt } }) => {
    conflictQueue = conflictQueue.then(async () => {
      const action = await askChoice({
        title: "保存位置已有同名内容",
        desc: `「${name}」已在 ${dir} 里。`,
        // 右上角关闭 = 取消这条任务
        buttons: bt ? [
          { label: "加存下载", value: "saveas" },
          { label: "重新下载", value: "recycle" },
          { label: "继续下载", value: "verify", primary: true },
        ] : [
          { label: "重新下载", value: "recycle" },
          { label: "加存下载", value: "saveas", primary: true },
        ],
      });
      await invoke("name_conflict_resolve", { id, action: action || "cancel", name, bt }).catch((e) => toast(String(e)));
      refreshTasks();
    });
  });
  window.__TAURI__.event.listen("wechat-share-detected", ({ payload }) => {
    const url = String(payload || "");
    if (WECHAT_SHARE.test(url)) wechatGuide(url);
  });
  window.__TAURI__.event.listen("clipboard-link", (ev) => {
    const link = String(ev.payload || "");
    if (link && document.hasFocus()) {
      toast("剪贴板里有下载链接", { action: { label: "新建下载", run: () => openNewSheet(link) } });
    }
  });

  // 最大化时 Rust 会通知一声：圆角要切成方角，不然屏幕四角会漏出桌面
  window.__TAURI__.event.listen("win-max", (ev) => {
    document.body.classList.toggle("maxed", ev.payload === true);
  });

  // 托盘菜单里点了暂停／继续，或者完成通知弹出来之后，界面得跟着刷新，
  // 否则用户从托盘回来会看到进度卡住不动。
  window.__TAURI__.event.listen("tasks-changed", () => {
    refreshTasks();
  });

  // 剪贴板浮窗点了「下载」：和粘贴框同一条路（开着确认就弹选项，字幕画质都在里面选），
  // 别再直接 invoke add_task——那样永远看不到字幕/画质选项
  window.__TAURI__.event.listen("float-download", async (ev) => {
    const links = Array.isArray(ev.payload) ? ev.payload : [];
    if (!links.length) return;
    pendingLinks = links.map((url) => ({ url, opts: {} }));
    nextPending();
  });

  // 浏览器扩展投来的任务：跟粘贴链接同一条路（确认框或直接投），
  // headers 是网盘直链的鉴权头，name 是扩展解析出的文件名，都要带下去
  // 交给迅雷后迅雷那边没接住（比如在迅雷里取消了新建）：撤回，原任务回到这里接着下
  window.__TAURI__.event.listen("thunder-abandoned", ({ payload }) => {
    const back = payload?.back ? "，改回由闪电全能下载继续下载" : "";
    if (payload?.duplicate) thunderDuplicateNotice(`迅雷已有这条任务${back}`);
    else toast(payload?.cancelled ? `已取消交给迅雷${back}` : `迅雷没有接收「${payload?.name || "任务"}」${back}`);
    refreshTasks();
  });
  // 迅雷的新建窗口没能自动确认：窗口已放回屏幕最前，请用户自己点
  window.__TAURI__.event.listen("thunder-stuck", ({ payload }) => {
    toast(`迅雷的新建窗口在等你确认「${payload?.name || "任务"}」，请在迅雷里点「立即下载」或取消`);
  });
  window.__TAURI__.event.listen("pikpak-linked", () => {
    toast("本机 PikPak 已关联");
    loadPikpak();
  });
  window.__TAURI__.event.listen("mcp-add", ({ payload }) => {
    if (!payload?.url) return;
    submitLink(payload.url, { confirm: true, mcpTarget: payload.target }).catch(error => toast(String(error)));
  });

  window.__TAURI__.event.listen("ext-add", (ev) => {
    const p = ev.payload || {};
    const url = String(p.url || "").trim();
    if (!url) return;
    if (p.collection) return submitLink(url, { collection: true });
    if (BAIDU_SHARE.test(url)) return submitLink(url, {
      baiduCodeRequired: p.baiduCodeRequired === true,
      baiduSelection: p.baiduSelection,
      name: p.name || "",
    });
    if (isFeishuDocument(url)) return submitLink(url);
    // 网盘助手推文件夹是一个文件一次请求：攒一小会儿，合成一个确认框
    const size = Number.isSafeInteger(p.sizeBytes) && p.sizeBytes >= 0 ? p.sizeBytes : undefined;
    const sizeLabel = typeof p.sizeLabel === "string" && /^\d+(?:\.\d+)? [KMGT]?B$/.test(p.sizeLabel) ? p.sizeLabel : "";
    extBuffer.push({ url, name: p.name || "", headers: Array.isArray(p.headers) ? p.headers : null, size, sizeLabel });
    clearTimeout(extTimer);
    if (p.group) extTimer = setTimeout(flushExtBuffer, 1500);
    else flushExtBuffer();
  });

  window.__TAURI__.event.listen("ext-grab", ({ payload }) => openGrab(payload?.links || [], payload || {}));

  // 更新进度由后端推过来：下载带百分比，安装不带
  window.__TAURI__.event.listen("update-progress", ({ payload }) => {
    if (!payload || payload.version !== availableUpdate?.version) return;
    if (payload.phase === "retry" || payload.phase === "failed") {
      downloadingUpdate = false;
      showUpdateState(payload.message || "安装已关闭，可重试");
      return;
    }
    downloadingUpdate = true;
    showUpdateState(payload.phase === "installing" ? "正在安装…" :
      payload.phase === "verifying" ? "正在校验…" : "下载 " + Math.round(payload.percent || 0) + "%");
  });
}

$("player-close").addEventListener("click", closePlayer);
// 点遮罩空白处也关，和确认框一致
$("player").addEventListener("click", (e) => { if (e.target.id === "player") closePlayer(); });

$("engine-btn").addEventListener("click", (e) => { e.stopPropagation(); toggleEngine(); });
// 侧栏那行提示（只有异常时可见）：点整行直接重启
document.querySelector(".engine")?.addEventListener("click", () => {
  if (!state.engineRunning && !state.busy) toggleEngine();
});
// 设置 → 关于 → 重启内核
$("btn-engine-restart")?.addEventListener("click", async () => {
  if (!invoke || state.busy) return;
  try {
    await invoke("engine_stop");
  } catch (e) {}
  state.engineRunning = false;
  await toggleEngine();
});
$("pause-all").addEventListener("click", pauseAll);
$("start-all").addEventListener("click", startAll);
// 新建下载：弹居中弹窗贴链接（老做法焦点从顶栏跳到底部输入框，很怪）
$("new-btn").addEventListener("click", openNewSheet);
// 关于信息在设置页底部：版本号、内核状态与端口、数据目录
document.querySelector("[data-open-engine]")?.addEventListener("click", async () => {
  if (!state.engine?.data_dir) return;
  try { await invoke("open_dir", { path: state.engine.data_dir }); } catch (e) { toast("打不开：" + e); }
});
document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "b") {
    e.preventDefault();
    $("win-max")?.click(); // 最大化 / 恢复
  }
  if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "m") {
    e.preventDefault();
    $("win-min")?.click(); // 最小化
  }
  // Ctrl+Q 彻底退出（关窗口只是收进托盘）
  if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "q") {
    e.preventDefault();
    invoke("quit_app").catch(() => {});
  }
});
document.addEventListener("paste", (e) => {
  if (DIALOG || e.defaultPrevented || e.target.closest?.("input, textarea, [contenteditable], .modal-mask")) return;
  if (state.view === "settings" || document.querySelector(".modal-mask:not([hidden])")) return;
  const text = e.clipboardData?.getData("text/plain") || "";
  if (!linksWithBaiduCode(text).length) return;
  e.preventDefault();
  openNewSheet(text);
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    if (closeCustomSelects()) { e.preventDefault(); e.stopImmediatePropagation(); return; }
    if (closeChoice) return closeChoice(null);
    if (ctxOpen()) return closeCtx();
    if (!$("player").hidden) return closePlayer();
    const sheet = $("new-sheet");
    if (sheet && !sheet.hidden) return closeNewSheet();
    if (!$("rm-confirm").hidden) return closeRemove(false);
    if (!$("confirm").hidden) closeConfirm();
    return;
  }
  if (isSelectAllShortcut(e) && !$("pane-downloads").hidden && !ctxOpen() &&
      !document.querySelector(".modal-mask:not([hidden]), .player-mask:not([hidden])")) {
    if (filesPanelId && filesList.length) {
      e.preventDefault();
      setDrawerSelection(true);
      drawerAnchor ??= drawerFileKey(filesList[0]);
    } else if (!e.target.closest?.("#files-panel") && selectAllVisibleTasks()) e.preventDefault();
    return;
  }
  // 方向键和单条任务的快捷键只在任务行有焦点时处理。
  const row = e.target.closest?.("#task-list .task");
  if (!row || e.target !== row) return;
  const rows = [...$("task-list").querySelectorAll(".task")];
  const i = rows.indexOf(row);
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault(); // 别跟着方向键滚页面
    const next = rows[e.key === "ArrowDown" ? Math.min(i + 1, rows.length - 1) : Math.max(i - 1, 0)];
    if (next) {
      selectRow(next.dataset.id);
      next.focus();
    }
  } else if (e.key === " " || e.key === "Enter") {
    e.preventDefault(); // 空格别把页面滚下去
    primaryAction(row.dataset.id);
  } else if (e.key === "Delete") {
    e.preventDefault();
    taskAction(row.dataset.id, "del"); // 走行内按钮那套移除流程
  }
});

$("search").addEventListener("input", (e) => {
  if (state.view === "settings") return filterSettings(e.target.value);
  state.query = e.target.value;
  renderTasks();
});

// 搜索历史：样式和新建下载的保存目录下拉一致，悬停露出 × 删除单条。
// 下载列表和设置页各记各的，存在本机界面，最多 20 条
(() => {
  const inp = $("search");
  const box = inp.closest(".search");
  const menu = document.createElement("div");
  menu.id = "search-history";
  menu.className = "ctx-menu cf-dir-menu";
  menu.hidden = true;
  menu.style.position = "fixed";
  document.body.appendChild(menu);
  const key = () => (state.view === "settings" ? "sd.searchHistory.settings" : "sd.searchHistory");
  const load = () => {
    try { return JSON.parse(localStorage.getItem(key()) || "[]"); } catch { return []; }
  };
  const save = (list) => localStorage.setItem(key(), JSON.stringify(list.slice(0, 20)));
  const remember = () => {
    const q = inp.value.trim();
    if (q) save([q, ...load().filter((x) => x !== q)]);
  };
  const close = () => { menu.hidden = true; };
  let typing = false;
  function draw() {
    // 点开时列全部（框里留着上次的词也一样）；正在输入时只列相关的
    const q = typing ? inp.value.trim().toLowerCase() : "";
    const list = load().filter((x) => x.toLowerCase().includes(q) && !(typing && x.toLowerCase() === q));
    if (!list.length) return close();
    menu.innerHTML = "";
    for (const text of list) {
      const row = document.createElement("div");
      row.className = "cf-row";
      const b = document.createElement("button");
      b.className = "ctx-item";
      b.textContent = text;
      b.title = text;
      // mousedown 里阻止默认：焦点留在输入框，不触发失焦
      b.addEventListener("mousedown", (ev) => ev.preventDefault());
      b.addEventListener("click", () => {
        inp.value = text;
        inp.dispatchEvent(new Event("input", { bubbles: true }));
        remember();
        close();
      });
      const del = document.createElement("button");
      del.className = "cf-del";
      del.title = "删除这条搜索历史";
      del.innerHTML = '<svg class="ic"><use href="#i-x"/></svg>';
      del.addEventListener("mousedown", (ev) => ev.preventDefault());
      del.addEventListener("click", (ev) => {
        ev.stopPropagation();
        save(load().filter((x) => x !== text));
        draw();
      });
      row.append(b, del);
      menu.appendChild(row);
    }
    const r = box.getBoundingClientRect();
    menu.style.left = `${r.left}px`;
    menu.style.top = `${r.bottom + 4}px`;
    // 和搜索框同宽
    menu.style.boxSizing = "border-box";
    menu.style.minWidth = "0";
    menu.style.maxWidth = "none";
    menu.style.width = `${r.width}px`;
    menu.style.right = "auto";
    menu.hidden = false;
  }
  inp.addEventListener("focus", () => { typing = false; draw(); });
  inp.addEventListener("click", () => { typing = false; draw(); });
  inp.addEventListener("input", () => { typing = true; draw(); });
  inp.addEventListener("blur", () => { remember(); close(); });
  inp.addEventListener("keydown", (e) => {
    if (e.key === "Enter") { remember(); close(); }
    else if (e.key === "Escape") close();
  });
  // 点顶栏空白（窗口拖动区）时输入框不会失焦，只靠 blur 收不起来：在别处按下就收
  document.addEventListener("pointerdown", (e) => {
    if (menu.hidden || menu.contains(e.target) || box.contains(e.target)) return;
    close();
    inp.blur();
  }, true);
})();

$("task-list").addEventListener("click", (e) => {
  // 点空白：取消选中（和 Finder/资源管理器一致）
  const row = e.target.closest(".task");
  if (!row) {
    selectRow(null);
    return;
  }
  // 多选：Ctrl/⌘ 切换、Shift 范围；普通点击只选这一条
  const add = e.ctrlKey || e.metaKey;
  const range = e.shiftKey;
  const btn = e.target.closest("[data-act]");
  // 点已选中多条里某一行的按钮：操作的是这一批，不能先把选择缩成一条
  // 下载历史的按钮是 data-trash-act，同样要保留选择
  const keep = (btn || e.target.closest("[data-trash-act]")) && !add && !range && state.selIds.size > 1 && state.selIds.has(row.dataset.id);
  if (!keep) selectRow(row.dataset.id, { add, range });
  if (btn) taskAction(row.dataset.id, btn.dataset.act);
  // 回收站行上的按钮
  const tAct = e.target.closest("[data-trash-act]");
  if (tAct) trashAction(row.dataset.trashId || row.dataset.id, tAct.dataset.trashAct);
});

// 列表下方/侧边空白也在 .scroll 上：点到没行的地方取消选中
document.querySelector("#pane-downloads .scroll")?.addEventListener("click", (e) => {
  if (!e.target.closest(".task")) selectRow(null);
});

// 双击行 = 主操作：下完打开、失败重试、下载中暂停/继续（对齐 Mac）
// 只有落在「可选中文字」的区域才让给浏览器（双击选词、三击选行）；
// 缩略图、进度条、行空白处一律当主操作——不要用「当前有无选区」来拦，
// 那样之前选过一次文字后，双击会一直不响应
$("task-list").addEventListener("dblclick", (e) => {
  if (e.target.closest("[data-act], [data-subtitle-action]")) return; // 双击按钮算连点按钮，别再触发行为主操作
  const row = e.target.closest(".task");
  if (!row) return;
  // 下载历史：文件还在原位置时双击 = 打开文件（和已完成一致）
  if (row.dataset.trashId) {
    const r = state.trash.find((x) => x.id === row.dataset.trashId);
    if (canOpenHistory(r)) openHistoryLocation(r, true);
    return;
  }
  primaryAction(row.dataset.id);
});

/// 下载历史的右键菜单：和任务行共用同一个容器，动作走 trashAction
function openTrashCtx(x, y, id) {
  const r = state.trash.find((q) => q.id === id);
  if (!r) return;
  const menu = ctxEl();
  menu.innerHTML = "";
  const item = (label, fn, danger) => {
    const b = document.createElement("button");
    b.className = "ctx-item" + (danger ? " danger" : "");
    setMenuLabel(b, label, HISTORY_MENU_ICONS[label]);
    b.addEventListener("click", () => {
      closeCtx();
      fn();
    });
    menu.appendChild(b);
  };
  const sep = () => {
    const d = document.createElement("div");
    d.className = "ctx-sep";
    menu.appendChild(d);
  };
  const copy = async (text) => {
    try {
      if (!(await copyText(text))) throw new Error("复制失败");
      toast("已复制");
    } catch (e) {
      toast("复制失败");
    }
  };

  if (r.link) item("重新下载", () => trashAction(id, "redl"));
  if (canOpenHistory(r)) item("打开所在文件夹", () => trashAction(id, "open"));
  if (r.link) item("打开链接", () => invoke("open_url", { url: r.link }).catch((e) => toast(String(e))));
  sep();
  if (r.link) item("复制链接", () => copy(r.link));
  if (r.path) item("复制文件路径", () => copy(r.path));
  sep();
  item("删除记录", () => trashAction(id, "del"), true);

  menu.hidden = false;
  menu.style.left = `${x}px`;
  menu.style.top = `${y}px`;
  const box = menu.getBoundingClientRect();
  if (box.right > window.innerWidth) menu.style.left = `${Math.max(0, x - box.width)}px`;
  if (box.bottom > window.innerHeight) menu.style.top = `${Math.max(0, y - box.height)}px`;
}

// 右键任务行：弹自己的菜单（WebView2 自带的共享/打印那个在下面全局禁掉了）
$("task-list").addEventListener("contextmenu", (e) => {
  const row = e.target.closest(".task");
  if (!row) return;
  e.preventDefault();
  // 下载历史的行没有 data-id，只有 data-trash-id，以前落到 openCtx 就什么都不弹
  if (row.dataset.trashId) {
    openTrashCtx(e.clientX, e.clientY, row.dataset.trashId);
    return;
  }
  // 右键点在已选中的那批里：保留多选，菜单作用于这一批
  if (!state.selIds.has(row.dataset.id)) selectRow(row.dataset.id);
  openCtx(e.clientX, e.clientY, row.dataset.id);
});

// 干掉 WebView2 默认右键菜单（共享/打印那些用不上）；输入框里放行，复制粘贴还要用
document.addEventListener("contextmenu", (e) => {
  const tag = e.target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || e.target.isContentEditable) return;
  // 设置页只给「复制」这一项：WebView2 原生菜单里的剪切、粘贴、全选在这儿都用不上，
  // 整片弹出来反而挡事
  if (e.target.closest(".pane.settings")) {
    e.preventDefault();
    openCopyCtx(e.clientX, e.clientY);
    return;
  }
  e.preventDefault();
  // 右键点在任务行之外：把开着的菜单收掉（行里的右键在上面已经处理过，别误关）
  if (!e.target.closest("#task-list .task")) closeCtx();
});

// 菜单收摊：点别处、滚动都关（Esc 在 keydown 里处理）
// 用捕获阶段的 pointerdown：窗口拖动区域会吃掉 mousedown，按在顶栏上菜单就收不回去
document.addEventListener("pointerdown", (e) => {
  if (!e.target.closest("#ctx-menu")) closeCtx();
}, true);
window.addEventListener("scroll", closeCtx, { capture: true, passive: true });

// 其余输入框统一挂「清空」：包一层 relative 容器，按钮绝对定位在输入框内右侧
function attachClear(inp) {
  if (!inp || inp.closest(".clear-wrap")) return;
  const wrap = document.createElement("span");
  wrap.className = "clear-wrap";
  wrap.style.display = "flex";
  wrap.style.flex = "1";
  wrap.style.minWidth = "0";
  wrap.style.position = "relative";
  wrap.style.alignItems = "center";
  inp.parentNode.insertBefore(wrap, inp);
  wrap.appendChild(inp);
  inp.style.flex = "1";
  inp.style.minWidth = "0";
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "input-clear";
  // 不要 title/data-tip：清空意图明显，提示反而是噪音（用户反馈指向还反）
  btn.innerHTML = '<svg class="ic"><use href="#i-x"/></svg>';
  btn.hidden = true;
  wrap.appendChild(btn);
  const sync = () => { btn.hidden = !inp.value; };
  inp.addEventListener("input", sync);
  sync();
  btn.addEventListener("click", () => {
    inp.value = "";
    sync();
    inp.dispatchEvent(new Event("input", { bubbles: true }));
    inp.focus();
  });
}

// 搜索框：HTML 里已放好 #search-clear，这里只挂事件
(() => {
  const inp = $("search");
  const btn = $("search-clear");
  if (!inp || !btn) return;
  const sync = () => { btn.hidden = !inp.value; };
  inp.addEventListener("input", sync);
  sync();
  btn.addEventListener("click", () => {
    inp.value = "";
    sync();
    inp.dispatchEvent(new Event("input", { bubbles: true }));
    inp.focus();
  });
})();

for (const id of ["cf-name"]) {
  attachClear($(id));
}

// ---------- 收起 / 展开侧边栏（Mac 工具栏左上那个开关） ----------
const SB_KEY = "sd.sidebarCollapsed";

/// collapsed 状态只改一个 class + 记进 localStorage，下次开窗还是上次的样子
function applySidebar(collapsed) {
  // class 打在 body 上：开关按钮现在固定在页面层，不在 #app 里面
  document.body.classList.toggle("sb-collapsed", collapsed);
  const btn = $("sb-toggle");
  // 提示走 data-tip（自绘 tip），别写 title——title 会再弹一层系统白框，变成两个提示
  if (btn) btn.dataset.tip = collapsed ? "展开侧边栏（Tab）" : "收起侧边栏（Tab）";
  try {
    localStorage.setItem(SB_KEY, collapsed ? "1" : "0");
  } catch (e) {}
}

$("sb-toggle")?.addEventListener("click", () => {
  applySidebar(!document.body.classList.contains("sb-collapsed"));
});

/// 快捷键 Tab：收起 / 展开侧边栏（另外顺手认一下 Ctrl+B，习惯这个的人也能用）。
/// 光标在输入框里、或者开着右键菜单/取色面板时让路，否则打字时按 Tab 会莫名其妙收栏
document.addEventListener("keydown", (e) => {
  if (e.isComposing) return;
  // Ctrl+N / Ctrl+Shift+N 都走新建下载（和浏览器一致）
  const ctrlN = (e.ctrlKey || e.metaKey) && !e.altKey && (e.key === "n" || e.key === "N");
  if (ctrlN) {
    // 输入框里 Ctrl+N 有时是「新窗口」语义，这里仍打开新建弹窗（本应用没有多窗口）
    e.preventDefault();
    openNewSheet();
    return;
  }
  const inField = e.target.closest && e.target.closest("input, textarea, select, [contenteditable='true']");
  const plainTab = e.key === "Tab" && !e.ctrlKey && !e.altKey && !e.metaKey && !e.shiftKey;
  const ctrlB = (e.ctrlKey || e.metaKey) && !e.altKey && (e.key === "b" || e.key === "B");
  if (!(plainTab && !inField) && !ctrlB) return;
  // 有浮层（右键菜单 / 取色面板）时不抢 Tab
  if (ctxOpen() || ($("acc-pop") && !$("acc-pop").hidden)) return;
  e.preventDefault();
  applySidebar(!document.body.classList.contains("sb-collapsed"));
});
try {
  applySidebar(localStorage.getItem(SB_KEY) === "1");
} catch (e) {}

// ---------- 自绘窗口按钮（窗口已去掉系统标题栏） ----------
// Tauri v2 里取当前窗口有几种写法，逐个试；拿不到就什么都别做（不报错刷屏）
const winCur = (() => {
  const T = window.__TAURI__;
  try {
    return (
      T?.window?.getCurrentWindow?.() ||
      T?.window?.getCurrent?.() ||
      T?.webviewWindow?.getCurrentWebviewWindow?.() ||
      null
    );
  } catch (e) {
    return null;
  }
})();

/// 窗口动作统一走这里：失败要看得见（权限没给的时候以前是静默失败）
async function winDo(label, fn) {
  if (!winCur) return toast("窗口接口不可用");
  try {
    await fn();
  } catch (e) {
    toast(`${label}失败：${e}`);
  }
}

$("win-min")?.addEventListener("click", () => winDo("最小化", () => winCur.minimize()));
$("win-max")?.addEventListener("click", () =>
  winDo("最大化", async () => {
    await winCur.toggleMaximize();
    refreshMaxIcon();
  })
);
$("win-close")?.addEventListener("click", () => winDo("关闭", () => winCur.close()));

// 拖动兜底：data-tauri-drag-region 在某些子元素上不生效时，手动开始拖动。
// 双击时（e.detail > 1）不再开始拖动，把第二次点击让给 dblclick——那样才能最大化/还原
[".topbar"].forEach((sel) => {
  const el = document.querySelector(sel);
  if (!el) return;
  // 拖动交给原生 data-tauri-drag-region（权限已给），这里只补双击。
  // 之前用手动 startDragging 兜底，会把双击的第二下吞掉，双击最大化就失灵
  // 双击顶栏/侧栏空白 = 最大化 / 还原（和系统标题栏一致）
  el.addEventListener("dblclick", (e) => {
    if (e.target.closest("button, input, textarea, a, .nav-item, .win-ctrls")) return;
    winDo("最大化", async () => {
      await winCur.toggleMaximize();
      refreshMaxIcon();
    });
  });
});

/// 最大化后按钮图标换成「还原」，不然看不出当前状态
async function refreshMaxIcon() {
  try {
    const max = await winCur?.isMaximized();
    const use = $("win-max")?.querySelector("use");
    if (use) use.setAttribute("href", max ? "#i-win-restore" : "#i-win-max");
  } catch (e) {}
}
refreshMaxIcon();
winCur?.onResized?.(() => refreshMaxIcon());

// ---------- 强调色 ----------
const ACCENT_KEY = "sd.accent";

/// #rrggbb → 色相/饱和度（亮度由主题决定，不用存）
/// 深色主题里强调色固定 52% 亮度，靠这判断该配白字还是近黑字
function needsDarkText(h, s) {
  const l = 0.52;
  const c = (1 - Math.abs(2 * l - 1)) * (s / 100);
  const hp = h / 60;
  const x = c * (1 - Math.abs((hp % 2) - 1));
  let rgb = [0, 0, 0];
  if (hp < 1) rgb = [c, x, 0];
  else if (hp < 2) rgb = [x, c, 0];
  else if (hp < 3) rgb = [0, c, x];
  else if (hp < 4) rgb = [0, x, c];
  else if (hp < 5) rgb = [x, 0, c];
  else rgb = [c, 0, x];
  const m = l - c / 2;
  const lum = 0.2126 * (rgb[0] + m) + 0.7152 * (rgb[1] + m) + 0.0722 * (rgb[2] + m);
  return lum > 0.55;
}

/// 换色：预设只改 html 上的 data-accent；自定义则把 h/s 写成内联变量
function applyAccent(name, hs) {
  const root = document.documentElement;
  const dot = $("acc-custom-btn");
  if (name === "custom" && hs) {
    const { h, s } = hs;
    const onDark = needsDarkText(h, s) ? "#1a1400" : "#fff";
    root.style.setProperty("--accent-h", String(h));
    root.style.setProperty("--accent-s", `${s}%`);
    root.style.setProperty("--on-dark", onDark);
    root.dataset.accent = "custom";
    if (dot) dot.style.background = `hsl(${h} ${s}% 55%)`;
    try {
      localStorage.setItem(ACCENT_KEY, "custom");
      localStorage.setItem("sd.accentVars", JSON.stringify({ h: String(h), s: `${s}%`, onDark }));
    } catch (e) {}
  } else {
    root.style.removeProperty("--accent-h");
    root.style.removeProperty("--accent-s");
    root.style.removeProperty("--on-dark");
    root.dataset.accent = name || "amber";
    if (dot) dot.style.removeProperty("background");
    try {
      localStorage.setItem(ACCENT_KEY, name);
      localStorage.removeItem("sd.accentVars");
    } catch (e) {}
  }
  document.querySelectorAll("#accent-row .acc").forEach((b) => {
    const on = name === "custom" ? b.classList.contains("acc-custom") : b.dataset.accent === name;
    b.classList.toggle("on", on);
  });
}

// ---------- 自定义取色面板（左边 饱和度/亮度 方块，右边 竖直色相条） ----------
const PRESET_HS = {
  amber: [37, 82], blue: [212, 92], teal: [178, 62], green: [148, 60],
  purple: [266, 70], rose: [342, 76], indigo: [232, 62], graphite: [220, 8],
};
let accH = 37;   // 色相 0-360
let accS = 82;   // 饱和度 0-100
let accV = 92;   // 亮度 0-100
let accDragging = null; // "sv" | "hue" | null
/// 取色面板这次在给谁用：null = 设置页的强调色；函数 = 别处（字幕颜色）
let accTarget = null;
let accAnchor = null;
/// 用户手敲的色值：手敲的 #RRGGBB 要原样用，不能再经 HSB 转一圈
/// （整数取整会漂一两级，比如 #00FFAC 变 #00FFAA）。一动圆点就清掉
let accLockHex = "";

/// HSB → #RRGGBB（色区是「饱和度 × 亮度」，用 HSB 才对得上手指按下的位置）
function hsbToHex(h, sat, v) {
  const s = sat / 100;
  const val = v / 100;
  const c = val * s;
  const hp = h / 60;
  const x = c * (1 - Math.abs((hp % 2) - 1));
  let r = 0;
  let g = 0;
  let b = 0;
  if (hp < 1) [r, g, b] = [c, x, 0];
  else if (hp < 2) [r, g, b] = [x, c, 0];
  else if (hp < 3) [r, g, b] = [0, c, x];
  else if (hp < 4) [r, g, b] = [0, x, c];
  else if (hp < 5) [r, g, b] = [x, 0, c];
  else [r, g, b] = [c, 0, x];
  const m = val - c;
  const to = (n) => Math.round((n + m) * 255).toString(16).padStart(2, "0").toUpperCase();
  return `#${to(r)}${to(g)}${to(b)}`;
}

/// #RRGGBB → {h,s,v}
function hexToHsb(hex) {
  const m = /^#?([0-9a-f]{6})$/i.exec(String(hex || "").trim());
  if (!m) return null;
  const n = parseInt(m[1], 16);
  const r = ((n >> 16) & 255) / 255;
  const g = ((n >> 8) & 255) / 255;
  const b = (n & 255) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const d = max - min;
  let h = 0;
  if (d !== 0) {
    if (max === r) h = ((g - b) / d + (g < b ? 6 : 0)) / 6;
    else if (max === g) h = ((b - r) / d + 2) / 6;
    else h = ((r - g) / d + 4) / 6;
  }
  return {
    h: Math.round(h * 360) % 360,
    s: Math.round((max === 0 ? 0 : d / max) * 100),
    v: Math.round(max * 100),
  };
}

/// #RRGGBB → {h,s,l}
function hexToHsl(hex) {
  const m = /^#?([0-9a-f]{6})$/i.exec(String(hex || "").trim());
  if (!m) return null;
  const n = parseInt(m[1], 16);
  const r = ((n >> 16) & 255) / 255;
  const g = ((n >> 8) & 255) / 255;
  const b = (n & 255) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  let h = 0;
  let sat = 0;
  if (max !== min) {
    const d = max - min;
    sat = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    if (max === r) h = ((g - b) / d + (g < b ? 6 : 0)) / 6;
    else if (max === g) h = ((b - r) / d + 2) / 6;
    else h = ((r - g) / d + 4) / 6;
  }
  return { h: Math.round(h * 360), s: Math.round(sat * 100), l: Math.round(l * 100) };
}

/// {h,s,l} → #RRGGBB
function hslToHex(h, sat, l) {
  const S = sat / 100;
  const L = l / 100;
  const c = (1 - Math.abs(2 * L - 1)) * S;
  const hp = h / 60;
  const x = c * (1 - Math.abs((hp % 2) - 1));
  let r = 0;
  let g = 0;
  let b = 0;
  if (hp < 1) [r, g, b] = [c, x, 0];
  else if (hp < 2) [r, g, b] = [x, c, 0];
  else if (hp < 3) [r, g, b] = [0, c, x];
  else if (hp < 4) [r, g, b] = [0, x, c];
  else if (hp < 5) [r, g, b] = [x, 0, c];
  else [r, g, b] = [c, 0, x];
  const m = L - c / 2;
  const to = (v) => Math.round((v + m) * 255).toString(16).padStart(2, "0").toUpperCase();
  return `#${to(r)}${to(g)}${to(b)}`;
}

/// 面板当前代表哪个颜色。强调色不接受任意亮度（整套配色是按 52% 亮度调的），
/// 所以那个场景下显示的是实际生效的颜色，免得给的色值和界面对不上
function pickerHex() {
  if (accTarget && accLockHex) return accLockHex;
  const raw = hsbToHex(accH, accS, accV);
  if (accTarget) return raw;
  const hsl = hexToHsl(raw);
  return hsl ? hslToHex(hsl.h, hsl.s, 52) : raw;
}

/// 圆点、色相条游标、色块、十六进制值都跟着当前 H/S/V 走
function paintPicker() {
  const sv = $("acc-sv");
  const thumb = $("acc-thumb");
  if (sv) sv.style.backgroundColor = `hsl(${accH} 100% 50%)`;
  if (thumb) {
    thumb.style.left = `${accS}%`;
    thumb.style.top = `${100 - accV}%`;
  }
  if ($("acc-hue-mark")) $("acc-hue-mark").style.top = `${(accH / 360) * 100}%`;
  const hex = pickerHex();
  if ($("acc-chip")) $("acc-chip").style.background = hex;
  // 正在输入的时候别回写，否则光标会被顶走、字也敲不进去
  const box = $("acc-hex");
  if (box && document.activeElement !== box) box.value = hex.toUpperCase();
  return hex;
}

/// 选好之后把颜色交出去：字幕色写回调用方，强调色写回主题变量
function commitPicker() {
  const hex = paintPicker();
  if (accTarget) {
    accTarget(hex);
    return;
  }
  const hsl = hexToHsl(hex);
  if (hsl) applyAccent("custom", { h: hsl.h, s: hsl.s });
}

/// 色区里按下/拖动：横轴饱和度、纵轴亮度
function pickFromSv(e) {
  const sv = $("acc-sv");
  if (!sv) return;
  const rect = sv.getBoundingClientRect();
  const x = Math.min(Math.max(e.clientX - rect.left, 0), rect.width);
  const y = Math.min(Math.max(e.clientY - rect.top, 0), rect.height);
  accS = Math.round((x / rect.width) * 100);
  accV = Math.round((1 - y / rect.height) * 100);
  accLockHex = "";
  commitPicker();
}

/// 色相条：纵轴 0-360°
function pickFromHue(e) {
  const bar = $("acc-hue");
  if (!bar) return;
  const rect = bar.getBoundingClientRect();
  const y = Math.min(Math.max(e.clientY - rect.top, 0), rect.height);
  accH = Math.round((y / rect.height) * 360) % 360;
  accLockHex = "";
  commitPicker();
}

/// 打开取色面板。anchor 是贴在谁旁边；hex 给了就从它反推初始位置；
/// cb 是改动时的回调（不给就改强调色，那是设置页的用法）
function openPicker(anchor, hex, cb) {
  const root = document.documentElement;
  accTarget = cb || null;
  accAnchor = anchor || null;
  accLockHex = "";
  const parsed = hex ? hexToHsb(hex) : null;
  if (parsed) {
    accH = parsed.h;
    accS = parsed.s;
    accV = Math.max(6, parsed.v);
  } else {
    const cur = root.dataset.accent;
    if (cur === "custom") {
      accH = Number(root.style.getPropertyValue("--accent-h")) || accH;
      accS = parseInt(root.style.getPropertyValue("--accent-s"), 10) || accS;
      accV = 92;
    } else if (PRESET_HS[cur]) {
      [accH, accS] = PRESET_HS[cur];
      accV = 92;
    }
  }
  paintPicker();
  const pop = $("acc-pop");
  pop.hidden = false;
  if (anchor) {
    const r = anchor.getBoundingClientRect();
    const w = pop.offsetWidth;
    const h = pop.offsetHeight;
    let left = Math.min(r.left, window.innerWidth - w - 10);
    // 默认向下展开，不往上翻：翻上去会盖住上面的输入项，也不符合下拉的直觉。
    // 底下实在放不下时整体往上顶一点，保证整块面板还在窗口里
    let top = r.bottom + 8;
    const maxTop = window.innerHeight - h - 8;
    if (top > maxTop) top = Math.max(8, maxTop);
    pop.style.left = `${Math.round(Math.max(8, left))}px`;
    pop.style.top = `${Math.round(top)}px`;
  }
  if (!cb) commitPicker();
}

function closePicker() {
  $("acc-pop").hidden = true;
  accTarget = null;
  accAnchor = null;
}

$("acc-custom-btn")?.addEventListener("click", (e) => {
  e.stopPropagation();
  if ($("acc-pop").hidden) openPicker($("acc-custom-btn"), null, null);
  else closePicker();
});

// 色区和色相条：按下开始拖，抬起结束；拖动全程即时生效
const bindDrag = (el, kind, fn) => {
  el?.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    accDragging = kind;
    el.setPointerCapture(e.pointerId);
    fn(e);
  });
  el?.addEventListener("pointermove", (e) => {
    if (accDragging === kind) fn(e);
  });
  const stop = () => {
    if (accDragging === kind) accDragging = null;
  };
  el?.addEventListener("pointerup", stop);
  el?.addEventListener("lostpointercapture", stop);
};
bindDrag($("acc-sv"), "sv", pickFromSv);
bindDrag($("acc-hue"), "hue", pickFromHue);

/// 把输入框里的 #RRGGBB 吃进来：合法就同步圆点和颜色，不合法恢复原值
function applyHexInput() {
  const box = $("acc-hex");
  if (!box) return false;
  let v = String(box.value || "").trim().toUpperCase();
  if (v && !v.startsWith("#")) v = "#" + v;
  const hsb = /^#[0-9A-F]{6}$/.test(v) ? hexToHsb(v) : null;
  if (!hsb) {
    box.value = pickerHex().toUpperCase(); // 打错了就退回当前色，不弹窗打断
    return false;
  }
  accH = hsb.h;
  accS = hsb.s;
  accV = Math.max(0, hsb.v);
  // 手敲的字幕色原样保留（转 HSB 再转回来会掉一两级）；强调色那边的亮度是固定的，
  // 显示实际生效的那个颜色，所以只在字幕色这种场景锁值
  accLockHex = accTarget ? v : "";
  paintPicker();
  // 强调色的亮度固定，pickerHex() 给的是实际生效的颜色，写回去免得显示和生效不一致；
  // 字幕颜色则原样保留用户敲的值
  box.value = pickerHex().toUpperCase();
  // 边敲边生效：强调色的圆点、字幕着色的色点都要立刻跟着变，
  // 不能等回车或失焦（以前这里只画了面板，外面那圈点还是旧颜色）
  commitPicker();
  return true;
}

// 吸管：系统取色，取到的值和手敲的一样原样用
if (window.EyeDropper && $("acc-drop")) {
  $("acc-drop").hidden = false;
  $("acc-drop").addEventListener("click", async (e) => {
    e.stopPropagation();
    try {
      const { sRGBHex } = await new EyeDropper().open();
      $("acc-hex").value = sRGBHex.toUpperCase();
      applyHexInput();
    } catch {} // 按 Esc 取消
  });
}

// 色块：点一下复制当前色值
$("acc-chip")?.addEventListener("click", async (e) => {
  e.stopPropagation();
  const hex = pickerHex().toUpperCase();
  toast((await copyText(hex)) ? `已复制 ${hex}` : "复制失败");
});

// 点进去先把旧值全选上，直接敲就是替换，不用先删一遍
$("acc-hex")?.addEventListener("focus", () => $("acc-hex").select());

// 色值：点进去就能直接敲，#RRGGBB；回车或失焦时生效
$("acc-hex")?.addEventListener("input", () => {
  // 敲满 7 位就直接生效，不用等回车
  if (/^#?[0-9A-Fa-f]{6}$/.test($("acc-hex").value.trim())) applyHexInput();
});
$("acc-hex")?.addEventListener("keydown", (e) => {
  e.stopPropagation(); // Esc 别把面板关了
  if (e.key === "Enter") {
    applyHexInput();
    $("acc-hex").blur();
  } else if (e.key === "Escape") {
    $("acc-hex").value = pickerHex().toUpperCase();
    $("acc-hex").blur();
  }
});
$("acc-hex")?.addEventListener("blur", () => {
  applyHexInput(); // 打了一半就走开：合法就生效，不合法退回当前色
});

// 点面板内部不收起；点外面收起；Esc 也收起
$("acc-pop")?.addEventListener("pointerdown", (e) => e.stopPropagation());
document.addEventListener("pointerdown", () => {
  if ($("acc-pop") && !$("acc-pop").hidden) closePicker();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && $("acc-pop") && !$("acc-pop").hidden) {
    closePicker();
    e.stopPropagation();
  }
});
// 色点这一行不需要右键菜单
$("accent-row")?.addEventListener("contextmenu", (e) => e.preventDefault());

// 预设点：只认有 data-accent 的
$("accent-row")?.addEventListener("click", (e) => {
  const b = e.target.closest(".acc");
  if (!b || b.classList.contains("acc-custom")) return;
  const started = performance.now();
  applyAccent(b.dataset.accent);
  invoke?.("log_ui_event", { kind: "accent", detail: b.dataset.accent, elapsedMs: performance.now() - started }).catch(() => {});
});

try {
  const saved = localStorage.getItem(ACCENT_KEY) || "amber";
  const vars = localStorage.getItem("sd.accentVars");
  if (saved === "custom" && vars) {
    const v = JSON.parse(vars);
    accH = Number(v.h) || 36;
    accS = parseInt(v.s, 10) || 100;
    applyAccent("custom", { h: accH, s: accS });
  } else {
    applyAccent(saved);
  }
} catch (e) {}

// ---------- 清空下载历史（只在「下载历史」页出现） ----------
$("clear-trash-btn")?.addEventListener("click", async () => {
  if (!invoke) return;
  if (!(await askYes("清空全部下载历史？", "只清这份记录，已经下载到硬盘上的文件不受影响。", "清空"))) return;
  try {
    await invoke("trash_clear");
    ++trashRevision;
    trashLoad = null;
    trashLoadError = "";
    state.trash = mergeTrash([]);
    toast("下载历史已清空");
    syncTrashClear();
    renderTasks();
    renderSidebar();
  } catch (e) {
    toast("清空失败：" + e);
  }
});

// ---------- 清理全部失效记录（只在「已完成」页出现） ----------
$("purge-btn")?.addEventListener("click", async () => {
  if (!invoke) return;
  if (!(await askYes("清理所有失效记录？", "文件早就不在原位置了，这里只删列表里的任务记录。", "清理"))) return;
  try {
    const n = await invoke("purge_missing");
    toast(n ? `已清理 ${n} 条失效记录` : "没有失效记录");
    refreshTasks();
  } catch (e) {
    toast("清理失败：" + e);
  }
});

// ---------- 统一云盘交接与账号 ----------
// Shared cloud chooser. The executor is selected by the original task, never by
// whichever account happened to be logged in on this computer.
const cloudSourceRequests = new Map(), cloudSourceEpochs = new Map(), cloudSourceSnapshots = new Map();
const cloudProfiles = new Map(), cloudProfileRequests = new Map(), cloudLogoutRequests = new Set();
const CLOUD_OFFLINE_PROVIDERS = [{id:"pikpak",name:"PikPak"},{id:"guangya",name:"光鸭云盘"},{id:"xunlei",name:"迅雷云盘"},{id:"pan115",name:"115 网盘"},{id:"pan123",name:"123 云盘"}];
const CLOUD_HIDDEN_PROVIDERS = new Set(["pan115", "pan123"]);
let cloudSettingsOrigin="", cloudSettingsEpoch=0, cloudSettingsLoading=false, cloudTasksPanel=null;

function cloudTaskLink(task) {
  const link = String(task.meta?.req?.url || "");
  if (/^(magnet:|ed2k:|https?:)/i.test(link)) return link;
  const hash = String(task.meta?.res?.hash || "").trim();
  return task.protocol === "bt" && hash ? `magnet:?xt=urn:btih:${hash}` : "";
}
function canCloudOffline(task) {
  return task.status !== "done" && task.protocol !== "cloud" && !/^(?:nas:)?(?:bd:|cloud:|pp:|pp-|v:|v-)/.test(String(task.id)) && !!cloudTaskLink(task);
}
function cloudTarget(origin) { return String(origin || "").startsWith("nas:") ? "NAS" : "本机"; }
function cloudQuotaText(source) {
  const valid = (n) => typeof n === "number" && Number.isFinite(n) && n >= 0;
  const overLimit = valid(source.usedBytes) && valid(source.totalBytes) && source.usedBytes > source.totalBytes;
  const space = source.quotaKnown && valid(source.freeBytes)
    ? `${overLimit ? "空间已超额 · " : ""}可用 ${bytes(source.freeBytes)}${valid(source.totalBytes) ? ` / 总计 ${bytes(source.totalBytes)}` : ""}` : "可用空间未知";
  const file = valid(source.singleFileLimit) ? `单文件上限 ${bytes(source.singleFileLimit)}` : "单文件上限未知";
  const remaining = source.offlineQuota?.remaining;
  const offline = valid(remaining) ? `离线剩余 ${remaining} 次（额度周期以服务商为准）` : "离线次数限制未知";
  return [space, file, offline];
}
function invalidateCloudSources(origin) {
  const target=cloudTarget(origin);cloudSourceEpochs.set(target,(cloudSourceEpochs.get(target)||0)+1);cloudSourceSnapshots.delete(target);cloudProfiles.delete(target);
}
function queryCloudSources(task, refresh = false) {
  const link=cloudTaskLink(task),scheme=link.split(":")[0].toLowerCase(),size=Math.max(0,Number(task.meta?.res?.size)||0),target=cloudTarget(task.id),epoch=cloudSourceEpochs.get(target)||0;
  const key=JSON.stringify([target,size,scheme,epoch]);
  if(cloudSourceRequests.has(key))return cloudSourceRequests.get(key);
  const request=invoke("cloud_sources",{origin:task.id,size,scheme,refresh}).then(sources=>{
    if((cloudSourceEpochs.get(target)||0)===epoch){cloudSourceSnapshots.set(target,sources || []);const profile=cloudProfiles.get(target);if(profile)profile.sources=sources || [];}return sources;
  });
  cloudSourceRequests.set(key,request);request.finally(()=>{if(cloudSourceRequests.get(key)===request)cloudSourceRequests.delete(key)}).catch(()=>{});return request;
}
function cloudProfileTask(origin){return {id:origin || "",meta:{req:{url:"https://cloud-profile.invalid/"},res:{size:0}}};}
function cloudAccountState(source,origin){
  const state=source?.accountState;
  if(state==="disconnected")return {label:"NAS未连接",reason:"请先连接NAS",ready:false};
  if(state==="checking")return {label:"查询中…",reason:"正在查询账号状态",ready:false};
  if(state==="expired")return {label:"关联失效",reason:"请重新关联",ready:false};
  if(state==="connecting")return {label:"验证中…",reason:"正在验证关联",ready:false};
  if(state==="verify")return {label:"等待网页确认",reason:"请在网页完成登录",ready:false};
  if(state==="unknown")return {label:"查询失败",reason:"状态获取失败，请刷新",ready:false};
  if(state==="error")return {label:"关联异常",reason:"请重新关联",ready:false};
  if(!source?.loggedIn || state==="unlinked")return {label:"未关联",reason:"请关联",ready:false};
  return {label:"已关联",reason:source.quotaKnown?"":"空间获取失败，请刷新",ready:state==="ready"};
}
function cloudUnavailableReason(source,origin){const account=cloudAccountState(source,origin);return account.ready?`${cloudTarget(origin)} · ${source.reason || account.reason || "当前不可用"}`:account.reason;}
function cloudHasAccount(source){return !!source?.loggedIn || ["ready","expired","connecting","verify","error"].includes(source?.accountState);}
function cloudIsOffline(source){return typeof source?.canOffline==="boolean"?source.canOffline:CLOUD_OFFLINE_PROVIDERS.some(p=>p.id===source?.id);}
const CLOUD_HOME_URLS={
  pikpak:"https://mypikpak.com/drive/",guangya:"https://www.guangyapan.com/",pan115:"https://115.com/?mode=wangpan",
  pan123:"https://yun.123pan.cn/",xunlei:"https://pan.xunlei.com/",quark:"https://pan.quark.cn/",uc:"https://drive.uc.cn/",
  mobile:"https://yun.139.com/w/",baidu:"https://pan.baidu.com/disk/main",ali:"https://www.alipan.com/drive/file/all",
  tianyi:"https://cloud.189.cn/web/main",
};
async function openCloudHome(provider){
  const url=CLOUD_HOME_URLS[provider.id] || provider.loginUrl;
  if(!url)return toast("找不到该网盘的网页地址");
  try{await invoke("open_url",{url});}catch(error){toast(`未能打开${provider.name}：${String(error)}`);}
}
function cloudProfileSource(profile,provider){const stored=profile?.sources.find(source=>source.id===provider.id),account=profile?.accounts.find(account=>account.provider===provider.id);return stored?{...provider,...stored}:{...provider,loggedIn:account?.state==="ready",accountState:account?.state || "unlinked"};}
async function queryCloudProfile(origin,refresh=false){
  const target=cloudTarget(origin),epoch=cloudSourceEpochs.get(target)||0,key=JSON.stringify([target,epoch]);
  if(cloudProfileRequests.has(key))return cloudProfileRequests.get(key);
  const request=(async()=>{
    const metadata=await invoke("cloud_accounts",{origin,refresh});
    if(metadata?.scope && metadata.scope!==(target==="NAS"?"nas":"local"))throw new Error("云盘状态与当前执行端不一致，请刷新");
    const sources=Array.isArray(metadata?.sources)?metadata.sources:await queryCloudSources(cloudProfileTask(origin),refresh);
    const known=metadata?.providers || [],offline=new Set(CLOUD_OFFLINE_PROVIDERS.map(p=>p.id));
    const providers=[...CLOUD_OFFLINE_PROVIDERS.map(p=>({...p,canLink:false,...known.find(provider=>provider.id===p.id)})),...known.filter(p=>!offline.has(p.id))];
    const profile={providers,accounts:metadata?.accounts || [],sources:sources || [],error:""};
    if((cloudSourceEpochs.get(target)||0)===epoch){cloudProfiles.set(target,profile);cloudSourceSnapshots.set(target,profile.sources);}return profile;
  })();
  cloudProfileRequests.set(key,request);request.finally(()=>{if(cloudProfileRequests.get(key)===request)cloudProfileRequests.delete(key)}).catch(()=>{});return request;
}
function cloudModal(id, title) {
  let box = $(id);
  if (!box) {
    box = document.createElement("div"); box.id = id; box.className = "modal-mask cloud-mask";
    box.innerHTML = `<div class="modal cloud-modal" role="dialog" aria-modal="true" aria-labelledby="${id}-title"><button class="modal-x" data-cloud-close aria-label="关闭"><svg class="ic"><use href="#i-x"/></svg></button><h3 id="${id}-title"></h3><div class="cloud-body"></div><div class="modal-foot cloud-foot"></div></div>`;
    document.body.appendChild(box);
  }
  box.querySelector("h3").textContent = title;
  box.querySelector(".cloud-body").replaceChildren(); box.querySelector(".cloud-foot").replaceChildren();
  box.hidden = false; return box;
}
function cloudButton(label, action, primary = false) {
  const button = document.createElement("button"); button.type = "button"; button.className = primary ? "primary" : "ghost";
  button.textContent = label; button.addEventListener("click", action); return button;
}
const cloudLoginRequests = new Map(), cloudLoginStatus = new Map(), cloudSubmissions = new Map();
let cloudContextMenu = null;
function cloudStatusDot(source,origin){
  const dot=document.createElement("span"),account=cloudAccountState(source,origin);
  dot.className="cloud-status-dot "+(account.ready?"ready":["checking","connecting","verify"].includes(source?.accountState)?"pending":source?.accountState==="unlinked"?"idle":"error");
  dot.setAttribute("aria-hidden","true");return dot;
}
function cloudLoginKey(provider,origin){return cloudTarget(origin)+":"+provider;}
function cloudLoginLabel(source,origin){
  if(cloudAccountState(source,origin).ready)return "";
  return cloudLoginStatus.get(cloudLoginKey(source.id,origin)) || "";
}
async function startCloudLogin(provider,origin=""){
  if(!invoke || provider.canLink!==true)return toast(provider.linkMessage || "此网盘暂不支持关联");
  const key=cloudLoginKey(provider.id,origin);
  if(cloudLoginRequests.has(key))return cloudLoginRequests.get(key);
  cloudLoginStatus.set(key,"正在打开网页…");
  toast(`正在打开${provider.name}网页…`);
  const pending=Promise.resolve().then(async()=>{
    try{
      const result=await invoke("cloud_account_link",{provider:provider.id,origin});
      if(["error","cancelled"].includes(result?.state))throw new Error(result.message || "关联未完成，请重试");
      cloudLoginStatus.set(key,result?.state==="ready"?"":"等待网页确认");
      if(result?.state==="ready")await onCloudAccountLinked({provider:provider.id,target:cloudTarget(origin)==="NAS"?"nas":"local"});
      else toast(result?.message || "请在网页中完成关联");
      return result;
    }catch(error){cloudLoginStatus.set(key,String(error));toast(String(error));}
    finally{cloudLoginRequests.delete(key);renderCloudAccountRow(provider.id,origin);}
  });
  cloudLoginRequests.set(key,pending);
  renderCloudAccountRow(provider.id,origin);
  return pending;
}
async function submitCloudTask(task,source){
  if(!invoke || !source.suitable)return;
  if(cloudSubmissions.has(task.id))return cloudSubmissions.get(task.id);
  toast(`正在交给${source.name}…`);
  const keep=keepCloudSpot(task.id,cloudTaskLink(task));
  const pending=(async()=>{
    try{const result=await invoke("cloud_offline",{provider:source.id,origin:task.id});toast(result?.message || `已交给${source.name}`);await keep(result?.id);}
    catch(error){toast(`未能交给${source.name}：${String(error)}`);}
    finally{cloudSubmissions.delete(task.id);}
  })();
  cloudSubmissions.set(task.id,pending);return pending;
}
function attachCloudSubmenu(menu,task){
  const target=cloudTarget(task.id);
  const wrap=document.createElement("div");wrap.className="ctx-branch";
  const trigger=document.createElement("button");trigger.type="button";trigger.className="ctx-item";
  setMenuLabel(trigger,"交给云盘","i-cloud");trigger.dataset.cloudTrigger="true";trigger.setAttribute("aria-haspopup","menu");trigger.setAttribute("aria-expanded","false");
  const arrow=document.createElement("span");arrow.className="ctx-arrow";arrow.textContent="›";trigger.appendChild(arrow);
  const sub=document.createElement("div");sub.className="ctx-menu ctx-submenu";sub.setAttribute("role","menu");sub.setAttribute("aria-label",`${target}云盘`);sub.hidden=true;
  const heading=document.createElement("div");heading.className="cloud-menu-heading";heading.textContent=target==="NAS"?"NAS 云盘":"本机云盘";heading.setAttribute("role","presentation");
  wrap.append(trigger,sub);menu.appendChild(wrap);
  const session=cloudContextMenu={task,menu,sub,trigger};
  const current=()=>cloudContextMenu===session && !menu.hidden;
  function position(){
    sub.style.left="";sub.style.right="";sub.style.top="0px";
    const bounds=sub.getBoundingClientRect();if(bounds.right>window.innerWidth-6){sub.style.left="auto";sub.style.right="100%";}
    if(bounds.bottom>window.innerHeight-6)sub.style.top=Math.min(0,window.innerHeight-6-bounds.bottom)+"px";
  }
  function show(){sub.hidden=false;trigger.setAttribute("aria-expanded","true");position();}
  function hide(){sub.hidden=true;trigger.setAttribute("aria-expanded","false");}
  wrap.addEventListener("mouseenter",show);wrap.addEventListener("mouseleave",hide);trigger.addEventListener("focus",show);
  trigger.addEventListener("click",event=>{event.stopPropagation();show();});
  trigger.addEventListener("keydown",event=>{if(event.key==="ArrowRight"){event.preventDefault();show();sub.querySelector("button:not(:disabled)")?.focus();}});
  sub.addEventListener("keydown",event=>{
    if(event.key==="Escape" || event.key==="ArrowLeft"){event.preventDefault();event.stopPropagation();hide();trigger.focus();hide();}
    if(["ArrowDown","ArrowUp"].includes(event.key)){event.preventDefault();const buttons=[...sub.querySelectorAll("button")].filter(b=>!b.disabled),index=buttons.indexOf(event.target);buttons[(index+(event.key==="ArrowDown"?1:-1)+buttons.length)%buttons.length]?.focus();}
  });
  function render(sources,loading=false,error=""){
    sub.replaceChildren(heading);
    const scheme=cloudTaskLink(task).split(":")[0].toLowerCase();
    const rows=(sources || []).filter(source=>!CLOUD_HIDDEN_PROVIDERS.has(source.id) && cloudIsOffline(source) && source.protocols?.includes(scheme) && (source.accountState==="expired" ||
      (source.loggedIn && ["ready","unknown","error"].includes(source.accountState))));
    if(!rows.length){
      const notice=document.createElement(error?"button":"div");
      notice.textContent=loading?"正在查询云盘…":error?"查询失败 · 点击重试":"暂无支持此链接的已关联网盘";
      if(error){
        notice.type="button";notice.className="ctx-item";notice.setAttribute("role","menuitem");
        notice.addEventListener("click",()=>load(true));
      }else{
        notice.className="cloud-menu-heading";notice.setAttribute("role","status");
      }
      sub.appendChild(notice);
    }
    const order=new Map(CLOUD_OFFLINE_PROVIDERS.map((p,index)=>[p.id,index]));
    for(const source of rows.sort((a,b)=>(order.get(a.id)??100)-(order.get(b.id)??100))){
      const button=document.createElement("button");button.type="button";button.className="ctx-item cloud-menu-item";button.setAttribute("role","menuitem");
      const name=document.createElement("span");name.className="cloud-menu-name";name.textContent=source.name;
      const note=document.createElement("span");note.className="cloud-menu-state";
      const account=cloudAccountState(source,task.id);
      const retry=!!error || source.accountState==="unknown";
      const available=source.suitable && account.ready;
      const canAssociate=source.canLink===true && ["expired","error"].includes(source.accountState);
      if(loading)note.textContent="查询中…";
      else if(retry)note.textContent="状态查询失败 · 点击重试";
      else if(available)note.textContent=source.id==="guangya"
        ? "非会员超过 10 MB，需用光鸭官方客户端"
        : cloudQuotaText(source)[0].split(" / 总计")[0];
      else if(canAssociate)note.textContent=`${account.label} · 重新关联`;
      else if(account.ready)note.textContent=source.reason || account.reason || "当前不可用";
      else note.textContent=`${account.label}${source.canLink===false?" · 暂不支持电脑关联":""}`;
      const info=document.createElement("span");info.className="cloud-menu-info";info.append(name,note);
      const dot=cloudStatusDot(loading?{accountState:"checking"}:source,task.id);if(!loading && account.ready && !available)dot.className="cloud-status-dot error";button.append(dot,info);
      button.disabled=loading || !(retry || available || canAssociate || account.ready);
      button.title=available && source.id==="guangya"
        ? "大于 10 MB 的文件已存入云盘后，请在光鸭官方客户端取回"
        : note.textContent;
      button.addEventListener("click",async()=>{
        if(retry){await load(true);return;}
        if(available){closeCtx();return submitCloudTask(task,source);}
        if(canAssociate){button.disabled=true;note.textContent="正在打开网页…";await startCloudLogin(source,task.id);if(current())closeCtx();return;}
        toast(source.reason || account.reason || "当前不可用，请检查空间或任务类型");
      });sub.appendChild(button);
    }
    if(!sub.hidden)position();
  }
  async function load(refresh=false){
    const epoch=cloudSourceEpochs.get(target)||0;
    const stillCurrent=()=>current() && epoch===(cloudSourceEpochs.get(target)||0);
    const cached=cloudSourceSnapshots.get(target);
    render(cached,true);
    try{const sources=await queryCloudSources(task,refresh);if(stillCurrent())render(sources);}
    catch(error){if(stillCurrent())render(cached,false,String(error));}
  }
  session.reload=()=>load(false);
  load(false);return session;
}
function openCloudChooser(task){
  openCtx(Math.round(window.innerWidth/2),Math.round(window.innerHeight/2),task.id);
  $("ctx-menu")?.querySelector("[data-cloud-trigger]")?.click();
}
function openCloudAccounts(origin="",onClose,preferredProvider=""){
  if(preferredProvider){const provider=cloudProfiles.get(cloudTarget(origin))?.providers.find(p=>p.id===preferredProvider);if(provider)return startCloudLogin(provider,origin);}
  cloudSettingsOrigin=origin;switchView("settings");
  $("cloud-settings-target")?.querySelectorAll("[data-cloud-target]").forEach(button=>button.classList.toggle("on",button.dataset.cloudTarget===(cloudTarget(origin)==="NAS"?"nas":"local")));
  $("cloud-accounts-card")?.scrollIntoView({block:"nearest"});return refreshCloudSettings(false);
}

function cloudSettingsVisible(){return typeof state!=="undefined" && state.view==="settings";}
function suspendCloudSettings(){
  cloudSettingsEpoch++;cloudSettingsLoading=false;
  const card=$("cloud-accounts-card");
  if(card)card.style.minHeight="";
}
function renderCloudAccountRow(id,origin){
  if(!cloudSettingsVisible() || cloudSettingsOrigin!==origin)return;
  const profile=cloudProfiles.get(cloudTarget(origin));
  if(profile)renderCloudSettings(profile,cloudSettingsLoading,id);
}
function forgetCloudAccount(origin,provider,renderedProfile){
  const target=cloudTarget(origin),profile=cloudProfiles.get(target) || renderedProfile;
  cloudSourceEpochs.set(target,(cloudSourceEpochs.get(target)||0)+1);
  const snapshot=cloudSourceSnapshots.get(target);
  if(snapshot)cloudSourceSnapshots.set(target,snapshot.filter(source=>source.id!==provider));
  if(profile)cloudProfiles.set(target,{...profile,
    sources:profile.sources.filter(source=>source.id!==provider),
    accounts:profile.accounts.filter(account=>account.provider!==provider),
  });
}
function renderCloudSettings(profile,loading=false,onlyProvider=null){
  const list=$("cloud-settings-list");if(!list)return;
  const origin=cloudSettingsOrigin,target=cloudTarget(origin),unsupported=$("cloud-settings-unsupported-list");
  const oldRow=onlyProvider && [...list.children].find(row=>row.dataset.cloudProvider===onlyProvider);
  if(onlyProvider && !oldRow)return;
  if(!onlyProvider){
    list.replaceChildren();
    unsupported?.replaceChildren();
    $("cloud-settings-scope").dataset.tip=`账号供${target==="NAS"?" NAS ":"本机"}使用。右键「交给云盘」只列能接收新任务的网盘。`;
  }
  for(const provider of profile?.providers || CLOUD_OFFLINE_PROVIDERS){
    if(CLOUD_HIDDEN_PROVIDERS.has(provider.id)) continue;
    if(onlyProvider && provider.id!==onlyProvider)continue;
    const source=profile?cloudProfileSource(profile,provider):{...provider,accountState:loading?"checking":"unknown"},account=cloudAccountState(source,origin);
    if(!cloudIsOffline(source) || (provider.canLink!==true && !cloudHasAccount(source))){
      oldRow?.remove();
      continue;
    }
    const row=document.createElement("div");row.className="set-row cloud-settings-row";row.dataset.cloudProvider=provider.id;
    const name=document.createElement("span");name.className="set-label";name.textContent=provider.name;
    const detail=document.createElement("span");detail.className="set-path cloud-settings-detail";
    const label=document.createElement("span");label.className="cloud-account-state";label.append(cloudStatusDot(source,origin));const text=document.createElement("span");text.textContent=account.label;label.appendChild(text);detail.appendChild(label);
    const space=document.createElement("span");space.className="cloud-settings-secondary";
    space.textContent=cloudLoginLabel(source,origin) || (account.ready?(source.quotaKnown?cloudQuotaText(source)[0].split(" / 总计")[0]:"空间获取失败，请刷新"):"");
    if(account.ready)space.title=cloudQuotaText(source).join("；");
    if(space.textContent)detail.appendChild(space);
    const use=document.createElement("span");
    use.className="cloud-settings-secondary cloud-settings-purpose";
    use.textContent=provider.id==="guangya" ? "网盘先下，大文件可尝试，需光鸭客户端" : "网盘先下，闪电可拉回";
    if(provider.id==="guangya")use.title="大文件可尝试交给光鸭。非会员网页下载超过 10 MB，请在光鸭客户端取回。";
    detail.appendChild(use);
    const actions=document.createElement("div");actions.className="set-actions";
    const loginKey=cloudLoginKey(provider.id,origin),loginBusy=cloudLoginRequests.has(loginKey);
    const action=cloudButton(account.ready?"打开网盘":source.accountState==="unknown"?"刷新状态":loginBusy?"打开中…":cloudLoginLabel(source,origin)==="等待网页确认"?"继续关联":source.accountState==="expired"?"重新关联":"关联",async()=>{
      if(account.ready)await openCloudHome(provider);
      else if(source.accountState==="unknown")await refreshCloudSettings(true);
      else await startCloudLogin(provider,origin);
    });
    action.disabled=loading || loginBusy || source.accountState==="disconnected" || (!account.ready && source.accountState!=="unknown" && provider.canLink!==true);actions.appendChild(action);
    if(provider.id==="pikpak" && target==="本机"){const tasks=cloudButton("任务",()=>loadPikpakTasks());tasks.disabled=loading || !account.ready;actions.appendChild(tasks);}
    if(cloudHasAccount(source)){
      const signingOut=cloudLogoutRequests.has(loginKey);
      const out=cloudButton(signingOut?"正在退出…":"退出",async()=>{
        if(out.disabled || cloudLogoutRequests.has(loginKey))return;
        cloudLogoutRequests.add(loginKey);
        out.disabled=true;out.textContent="正在退出…";
        try{
          await invoke("cloud_account_remove",{provider:provider.id,origin});
          cloudLoginStatus.delete(loginKey);
          cloudLinkedRefreshes.delete(loginKey);
          forgetCloudAccount(origin,provider.id,profile);
        }catch(error){toast(String(error));}
        finally{
          cloudLogoutRequests.delete(loginKey);
          out.disabled=false;out.textContent="退出";
          renderCloudAccountRow(provider.id,origin);
        }
      });out.disabled=loading || signingOut;actions.appendChild(out);
    }
    row.append(name,detail,actions);
    if(oldRow)oldRow.replaceWith(row);else list.appendChild(row);
  }
  if($("cloud-settings-unsupported"))$("cloud-settings-unsupported").hidden=!unsupported?.children.length;
}
async function refreshCloudSettings(refresh=false){
  if(!$("cloud-settings-list") || !cloudSettingsVisible())return;
  const origin=cloudSettingsOrigin,target=cloudTarget(origin),epoch=++cloudSettingsEpoch;
  const card=$("cloud-accounts-card"),scroller=card?.closest(".scroll");
  const button=$("cloud-settings-refresh"),status=$("cloud-settings-status");
  const draw=(profile,loading,message="")=>{
    const top=scroller?.scrollTop;
    const height=card?.getBoundingClientRect().height;
    if(height>0)card.style.minHeight=`${height}px`;
    renderCloudSettings(profile,loading);
    status.textContent=message;
    if(scroller && top!=null)scroller.scrollTop=top;
  };
  cloudSettingsLoading=false;
  try{
    if(target==="NAS" && typeof window.__nasUsable==="function" && !window.__nasUsable()){
      draw({providers:[],accounts:[],sources:[]},false,"NAS未连接，请先保存并连接NAS。");
      return;
    }
    const cached=cloudProfiles.get(target);
    cloudSettingsLoading=!cached;
    button.disabled=true;button.setAttribute("aria-busy","true");button.dataset.tip="正在刷新状态…";
    draw(cached,cloudSettingsLoading,cached?.error || "");
    const profile=await queryCloudProfile(origin,refresh);
    if(epoch!==cloudSettingsEpoch || !cloudSettingsVisible() || cloudSettingsOrigin!==origin)return;
    draw(cloudProfiles.get(target) || profile,false,profile.error || "");
  }catch(error){if(epoch===cloudSettingsEpoch && cloudSettingsVisible())status.textContent=String(error)}
  finally{
    if(epoch===cloudSettingsEpoch){
      cloudSettingsLoading=false;button.disabled=false;button.setAttribute("aria-busy","false");button.dataset.tip="刷新状态";
    }
  }
}
function initCloudSettings(){
  const card=$("cloud-accounts-card");if(!card)return;$("cloud-settings-target").hidden=!$("nav-nas");if(card.dataset.cloudBound)return;card.dataset.cloudBound="true";
  card.querySelectorAll("[data-cloud-target]").forEach(button=>button.addEventListener("click",()=>{const origin=button.dataset.cloudTarget==="nas"?"nas:":"";if(origin===cloudSettingsOrigin)return;cloudSettingsOrigin=origin;card.querySelectorAll("[data-cloud-target]").forEach(node=>node.classList.toggle("on",node===button));refreshCloudSettings();}));
  $("cloud-settings-refresh").addEventListener("click",()=>refreshCloudSettings(true));
}
function closeCloudTasks(){if(!cloudTasksPanel)return false;cloudTasksPanel.box.hidden=true;cloudTasksPanel=null;return true;}
async function loadPikpakTasks(){
  if(!invoke)return;if(cloudTasksPanel?.loading)return;const box=cloudModal("cloud-tasks","本机 · PikPak云端任务"),session=cloudTasksPanel={box,loading:false};const body=box.querySelector(".cloud-body"),foot=box.querySelector(".cloud-foot");
  const status=document.createElement("p");status.className="cloud-hint";status.setAttribute("role","status");const list=document.createElement("div");body.append(status,list);box.onclick=event=>{if(event.target===box || event.target.closest?.("[data-cloud-close]"))closeCloudTasks()};
  const refresh=cloudButton("刷新任务",load),done=cloudButton("完成",closeCloudTasks);foot.append(refresh,done);
  async function load(){if(session.loading)return;session.loading=true;refresh.disabled=true;status.textContent="正在读取本机PikPak云端任务…";
    try{const tasks=await invoke("pikpak_tasks",{origin:""});if(cloudTasksPanel!==session)return;list.replaceChildren();status.textContent=tasks?.length?"":"云端还没有任务";
      for(const task of tasks || []){const row=document.createElement("div");row.className="pp-row";const name=document.createElement("span");name.className="pp-name";name.textContent=task.name || "（无名）";const detail=document.createElement("span");detail.className="pp-pct";detail.textContent=`${bytes(task.size||0)} · ${task.state==="done"?"已就绪":task.state==="error"?"失败":`云端下载中 ${task.progress||0}%`}`;row.append(name,detail);
        if(task.state==="done")row.appendChild(cloudButton("拉回本地",async event=>{const button=event.currentTarget || event.target;button.disabled=true;status.textContent="正在拉回本机…";try{const result=await invoke("pikpak_pull",{id:task.id,origin:""});toast(result);refreshTasks();if(cloudTasksPanel===session)status.textContent="已提交拉回任务"}catch(error){if(cloudTasksPanel===session)status.textContent=String(error)}finally{button.disabled=false}}));
        row.appendChild(cloudButton("删除",async()=>{if(!await askYes("连 PikPak 云端里的文件一起删除？","","删除"))return;status.textContent="正在删除云端任务…";try{await invoke("pikpak_remove",{id:task.id,origin:""});if(cloudTasksPanel===session)await load()}catch(error){if(cloudTasksPanel===session)status.textContent=String(error)}}));list.appendChild(row);}
    }catch(error){if(cloudTasksPanel===session){status.textContent=`无法读取云端任务：${String(error)}`;if(/过期|重新关联|失效/.test(String(error))){invalidateCloudSources("");if(cloudSettingsVisible() && cloudSettingsOrigin==="")refreshCloudSettings(true)}}}
    finally{session.loading=false;refresh.disabled=false}
  }
  await load();
}
const cloudLinkedRefreshes=new Map();
async function onCloudAccountLinked(payload={}){
  const provider=payload.provider;
  if(!provider)return;
  const origin=payload.target==="nas" || String(payload.origin || "").startsWith("nas:")?"nas:":"";
  const target=cloudTarget(origin),key=cloudLoginKey(provider,origin);
  if(["error","cancelled"].includes(payload.state)){
    const message=payload.message || "关联未完成，请重试";
    cloudLoginStatus.set(key,message);toast(message);renderCloudAccountRow(provider,origin);
    return;
  }
  if(cloudLinkedRefreshes.has(key))return cloudLinkedRefreshes.get(key);
  cloudLoginStatus.delete(key);
  if(payload.state==="ready")toast(`已关联到${target}`);
  const visible=cloudSettingsVisible() && cloudSettingsOrigin===origin;
  if(!visible && !cloudProfiles.has(target)){invalidateCloudSources(origin);return;}
  const pending=Promise.resolve().then(async()=>{
    try{
      if(!cloudProfiles.has(target)){
        const profile=await queryCloudProfile(origin);
        if(cloudLinkedRefreshes.get(key)===pending && visible && cloudSettingsVisible() && cloudSettingsOrigin===origin && !cloudSettingsLoading)renderCloudSettings(cloudProfiles.get(target) || profile);
      } else {
        const scope=origin?"nas":"local";
        const result=await invoke("cloud_accounts",{origin,provider,refresh:false});
        if(cloudLinkedRefreshes.get(key)!==pending)return;
        if(result?.scope && result.scope!==scope)throw new Error("云盘状态与当前执行端不一致");
        const source=result?.sources?.find(item=>item.id===provider);
        if(!source)throw new Error("暂时无法读取网盘状态");
        const profile=cloudProfiles.get(target);
        if(!profile)return;
        const account=result.accounts?.find(item=>item.provider===provider);
        const replace=(items,key,value)=>[...items.filter(item=>item[key]!==provider),value];
        cloudSourceEpochs.set(target,(cloudSourceEpochs.get(target)||0)+1);
        cloudSourceSnapshots.delete(target);
        cloudProfiles.set(target,{...profile,sources:replace(profile.sources,"id",source),accounts:account?replace(profile.accounts,"provider",account):profile.accounts});
        renderCloudAccountRow(provider,origin);
      }
    }catch(error){if(cloudLinkedRefreshes.get(key)===pending && cloudSettingsVisible() && cloudSettingsOrigin===origin)toast(`网盘状态获取失败：${String(error)}`);}
  });
  cloudLinkedRefreshes.set(key,pending);
  try{await pending}finally{if(cloudLinkedRefreshes.get(key)===pending)cloudLinkedRefreshes.delete(key)}
}
async function loadPikpak(){return onCloudAccountLinked({provider:"pikpak",target:"local"});}
if(!DIALOG && window.__TAURI__?.event?.listen)window.__TAURI__.event.listen("cloud-account-linked",event=>onCloudAccountLinked(event.payload || {}));
document.addEventListener("keydown",event=>{if(event.key==="Escape" && closeCloudTasks()){event.preventDefault();event.stopImmediatePropagation()}},true);
initCloudSettings();

// ---------- 统一云盘界面结束 ----------

// LinkSwift 这类油猴脚本走 Aria2 RPC 推网盘直链：把地址/端口/路径/密钥一次性填好
let rpcInfo = null;
async function loadRpcInfo() {
  if (!invoke) return;
  try {
    rpcInfo = await invoke("rpc_info");
  } catch (e) {
    return;
  }
  const el = $("rpc-token");
  if (el) el.textContent = rpcInfo?.token || "引擎启动后显示";
  const path = $("rpc-path");
  // 只显示地址本身：具体是 Aria2 RPC 这层意思放在标题旁的小 i 里讲，
  // 满屏 "Aria2 RPC：127.0.0.1:6800/jsonrpc" 对不熟这套的人等于没解释
  if (path && rpcInfo) path.textContent = `${rpcInfo.host}:${rpcInfo.port}`;
}

// 复制成一行行好对的文本，用户照着往 LinkSwift 里填就行
$("btn-copy-rpc")?.addEventListener("click", async () => {
  if (!rpcInfo) await loadRpcInfo();
  if (!rpcInfo) return toast("引擎还没启动，稍后再试");
  const text = [
    `地址：${rpcInfo.host}`,
    `端口：${rpcInfo.port}`,
    `路径：${rpcInfo.path}`,
    `密钥：${rpcInfo.token}`,
  ].join("\n");
  try {
    if (!(await copyText(text))) throw new Error("复制失败");
    toast("配置已复制，按内容填到 LinkSwift 的 Aria2 配置里");
  } catch (e) {
    toast("复制失败，请重试。");
  }
});

async function copyMcpConfig(codex) {
  try {
    const config = await invoke("mcp_config");
    const path = config.mcpServers.shandian.command;
    const command = path.includes("'") ? JSON.stringify(path.replaceAll("\\", "/")) : "'" + path + "'";
    const text = codex ? "[mcp_servers.shandian]\ncommand = " + command + '\nargs = ["--mcp"]'
      : JSON.stringify(config, null, 2);
    toast(await copyText(text) ? (codex ? "Codex 配置已复制" : "JSON 配置已复制") : "复制失败");
  } catch (error) {
    toast(String(error));
  }
}
$("btn-mcp-copy")?.addEventListener("click", () => copyMcpConfig(true));
$("btn-mcp-json")?.addEventListener("click", () => copyMcpConfig(false));

$("btn-linkswift")?.addEventListener("click", () => {
  if (!invoke) return;
  invoke("open_url", { url: "http://127.0.0.1:6800/linkswift-settings" })
    .catch((e) => toast("打不开连接设置：" + e));
});

// 设置页「GitHub 仓库」：交给系统默认浏览器打开（Rust 侧 open_url 只放行 http/https）
const aboutRepositoryUrl = "https://" + $("about-repo").textContent.trim();
$("btn-open-repo")?.addEventListener("click", () => {
  if (!invoke) return;
  invoke("open_url", { url: aboutRepositoryUrl })
    .catch((e) => toast("打不开仓库：" + e));
});

// ---------- 更新检查 ----------

let availableUpdate = null, checkingUpdate = false, downloadingUpdate = false;
const updateButton = $("btn-check-update");

function showUpdateState(message = "") {
  if (!updateButton) return;
  updateButton.disabled = checkingUpdate || downloadingUpdate;
  updateButton.textContent = checkingUpdate ? "正在检查" : downloadingUpdate ? "正在更新"
    : availableUpdate?.canInstall ? "立即更新" : "检查更新";
  const status = $("update-state");
  const text = message || (availableUpdate ? "发现 " + availableUpdate.version : "");
  status.textContent = text ? " · " + text : "";
  status.dataset.tip = availableUpdate?.message || "";
}

async function checkUpdate(silent = false) {
  if (!invoke || checkingUpdate || downloadingUpdate) return;
  checkingUpdate = true;
  if (!silent) showUpdateState();
  try {
    availableUpdate = await invoke("update_check", { force: !silent });
    if (!silent) toast(availableUpdate ? availableUpdate.message || "发现新版本 " + availableUpdate.version : "已是最新版本");
  } catch (error) {
    if (!silent) toast("检查更新失败：" + error);
  } finally {
    checkingUpdate = false;
    if (!downloadingUpdate) showUpdateState();
  }
}

updateButton?.addEventListener("click", async () => {
  if (!availableUpdate?.canInstall) return checkUpdate(false);
  downloadingUpdate = true;
  showUpdateState("正在下载安装包…");
  try { await invoke("update_install", { version: availableUpdate.version }); }
  catch (error) {
    downloadingUpdate = false;
    showUpdateState(String(error));
    toast("更新失败：" + error);
  }
});

// 启动后先探一次内核状态（可能上次退出了但进程还在），再开轮询
refreshEngine();
setInterval(refreshEngine, 2000);
// 内核是后台线程拉起的，页面通常比它快。头 15 秒查勤一点，起来了马上取数据，别等下一个 2 秒
{
  const bootPoll = setInterval(() => {
    if (state.engineRunning || performance.now() - BOOT_AT > 15000) clearInterval(bootPoll);
    else refreshEngine();
  }, 150);
}
setInterval(() => {
  const total = state.tasks
    .filter((t) => ACTIVE.has(t.status))
    .reduce((a, t) => a + (t.progress?.speed || 0), 0);
  const now = performance.now();
  state.speedPts.push({ t: now, v: total });
  // 超出窗口的老点丢掉，只留渐隐那一段需要的一点点余量
  while (state.speedPts.length > 2 && now - state.speedPts[0].t > SPARK_KEEP) {
    state.speedPts.shift();
  }
  ensureSparkLoop();
}, SPARK_TICK);
setInterval(() => {
  refreshTasks();
  // 速度卡跟着曲线走；曲线自己在逐帧跑，这里只更新数字
  renderSpeed();
}, 500);

async function openBrowserExtensions(browser) {
  try {
    if (await invoke("open_browser_extensions", { browser })) return;
    const page = browser === "edge" ? "edge://extensions/" : "chrome://extensions/";
    const copied = await copyText(page);
    toast(copied ? `未检测到可用扩展，已复制地址，请在 ${browser === "edge" ? "Edge" : "Chrome"} 地址栏粘贴打开` : `未检测到可用扩展，请手动打开 ${page}`);
  } catch (e) { toast(String(e)); }
}
$("btn-ext-chrome")?.addEventListener("click", () => openBrowserExtensions("chrome"));
$("btn-ext-edge")?.addEventListener("click", () => openBrowserExtensions("edge"));
$("btn-ext-folder")?.addEventListener("click", () => {
  invoke?.("extension_reveal")
    .then(async (dir) => {
      const el = $("ext-dir");
      if (el && dir) { el.textContent = dir; el.title = dir; }
      // 拖文件夹到扩展页并不可靠（Edge 基本没反应），真正的装法是「加载解压缩的扩展」。
      // 顺手把路径放剪贴板，选目录的对话框里直接粘贴就能进去
      const ok = dir ? await copyText(dir) : false;
      toast(
        ok
          ? "路径已复制：在扩展页点「加载解压缩的扩展」，粘到地址栏回车即可"
          : "在扩展页点「加载解压缩的扩展」，选中打开的 BrowserExtension 文件夹"
      );
    })
    .catch((e) => toast(String(e)));
});
// 导入站点的 cookies.txt：选文件 → 按域名拆开存好 → 之后对应站的任务自动带上
$("btn-import-cookie")?.addEventListener("click", async () => {
  if (!invoke) return;
  try {
    const p = await invoke("pick_text_file", { title: "选择浏览器导出的 cookies.txt" });
    if (!p) return;
    const msg = await invoke("import_cookies", { path: p });
    toast(msg);
    renderCookies();
  } catch (e) {
    toast(String(e));
  }
});

// 进设置页就刷一次，之后每 10 秒跟着扩展的 ping 走
setInterval(() => {
  if (state.view === "settings") refreshExt();
}, 10000);

// 滚动条：默认隐形，鼠标靠近滚动区右缘、或正在滚动时才出现（样式见 style.css 的 .sb-show）
// 全局一套：任何可滚动的元素都一样，新加的弹窗、列表不用再接
function scrollerAt(el) {
  for (; el && el.nodeType === 1 && el !== document.documentElement; el = el.parentElement) {
    if (el.scrollHeight > el.clientHeight + 1 && /auto|scroll/.test(getComputedStyle(el).overflowY)) return el;
  }
  return null;
}
{
  const timers = new WeakMap();
  const hideLater = (el, ms) => {
    clearTimeout(timers.get(el));
    timers.set(el, setTimeout(() => el.classList.remove("sb-show"), ms));
  };
  let hovered = null;
  document.addEventListener("scroll", (e) => {
    if (e.target.nodeType !== 1) return;
    e.target.classList.add("sb-show");
    hideLater(e.target, 900);
  }, { capture: true, passive: true });
  document.addEventListener("mousemove", (e) => {
    const el = scrollerAt(e.target);
    if (hovered && hovered !== el) hideLater(hovered, 300);
    hovered = el;
    if (!el) return;
    if (e.clientX >= el.getBoundingClientRect().right - 30) {
      // 靠近右缘：一直显示
      clearTimeout(timers.get(el));
      el.classList.add("sb-show");
    } else if (el.classList.contains("sb-show")) {
      // 离开右缘：短暂延续后收起
      hideLater(el, 300);
    }
  }, { passive: true });
}

// 版权和协议（内容在 legal.js，安装器里用的是同一份）
if (window.LEGAL && $("about-copy")) $("about-copy").textContent = `© ${LEGAL.year} ${LEGAL.author}`;
$("btn-legal")?.addEventListener("click", () => openLegal("terms"));
function openLegal(tab) {
  let box = $("legal");
  if (!box) {
    box = document.createElement("div");
    box.className = "modal-mask";
    box.id = "legal";
    box.innerHTML = `<div class="modal legal-modal" role="dialog" aria-modal="true">
      <button class="modal-x" data-legal="close" aria-label="关闭"><svg class="ic"><use href="#i-x"/></svg></button>
      <h3>用户协议与隐私说明</h3>
      <div class="seg legal-tabs"><button type="button" data-tab="terms">用户协议</button><button type="button" data-tab="privacy">隐私说明</button></div>
      <div class="legal-body" id="legal-body"></div>
      <div class="modal-foot"><span class="legal-meta" id="legal-meta"></span><button class="primary" data-legal="close">关闭</button></div>
    </div>`;
    document.body.appendChild(box);
    box.addEventListener("click", (e) => {
      const tb = e.target.closest("[data-tab]");
      if (tb) return openLegal(tb.dataset.tab);
      if (e.target === box || e.target.closest('[data-legal="close"]')) box.hidden = true;
    });
  }
  $("legal-body").innerHTML = LEGAL[tab] || "";
  $("legal-body").scrollTop = 0;
  box.querySelectorAll(".legal-tabs button").forEach((b) => b.classList.toggle("on", b.dataset.tab === tab));
  // 作者信息只在「关于」里写，协议这里只标更新时间
  $("legal-meta").textContent = `更新于 ${LEGAL.updated}`;
  box.hidden = false;
}

// 版本直接读取主程序，避免升级后前端仍显示旧的硬编码版本。
if (window.__TAURI__?.app) {
  Promise.all([window.__TAURI__.app.getVersion(), invoke("is_dev_build")]).then(([version, dev]) => {
    $("about-ver").textContent = `v${version}${$("nav-nas") ? " NAS" : ""}${dev ? " · 开发版" : ""}`;
  }).catch(() => {});
}
applyTheme();
// 独立窗口到这里就够了：取内容、显示，其余启动工作归主窗口
if (DIALOG) startDialogWindow().catch((e) => {
  console.error("独立窗口初始化失败", e);
  window.__TAURI__.window.getCurrentWindow().show();
});
else {
// 启动就读一次设置：主题要在第一帧就定下来，不能等进设置页
loadSettings();
loadAutostart();
loadTrash();
refreshExt();
// 启动时静默查一次更新，有新版只 toast 提一声
if (invoke) {
  setTimeout(() => checkUpdate(true), 8000);
  setInterval(() => checkUpdate(true), 60 * 60 * 1000);
}
loadRpcInfo();
// 按 state.view 切一次面板：只 renderAll() 的话，改动初始视图时界面不会跟着变
switchView(state.view);
// 内核还没起来时先显示上次的进行中任务，不让列表空着等
showTaskSnapshot();
// 首帧画完后回报一次，后端写进 startup.log（只用来量启动耗时）
requestAnimationFrame(() => {
  invoke?.("ui_ready", { uiMs: Math.round(performance.now()) }).catch(() => {});
});
}

async function feedbackInfo() {
  const report = await invoke("diagnostic_info");
  const edition = $("nav-nas") ? "NAS 版" : "普通版";
  return `${report}客户端：${edition}\n`;
}
$("btn-open-logs")?.addEventListener("click", () => invoke("open_logs_dir").catch(e => toast(String(e))));
$("btn-export-feedback-log")?.addEventListener("click", async (event) => {
  const button = event.currentTarget;
  if (button.disabled) return;
  button.disabled = true;
  button.textContent = "正在导出…";
  try {
    const path = await invoke("export_recent_logs");
    toast("脱敏日志已保存到「下载」文件夹");
    await invoke("reveal_path", { path }).catch(() => {});
  } catch (error) {
    toast(String(error));
  } finally {
    button.disabled = false;
    button.textContent = "导出脱敏日志";
  }
});
$("btn-copy-diagnostics")?.addEventListener("click", async () => {
  try { toast(await copyText(await feedbackInfo()) ? "诊断信息已复制，可粘贴到 GitHub Issue；日志可按需附加" : "复制失败"); }
  catch (e) { toast(String(e)); }
});
$("btn-report-issue")?.addEventListener("click", async () => {
  try {
    const report = await feedbackInfo();
    const issue = new URL(aboutRepositoryUrl + "/issues/new");
    issue.searchParams.set("template", "windows-bug.yml");
    const version = $("about-ver").textContent.trim();
    if (/^v?\d+\.\d+\.\d+$/.test(version)) issue.searchParams.set("version", version.replace(/^v/, ""));
    issue.searchParams.set("diagnostics", report);
    await invoke("open_url", { url: issue.toString() });
  } catch (e) { toast(String(e)); }
});

window.addEventListener("error", () => invoke?.("log_ui_event", { kind: "script-error", detail: "", elapsedMs: 0 }).catch(() => {}));

$("btn-clean-cache")?.addEventListener("click", async () => {
  const button = $("btn-clean-cache");
  if (button.disabled) return;
  button.disabled = true;
  try {
    const info = await invoke("cache_info");
    if (!info.files) { toast("当前没有可清理的缓存"); return; }
    if (!await askYes("清理本地缓存？", `约可释放 ${bytes(info.bytes)}。清理封面缓存、浏览器磁盘缓存和过期临时文件；封面之后会重新生成。账号、下载记录、续传数据、设置和运行工具都会保留。浏览器缓存将在下次启动时清理，正在使用的文件会跳过。`, "清理缓存")) return;
    const result = await invoke("clear_cache");
    toast(`已清理 ${bytes(result.bytes)}${result.pendingBrowser ? "，浏览器缓存将在下次启动时清理" : ""}${result.skipped ? "，部分文件正在使用，已跳过" : ""}`);
  } catch (e) { toast(String(e)); }
  finally { button.disabled = false; }
});
