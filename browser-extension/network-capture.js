var T = globalThis.T || ((text, values) => text.replace(/\{(\w+)\}/g, (_, key) => values?.[key]));  // 测试环境没有 i18n.js
// 可选的当前标签页网络资源发现。只保存媒体 URL 与类型到浏览器会话内存，不读取请求体、Cookie 或授权头。
const captureKey = 'shandianActiveCapture';
const captureTTL = 5 * 60 * 1000;
let captureState = null;
let captureListenerAttached = false;
let captureWrites = Promise.resolve();

function headerValue(headers, name) {
  const lower = name.toLowerCase();
  return (headers || []).find(h => h.name?.toLowerCase() === lower)?.value || '';
}

function parseDispositionFilename(disposition) {
  if (!disposition) return '';
  const utf8 = disposition.match(/filename\*\s*=\s*UTF-8''([^;\r\n]+)/i);
  if (utf8) {
    try { return decodeURIComponent(utf8[1].trim().replace(/^["']|["']$/g, '')); } catch (_) {}
  }
  const plain = disposition.match(/filename\s*=\s*("([^"]+)"|([^;\r\n]+))/i);
  if (plain) return (plain[2] || plain[3] || '').trim();
  return '';
}

function isReadableFilename(name) {
  if (!name || name.length < 2) return false;
  if (/[\u4e00-\u9fff]/.test(name)) return true;
  const lower = name.toLowerCase();
  if (/^(play|stream|audio|video|media|index|master|playlist|get|download|file|preview|sound|music|track)$/.test(lower)) return false;
  if (/^[0-9a-f_-]{6,}$/i.test(name)) return false;
  if (/^[a-z0-9_-]+$/i.test(name) && /\d{4,}/.test(name)) return false;
  if (/^[a-z0-9]{16,}$/i.test(name)) return false;
  return /[a-z]{3,}/i.test(name);
}

function parseMediaSize(headers) {
  const range = headerValue(headers, 'content-range');
  const total = range.match(/\/(\d+)\s*$/);
  if (total) return Number(total[1]) || 0;
  const len = Number(headerValue(headers, 'content-length'));
  return Number.isFinite(len) && len > 0 ? len : 0;
}

function classifyCapturedMedia(details) {
  if (details.method !== 'GET' || ![200, 206].includes(details.statusCode)) return null;
  if (typeof details.url !== 'string' || details.url.length > 8192) return null;
  let parsed;
  try { parsed = new URL(details.url); } catch (_) { return null; }
  if (!['http:', 'https:'].includes(parsed.protocol)) return null;
  // 分片不是完整媒体，不能当成独立文件下载。
  if (/\.(m4s|ts|cmfv|cmfa)(?:$)/i.test(parsed.pathname)) return null;
  const headers = details.responseHeaders || [];
  const mime = headerValue(headers, 'content-type').split(';')[0].trim().toLowerCase();
  const types = {
    'application/vnd.apple.mpegurl': ['m3u8', '视频', true],
    'application/x-mpegurl': ['m3u8', '视频', true],
    'audio/mpegurl': ['m3u8', '视频', true],
    'audio/x-mpegurl': ['m3u8', '视频', true],
    'application/dash+xml': ['mpd', '视频', true],
    'video/mp4': ['mp4', '视频', false],
    'video/x-m4v': ['m4v', '视频', false],
    'video/webm': ['webm', '视频', false],
    'video/quicktime': ['mov', '视频', false],
    'audio/mpeg': ['mp3', '音频', false],
    'audio/mp3': ['mp3', '音频', false],
    'audio/mpeg3': ['mp3', '音频', false],
    'audio/x-mpeg-3': ['mp3', '音频', false],
    'audio/mp4': ['m4a', '音频', false],
    'audio/x-m4a': ['m4a', '音频', false],
    'audio/m4a': ['m4a', '音频', false],
    'audio/aac': ['aac', '音频', false],
    'audio/x-aac': ['aac', '音频', false],
    'audio/wav': ['wav', '音频', false],
    'audio/x-wav': ['wav', '音频', false],
    'audio/wave': ['wav', '音频', false],
    'audio/ogg': ['ogg', '音频', false],
    'application/ogg': ['ogg', '音频', false],
    'audio/flac': ['flac', '音频', false],
    'audio/x-flac': ['flac', '音频', false],
    'audio/webm': ['webm', '音频', false]
  };
  const extTypes = {
    m3u8: ['m3u8', '视频', true],
    mpd: ['mpd', '视频', true],
    mp4: ['mp4', '视频', false],
    m4v: ['m4v', '视频', false],
    webm: ['webm', '视频', false],
    mov: ['mov', '视频', false],
    mp3: ['mp3', '音频', false],
    m4a: ['m4a', '音频', false],
    aac: ['aac', '音频', false],
    wav: ['wav', '音频', false],
    ogg: ['ogg', '音频', false],
    flac: ['flac', '音频', false]
  };
  const dispName = parseDispositionFilename(headerValue(headers, 'content-disposition'));
  let pathName = '';
  try { pathName = decodeURIComponent(parsed.pathname.split('/').pop() || ''); } catch (_) { pathName = parsed.pathname.split('/').pop() || ''; }
  const extFromName = (dispName || pathName).match(/\.([a-z0-9]{2,5})$/i)?.[1]?.toLowerCase() || '';
  let type = types[mime];
  if (!type && (!mime || mime === 'application/octet-stream' || mime === 'binary/octet-stream' || mime === 'text/plain')) {
    type = extTypes[extFromName];
  }
  if (!type && mime.startsWith('audio/')) {
    type = extTypes[extFromName]?.[1] === '音频' ? extTypes[extFromName] : ['mp3', '音频', false];
  }
  if (!type && mime.startsWith('video/')) {
    type = extTypes[extFromName]?.[1] === '视频' ? extTypes[extFromName] : ['mp4', '视频', false];
  }
  if (!type) return null;
  const rawName = (dispName || pathName).replace(/\.[^.]+$/, '').replace(/[\\/:*?"<>|]/g, ' ').trim().slice(0, 80);
  const filename = isReadableFilename(rawName) ? rawName : '';
  const size = parseMediaSize(headers);
  return {
    url: parsed.href,
    ext: type[0],
    kind: type[1],
    stream: type[2],
    label: type[1] + ' · 本页侦测',
    ...(filename ? { filename } : {}),
    ...(size > 0 ? { size } : {})
  };
}

function updateCaptureBadge(tabId, count) {
  if (tabId == null || !chrome.action?.setBadgeText) return;
  try {
    const text = count > 0 ? String(count) : '';
    chrome.action.setBadgeText({ tabId, text })?.catch?.(() => {});
    if (count > 0 && chrome.action.setBadgeBackgroundColor) {
      chrome.action.setBadgeBackgroundColor({ tabId, color: '#E0992F' })?.catch?.(() => {});
    }
  } catch (_) {}
}

function attachCaptureListener() {
  if (captureListenerAttached || !chrome.webRequest?.onHeadersReceived) return;
  try {
    chrome.webRequest.onHeadersReceived.addListener(handleCapturedResponse, { urls: ['http://*/*', 'https://*/*'] }, ['responseHeaders']);
    captureListenerAttached = true;
  } catch (_) { captureListenerAttached = false; }
}
function detachCaptureListener() {
  if (captureListenerAttached && chrome.webRequest?.onHeadersReceived) chrome.webRequest.onHeadersReceived.removeListener(handleCapturedResponse);
  captureListenerAttached = false;
}
function installClickTracker() {
  if (window.__shandianClickTracker) return;
  window.__shandianClickTracker = (e) => {
    window.__shandianLastClick = { el: e.target, time: Date.now() };
  };
  document.addEventListener('pointerdown', window.__shandianClickTracker, true);
}

function removeClickTracker() {
  if (!window.__shandianClickTracker) return;
  document.removeEventListener('pointerdown', window.__shandianClickTracker, true);
  window.__shandianClickTracker = null;
  window.__shandianLastClick = null;
}

function detectPlayingItemTitle() {
  const clean = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const isNoise = (s) => {
    if (!s || s.length < 2 || s.length > 80) return true;
    if (/^https?:\/\//i.test(s) || /^www\./i.test(s)) return true;
    if (/^\d{1,2}:\d{2}(:\d{2})?(\s*\/\s*\d{1,2}:\d{2})?$/.test(s)) return true;
    if (/^\d+(\.\d+)?\s*(kb|mb|gb|k|m|hz|khz|kbps|bpm|秒|s|次|人|币|积分|首|个)$/i.test(s)) return true;
    if (/^\d+\s*(秒|分钟|小时|天|周|个月|年)前$/.test(s)) return true;
    if (/^(作品集|专辑|来源|作者|分类|标签)[:：]/.test(s)) return true;
    if (/^(sq无损品质|无损品质|标准品质|hq高品质|高品质|版权|评论|播放|暂停|试听|下载|立即下载|免费下载|收藏|点赞|分享|详情|预览|音频|视频|音效|配乐|全部|更多|推荐|热门|最新|免费|会员|音频截取|在线编辑|以声搜声|排除同类|购买授权|举报|相似音频推荐|查看更多|mp3|wav|m4a|flac|vip|play|pause|download)$/i.test(s)) return true;
    if (/^(购买授权|在线编辑|音频截取|\d+条?评论|\d+下载|\d+收藏)/.test(s)) return true;
    return false;
  };
  const scoreNode = (node, text, cardRect) => {
    let score = Math.min(text.length, 20);
    const tag = (node.tagName || '').toLowerCase();
    const cls = typeof node.className === 'string' ? node.className.toLowerCase() : '';
    const href = tag === 'a' ? (node.getAttribute('href') || '') : '';
    const titleAttr = clean(node.getAttribute('title'));
    if (tag === 'a' && href && !href.startsWith('javascript:') && !href.startsWith('#')) score += 20;
    if (/\/(item|sound|audio|music|sfx|view|detail|s\/)/i.test(href)) score += 30;
    if (titleAttr && titleAttr === text) score += 20;
    if (/tit|title|subject|caption|song|track|sound-name|audio-name/.test(cls)) score += 30;
    else if (/name/.test(cls)) score += 10;
    if (/^(h[1-4]|strong|b)$/.test(tag)) score += 22;
    if (/tag|cat|cate|type|author|user|uploader|vip|badge|label|nav|menu|crumb|btn|button|time|dur|size|count|stat|album|collect|tool|action|edit|opt/.test(cls)) score -= 45;
    if (cardRect && node.getBoundingClientRect) {
      const nr = node.getBoundingClientRect();
      if (nr.width > 0 && nr.height > 0) {
        const dy = nr.top - cardRect.top;
        const dx = nr.left - cardRect.left;
        if (dy >= -4 && dy <= 55 && dx >= -4 && dx <= cardRect.width * 0.55) score += 35;
        else if (dx > cardRect.width * 0.55) score -= 25;
      }
    }
    return score;
  };
  const extractFromCard = (el) => {
    let cur = el;
    for (let depth = 0; cur && depth < 8; depth++, cur = cur.parentElement) {
      if (cur === document.body || cur === document.documentElement) break;
      const rect = cur.getBoundingClientRect ? cur.getBoundingClientRect() : null;
      if (rect && rect.height > 480 && depth >= 2) break;
      let bestText = '';
      let bestScore = -999;
      for (const node of cur.querySelectorAll('a[title], [class*="tit"], [class*="title"], [class*="name"], h1, h2, h3, h4, strong, b, a[href]')) {
        if (node.closest && node.closest('button')) continue;
        const rawTitle = clean(node.getAttribute('title'));
        const t = (!rawTitle || /^https?:\/\//i.test(rawTitle)) ? clean(node.textContent) : rawTitle;
        if (isNoise(t)) continue;
        const s = scoreNode(node, t, rect);
        if (s > bestScore) { bestScore = s; bestText = t; }
      }
      if (bestText && bestScore >= 15) return bestText.slice(0, 60);
    }
    return '';
  };
  try {
    const clicked = window.__shandianLastClick;
    if (clicked?.el && Date.now() - clicked.time < 10000) {
      const t = extractFromCard(clicked.el);
      if (t) return t;
    }
  } catch (_) {}
  try {
    const msTitle = clean(navigator.mediaSession?.metadata?.title);
    if (!isNoise(msTitle)) return msTitle.slice(0, 60);
  } catch (_) {}
  try {
    const active = document.activeElement;
    if (active && active !== document.body && active !== document.documentElement) {
      const t = extractFromCard(active);
      if (t) return t;
    }
  } catch (_) {}
  try {
    const playingMedia = [...document.querySelectorAll('audio, video')].find((m) => !m.paused);
    if (playingMedia) {
      const own = clean(playingMedia.getAttribute('title') || playingMedia.getAttribute('aria-label'));
      if (!isNoise(own)) return own.slice(0, 60);
      const t = extractFromCard(playingMedia);
      if (t) return t;
    }
  } catch (_) {}
  try {
    const pauseEl = document.querySelector('[title*="暂停"], [aria-label*="暂停"], .playing, .is-playing, .icon-pause, [class*="pause"]:not([class*="hidden"])');
    if (pauseEl) {
      const t = extractFromCard(pauseEl);
      if (t) return t;
    }
  } catch (_) {}
  return '';
}

function enrichCapturedTitle(tabId, url) {
  if (!chrome.scripting?.executeScript) return;
  chrome.scripting.executeScript({ target: { tabId }, func: detectPlayingItemTitle }).then((frames) => {
    const trackTitle = frames?.[0]?.result;
    if (!trackTitle || !captureState || captureState.tabId !== tabId) return;
    const target = captureState.items.find((x) => x.url === url);
    if (!target || target.trackTitle) return;
    target.trackTitle = trackTitle;
    const snapshot = { ...captureState, items: [...captureState.items] };
    captureWrites = captureWrites.then(() => chrome.storage.session.set({ [captureKey]: snapshot })).catch(() => {});
  }).catch(() => {});
}

function handleCapturedResponse(details) {
  if (!captureState || details.tabId !== captureState.tabId || Date.now() >= captureState.expires) return;
  const item = classifyCapturedMedia(details);
  if (!item || captureState.items.some(x => x.url === item.url) || captureState.items.length >= 100) return;
  captureState.items.push(item);
  updateCaptureBadge(captureState.tabId, captureState.items.length);
  // session 存储在浏览器内存中；停止捕获、过期、标签页关闭或浏览器退出时清除。
  const snapshot = { ...captureState, items: [...captureState.items] };
  captureWrites = captureWrites.then(() => chrome.storage.session.set({ [captureKey]: snapshot })).catch(() => {});
  enrichCapturedTitle(captureState.tabId, item.url);
}
async function startNetworkCapture(tabId, pageURL, title) {
  await restoreCapture;
  await captureWrites;
  if (!(await chrome.permissions.contains({ permissions: ['webRequest'] }))) return false;
  const url = new URL(pageURL);
  if (!['http:', 'https:'].includes(url.protocol)) return false;
  if (captureState && captureState.tabId !== tabId) updateCaptureBadge(captureState.tabId, 0);
  // 来源页的查询参数可能包含私密数据；仅保留地址路径。
  url.username = ''; url.password = ''; url.search = ''; url.hash = '';
  captureState = { tabId, pageURL: url.href, title: (title || T('视频')).slice(0, 100), expires: Date.now() + captureTTL, items: [] };
  await chrome.storage.session.set({ [captureKey]: captureState });
  attachCaptureListener();
  if (!captureListenerAttached) { await stopNetworkCapture(); return false; }
  updateCaptureBadge(tabId, 0);
  if (chrome.scripting?.executeScript) {
    chrome.scripting.executeScript({ target: { tabId }, func: installClickTracker }).catch(() => {});
  }
  await chrome.alarms.create('shandian-capture-expire', { when: captureState.expires });
  return true;
}
async function stopNetworkCapture() {
  const prevTabId = captureState?.tabId;
  captureState = null;
  detachCaptureListener();
  if (prevTabId != null) {
    updateCaptureBadge(prevTabId, 0);
    if (chrome.scripting?.executeScript) {
      chrome.scripting.executeScript({ target: { tabId: prevTabId }, func: removeClickTracker }).catch(() => {});
    }
  }
  await captureWrites;
  // 此权限只服务临时侦测，结束后主动撤销，下次需重新授权。
  await Promise.allSettled([
    chrome.storage.session.remove(captureKey),
    chrome.alarms.clear('shandian-capture-expire'),
    chrome.permissions.remove({ permissions: ['webRequest'] })
  ]);
}
async function capturedForTab(tabId) {
  await restoreCapture;
  if (!captureState) captureState = (await chrome.storage.session.get(captureKey))[captureKey] || null;
  if (captureState && Date.now() >= captureState.expires) await stopNetworkCapture();
  if (!captureState || captureState.tabId !== tabId) return { active: false, items: [] };
  return { active: true, items: captureState.items.map(item => ({ ...item, referrer: captureState.pageURL, title: captureState.title })) };
}
// 同步注册监听器，让浏览器可在 Service Worker 休眠时唤醒它。
if (chrome.webRequest?.onHeadersReceived) attachCaptureListener();
// 在 Service Worker 重启后恢复尚未过期的当前页捕获状态。
const restoreCapture = chrome.storage.session.get(captureKey).then(async data => {
  const state = data[captureKey];
  if (!state) {
    detachCaptureListener();
    return;
  }
  if (Date.now() >= state.expires || !(await chrome.permissions.contains({ permissions: ['webRequest'] }))) { await stopNetworkCapture(); return; }
  captureState = state; attachCaptureListener();
}).catch(async () => {
  captureState = null;
  detachCaptureListener();
  try { await chrome.permissions.remove({ permissions: ['webRequest'] }); } catch (_) {}
});
chrome.alarms.onAlarm.addListener(alarm => { if (alarm.name === 'shandian-capture-expire') stopNetworkCapture().catch(() => {}); });
chrome.runtime.onStartup.addListener(() => {
  restoreCapture.then(async () => { if (!captureState) await chrome.permissions.remove({ permissions: ['webRequest'] }); }).catch(() => {});
});
chrome.tabs.onRemoved.addListener(tabId => { if (captureState?.tabId === tabId) stopNetworkCapture().catch(() => {}); });
chrome.tabs.onUpdated.addListener((tabId, change) => { if (captureState?.tabId === tabId && change.status === 'loading') stopNetworkCapture().catch(() => {}); });
