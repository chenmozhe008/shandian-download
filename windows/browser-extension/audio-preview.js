// Execute in the source tab so the media request uses its original referrer.
function probeAudioDuration(url) {
  const audio = new Audio();
  audio.preload = "metadata";
  return new Promise(resolve => {
    const finish = () => {
      clearTimeout(timeout);
      const duration = Number.isFinite(audio.duration) ? audio.duration : 0;
      audio.onloadedmetadata = audio.onerror = null;
      audio.removeAttribute("src");
      audio.load();
      resolve(duration);
    };
    const timeout = setTimeout(finish, 4500);
    audio.onloadedmetadata = audio.onerror = finish;
    audio.src = url;
  });
}

async function capturedAudioDuration(tabId, url) {
  const capture = await capturedForTab(tabId);
  const item = capture.items.find(item => item.url === url && item.kind === "音频" && !item.stream);
  if (!item) return 0;
  if (item.duration) return item.duration;
  const [frame] = await chrome.scripting.executeScript({
    target: { tabId }, func: probeAudioDuration, args: [item.url]
  });
  const duration = frame?.result || 0;
  rememberCapturedDuration(tabId, item.url, duration);
  return duration;
}

function controlCapturedAudio(owner, command, value) {
  let player = window.__shandianCapturedAudio;
  if (command === "play") {
    player?.audio.pause();
    player?.audio.removeAttribute("src");
    player?.audio.load();
    for (const audio of document.querySelectorAll("audio")) {
      if (!audio.paused) audio.pause();
    }
    player = { owner, audio: new Audio(value), failed: false };
    window.__shandianCapturedAudio = player;
    player.audio.play().catch(() => { player.failed = true; });
  }
  if (player?.owner !== owner) return { ended: true };
  const audio = player.audio;
  if (command === "stop") {
    audio.pause();
    audio.removeAttribute("src");
    audio.load();
    delete window.__shandianCapturedAudio;
    return { ended: true };
  }
  const duration = Number.isFinite(audio.duration) ? audio.duration : 0;
  if (command === "seek" && duration > 0 && Number.isFinite(value)) {
    audio.currentTime = duration * Math.max(0, Math.min(1, value));
  }
  return {
    time: audio.currentTime, duration,
    playing: !audio.paused && !audio.seeking && audio.readyState >= 3,
    ended: audio.ended, failed: player.failed || !!audio.error
  };
}

chrome.runtime.onConnect?.addListener(port => {
  if (port.name !== "captured-audio" || port.sender?.url !== chrome.runtime.getURL("popup.html") || port.sender.tab) return;
  const owner = crypto.randomUUID();
  let tabId;
  let playingUrl;
  let queue = Promise.resolve();
  let disconnected = false;
  async function control(command, value) {
    if (tabId == null) return { ended: true };
    const [frame] = await chrome.scripting.executeScript({
      target: { tabId }, func: controlCapturedAudio, args: [owner, command, value ?? null]
    });
    return frame?.result || { failed: true };
  }
  port.onMessage.addListener(({ command, url, tab, ratio, request }) => {
    queue = queue.then(async () => {
      if (disconnected) return;
      let state;
      try {
        if (command === "play") {
          const captured = await capturedForTab(tab);
          const item = captured.items.find(item => item.url === url && item.kind === "音频" && !item.stream);
          if (!item || (tabId != null && tabId !== tab)) throw new Error();
          tabId = tab;
          state = await control("play", item.url);
          playingUrl = item.url;
        } else if (["state", "seek", "stop"].includes(command)) {
          state = await control(command, ratio);
        }
      } catch (_) {
        state = { failed: true };
      }
      if (state?.duration && playingUrl) rememberCapturedDuration(tabId, playingUrl, state.duration);
      if (!disconnected && state) port.postMessage({ request, ...state });
    }).catch(() => {});
  });
  port.onDisconnect.addListener(() => {
    disconnected = true;
    queue.then(() => control("stop")).catch(() => {});
  });
});
