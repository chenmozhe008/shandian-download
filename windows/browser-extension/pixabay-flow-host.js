// This document has no extension API access and cannot create downloads.
function openOriginalPage(){
  try {
    const target=new URL(decodeURIComponent(location.hash.slice(1)));
    if(target.protocol!=='https:' || !/^(www\.)?pixabay\.com$/.test(target.hostname) || !/^\/(music|sound-effects|videos)\/.+-\d+\/?$/.test(target.pathname))throw Error('Invalid original-site URL');
    location.replace(target.href);
  } catch (_) { document.getElementById('status').textContent='原站下载地址无效，请关闭此页。'; }
}
// A failed navigation can leave this sandbox open; reuse it for the next item.
window.addEventListener('hashchange',openOriginalPage);
openOriginalPage();
