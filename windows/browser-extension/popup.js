// 点扩展图标弹出的小面板：视频网站页面上直接「闪电下载」；下面是接管下载的开关
const VIDEO_SITES = ["douyin.com", "xiaohongshu.com", "bilibili.com", "youtube.com", "x.com", "twitter.com", "weibo.com", "ixigua.com", "kuaishou.com", "vimeo.com", "xinpianchang.com", "b23.tv", "youtu.be"];

function showFeishuDownloads(tab, page) {
  page.querySelector("#title").textContent = tab.title || "飞书文档";
  const scan = page.querySelector("#dl");
  const hint = page.querySelector("#hint");
  const list = document.createElement("div");
  list.className = "results";
  list.hidden = true;
  const actions = document.createElement("div");
  actions.className = "batch-actions";
  actions.hidden = true;
  const label = document.createElement("label");
  const all = document.createElement("input");
  all.type = "checkbox";
  label.append(all, " 全选");
  const download = document.createElement("button");
  download.type = "button";
  actions.append(label, download);
  scan.parentElement.after(list, actions);
  scan.hidden = false;
  scan.disabled = false;
  scan.textContent = "扫描飞书原始音视频";

  const showError = (error, fallback) => {
    hint.textContent = error?.message || fallback;
    if (!/^https:\/\/open\.feishu\.cn\/app(?:\/cli_[a-z0-9]+\/auth)?$/i.test(error?.permissionUrl || "")) return;
    const instructions = document.createElement("p");
    instructions.textContent = "选「用户身份权限」，勾选查看新版文档、下载云文档中的图片和附件、查看知识空间节点信息，确认开通后重新扫描。";
    if (error.policyBlocked) {
      instructions.textContent += "已开通并发布仍报 21001 时，请联系飞书核查数据安全策略。";
    }
    const open = document.createElement("button");
    open.type = "button";
    open.className = "scan";
    open.textContent = "打开飞书权限管理";
    open.onclick = () => chrome.tabs.create({ url: error.permissionUrl }).catch(() => {
      hint.textContent = "未能打开飞书权限管理，请重试。";
    });
    hint.append(instructions, open);
  };
  const send = (type, extra = {}) => chrome.runtime.sendMessage({type, tabId:tab.id, ...extra});
  const openApp = async () => {
    const result = await send("downloadFeishuDocument");
    hint.textContent = result?.ok ? "请在闪电中完成授权并选择附件。" : "请打开闪电全能下载后重试。";
  };

  scan.onclick = async () => {
    scan.disabled = true;
    list.hidden = actions.hidden = true;
    list.replaceChildren();
    hint.textContent = "正在读取整篇飞书文档的原始音视频…";
    try {
      const result = await send("scanFeishuMedia");
      if (!result?.ok) {
        if (result?.error?.permissionUrl) showError(result.error, "读取飞书文档失败，请重试。");
        else if (result?.error?.needsAuthorization || result?.unavailable) await openApp();
        else showError(result?.error, "读取飞书文档失败，请重试。");
        return;
      }
      const doc = result.document;
      if (!doc?.ticket || !Array.isArray(doc.files) || !doc.files.length) {
        hint.textContent = "没有找到可下载的原始音视频附件。";
        return;
      }
      const picks = doc.files.map((file,index) => {
        const row = document.createElement("label");
        row.className = "batch-row";
        const box = document.createElement("input");
        box.type = "checkbox";
        box.checked = true;
        box.dataset.index = index;
        const name = document.createElement("span");
        name.textContent = file.name;
        name.title = file.name;
        name.style.cssText = "flex:1;min-width:0;overflow-wrap:anywhere";
        const size = document.createElement("small");
        size.textContent = file.size >= 1024*1024 ? `${(file.size/(1024*1024)).toFixed(1)} MB`
          : file.size >= 1024 ? `${Math.round(file.size/1024)} KB` : file.size ? `${file.size} B` : "";
        size.style.whiteSpace = "nowrap";
        row.append(box,name,size);
        list.append(row);
        return box;
      });
      const selected = () => picks.filter(box => box.checked).map(box => Number(box.dataset.index));
      const render = () => {
        const count = selected().length;
        all.checked = count === picks.length;
        all.indeterminate = count > 0 && count < picks.length;
        download.disabled = count === 0;
        download.textContent = `下载所选（${count}）`;
      };
      all.onchange = () => { for (const box of picks) box.checked = all.checked; render(); };
      for (const box of picks) box.onchange = render;
      render();
      list.hidden = actions.hidden = false;
      hint.textContent = doc.title || "";
      download.onclick = async () => {
        const chosen = selected();
        if (!chosen.length) return;
        scan.disabled = download.disabled = all.disabled = true;
        for (const box of picks) box.disabled = true;
        hint.textContent = "正在获取所选附件的下载地址…";
        let submitted = false;
        try {
          const response = await send("downloadFeishuMedia", {ticket:doc.ticket,selected:chosen});
          submitted = response?.ok === true;
          if (submitted) hint.textContent = "已交给闪电，请在应用中查看。";
          else showError(response?.error, "未能提交下载，请重试。");
        } catch (_) {
          hint.textContent = "未能连接闪电，请重试。";
        } finally {
          scan.disabled = false;
          if (!submitted) {
            all.disabled = false;
            for (const box of picks) box.disabled = false;
            render();
          }
        }
      };
    } catch (_) {
      hint.textContent = "读取飞书文档失败，请打开闪电后重试。";
    } finally {
      scan.disabled = false;
    }
  };
}

