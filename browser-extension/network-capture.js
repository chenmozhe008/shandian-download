var T = globalThis.T || ((text, values) => text.replace(/\{(\w+)\}/g, (_, key) => values?.[key]));  // 测试环境没有 i18n.js
// 可选的当前标签页网络资源发现。只保存媒体 URL 与类型到浏览器会话内存，不读取请求体、Cookie 或授权头。
const captureKey = 'shandianActiveCapture';
const captureTTL = 5 * 60 * 1000;
let captureState = null;
let captureListenerAttached = false;
let captureWrites = Promise.resolve();

function classifyCapturedMedia(details) {
  if (details.method !== 'GET' || ![200, 206].includes(details.statusCode)) return null;
  if (typeof details.url !== 'string' || details.url.length > 8192) return null;
  let parsed;
  try { parsed = new URL(details.url); } catch (_) { return null; }
  if (!['http:', 'https:'].includes(parsed.protocol)) return null;
  // 分片不是完整媒体，不能当成独立文件下载。
  if (/\.(m4s|ts|cmfv|cmfa)(?:$)/i.test(parsed.pathname)) return null;
  const mime = (details.responseHeaders || []).find(h => h.name.toLowerCase() === 'content-type')?.value?.split(';')[0].trim().toLowerCase() || '';
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
    'audio/ogg': ['ogg', '音频', false]
  };
  const type = types[mime];
  if (!type) return null;
  return { url: parsed.href, ext: type[0], kind: type[1], stream: type[2], label: type[1] + ' · 本页侦测' };
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
function handleCapturedResponse(details) {
  if (!captureState || details.tabId !== captureState.tabId || Date.now() >= captureState.expires) return;
  const item = classifyCapturedMedia(details);
  if (!item || captureState.items.some(x => x.url === item.url) || captureState.items.length >= 100) return;
  captureState.items.push(item);
  // session 存储在浏览器内存中；停止捕获、过期、标签页关闭或浏览器退出时清除。
  const snapshot = { ...captureState, items: [...captureState.items] };
  captureWrites = captureWrites.then(() => chrome.storage.session.set({ [captureKey]: snapshot })).catch(() => {});
}
async function startNetworkCapture(tabId, pageURL, title) {
  await restoreCapture;
  await captureWrites;
  if (!(await chrome.permissions.contains({ permissions: ['webRequest'] }))) return false;
  const url = new URL(pageURL);
  if (!['http:', 'https:'].includes(url.protocol)) return false;
  // 来源页的查询参数可能包含私密数据；仅保留地址路径。
  url.username = ''; url.password = ''; url.search = ''; url.hash = '';
  captureState = { tabId, pageURL: url.href, title: (title || T('视频')).slice(0, 100), expires: Date.now() + captureTTL, items: [] };
  await chrome.storage.session.set({ [captureKey]: captureState });
  attachCaptureListener();
  if (!captureListenerAttached) { await stopNetworkCapture(); return false; }
  await chrome.alarms.create('shandian-capture-expire', { when: captureState.expires });
  return true;
}
async function stopNetworkCapture() {
  captureState = null;
  detachCaptureListener();
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
