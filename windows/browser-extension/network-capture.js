// 可选的当前标签页网络资源发现。只保存媒体 URL 与类型到浏览器会话内存，不读取请求体、Cookie 或授权头。
const captureKey = 'shandianActiveCapture';
const captureTTL = 5 * 60 * 1000;
let captureState = null;
let captureListenerAttached = false;
let captureWrites = Promise.resolve();
// Temporary address observers share the optional permission with media capture.
// A batch holds one lease until its existing queue finishes, not one per file.
const networkPermissionLeases = new Set();
function retainNetworkPermission(owner) { networkPermissionLeases.add(owner); }
async function releaseUnusedNetworkPermission() {
  if (!captureState && !networkPermissionLeases.size) await chrome.permissions.remove({ permissions: ['webRequest'] });
}
async function releaseNetworkPermission(owner) {
  networkPermissionLeases.delete(owner);
  await releaseUnusedNetworkPermission();
}

function classifyCapturedMedia(details) {
  if (details.method !== 'GET' || ![200, 206].includes(details.statusCode)) return null;
  if (typeof details.url !== 'string' || details.url.length > 8192) return null;
  let parsed;
  try { parsed = new URL(details.url); } catch (_) { return null; }
  if (!['http:', 'https:'].includes(parsed.protocol)) return null;
  if (/(^|\.)feishu\.cn$/i.test(parsed.hostname) &&
      /^\/space\/api\/box\/stream\/download\/preview(?:\/|$)/i.test(parsed.pathname)) return null;
  // 分片不是完整媒体，不能当成独立文件下载。
  if (/\.(m4s|ts|cmfv|cmfa)(?:$)/i.test(parsed.pathname)) return null;
  const headers = details.responseHeaders || [];
  const header = (name) => headers.find(h => h.name.toLowerCase() === name)?.value || '';
  const mime = header('content-type').split(';')[0].trim().toLowerCase();
  const types = {
    'application/vnd.apple.mpegurl': ['m3u8', '视频', true],
    'application/x-mpegurl': ['m3u8', '视频', true],
    'audio/mpegurl': ['m3u8', '视频', true],
    'audio/x-mpegurl': ['m3u8', '视频', true],
    'application/dash+xml': ['mpd', '视频', true],
    'video/mp4': ['mp4', '视频', false],
    'video/webm': ['webm', '视频', false],
    'video/quicktime': ['mov', '视频', false],
    'audio/mpeg': ['mp3', '音频', false],
    'audio/mp4': ['m4a', '音频', false],
    'audio/ogg': ['ogg', '音频', false],
    'audio/aac': ['aac', '音频', false],
    'audio/wav': ['wav', '音频', false],
    'audio/x-wav': ['wav', '音频', false],
    'audio/flac': ['flac', '音频', false],
    'audio/x-flac': ['flac', '音频', false],
    'audio/mp3': ['mp3', '音频', false],
    'audio/x-m4a': ['m4a', '音频', false],
    'audio/opus': ['opus', '音频', false],
    'audio/webm': ['webm', '音频', false],
    'audio/aiff': ['aiff', '音频', false],
    'audio/x-aiff': ['aiff', '音频', false]
  };
  let type = types[mime];
  if (!type) {
    // 有些 CDN 把 mp4 标成 octet-stream，或者干脆不给 Content-Type：按扩展名兜底。
    // 只认媒体后缀，且响应体不能是明显很小的接口返回，避免把 JSON / 网页当视频。
    if (mime && !['application/octet-stream', 'binary/octet-stream'].includes(mime)) return null;
    const ext = parsed.pathname.match(/\.(mp4|m4v|mov|webm|mp3|m4a|m4b|aac|ogg|oga|opus|wav|flac|aif|aiff|m3u8|mpd)$/i)?.[1]?.toLowerCase();
    const byExt = {
      mp4: ['mp4', '视频', false], m4v: ['m4v', '视频', false], mov: ['mov', '视频', false],
      webm: ['webm', '视频', false], mp3: ['mp3', '音频', false], m4a: ['m4a', '音频', false],
      aac: ['aac', '音频', false], ogg: ['ogg', '音频', false],
      wav: ['wav', '音频', false], flac: ['flac', '音频', false], m4b: ['m4b', '音频', false],
      oga: ['oga', '音频', false], opus: ['opus', '音频', false], aif: ['aif', '音频', false], aiff: ['aiff', '音频', false],
      m3u8: ['m3u8', '视频', true], mpd: ['mpd', '视频', true]
    };
    type = ext && byExt[ext];
    if (!type) return null;
    const length = Number(header('content-length')) || 0;
    if (type[1] !== '音频' && length > 0 && length < 256 * 1024) return null;
  }
  let filename = parsed.pathname.split('/').pop().replace(/\.[a-z0-9]{2,5}$/i, '');
  try { filename = decodeURIComponent(filename).trim(); } catch (_) {}
  if (/^(?:[a-f\d_-]{16,}|[a-z\d]{20,}|\d+)$/i.test(filename)) filename = '';
  const size = Number(details.statusCode === 206 ? header('content-range').match(/\/(\d+)$/)?.[1] : header('content-length'));
  return {
    url: parsed.href, ext: type[0], kind: type[1], stream: type[2],
    label: type[1] + ' · 本页侦测', filename: filename.slice(0, 80),
    ...(Number.isSafeInteger(size) && size > 0 ? { size } : {})
  };
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

// Record the clicked card's title only while this tab is being checked for media.
function trackAigeiCard(token) {
  const previous = window.__shandianAigeiCard;
  if (previous) {
    document.removeEventListener('click', previous.onClick, true);
    window.removeEventListener('message', previous.onPlayback);
  }
  delete window.__shandianAigeiCard;
  if (!token) return;
  const current = { title: '', time: 0 };
  current.onClick = event => {
    if (!event.isTrusted) return;
    const row = event.target.closest?.('[id^="audioItemBox_"], .mpic-tpl');
    current.title = (row?.querySelector('.title-name')?.textContent || '').trim().slice(0, 80);
    current.time = Date.now();
  };
  current.onPlayback = event => {
    const data = event.data;
    if (event.source !== window || data?.type !== 'shandian-aigei-playback' || data.token !== token) return;
    chrome.runtime.sendMessage({
      type: 'aigeiPlayback', token, url: data.url, duration: data.duration,
      trackTitle: Date.now() - current.time < 10000 ? current.title : ''
    }).catch(() => {});
  };
  window.__shandianAigeiCard = current;
  document.addEventListener('click', current.onClick, true);
  window.addEventListener('message', current.onPlayback);
}

// Report successful playback, including Audio objects that never enter the DOM.
function observeAigeiPlayback(token) {
  window.__shandianAigeiPlayback?.stop();
  if (!token) return;
  const prototype = HTMLMediaElement.prototype;
  const originalPlay = prototype.play;
  const observer = {
    stop() {
      document.removeEventListener('playing', playing, true);
      if (prototype.play === wrappedPlay) prototype.play = originalPlay;
      if (window.__shandianAigeiPlayback === observer) delete window.__shandianAigeiPlayback;
    }
  };
  const report = audio => {
    if (window.__shandianAigeiPlayback !== observer || !(audio instanceof HTMLAudioElement) || audio.paused) return;
    const url = audio.currentSrc || audio.src;
    if (/^https?:/i.test(url)) window.postMessage({ type: 'shandian-aigei-playback', token, url, duration: audio.duration }, location.origin);
  };
  function wrappedPlay(...args) {
    const started = Reflect.apply(originalPlay, this, args);
    started?.then(() => report(this), () => {});
    return started;
  }
  const playing = event => report(event.target);
  window.__shandianAigeiPlayback = observer;
  prototype.play = wrappedPlay;
  document.addEventListener('playing', playing, true);
  for (const audio of document.querySelectorAll('audio')) report(audio);
}

async function setAigeiObservers(tabId, token) {
  const target = { tabId };
  await chrome.scripting.executeScript({ target, func: trackAigeiCard, args: [token] }).catch(() => {});
  await chrome.scripting.executeScript({ target, world: 'MAIN', func: observeAigeiPlayback, args: [token] }).catch(() => {});
}

function storeCapture(state) {
  const snapshot = { ...state, items: state.items.map(item => ({ ...item })) };
  captureWrites = captureWrites.then(() => chrome.storage.session.set({ [captureKey]: snapshot })).catch(() => {});
}

function updateCaptureBadge(state, clear = false) {
  if (!state?.aigei || !chrome.action) return;
  const count = state.items.filter(item => !item.stream).length;
  const visible = !clear && count > 0;
  const tabId = state.tabId;
  chrome.action.setBadgeBackgroundColor?.({ tabId, color: visible ? "#F2C15B" : "#8E8E93" })?.catch(() => {});
  chrome.action.setBadgeTextColor?.({ tabId, color: visible ? "#2A1C00" : "#FFFFFF" })?.catch(() => {});
  chrome.action.setBadgeText({ tabId, text: visible ? String(count) : null }).catch(() => {});
}

function recordCapturedMedia(state, item) {
  const existing = state.items.find(x => x.url === item.url);
  if (existing) {
    let changed = false;
    for (const key of ['size', 'duration', 'trackTitle']) {
      if (item[key] && item[key] !== existing[key]) {
        existing[key] = item[key];
        changed = true;
      }
    }
    if (changed) storeCapture(state);
    return false;
  }
  if (state.items.length >= 100) return false;
  state.items.push(item);
  storeCapture(state);
  updateCaptureBadge(state);
  return true;
}

async function handleAigeiPlayback(message, sender) {
  await restoreCapture;
  const state = captureState;
  if (!state?.aigei || state.playerToken !== message.token || state.tabId !== sender.tab?.id ||
      sender.frameId !== 0 || Date.now() >= state.expires) return;
  try {
    const page = new URL(sender.url);
    page.search = page.hash = '';
    if (page.href !== state.pageURL) return;
    const url = new URL(message.url);
    if (url.username || url.password || !/(^|\.)aigei\.com$/i.test(url.hostname)) return;
    const item = classifyCapturedMedia({ method: 'GET', statusCode: 200, url: url.href });
    if (item?.kind !== '音频' || item.stream) return;
    if (typeof message.trackTitle === 'string') item.trackTitle = message.trackTitle.trim().slice(0, 80);
    if (Number.isFinite(message.duration) && message.duration > 0) item.duration = message.duration;
    recordCapturedMedia(state, item);
  } catch (_) {}
}

function handleCapturedResponse(details) {
  if (!captureState || details.tabId !== captureState.tabId || Date.now() >= captureState.expires) return;
  const item = classifyCapturedMedia(details);
  const state = captureState;
  if (!item || !recordCapturedMedia(state, item)) return;
  if (state.aigei && item.kind === '音频') {
    chrome.scripting.executeScript({
      target: { tabId: state.tabId },
      func: time => {
        const picked = window.__shandianAigeiCard;
        const age = time - (picked?.time || 0);
        return age >= 0 && age < 10000 ? picked.title : '';
      },
      args: [Date.now()]
    }).then(([frame]) => {
      if (captureState !== state || !frame?.result || item.trackTitle) return;
      item.trackTitle = frame.result;
      storeCapture(state);
    }).catch(() => {});
  }
}
async function startNetworkCapture(tabId, pageURL, title) {
  await restoreCapture;
  await captureWrites;
  if (!(await chrome.permissions.contains({ permissions: ['webRequest'] }))) return false;
  const url = new URL(pageURL);
  if (!['http:', 'https:'].includes(url.protocol)) return false;
  if (captureState?.aigei && captureState.tabId !== tabId) {
    updateCaptureBadge(captureState, true);
    await setAigeiObservers(captureState.tabId, null);
    await captureWrites;
  }
  // 来源页的查询参数可能包含私密数据；仅保留地址路径。
  url.username = ''; url.password = ''; url.search = ''; url.hash = '';
  const aigei = /^(www\.)?aigei\.com$/.test(url.hostname);
  const playerToken = aigei ? crypto.randomUUID() : null;
  captureState = { tabId, pageURL: url.href, title: (title || '视频').slice(0, 100), expires: Date.now() + captureTTL, items: [], aigei, playerToken };
  await chrome.storage.session.set({ [captureKey]: captureState });
  updateCaptureBadge(captureState);
  attachCaptureListener();
  if (!captureListenerAttached) { await stopNetworkCapture(); return false; }
  await chrome.alarms.create('shandian-capture-expire', { when: captureState.expires });
  if (aigei) await setAigeiObservers(tabId, playerToken);
  return true;
}
async function stopNetworkCapture() {
  const state = captureState;
  captureState = null;
  detachCaptureListener();
  updateCaptureBadge(state, true);
  await captureWrites;
  // 此权限只服务临时侦测，结束后主动撤销，下次需重新授权。
  await Promise.allSettled([
    chrome.storage.session.remove(captureKey),
    chrome.alarms.clear('shandian-capture-expire'),
    releaseUnusedNetworkPermission(),
    state?.aigei && setAigeiObservers(state.tabId, null)
  ]);
}
async function capturedForTab(tabId) {
  await restoreCapture;
  if (!captureState) captureState = (await chrome.storage.session.get(captureKey))[captureKey] || null;
  if (captureState && Date.now() >= captureState.expires) await stopNetworkCapture();
  if (!captureState || captureState.tabId !== tabId) return { active: false, items: [] };
  return { active: true, items: captureState.items.map(item => ({ ...item, referrer: captureState.pageURL, title: captureState.title })) };
}

function rememberCapturedDuration(tabId, url, duration) {
  if (captureState?.tabId !== tabId || !Number.isFinite(duration) || duration <= 0) return;
  const item = captureState.items.find(item => item.url === url);
  if (!item || item.duration === duration) return;
  item.duration = duration;
  storeCapture(captureState);
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
  captureState = state; attachCaptureListener(); updateCaptureBadge(state);
}).catch(async () => {
  captureState = null;
  detachCaptureListener();
  try { await releaseUnusedNetworkPermission(); } catch (_) {}
});
chrome.alarms.onAlarm.addListener(alarm => { if (alarm.name === 'shandian-capture-expire') stopNetworkCapture().catch(() => {}); });
chrome.runtime.onStartup.addListener(() => {
  restoreCapture.then(releaseUnusedNetworkPermission).catch(() => {});
});
chrome.tabs.onRemoved.addListener(tabId => { if (captureState?.tabId === tabId) stopNetworkCapture().catch(() => {}); });
chrome.tabs.onUpdated.addListener((tabId, change) => { if (captureState?.tabId === tabId && change.status === 'loading') stopNetworkCapture().catch(() => {}); });