function showDetectedMedia(tab, capture, hint) {
  document.body.classList.add("media-mode");
  document.getElementById("more").hidden = false;
  document.getElementById("scan").hidden = true;
  capture.hidden = false;
  const results = document.getElementById("media-results");
  const actions = document.getElementById("media-actions");
  const selectAll = document.getElementById("media-all");
  const download = document.getElementById("media-download");
  const player = document.getElementById("media-player");
  const wave = document.getElementById("media-wave");
  const seek = wave.querySelector("input");
  const time = player.querySelector(".media-time");
  const peakLayers = wave.querySelectorAll(".media-peaks");
  const waveStatus = wave.querySelector(".media-wave-status");
  const waveformPoints = 512;
  const send = (type, data) => chrome.runtime.sendMessage({ type, tabId: tab.id, ...data });
  const selected = new Set();
  let rows = [];
  let active = false;
  let submitting = null;
  let previewPort;
  let previewRow;
  let previewTimer;
  let previewFrame;
  let previewRequest = 0;
  let previewStartedAt = 0;
  let playback;
  let waveformRow;
  let waveformLoading;
  let waveformAbort;
  let waveformHoverTimer;
  let waveformJob;
  let previewReady = false;
  let scrubbing = false;
  let durationPending = false;
  const visibleAudio = new Set();
  const durationObserver = new IntersectionObserver(entries => {
    for (const entry of entries) {
      const row = rows.find(row => row.card === entry.target);
      if (row && entry.isIntersecting) visibleAudio.add(row);
      else if (row) visibleAudio.delete(row);
    }
    loadNextDuration();
    loadNextWaveform();
  }, { root: results });

  function updateSelection() {
    const count = selected.size;
    selectAll.checked = rows.length > 0 && count === rows.length;
    selectAll.indeterminate = count > 0 && count < rows.length;
    selectAll.disabled = !!submitting;
    download.disabled = !!submitting || count === 0;
    download.textContent = submitting === download ? "正在交给闪电…" : `下载所选（${count}）`;
    for (const row of rows) {
      row.check.checked = selected.has(row.url);
      row.check.disabled = row.download.disabled = !!submitting;
      row.download.textContent = submitting === row.download ? "发送中" : "下载";
    }
  }

  function stopPreview(message = "", switching = false) {
    clearTimeout(previewTimer);
    cancelAnimationFrame(previewFrame);
    previewFrame = null;
    previewReady = false;
    playback = null;
    scrubbing = false;
    seek.disabled = true;
    if (!previewRow) return;
    previewRow.play.textContent = "▶";
    previewRow.play.title = previewRow.play.ariaLabel = "试听";
    previewRow = null;
    previewPort?.postMessage({ command: "stop", request: ++previewRequest });
    if (!switching) {
      showIdleWaveform(message);
      loadNextDuration();
      loadNextWaveform();
    }
  }

  function formatTime(seconds) {
    const whole = Math.floor(seconds || 0);
    return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
  }

  function drawPeaks(row) {
    const peaks = row?.peaks;
    const note = row && !peaks ? (peaks === undefined ? "正在读取完整波形…" : "无法读取完整波形") : "";
    if (waveStatus.textContent !== note) waveStatus.textContent = note;
    waveStatus.hidden = !note;
    const scale = devicePixelRatio || 1;
    const width = Math.round(wave.clientWidth * scale);
    const height = Math.round(wave.clientHeight * scale);
    if (!width || !height) return;
    const stride = Math.max(2, Math.round(1.5 * scale));
    const bars = Math.min(peaks?.length || 0, Math.floor(width / stride));
    const spacing = bars ? Math.floor(width / bars) : 0;
    const stroke = Math.max(1, Math.min(spacing - 1, Math.round(scale * 0.8)));
    const offset = Math.floor((width - (bars - 1) * spacing - stroke) / 2);
    for (const layer of peakLayers) {
      if (layer.width !== width) layer.width = width;
      if (layer.height !== height) layer.height = height;
      const paint = layer.getContext("2d");
      if (!paint) continue;
      paint.clearRect(0, 0, width, height);
      paint.fillStyle = getComputedStyle(layer).color;
      if (!bars && peaks === null) paint.fillRect(0, height - Math.ceil(scale), width, Math.ceil(scale));
      for (let i = 0; i < bars; i++) {
        const first = Math.floor(i * peaks.length / bars);
        const last = Math.floor((i + 1) * peaks.length / bars);
        let peak = 0;
        for (let j = first; j < last; j++) peak = Math.max(peak, peaks[j]);
        const bar = Math.max(Math.ceil(scale), Math.round(peak * (height - 4 * scale)));
        paint.fillRect(offset + i * spacing, Math.floor((height - bar) / 2), stroke, bar);
      }
    }
  }

  new ResizeObserver(() => drawPeaks(waveformRow)).observe(wave);

  function renderRowMeta(row) {
    const { rank, format, sizeLabel, duration } = row;
    if (!previewRow && waveformRow === row && duration > 0) {
      time.textContent = "0:00 / " + formatTime(duration);
    }
    const details = [format, sizeLabel, duration > 0 ? formatTime(duration) : ""].filter(Boolean);
    const tooltip = [rank, ...details].filter(Boolean).join(" · ");
    if (row.meta.title === tooltip) return;
    row.meta.title = tooltip;
    row.meta.replaceChildren();
    for (const detail of details) {
      const value = document.createElement("span");
      value.textContent = detail;
      row.meta.append(value, " ");
    }
  }

  async function loadNextDuration() {
    if (durationPending || previewRow) return;
    const row = [...visibleAudio].find(row => !row.duration && !row.durationChecked);
    if (!row) return;
    row.durationChecked = durationPending = true;
    try {
      const result = await send("capturedAudioDuration", { url: row.url });
      if (result?.duration && row.card.isConnected) {
        row.duration = result.duration;
        renderRowMeta(row);
      }
    } catch (_) {
    } finally {
      durationPending = false;
      loadNextDuration();
    }
  }

  function showIdleWaveform(message = "") {
    player.classList.toggle("on", !!waveformRow);
    drawProgress(0);
    drawPeaks(waveformRow);
    const duration = waveformRow?.duration;
    time.textContent = message || (duration ? "0:00 / " + formatTime(duration) : "");
  }

  function selectWaveform(row) {
    clearTimeout(waveformHoverTimer);
    if (waveformLoading && waveformLoading !== row && (!row?.peaks || previewRow)) {
      waveformAbort.abort();
      waveformAbort = null;
      waveformLoading = null;
      waveformJob = null;
    }
    if (waveformRow !== row) {
      waveformRow = row;
      if (!previewRow) showIdleWaveform();
    }
    loadNextWaveform();
  }

  function loadNextWaveform() {
    if (waveformAbort || !waveformRow) return;
    const row = waveformRow.peaks === undefined ? waveformRow :
      !previewRow && [...visibleAudio].find(item => item.peaks === undefined);
    if (row) loadPeaks(row);
  }

  function loadPeaks(row) {
    if (row.peaks !== undefined) return Promise.resolve(row.peaks);
    if (waveformLoading === row) return waveformJob;
    const controller = new AbortController();
    waveformAbort = controller;
    waveformLoading = row;
    waveformJob = (async () => {
      const direct = await readWaveform(row.url, row.size, waveformPoints, controller.signal, "include", row.duration || 0).catch(() => null);
      if (direct || controller.signal.aborted) return direct;
      const [frame] = await chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: readWaveform,
        args: [row.url, row.size || 0, waveformPoints, null, "same-origin", row.duration || 0]
      }).catch(() => []);
      return frame?.result || null;
    })().then(peaks => {
      if (controller.signal.aborted) return null;
      row.peaks = peaks;
      if (waveformRow === row) drawPeaks(row);
      return peaks;
    }).finally(() => {
      if (waveformAbort !== controller) return;
      waveformAbort = null;
      waveformLoading = null;
      waveformJob = null;
      loadNextWaveform();
    });
    return waveformJob;
  }

  function drawProgress(ratio) {
    wave.style.setProperty("--progress", `${ratio * 100}%`);
  }

  function animateProgress(now) {
    previewFrame = null;
    if (!playback || !previewRow) return;
    const elapsed = playback.playing ? Math.max(0, Math.min(600, now - playback.sampledAt)) / 1000 : 0;
    const position = Math.min(playback.duration, playback.time + elapsed);
    const ratio = playback.duration ? position / playback.duration : 0;
    if (!scrubbing) {
      seek.value = ratio * 1000;
      drawProgress(ratio);
    }
    const label = playback.duration ? `${formatTime(position)} / ${formatTime(playback.duration)}` : "正在加载…";
    if (time.textContent !== label) {
      time.textContent = label;
      seek.setAttribute("aria-valuetext", label);
    }
    if (playback.playing && elapsed < 0.6) previewFrame = requestAnimationFrame(animateProgress);
  }

  function updatePreview(state) {
    if (!previewRow || !previewReady || state.request !== previewRequest) return;
    clearTimeout(previewTimer);
    if (state.failed || (!state.duration && Date.now() - previewStartedAt > 12000)) {
      stopPreview("无法试听");
      return;
    }
    if (state.ended) { stopPreview(); return; }
    seek.disabled = !state.duration;
    if (state.duration && previewRow.duration !== state.duration) {
      previewRow.duration = state.duration;
      renderRowMeta(previewRow);
    }
    playback = { ...state, sampledAt: performance.now() };
    if (previewFrame == null) previewFrame = requestAnimationFrame(animateProgress);
    previewTimer = setTimeout(() => previewPort?.postMessage({ command: "state", request: previewRequest }), 400);
  }

  seek.onpointerdown = () => { scrubbing = true; };
  seek.oninput = () => drawProgress(Number(seek.value) / 1000);
  seek.onpointerup = seek.onpointercancel = seek.onblur = () => { scrubbing = false; };
  seek.onchange = () => {
    if (!previewRow || !previewReady || !previewPort) return;
    scrubbing = false;
    clearTimeout(previewTimer);
    playback = null;
    previewPort.postMessage({ command: "seek", ratio: Number(seek.value) / 1000, request: ++previewRequest });
  };

  async function playPreview(row) {
    if (previewRow === row) { stopPreview(); return; }
    stopPreview("", true);
    previewRow = row;
    const request = ++previewRequest;
    row.play.textContent = "■";
    row.play.title = row.play.ariaLabel = "停止试听";
    player.classList.add("on");
    time.textContent = row.duration ? "0:00 / " + formatTime(row.duration) : "正在准备…";
    seek.value = 0;
    drawProgress(0);
    selectWaveform(row);
    drawPeaks(row);

    await loadPeaks(row);
    if (previewRow !== row || request !== previewRequest) return;
    if (!previewPort) {
      previewPort = chrome.runtime.connect({ name: "captured-audio" });
      previewPort.onMessage.addListener(updatePreview);
      previewPort.onDisconnect.addListener(() => {
        previewPort = null;
        if (previewRow) stopPreview("试听已中断");
      });
    }
    previewReady = true;
    previewStartedAt = Date.now();
    previewPort.postMessage({ command: "play", url: row.url, tab: tab.id, request });
  }

  function createRow(item) {
    const card = document.createElement("div");
    card.className = "media-card";
    const top = document.createElement("div");
    top.className = "media-top";
    const label = document.createElement("label");
    label.className = "batch-row";
    const check = document.createElement("input");
    check.type = "checkbox";
    check.onchange = () => {
      if (check.checked) selected.add(item.url);
      else selected.delete(item.url);
      updateSelection();
    };
    const copy = document.createElement("span");
    copy.className = "batch-name";
    const name = document.createElement("strong");
    const meta = document.createElement("small");
    meta.className = "media-meta";
    copy.append(name, meta);
    label.append(check, copy);
    const single = document.createElement("button");
    single.type = "button";
    single.className = "media-single";
    single.onclick = () => handoff([item.url], single);
    const row = { card, check, name, meta, download: single, url: item.url };
    top.append(label);
    if (item.kind === "音频") {
      const play = document.createElement("button");
      play.type = "button";
      play.className = "media-play";
      play.textContent = "▶";
      play.title = play.ariaLabel = "试听";
      play.onclick = () => playPreview(row);
      play.onfocus = () => { if (!previewRow) selectWaveform(row); };
      card.onpointerenter = () => {
        if (previewRow) return;
        clearTimeout(waveformHoverTimer);
        waveformHoverTimer = setTimeout(() => {
          if (!previewRow && card.isConnected) selectWaveform(row);
        }, 140);
      };
      card.onpointerleave = () => clearTimeout(waveformHoverTimer);
      top.append(play);
      row.play = play;
    }
    top.append(single);
    card.append(top);
    return row;
  }

  function render(status) {
    active = !!status?.active;
    capture.textContent = active ? "停止侦测" : "侦测本页音视频";
    capture.className = active ? "scan" : "main";
    const items = (status?.items || []).filter(item => !item.stream);
    const available = new Set(items.map(item => item.url));
    for (const url of selected) if (!available.has(url)) selected.delete(url);
    const existing = new Map(rows.map(row => [row.url, row]));
    if (previewRow && !available.has(previewRow.url)) stopPreview();
    for (const row of rows) {
      if (available.has(row.url)) continue;
      durationObserver.unobserve(row.card);
      visibleAudio.delete(row);
      row.card.remove();
    }
    rows = items.map((item, index) => {
      const row = existing.get(item.url) || createRow(item);
      row.size = item.size;
      const title = item.trackTitle || item.filename || `${item.kind} ${index + 1}`;
      row.name.title = row.name.textContent = title;
      row.download.ariaLabel = "下载 " + title;
      const size = item.size >= 1048576 ? `${(item.size / 1048576).toFixed(1)} MB` :
        item.size >= 1024 ? `${Math.round(item.size / 1024)} KB` : item.size ? `${item.size} B` : "";
      row.rank = `#${items.length - index}${index === items.length - 1 ? " 最近侦测" : ""}`;
      row.format = item.ext.toUpperCase();
      row.sizeLabel = size;
      row.duration = item.duration || row.duration;
      renderRowMeta(row);
      if (!row.card.parentNode) {
        results.prepend(row.card);
        if (row.play) durationObserver.observe(row.card);
      }
      return row;
    });
    actions.hidden = !rows.length;
    player.hidden = !rows.some(row => row.play);
    if (!waveformRow || !available.has(waveformRow.url)) {
      selectWaveform(rows.findLast(row => row.play) || null);
    }
    updateSelection();
    if (!submitting) hint.textContent = active && !rows.length ? "回到网页播放要下载的素材，再打开扩展勾选。" : "";
  }

  selectAll.onchange = () => {
    for (const { check, url } of rows) {
      check.checked = selectAll.checked;
      if (check.checked) selected.add(url);
      else selected.delete(url);
    }
    updateSelection();
  };
  async function handoff(urls, button) {
    if (submitting) return;
    submitting = button;
    capture.disabled = true;
    updateSelection();
    try {
      const result = await send("downloadCapturedMedia", { urls });
      if (!result?.ok) throw new Error(result?.error || "未能交给闪电，请确认应用已打开。");
      window.close();
    } catch (error) {
      hint.textContent = error.message;
    } finally {
      submitting = null;
      capture.disabled = false;
      updateSelection();
    }
  }
  download.onclick = () => handoff([...selected], download);
  capture.onclick = async () => {
    capture.disabled = true;
    const previousLabel = capture.textContent;
    capture.textContent = active ? "正在停止…" : "正在开启侦测…";
    try {
      if (!active && !(await chrome.permissions.request({ permissions: ["webRequest"] }))) {
        hint.textContent = "请允许侦测权限后重试。";
        return;
      }
      const result = await send(active ? "stopCapture" : "startCapture");
      if (!result?.ok) throw new Error();
      render(await send("captureStatus"));
    } catch (_) {
      hint.textContent = "侦测操作失败，请重试。";
    } finally {
      capture.disabled = false;
      if (capture.textContent.endsWith("…")) capture.textContent = previousLabel;
    }
  };
  render(null);
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area === "session" && changes.shandianActiveCapture) {
      const state = changes.shandianActiveCapture.newValue;
      render(state?.tabId === tab.id ? { active: true, items: state.items } : null);
    }
  });
  send("captureStatus").then(render).catch(() => {});
}

