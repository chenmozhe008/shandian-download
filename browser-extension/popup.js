var T = globalThis.T || ((text, values) => text.replace(/\{(\w+)\}/g, (_, key) => values?.[key]));  // 测试环境没有 i18n.js
// 点扩展图标弹出的小面板：视频网站页面上直接「下载这个视频」；下面是接管下载的开关
const VIDEO_SITES = ["douyin.com", "xiaohongshu.com", "bilibili.com", "youtube.com", "x.com", "twitter.com", "weibo.com", "ixigua.com", "kuaishou.com", "vimeo.com", "xinpianchang.com"];

async function main() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  let url;
  try { url = new URL(tab?.url || ""); } catch (_) {}
  const host = url?.hostname || "";
  const onVideoSite = VIDEO_SITES.some((d) => host === d || host.endsWith("." + d)) ||
    (host === "movie.douban.com" && /^\/trailer\/\d+\/?$/.test(url.pathname));
  const isWebPage = url && ["http:", "https:"].includes(url.protocol);
  const page = document.getElementById("page");
  if (host === "yuanbao.tencent.com") {
    const toggle = document.getElementById("wechat-toggle");
    const panel = document.getElementById("wechat");
    toggle.hidden = false;
    toggle.onclick = () => { panel.hidden = !panel.hidden; toggle.textContent = panel.hidden ? T("下载视频号 ›") : T("收起视频号下载"); };
    const wechatButton = document.getElementById("wechat-download");
    wechatButton.onclick = async () => {
      const share = document.getElementById("wechat-link").value.trim();
      let parsed; try { parsed = new URL(share); } catch (_) {}
      if (!parsed || parsed.protocol !== "https:" || parsed.hostname !== "weixin.qq.com" || !parsed.pathname.startsWith("/sph/")) {
        page.querySelector("#hint").textContent = T("请粘贴完整的视频号分享链接。"); return;
      }
      wechatButton.disabled = true; wechatButton.textContent = T("正在解析…");
      try {
        const r = await chrome.runtime.sendMessage({ type: "downloadWeChat", tabId: tab.id, share });
        page.querySelector("#hint").textContent = r?.ok ? T("已交给闪电全能下载，请在应用中查看进度。") : r?.login ? T("请先登录腾讯元宝，再重新解析。") : T("解析或添加失败，请确认元宝已登录且闪电全能下载已打开。");
      } catch (_) { page.querySelector("#hint").textContent = T("解析失败，请重试。"); }
      finally { wechatButton.disabled = false; wechatButton.textContent = T("解析并下载视频号"); }
    };
  }
  if (host === "mypikpak.com") {
    // PikPak 网页上：把这里登录着的账号关联到闪电全能下载（云盘离线用）
    page.querySelector("#title").textContent = T("PikPak 云盘");
    const btn = page.querySelector("#dl");
    btn.textContent = T("关联到闪电全能下载");
    page.querySelector("#hint").textContent = T("关联后磁力太慢时会自动交给 PikPak 离线下载。这个网页会退出登录（同一份登录不能两边同时用）。");
    btn.onclick = async () => {
      btn.disabled = true; btn.textContent = T("正在关联…");
      const r = await chrome.runtime.sendMessage({ type: "linkPikPak", tabId: tab.id });
      btn.textContent = r?.ok ? T("已关联") : T("没找到登录信息");
      if (!r?.ok) page.querySelector("#hint").textContent = T("请先在这个网页里登录 PikPak，再点一次。");
    };
  } else if (!isWebPage) {
    page.querySelector("#title").textContent = T("请先打开包含视频的网页");
    page.querySelector("#dl").style.display = "none";
    page.querySelector("#hint").textContent = T("浏览器设置等页面无法提取视频。");
  } else {
    page.querySelector("#title").textContent = tab.title || tab.url;
    const btn = page.querySelector("#dl");
    if (!onVideoSite) {
      btn.textContent = T("下载当前音视频");
      page.querySelector("#hint").textContent = T("播放网页中的音频或视频后，点此直接下载；识别不到时可先开启下方侦测。");
    }
    btn.onclick = async () => {
      btn.disabled = true; btn.textContent = T("正在交给闪电全能下载…");
      const r = await chrome.runtime.sendMessage({ type: "downloadPage", tabId: tab.id, requireVideo: !onVideoSite });
      btn.textContent = r?.ok ? T("已交给闪电全能下载") : (onVideoSite ? T("未能添加视频") : T("未能添加音视频"));
      page.querySelector("#hint").textContent = r?.ok ? T("在闪电全能下载里查看任务状态。") : r?.noVideo ? (onVideoSite ? T("没有找到可直接下载的视频，请先播放视频后重试。部分网站不支持此方式。") : T("没有找到正在播放的音视频。请先播放，或点击「侦测本页面音视频」后再试。")) : T("没能交给闪电全能下载，请确认应用已打开后重试。");
      if (!r?.ok) btn.disabled = false;
      if (r?.ok) setTimeout(() => window.close(), 1200);
    };
  }
  const scan = document.getElementById("scan");
  const capture = document.getElementById("capture");
  const stopConfirm = document.getElementById("stop-confirm");
  const stopConfirmText = document.getElementById("stop-confirm-text");
  const stopCancel = document.getElementById("stop-cancel");
  const stopOk = document.getElementById("stop-ok");
  const results = document.getElementById("results");
  const mediaBatchBar = document.getElementById("media-batch-actions");
  const mediaSelectAll = document.getElementById("media-select-all");
  const mediaDownloadSelected = document.getElementById("media-download-selected");
  if (isWebPage && host !== "mypikpak.com") {
    let capturing = false;
    let capturedCount = 0;
    let activePreviewBtn = null;
    let activeWaveBox = null;
    let previewPollTimer = null;
    const formatDuration = (sec) => {
      if (!Number.isFinite(sec) || sec <= 0) return "0:00";
      const total = Math.floor(sec);
      const m = Math.floor(total / 60);
      const s = total % 60;
      return `${m}:${String(s).padStart(2, "0")}`;
    };
    const buildWaveHeights = (seedText, count = 28) => {
      let h = 2166136261;
      for (const ch of seedText || "wave") h = Math.imul(h ^ ch.charCodeAt(0), 16777619);
      const out = [];
      for (let i = 0; i < count; i++) {
        h = Math.imul(h ^ (i + 17), 16777619) >>> 0;
        const env = Math.sin(((i + 0.5) / count) * Math.PI);
        const rnd = (h % 100) / 100;
        out.push(Math.round(24 + (0.35 + 0.65 * env) * rnd * 74));
      }
      return out;
    };
    const stopPreview = () => {
      if (previewPollTimer) { clearInterval(previewPollTimer); previewPollTimer = null; }
      if (activeWaveBox) { activeWaveBox.hidden = true; activeWaveBox = null; }
      if (activePreviewBtn) {
        activePreviewBtn.textContent = "▶";
        activePreviewBtn.classList?.remove("playing");
        activePreviewBtn = null;
        chrome.runtime.sendMessage({ type: "stopPreviewAudio", tabId: tab.id }).catch?.(() => {});
      }
    };
    const btn = page.querySelector("#dl");
    const updateMainButtonForMedia = (items) => {
      if (onVideoSite || !items?.length) return;
      const mediaList = items.filter((x) => x.kind === "音频" || x.kind === "视频");
      const latest = mediaList[0];
      if (!latest) return;
      if (latest.kind === "音频") btn.textContent = mediaList.length > 1 ? T("下载最新音频") : T("下载这条音频");
      else if (latest.kind === "视频") btn.textContent = mediaList.length > 1 ? T("下载最新视频") : T("下载这个视频");
    };
    const formatSize = (bytes) => {
      if (!bytes || bytes <= 0) return "";
      if (bytes < 1024) return `${bytes} B`;
      if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
      return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    };
    const cleanFilePart = (s, fallback) => (s || "").replace(/[\\/:*?"<>|]/g, " ").replace(/\s+/g, " ").trim().slice(0, 70) || fallback;
    const renderMediaItems = (items) => {
      if (!results.replaceChildren || !document.createElement) return;
      stopPreview();
      results.replaceChildren();
      const showBatch = items.length > 1 && mediaBatchBar && mediaSelectAll && mediaDownloadSelected;
      if (mediaBatchBar) mediaBatchBar.hidden = !showBatch;
      const rows = [];
      const syncBatchBar = () => {
        if (!showBatch) return;
        const checked = rows.filter((r) => r.checkbox.checked);
        mediaSelectAll.checked = rows.length > 0 && checked.length === rows.length;
        mediaDownloadSelected.disabled = checked.length === 0;
        if (checked.length >= 2) mediaDownloadSelected.textContent = T("打包下载（{n}个）", { n: checked.length });
        else if (checked.length === 1) mediaDownloadSelected.textContent = T("下载所选（{n}个）", { n: 1 });
        else mediaDownloadSelected.textContent = T("下载所选");
      };
      items.forEach((item, idx) => {
        const card = document.createElement("div"); card.className = "media-card";
        const row = document.createElement("div"); row.className = "media-row";
        const displayName = item.trackTitle || item.filename || item.title || T(item.kind);
        let checkbox = null;
        if (showBatch) {
          checkbox = document.createElement("input");
          checkbox.type = "checkbox";
          checkbox.checked = true;
          checkbox.onchange = syncBatchBar;
          row.appendChild(checkbox);
        }
        const info = document.createElement("div"); info.className = "media-info";
        const nameEl = document.createElement("div"); nameEl.className = "media-name";
        nameEl.textContent = displayName;
        nameEl.title = displayName;
        const metaEl = document.createElement("div"); metaEl.className = "media-meta";
        const parts = [];
        if (items.length > 1) parts.push(idx === 0 ? `#1 ${T("最新")}` : `#${idx + 1}`);
        parts.push(T(item.kind), item.ext.toUpperCase());
        if (item.size > 0) parts.push(formatSize(item.size));
        metaEl.textContent = parts.join(" · ");
        info.appendChild(nameEl);
        info.appendChild(metaEl);
        row.appendChild(info);
        let waveBox = null;
        if (item.kind === "音频" && !item.stream && !["m3u8", "mpd"].includes(item.ext)) {
          waveBox = document.createElement("div"); waveBox.className = "wave-player"; waveBox.hidden = true;
          const barsWrap = document.createElement("div"); barsWrap.className = "wave-bars";
          const heights = buildWaveHeights(item.url, 44);
          const barEls = heights.map((pct) => {
            const b = document.createElement("span");
            b.className = "wave-bar";
            b.style.height = `${pct}%`;
            barsWrap.appendChild(b);
            return b;
          });
          const cursorEl = document.createElement("span"); cursorEl.className = "wave-cursor";
          barsWrap.appendChild(cursorEl);
          const timeEl = document.createElement("span"); timeEl.className = "wave-time";
          timeEl.textContent = "0:00 / 0:00";
          let lastDuration = 0;
          const paintProgress = (cur, dur) => {
            if (dur > 0) lastDuration = dur;
            const ratio = lastDuration > 0 ? Math.max(0, Math.min(1, cur / lastDuration)) : 0;
            const playedCount = Math.floor(ratio * barEls.length);
            barEls.forEach((b, i) => {
              b.classList.toggle("played", ratio > 0 && i < playedCount);
            });
            cursorEl.style.left = `${(ratio * 100).toFixed(2)}%`;
            timeEl.textContent = lastDuration > 0
              ? `${formatDuration(cur)} / ${formatDuration(lastDuration)}`
              : formatDuration(cur);
          };
          barsWrap.onclick = (ev) => {
            if (lastDuration <= 0 || !barsWrap.getBoundingClientRect) return;
            const rect = barsWrap.getBoundingClientRect();
            const ratio = Math.max(0, Math.min(1, (ev.clientX - rect.left) / Math.max(1, rect.width)));
            const targetTime = ratio * lastDuration;
            paintProgress(targetTime, lastDuration);
            chrome.runtime.sendMessage({ type: "seekPreviewAudio", tabId: tab.id, time: targetTime }).catch?.(() => {});
          };
          waveBox.appendChild(barsWrap);
          waveBox.appendChild(timeEl);

          const playBtn = document.createElement("button"); playBtn.className = "media-preview";
          playBtn.textContent = "▶";
          playBtn.title = T("试听");
          playBtn.onclick = async () => {
            if (activePreviewBtn === playBtn) { stopPreview(); return; }
            stopPreview();
            activePreviewBtn = playBtn;
            activeWaveBox = waveBox;
            playBtn.textContent = "⏹";
            playBtn.classList?.add("playing");
            waveBox.hidden = false;
            paintProgress(0, 0);
            try {
              const res = await chrome.runtime.sendMessage({ type: "previewAudio", tabId: tab.id, url: item.url });
              if (!res?.ok && activePreviewBtn === playBtn) {
                stopPreview();
                page.querySelector("#hint").textContent = T("当前站点不支持扩展内试听，请直接下载。");
                return;
              }
              if (res?.duration > 0) paintProgress(0, res.duration);
              previewPollTimer = setInterval(async () => {
                if (activePreviewBtn !== playBtn) { clearInterval(previewPollTimer); return; }
                try {
                  const st = await chrome.runtime.sendMessage({ type: "previewAudioState", tabId: tab.id });
                  if (activePreviewBtn !== playBtn) return;
                  if (st?.ended) { stopPreview(); return; }
                  paintProgress(st?.currentTime || 0, st?.duration || lastDuration);
                } catch (_) {}
              }, 200);
            } catch (_) {
              if (activePreviewBtn === playBtn) {
                stopPreview();
                page.querySelector("#hint").textContent = T("当前站点不支持扩展内试听，请直接下载。");
              }
            }
          };
          row.appendChild(playBtn);
        }
        const dlBtn = document.createElement("button"); dlBtn.className = "media-dl";
        dlBtn.textContent = T("下载");
        const entry = { item, idx, checkbox, dlBtn, displayName };
        const doDownload = async (quiet = false) => {
          dlBtn.disabled = true;
          dlBtn.textContent = "…";
          try {
            const response = await chrome.runtime.sendMessage({ type: "downloadMedia", tabId: tab.id, item });
            dlBtn.disabled = false;
            if (response?.ok) {
              dlBtn.textContent = "✓";
              dlBtn.classList?.add("done");
              setTimeout(() => {
                dlBtn.textContent = T("下载");
                dlBtn.classList?.remove("done");
              }, 1500);
              if (!quiet) page.querySelector("#hint").textContent = T("已交给闪电全能下载。");
              return true;
            }
            dlBtn.textContent = T("重试");
            dlBtn.classList?.remove("done");
            if (!quiet) page.querySelector("#hint").textContent = T("没能交给闪电全能下载，请确认应用已打开后重试。");
            return false;
          } catch (_) {
            dlBtn.disabled = false;
            dlBtn.textContent = T("重试");
            dlBtn.classList?.remove("done");
            return false;
          }
        };
        dlBtn.onclick = () => doDownload(false);
        row.appendChild(dlBtn);
        card.appendChild(row);
        if (waveBox) card.appendChild(waveBox);
        results.appendChild(card);
        rows.push({ ...entry, doDownload });
      });
      if (showBatch) {
        mediaSelectAll.onchange = () => {
          for (const r of rows) r.checkbox.checked = mediaSelectAll.checked;
          syncBatchBar();
        };
        mediaDownloadSelected.onclick = async () => {
          const targets = rows.filter((r) => r.checkbox.checked);
          if (!targets.length) return;
          if (targets.length === 1) {
            await targets[0].doDownload(false);
            return;
          }
          mediaDownloadSelected.disabled = true;
          mediaDownloadSelected.textContent = T("正在打包…");
          const archiveStem = cleanFilePart(tab.title, T("音频合集"));
          const archiveName = `${archiveStem} (${targets.length}个).zip`;
          const directItems = targets.filter((r) => !r.item.stream && !["m3u8", "mpd"].includes(r.item.ext));
          let zipSent = false;
          if (directItems.length >= 2) {
            try {
              const res = await chrome.runtime.sendMessage({
                type: "downloadMediaZip",
                tabId: tab.id,
                archiveName,
                items: directItems.map((r) => r.item)
              });
              zipSent = !!res?.ok;
            } catch (_) {}
          }
          if (zipSent) {
            for (const r of directItems) {
              r.dlBtn.textContent = "✓";
              r.dlBtn.classList?.add("done");
              setTimeout(() => { r.dlBtn.textContent = T("下载"); r.dlBtn.classList?.remove("done"); }, 1500);
            }
            page.querySelector("#hint").textContent = T("已交给闪电全能下载打包为单个压缩包。");
          } else {
            page.querySelector("#hint").textContent = T("打包失败，请确认闪电全能下载已打开后重试。");
          }
          syncBatchBar();
        };
        syncBatchBar();
      }
    };
    const setCaptureLabel = () => {
      capture.textContent = capturing ? T("侦测中 · 停止") : T("侦测本页面音视频");
      capture.classList?.toggle("capture-active", capturing);
      capture.classList?.toggle("capture-prominent", !capturing && !onVideoSite);
    };
    const performStopCapture = async () => {
      if (stopConfirm) stopConfirm.hidden = true;
      capture.disabled = true;
      try {
        const result = await chrome.runtime.sendMessage({ type: "stopCapture", tabId: tab.id });
        if (!result?.ok) throw new Error("停止失败");
        capturing = false;
        capturedCount = 0;
        stopPreview();
        results.replaceChildren();
        if (mediaBatchBar) mediaBatchBar.hidden = true;
        page.querySelector("#hint").textContent = T("已停止侦测并清空记录。");
      } catch (_) {
        page.querySelector("#hint").textContent = T("侦测操作失败，请重新打开扩展后重试。");
      }
      finally { capture.disabled = false; setCaptureLabel(); }
    };
    if (stopCancel) stopCancel.onclick = () => { if (stopConfirm) stopConfirm.hidden = true; };
    if (stopOk) stopOk.onclick = performStopCapture;
    let captured = null;
    try { captured = await chrome.runtime.sendMessage({ type: "captureStatus", tabId: tab.id }); capturing = !!captured?.active; } catch (_) {}
    capturedCount = captured?.items?.length || 0;
    setCaptureLabel();
    if (capturing && capturedCount > 0) {
      const ordered = [...captured.items].reverse();
      updateMainButtonForMedia(ordered);
      renderMediaItems(ordered);
      page.querySelector("#hint").textContent = T("已捕获 {n} 条音视频（最新在最前），播放片刻即可下载完整文件。", { n: capturedCount });
    } else if (!onVideoSite && results.replaceChildren && document.createElement) {
      try {
        const scanned = await chrome.runtime.sendMessage({ type: "scanMedia", tabId: tab.id });
        const mediaItems = (scanned?.items || []).filter((x) => x.kind === "音频" || x.kind === "视频");
        if (mediaItems.length) {
          const ordered = [...mediaItems].reverse();
          updateMainButtonForMedia(ordered);
          renderMediaItems(ordered);
          page.querySelector("#hint").textContent = T("已发现 {n} 条音视频（最新在最前），点击右侧按钮可试听或下载。", { n: mediaItems.length });
        }
      } catch (_) {}
    }
    capture.onclick = async () => {
      if (capturing) {
        if (capturedCount > 0 && stopConfirm) {
          if (stopConfirmText) stopConfirmText.textContent = T("停止侦测并清空这 {n} 条记录？", { n: capturedCount });
          stopConfirm.hidden = false;
          return;
        }
        await performStopCapture();
        return;
      }
      if (stopConfirm) stopConfirm.hidden = true;
      capture.disabled = true;
      try {
        // 浏览器只允许在用户点击时申请可选权限；不会在安装扩展时自动授予。
        const granted = await chrome.permissions.request({ permissions: ["webRequest"] });
        if (!granted) { page.querySelector("#hint").textContent = T("未授权网络侦测，其他下载功能仍可正常使用。"); return; }
        const result = await chrome.runtime.sendMessage({ type: "startCapture", tabId: tab.id });
        if (!result?.ok) { await chrome.permissions.remove({ permissions: ["webRequest"] }); throw new Error("启动失败"); }
        capturing = true;
        capturedCount = 0;
        page.querySelector("#hint").textContent = T("已开启侦测。现在播放网页中的音频或视频，稍后重新点开扩展即可下载。");
      } catch (_) {
        if (!capturing) try { await chrome.permissions.remove({ permissions: ["webRequest"] }); } catch (_) {}
        page.querySelector("#hint").textContent = T("侦测操作失败，请重新打开扩展后重试。");
      }
      finally { capture.disabled = false; setCaptureLabel(); }
    };
    scan.onclick = async () => {
      if (stopConfirm) stopConfirm.hidden = true;
      scan.disabled = true; scan.textContent = T("正在扫描…"); stopPreview(); results.replaceChildren(); document.body.classList.remove("batch-mode"); document.getElementById("batch-actions").hidden = true; if (mediaBatchBar) mediaBatchBar.hidden = true;
      try {
        const response = await chrome.runtime.sendMessage({ type: "scanMedia", tabId: tab.id });
        const items = response?.items || [];
        page.querySelector("#hint").textContent = items.length ? T("已发现 {n} 条音视频或字幕，点击右侧按钮下载。", { n: items.length }) + (items.some(item => ["m3u8", "mpd"].includes(item.ext)) ? T("部分分片流需登录或受版权保护，可能无法下载。") : "") : T("未扫描到音视频或字幕，请先播放后重试，或开启「侦测本页面音视频」。");
        const ordered = [...items].reverse();
        updateMainButtonForMedia(ordered);
        renderMediaItems(ordered);
      } catch (_) { page.querySelector("#hint").textContent = T("读取页面失败，请刷新页面后重试。"); }
      finally { scan.disabled = false; scan.textContent = T("重新扫描"); }
    };
  } else { scan.style.display = "none"; capture.style.display = "none"; document.getElementById("capture-note").style.display = "none"; }
  const box = document.getElementById("takeover");
  const { takeover = true } = await chrome.storage.local.get("takeover");
  box.checked = takeover;
  box.onchange = async () => {
    await chrome.storage.local.set({ takeover: box.checked });
    chrome.runtime.sendMessage({ type: "refreshBadge" });
  };
}
main();
