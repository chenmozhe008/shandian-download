// Shared by the worker and isolated page/popup scripts. Only exact provider hosts match.
const EXTRA_CLOUD_SITES = [
  { id: "pan123", name: "123 云盘", hosts: ["123pan.com", "123pan.cn", "123684.com", "123865.com", "123952.com", "123912.com", "123957.com"], subdomains: ["www", "yun", "share"], home: "https://yun.123pan.cn/", bar: '.home-operator .home-operator-button-group, .content .content-header-container-wrap .rightInfo, .single-file-sharing-container-content-file-operate, .rightInfo' },
  { id: "tianyi", name: "天翼云盘", hosts: ["cloud.189.cn"], home: "https://cloud.189.cn/web/main/", bar: '[class*="FileHead_file-head-left"], .nav-opea' },
  { id: "mobile", name: "移动云盘", hosts: ["yun.139.com", "caiyun.139.com"], home: "https://yun.139.com/w/", bar: ".top_button, .top-btns" },
  { id: "xunlei", name: "迅雷云盘", hosts: ["pan.xunlei.com"], home: "https://pan.xunlei.com/", bar: '[class*="FileMenu__menus--"], [class*="FileMenu__menu--"], [class^="Share__batchActionBox--"]' },
  { id: "guangya", name: "光鸭云盘", hosts: ["www.guangyapan.com", "guangyapan.com"], home: "https://www.guangyapan.com/", bar: '#root #app div:has(> .ant-btn .swangpan-icon-upload):has(> .ant-btn .swangpan-icon-addfolder), #root #app div:has(> .ant-btn .swangpan-icon-download)' },
];
function isBaiduSharePage(url) {
  return url?.hostname === "pan.baidu.com"
    && /^\/(?:s\/|share\/(?:init|link)\/?$)/.test(url.pathname);
}

function extraCloudSite(value) {
  try {
    const url = new URL(value);
    if (url.protocol !== "https:") return null;
    return EXTRA_CLOUD_SITES.find(site => site.hosts.some(host => url.hostname === host ||
      (site.subdomains || []).some(sub => url.hostname === `${sub}.${host}` || (sub === "share" && url.hostname.endsWith(`.share.${host}`))))) || null;
  } catch (_) { return null; }
}

// Account association is deliberately narrower than file-download support.
// Only providers with a matching native session importer appear here.
const CLOUD_ACCOUNT_LINK_SITES = [
  { id:"pikpak", name:"PikPak", hosts:["mypikpak.com"], home:"https://mypikpak.com/drive/", transferSession:true },
  { id:"guangya", name:"光鸭云盘", hosts:["www.guangyapan.com","guangyapan.com"], home:"https://www.guangyapan.com/", transferSession:true },
  { id:"quark", name:"夸克网盘", hosts:["pan.quark.cn"], home:"https://pan.quark.cn/", cookieDomain:"quark.cn", transferSession:false },
  { id:"uc", name:"UC 网盘", hosts:["drive.uc.cn"], home:"https://drive.uc.cn/", cookieDomain:"uc.cn", transferSession:false },
  { id:"pan115", name:"115 网盘", hosts:["115.com","www.115.com","115cdn.com","www.115cdn.com"], home:"https://115.com/?mode=wangpan", cookieDomains:["115.com","115cdn.com"], transferSession:false },
  { id:"pan123", name:"123 云盘", hosts:EXTRA_CLOUD_SITES.find(site=>site.id==="pan123").hosts.flatMap(host=>[host,"www."+host,"yun."+host]), home:"https://yun.123pan.cn/", transferSession:false },
  { id:"mobile", name:"移动云盘", hosts:["yun.139.com","caiyun.139.com"], home:"https://yun.139.com/w/", cookieDomain:"139.com", transferSession:false },
];
function cloudAccountLinkSite(value) {
  try {
    const url=new URL(value);
    if(url.protocol!=="https:" || url.username || url.password || (url.port && url.port!=="443"))return null;
    return CLOUD_ACCOUNT_LINK_SITES.find(site=>site.hosts.includes(url.hostname)) || null;
  } catch { return null; }
}

function cloudDownloadTab(sender, tabId) {
  if (sender.tab) return sender.tab;
  if (sender.url === chrome.runtime.getURL("popup.html") && Number.isInteger(tabId)) {
    return chrome.tabs.get(tabId);
  }
  return null;
}