async function main() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  let url;
  try { url = new URL(tab?.url || ""); } catch (_) {}
  const host = url?.hostname || "";
  const onChannelPage = host === "channels.weixin.qq.com" && url.pathname.startsWith("/finder-preview/pages/sph");
  const onVideoSite = VIDEO_SITES.some((d) => host === d || host.endsWith("." + d)) ||
    (host === "movie.douban.com" && /^\/trailer\/\d+\/?$/.test(url.pathname)) || onChannelPage;
  const isWebPage = url && ["http:", "https:"].includes(url.protocol);
  const yuanbao = host === "yuanbao.tencent.com";
  const onWeChat = host === "mp.weixin.qq.com";
  const feishu = url?.protocol === "https:" && (host === "feishu.cn" || host.endsWith(".feishu.cn"))
    && /^\/(wiki|docx)\/[a-z0-9]+\/?$/i.test(url.pathname);
  const extraCloud = extraCloudSite(tab?.url);
  const accountSite=cloudAccountLinkSite(tab?.url);
  const aliCloud = isWebPage && /^www\.(alipan|aliyundrive)\.com$/.test(host);
  const baiduShare = isBaiduSharePage(url);
  const cloudDownload = baiduShare ? "baiduShareDownload" : extraCloud ? "extraCloudSelected" : aliCloud && /^\/(drive|s)(\/|$)/.test(url.pathname) ? "aliSelected"
    : ["quark", "uc"].includes(accountSite?.id) ? "quarkSelected" : "";
  const onCloudPage = !!(extraCloud || accountSite || host === "pan.baidu.com" || aliCloud);
  const page = document.getElementById("page");
  const batch = document.getElementById("batch");
  batch.hidden = true;
  if (yuanbao) {
    const toggle = document.getElementById("wechat-toggle");
    const panel = document.getElementById("wechat");
    toggle.hidden = false;
    toggle.onclick = () => { panel.hidden = !panel.hidden; toggle.textContent = panel.hidden ? "下载视频号 ›" : "收起视频号下载"; };
    const wechatButton = document.getElementById("wechat-download");
    wechatButton.onclick = async () => {
      const share = document.getElementById("wechat-link").value.trim();
      let parsed; try { parsed = new URL(share); } catch (_) {}
      if (!parsed || parsed.protocol !== "https:" || parsed.hostname !== "weixin.qq.com" || !parsed.pathname.startsWith("/sph/")) {
        page.querySelector("#hint").textContent = "请粘贴完整的视频号分享链接。"; return;
      }
      wechatButton.disabled = true; wechatButton.textContent = "正在解析…";
      try {
        const r = await chrome.runtime.sendMessage({ type: "downloadWeChat", tabId: tab.id, share });
        page.querySelector("#hint").textContent = r?.ok ? "已交给闪电全能下载，请在应用中查看进度。" : r?.login ? "元宝页面尚未就绪，请查看该页提示后重试。" : "解析或添加失败，请确认元宝页面已就绪且闪电全能下载已打开。";
      } catch (_) { page.querySelector("#hint").textContent = "解析失败，请重试。"; }
      finally { wechatButton.disabled = false; wechatButton.textContent = "闪电下载"; }
    };
  }
  let context = null;
  if (accountSite) {
    page.querySelector("#title").textContent=accountSite.name;
    const btn=page.querySelector("#dl"),hint=page.querySelector("#hint");
    btn.disabled=true;btn.textContent="正在读取关联状态…";
    try{context=await chrome.runtime.sendMessage({type:"cloudAccountLinkContext",tabId:tab.id});}
    catch{}
  }
  const activeAssociation = !!(context?.ok && !context.inactive && ["local", "nas"].includes(context.target));
  const pikpakManualLink = accountSite?.id === "pikpak";
  if (activeAssociation || pikpakManualLink) {
    const btn=page.querySelector("#dl"),hint=page.querySelector("#hint");
    let busy=false;
    const target=context?.target || "local",targetLabel=target==="nas"?"NAS":"本机";
    btn.title=context?.targetLabel || targetLabel;
    document.body.classList.toggle("account-mode",activeAssociation);
    if(activeAssociation){document.body.classList.toggle("show-settings",false);document.getElementById("tab-dl").classList.toggle("on",true);document.getElementById("tab-set").classList.toggle("on",false);}
    btn.textContent=context?.completed?`已关联到${targetLabel}`:`关联到${targetLabel}`;
    btn.disabled=!!context?.completed;
    hint.textContent=context?.completed?"关联已完成，可以关闭此窗口。":"在网盘网页完成登录后，点击关联。";
    if(context?.transferSession || accountSite.transferSession)hint.textContent+="关联成功后，此次网页登录会退出。";
    btn.onclick=async()=>{
      if(busy || context?.completed)return;busy=true;btn.disabled=true;btn.textContent="正在关联…";hint.textContent=`正在验证并关联到${targetLabel}…`;
      document.body.classList.toggle("account-mode",true);
      try{
        const result=await chrome.runtime.sendMessage({type:"cloudAccountLinkSubmit",tabId:tab.id,target});
        if(result?.ok){context={...context,completed:true};btn.textContent=`已关联到${targetLabel}`;hint.textContent=result.message || "关联已完成，软件中的状态会自动更新。";}
        else{btn.textContent=`重新关联到${targetLabel}`;hint.textContent=result?.error || "关联未完成，请确认软件已打开，再重试。";}
      }catch{btn.textContent=`重新关联到${targetLabel}`;hint.textContent="关联未完成，请确认软件已打开，再重试。";}
      finally{busy=false;btn.disabled=!!context?.completed;}
    };
  } else if (feishu) {
    showFeishuDownloads(tab,page);
  } else if (cloudDownload) {
    page.querySelector("#title").textContent = baiduShare ? "百度网盘" : extraCloud?.name || accountSite?.name || "阿里云盘";
    const button = page.querySelector("#dl");
    const label = baiduShare ? "闪电下载" : "选择网盘文件下载";
    button.disabled = false;
    button.textContent = label;
    page.querySelector("#hint").textContent = baiduShare ? "下载选中的文件。下载整份他人分享时，可在闪电中确认保存位置。" : "先在网页选中文件，再点击此按钮或网页工具栏的「闪电下载」。";
    button.onclick = async () => {
      button.disabled = true; button.textContent = "正在获取…";
      try {
        const result = await chrome.runtime.sendMessage({type:cloudDownload,tabId:tab.id});
        button.textContent = result?.ok ? (baiduShare ? "已交给闪电" : `已交给闪电（${result.sent}）`) : label;
        page.querySelector("#hint").textContent = result?.ok
          ? result.needCode ? "请在闪电的新建下载窗口填写提取码。" : "请在应用中确认文件和保存位置，或查看下载进度。"
          : result?.error || "获取失败，请刷新网盘页面后重试。";
      } catch (_) { button.textContent = label; page.querySelector("#hint").textContent = "扩展连接已失效，请刷新网页后重试。"; }
      finally { button.disabled = false; }
    };
  } else if (accountSite || aliCloud) {
    if (aliCloud) page.querySelector("#title").textContent = "阿里云盘";
    page.querySelector("#dl").hidden = true;
    page.querySelector("#hint").textContent = "请在网盘网页选择文件下载。";
  } else if (!isWebPage) {
    page.querySelector("#title").textContent = "请先打开包含视频的网页";
    page.querySelector("#dl").style.display = "none";
    page.querySelector("#hint").textContent = "浏览器设置等页面无法提取视频。";
  } else {
    page.querySelector("#title").textContent = tab.title || tab.url;
    const btn = page.querySelector("#dl");
    // 普通网页不放视频按钮，只留抓取
    if (!onVideoSite && !onWeChat) btn.hidden = true;
    btn.onclick = async () => {
      btn.disabled = true; btn.textContent = "正在交给闪电全能下载…";
      const quality = host === "mp.weixin.qq.com" ? document.getElementById("mp-quality").value || "" : "";
      const r = await chrome.runtime.sendMessage({ type: "downloadPage", tabId: tab.id, requireVideo: !onVideoSite, quality, forceWake: onChannelPage });
      btn.textContent = r?.pending ? "请在应用中确认" : r?.ok ? "已交给闪电全能下载" : "未能添加视频";
      page.querySelector("#hint").textContent = r?.pending ? "新建下载窗口已打开，请确认名称和保存位置后开始。" : r?.ok ? "在闪电全能下载里查看任务状态。" : r?.noVideo ? "没找到可直接下载的视频。请先播放，再重试或在「更多下载工具」里查找音视频。" : r?.error || "没能交给闪电全能下载，请确认应用已打开后重试。";
      if (!r?.ok) btn.disabled = false;
      if (r?.ok) setTimeout(() => window.close(), 1200);
    };
    if (/(^|\.)(youtube\.com|bilibili\.com)$/.test(host) || host === "youtu.be") {
      chrome.runtime.sendMessage({ type: "videoCollectionAction", tabId: tab.id }).then((action) => {
        if (!action) return;
        const button = action.explicit ? btn : page.querySelector("#download-collection");
        if (!button || button.disabled) return;
        if (!action.explicit && !btn.disabled) btn.textContent = action.currentLabel;
        const label = action.label;
        button.hidden = false;
        button.textContent = label;
        button.onclick = async () => {
          button.disabled = true;
          button.textContent = "正在打开…";
          const result = await chrome.runtime.sendMessage({ type: "downloadCollection", tabId: tab.id, page: tab.url }).catch(() => null);
          button.textContent = result?.ok ? "请在应用中勾选" : label;
          page.querySelector("#hint").textContent = result?.ok
            ? "在「新建下载」里选择要下载的视频。"
            : "没能打开合集，请确认应用已打开后重试。";
          if (result?.ok) setTimeout(() => window.close(), 1200);
          else button.disabled = false;
        };
      }).catch(() => {});
    }
  }
  // 公众号文章先读取自带的视频数据，找不到时再提示播放后侦测。
  if (onChannelPage) {
    page.querySelector("#dl").textContent = "闪电下载";
    page.querySelector("#hint").textContent = "此按钮会先唤醒腾讯元宝再解析，作为保底入口；视频上的悬浮按钮优先在后台解析。";
  }
  if (onWeChat && /^\/s(?:\/|$)/.test(url.pathname)) {
    const result = await chrome.runtime.sendMessage({ type: "wechatFormats", tabId: tab.id }).catch(() => null);
    if (result?.formats?.length) {
      const select = document.getElementById("mp-quality");
      for (const format of result.formats) {
        const option = document.createElement("option"); option.value = format.value; option.textContent = format.label; select.append(option);
      }
      document.getElementById("mp-quality-row").hidden = false;
    }
  }
  const scan = document.getElementById("scan");
  const capture = document.getElementById("capture");
  if (huggingFaceTree(tab.url)) {
    batch.hidden = false;
    scan.hidden = true;
    capture.hidden = true;
  } else if (/^(www\.)?aigei\.com$/.test(host)) {
    showDetectedMedia(tab, capture, page.querySelector("#hint"));
  } else if (isWebPage && !onCloudPage) {
    const more = document.getElementById("more");
    const tools = document.getElementById("download-tools");
    const summary = tools.querySelector("summary");
    const results = document.getElementById("media-results");
    const hint = document.getElementById("media-hint");
    const scanLabel = "查找音视频和字幕";
    const captureHint = `回到网页播放，再点「${scanLabel}」。侦测会在 5 分钟后停止。`;
    const hasDownloadAction = feishu || onVideoSite || onWeChat || yuanbao;
    const hasSiteCatalog = /^(www\.)?pixabay\.com$/.test(host) || host === "docs.qingque.cn";
    if (hasDownloadAction) {
      batch.classList.replace("main", "scan");
      tools.append(batch);
    }
    if (hasSiteCatalog || hasDownloadAction) summary.after(more);
    tools.hidden = !tools.contains(batch) && !tools.contains(more);
    batch.hidden = false;
    more.hidden = false;
    let capturing = false;
    let searched = false;
    const setCaptureState = (active) => {
      capturing = active;
      capture.hidden = !active && !searched;
      capture.textContent = active ? "停止播放侦测" : "开始播放侦测";
      summary.textContent = active && tools.contains(more) ? "更多下载工具（侦测中）" : "更多下载工具";
    };
    capture.onclick = async () => {
      capture.disabled = true;
      capture.textContent = capturing ? "正在停止…" : "正在开启侦测…";
      try {
        if (capturing) {
          const result = await chrome.runtime.sendMessage({ type: "stopCapture", tabId: tab.id });
          if (!result?.ok) throw new Error("停止失败");
          capturing = false;
          results.replaceChildren();
          hint.textContent = "已停止侦测并清除临时结果。";
        } else {
          // 浏览器只允许在用户点击时申请可选权限；不会在安装扩展时自动授予。
          const granted = await chrome.permissions.request({ permissions: ["webRequest"] });
          if (!granted) { hint.textContent = "未授权网络资源侦测，其他下载功能仍可使用。"; return; }
          const result = await chrome.runtime.sendMessage({ type: "startCapture", tabId: tab.id });
          if (!result?.ok) { await chrome.permissions.remove({ permissions: ["webRequest"] }); throw new Error("启动失败"); }
          capturing = true;
          hint.textContent = captureHint;
        }
      } catch (_) {
        if (!capturing) try { await chrome.permissions.remove({ permissions: ["webRequest"] }); } catch (_) {}
        hint.textContent = "侦测操作失败，请重新打开扩展后重试。";
      }
      finally { capture.disabled = false; setCaptureState(capturing); }
    };
    scan.onclick = async () => {
      scan.disabled = true; scan.textContent = "正在查找…"; results.replaceChildren();
      try {
        const response = await chrome.runtime.sendMessage({ type: "scanMedia", tabId: tab.id });
        const items = response?.items || [];
        hint.textContent = items.length ? `找到 ${items.length} 项，点击添加。${items.some(item => ["m3u8", "mpd"].includes(item.ext)) ? "部分分片流需登录或受版权保护，可能无法下载。" : ""}` : "暂未找到音视频或字幕。可开启侦测后播放，再回来查找。";
        searched = true;
        if (!capture.disabled) setCaptureState(response?.captureActive ?? capturing);
        for (const item of items) {
          const button = document.createElement("button"); button.className = "result";
          button.textContent = [item.label, item.ext?.toUpperCase()].filter(Boolean).join(" · ");
          button.onclick = async () => {
            button.disabled = true;
            try {
              const response = await chrome.runtime.sendMessage({ type: "downloadMedia", tabId: tab.id, item });
              button.textContent = response?.ok ? "已交给闪电全能下载" : "添加失败，点击重试";
              if (!response?.ok) button.disabled = false;
            } catch (_) { button.textContent = "添加失败，点击重试"; button.disabled = false; }
          };
          results.appendChild(button);
        }
      } catch (_) { hint.textContent = "读取页面失败，请刷新页面后重试。"; }
      finally { scan.disabled = false; scan.textContent = scanLabel; }
    };
    chrome.runtime.sendMessage({ type: "captureStatus", tabId: tab.id }).then((status) => {
      if (!searched && !capture.disabled && status?.active) {
        setCaptureState(true);
        hint.textContent = captureHint;
      }
    }).catch(() => {});
  }
  const cloudKeys = ["baiduButton", "quarkButton", "aliButton", "extraCloudButton"];
  const cloudSettings = await chrome.storage.local.get([...cloudKeys, "baiduAppKey"]);
  const cloudToggle = document.getElementById("cloud-toolbar");
  const enabledClouds = cloudKeys.filter((key) => cloudSettings[key] ?? true).length;
  cloudToggle.checked = enabledClouds === cloudKeys.length;
  cloudToggle.indeterminate = enabledClouds > 0 && !cloudToggle.checked;
  cloudToggle.addEventListener("change", () =>
    chrome.storage.local.set(Object.fromEntries(cloudKeys.map((key) => [key, cloudToggle.checked]))));
  const keyBox = document.getElementById("baidu-appkey");
  keyBox.value = cloudSettings.baiduAppKey || "";
  keyBox.addEventListener("input", () => chrome.storage.local.set({ baiduAppKey: keyBox.value.trim() }));
  const hover = document.getElementById("hover-button");
  const channelWait = document.getElementById("channel-background-wait");
  const { channelBackgroundWait = 5 } = await chrome.storage.local.get("channelBackgroundWait");
  channelWait.value = String([3,5,10].includes(Number(channelBackgroundWait)) ? Number(channelBackgroundWait) : 5);
  channelWait.addEventListener("change", () => chrome.storage.local.set({channelBackgroundWait:Number(channelWait.value)}));
  const { hoverButton = true } = await chrome.storage.local.get("hoverButton");
  hover.checked = hoverButton;
  hover.addEventListener("change", () => chrome.storage.local.set({ hoverButton: hover.checked }));
  const box = document.getElementById("takeover");
  const { takeover = true } = await chrome.storage.local.get("takeover");
  box.checked = takeover;
  box.onchange = async () => {
    await chrome.storage.local.set({ takeover: box.checked });
    chrome.runtime.sendMessage({ type: "refreshBadge" });
  };
}
// 下载 / 设置 切换，记住上次看的
{
  const tabs = { dl: document.getElementById("tab-dl"), set: document.getElementById("tab-set") };
  const show = (settings) => {
    document.body.classList.toggle("show-settings", settings);
    tabs.dl.classList.toggle("on", !settings);
    tabs.set.classList.toggle("on", settings);
    chrome.storage.local.set({ popupSettings: settings });
  };
  tabs.dl.onclick = () => show(false);
  tabs.set.onclick = () => show(true);
  chrome.storage.local.get("popupSettings").then((v) => show(!!v.popupSettings));
}
document.addEventListener?.("keydown", (event) => {
  if (event.defaultPrevented || event.isComposing || event.shiftKey || event.altKey ||
      !(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== "a" ||
      event.target.closest?.("textarea, select, input:not([type=checkbox]), [contenteditable]:not([contenteditable=false])")) return;
  const selectAll = [...document.querySelectorAll("#page .batch-actions:not([hidden]) input[type=checkbox]:not(:disabled)")]
    .find(control => control.getClientRects().length > 0);
  if (!selectAll) return;
  event.preventDefault();
  if (selectAll.checked && !selectAll.indeterminate) return;
  selectAll.checked = true;
  selectAll.indeterminate = false;
  selectAll.dispatchEvent(new Event("change", { bubbles: true }));
});

main();
