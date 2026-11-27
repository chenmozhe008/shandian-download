// Source names are independent of the transport protocol (a cloud file still uses HTTP).
(() => {
  const groups = [
    ["百度网盘",["pan.baidu.com","baidupcs.com","pcs.baidu.com"]],
    ["阿里云盘",["alipan.com","aliyundrive.com","aliyundrive.net"]],
    ["夸克网盘",["pan.quark.cn","drive.quark.cn","drive-pc.quark.cn"]],
    ["UC 网盘",["drive.uc.cn","drive-pc.uc.cn","pds.uc.cn"]],
    ["123 云盘",["123pan.com","123pan.cn","123684.com","123865.com","123952.com","123912.com","123957.com"]],
    ["天翼云盘",["cloud.189.cn","ctyunxs.cn"]],
    ["移动云盘",["yun.139.com","caiyun.139.com","mcloud.139.com","caiyun.com"]],
    ["迅雷云盘",["pan.xunlei.com","xunlei.com","sandai.net"]],
    ["光鸭云盘",["guangyapan.com","guangya.com","guangyacdn.com"]],
    ["115 网盘",["115.com","115cdn.net","115cdn.com"]],
    ["蓝奏云",["lanzou.com","lanzoui.com","lanzoux.com"]],
    ["GitHub",["github.com","githubusercontent.com","githubassets.com"]],
    ["Gitee",["gitee.com"]],
    ["轻雀文档",["docs.qingque.cn"]],
    ["Hugging Face",["huggingface.co","hf-mirror.com"]],
    ["Dropbox",["dropbox.com","dropboxusercontent.com"]],
    ["Google Drive",["drive.google.com","drive.usercontent.google.com"]],
    ["OneDrive",["1drv.ms","onedrive.live.com","sharepoint.com"]],
    ["MEGA",["mega.nz","mega.io"]],
    ["PikPak",["mypikpak.com"]],
    ["B 站",["bilibili.com","b23.tv"]],["YouTube",["youtube.com","youtu.be"]],
    ["Vimeo",["vimeo.com"]],["抖音",["douyin.com"]],["小红书",["xiaohongshu.com","xhslink.com"]],
    ["Facebook",["facebook.com","fb.watch"]],
    ["Instagram",["instagram.com"]],
    ["微博",["weibo.com","weibo.cn"]],["X",["x.com","twitter.com"]],
    ["Pixabay",["pixabay.com"]],["爱给",["aigei.com"]],
  ];
  function label(value, page) {
    for (const link of [value,page]) {
      try {
        const host=new URL(link).hostname.toLowerCase();
        const match=groups.find(([,hosts])=>hosts.some(root=>host===root || host.endsWith("."+root)));
        if (match) return match[0];
      } catch (_) {}
    }
    return "";
  }
  const explicit=value=>typeof value==="string" && !["直链","网盘","视频网站",""].includes(value) ? value.replace(/^NAS · /,"") : "";
  function task(item) {
    const req=item.meta?.req || {}, extra=req.extra || {};
    const file=extra.files?.[0];
    const headers=file?.header || extra.header || {};
    const referer=Object.entries(headers).find(([key])=>/^(referer|origin)$/i.test(key))?.[1];
    return explicit(item.source) || explicit(req.labels?.["shandian.source"]) ||
      label(file?.url || req.url || item.url || item.link, referer || item.page) ||
      (Object.entries(headers).some(([key,value])=>/^user-agent$/i.test(key)&&value==="pan.baidu.com")?"百度网盘":"");
  }
  function socialVideo(url) {
    if (!url || !["http:", "https:"].includes(url.protocol)) return false;
    const host = url.hostname;
    const path = url.pathname.replace(/\/$/, "");
    if (host === "instagram.com" || host.endsWith(".instagram.com")) {
      return /^\/(?:(?!(?:share|accounts|explore|stories)\/)[^/]+\/)?(?:p|reels?|tv)\/(?!audio$)[\w-]+$/.test(path);
    }
    if (host === "fb.watch" || host.endsWith(".fb.watch")) return /^\/[\w-]+$/.test(path);
    if (host !== "facebook.com" && !host.endsWith(".facebook.com")) return false;
    return /^\/share\/[vr]\/[\w-]+$/.test(path) || /^\/reels?\/\d+$/.test(path) ||
      /(?:^|\/)videos\/(?:[^/]+\/)*\d+$/.test(path) ||
      (/^\/(?:watch(?:\/live)?|video\.php)$/.test(path) && /^\d+$/.test(url.searchParams.get("v") || ""));
  }
  globalThis.ShSources={label,task,socialVideo};
})();
