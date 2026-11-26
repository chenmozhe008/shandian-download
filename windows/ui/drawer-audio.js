
function createDrawerAudioPlayer(panel, changed, ended = () => {}) {
  const media = panel.querySelector("audio");
  const button = panel.querySelector("#files-audio-play");
  const canvas = panel.querySelector("canvas");
  const wave = canvas.getContext("2d");
  const seek = panel.querySelector('input[type="range"]');
  const time = panel.querySelector(".fd-audio-time");
  const status = panel.querySelector(".fd-audio-status");
  const title = panel.querySelector(".fd-audio-title");
  const pointCount = 512;
  const maxWaveBytes = 32 * 1024 * 1024;
  let index = null;
  let request = null;
  let pending = false;
  let peaks = null;
  let frame = 0;
  let meter;
  let context;

  function clock(seconds) {
    if (!Number.isFinite(seconds)) return "0:00";
    const whole = Math.floor(Math.max(0, seconds));
    return Math.floor(whole / 60) + ":" + String(whole % 60).padStart(2, "0");
  }

  function setStatus(text) {
    status.textContent = text;
    status.hidden = !text;
  }

  function syncControls() {
    const paused = media.paused || media.ended;
    const label = paused ? "播放音频" : "暂停音频";
    button.setAttribute("aria-label", label);
    button.querySelector("use").setAttribute("href", paused ? "#i-media-play" : "#i-pause");
    button.disabled = pending || !media.hasAttribute("src");
    changed();
  }

  function draw() {
    const duration = Number.isFinite(media.duration) ? media.duration : 0;
    const position = media.currentTime;
    const progress = duration > 0 ? Math.min(1, position / duration) : 0;
    const label = clock(position) + " / " + clock(duration);
    const width = Math.max(1, Math.round(canvas.clientWidth * devicePixelRatio));
    const height = Math.max(1, Math.round(canvas.clientHeight * devicePixelRatio));
    if (canvas.width !== width) canvas.width = width;
    if (canvas.height !== height) canvas.height = height;
    if (time.textContent !== label) {
      time.textContent = label;
      seek.setAttribute("aria-valuetext", label);
    }
    seek.disabled = !duration;
    seek.value = String(Math.round(progress * 1000));

    wave.clearRect(0, 0, width, height);
    const colors = getComputedStyle(panel);
    const tint = colors.getPropertyValue("--tint").trim();
    const rest = colors.getPropertyValue("--sub").trim();
    if (!peaks) {
      const middle = height / 2 - 1;
      wave.fillStyle = rest;
      wave.fillRect(0, middle, width, 2);
      wave.fillStyle = tint;
      wave.fillRect(0, middle, width * progress, 2);
      return;
    }
    // At lower scales, one physical pixel per bar keeps thin lines from blending unevenly.
    const snapPixels = devicePixelRatio < 1.5;
    const targetBars = snapPixels ? Math.floor(width / 2) : Math.round(canvas.clientWidth / 1.5);
    const bars = Math.max(1, Math.min(peaks.length, targetBars));
    const step = width / bars;
    const stroke = snapPixels ? 1 : Math.min(devicePixelRatio, step * 0.65);
    const cursor = width * progress;
    for (let i = 0; i < bars; i++) {
      const first = Math.floor(i * peaks.length / bars);
      const last = Math.floor((i + 1) * peaks.length / bars);
      let peak = 0;
      for (let sample = first; sample < last; sample++) peak = Math.max(peak, peaks[sample]);
      const barHeight = Math.max(2, peak * (height - 4));
      const bar = snapPixels ? Math.round(barHeight) : barHeight;
      const x = snapPixels ? Math.floor(i * step) : i * step;
      const center = (height - bar) / 2;
      const y = snapPixels ? Math.floor(center) : center;
      const played = Math.max(0, Math.min(stroke, cursor - x));
      wave.fillStyle = played === stroke ? tint : rest;
      wave.fillRect(x, y, stroke, bar);
      if (played > 0 && played < stroke) {
        wave.fillStyle = tint;
        wave.fillRect(x, y, played, bar);
      }
    }
    if (progress > 0 && progress < 1) {
      wave.fillStyle = tint;
      wave.fillRect(cursor - devicePixelRatio / 2, height * 0.15, devicePixelRatio, height * 0.7);
    }
  }

  function tick() {
    if (meter?.samples && meter.live && media.duration > 0) {
      meter.analyser.getFloatTimeDomainData(meter.samples);
      const slot = Math.min(pointCount - 1, Math.floor(media.currentTime / media.duration * pointCount));
      for (const sample of meter.samples) peaks[slot] = Math.max(peaks[slot], Math.min(1, Math.sqrt(Math.abs(sample))));
    }
    draw();
    frame = !media.paused && !media.ended ? requestAnimationFrame(tick) : 0;
  }

  function useLiveWaveform() {
    if (!context) {
      context = new AudioContext();
      const source = context.createMediaElementSource(media);
      const analyser = context.createAnalyser();
      source.connect(analyser);
      analyser.connect(context.destination);
      meter = { analyser, samples: new Float32Array(analyser.fftSize), live: false };
    }
    context.resume().catch(() => {});
    if (meter) {
      meter.live = true;
      peaks = new Float32Array(pointCount);
    }
  }

  async function readWaveform(url, size, signal) {
    if (size > maxWaveBytes) {
      useLiveWaveform();
      return;
    }
    try {
      const response = await fetch(url, { signal });
      if (!response.ok) throw new Error("无法读取音频数据");
      const data = await response.arrayBuffer();
      if (signal.aborted) return;
      const audio = await new OfflineAudioContext(1, 1, 8000).decodeAudioData(data);
      if (signal.aborted) return;
      const waveform = new Float32Array(pointCount);
      for (let ch = 0; ch < audio.numberOfChannels; ch++) {
        const samples = audio.getChannelData(ch);
        for (let bar = 0; bar < pointCount; bar++) {
          const start = Math.floor(bar * samples.length / pointCount);
          const end = Math.floor((bar + 1) * samples.length / pointCount);
          const step = Math.max(1, Math.floor((end - start) / 1024));
          for (let i = start; i < end; i += step) waveform[bar] = Math.max(waveform[bar], Math.abs(samples[i]));
        }
      }
      const max = Math.max(0.02, ...waveform);
      peaks = waveform.map(peak => peak / max);
      if (meter) meter.live = false;
      draw();
    } catch {
      if (!signal.aborted) useLiveWaveform();
    }
  }

  async function toggle(nextIndex, name, resolve) {
    if (nextIndex === index && (pending || media.hasAttribute("src"))) {
      if (pending) return;
      if (media.paused) {
        context?.resume().catch(() => {});
        await media.play().catch(() => setStatus("无法播放这个音频"));
      } else media.pause();
      return;
    }

    close();
    index = nextIndex;
    const current = new AbortController();
    request = current;
    pending = true;
    panel.hidden = false;
    title.textContent = name;
    setStatus("正在打开音频…");
    syncControls();
    draw();
    try {
      const { url, size } = await resolve();
      if (current.signal.aborted) return;
      context?.resume().catch(() => {});
      media.src = url;
      readWaveform(url, size, current.signal).catch(() => {});
      await media.play();
      if (!current.signal.aborted) setStatus("");
    } catch (error) {
      if (!current.signal.aborted) setStatus(error?.message || String(error));
    } finally {
      if (!current.signal.aborted) {
        pending = false;
        syncControls();
      }
    }
  }

  function close() {
    request?.abort();
    request = null;
    index = null;
    pending = false;
    peaks = null;
    if (meter) meter.live = false;
    cancelAnimationFrame(frame);
    frame = 0;
    if (media.hasAttribute("src")) {
      media.pause();
      media.removeAttribute("src");
      media.load();
    }
    panel.hidden = true;
    syncControls();
  }

  new ResizeObserver(() => {
    if (!panel.hidden) draw();
  }).observe(canvas, { box: "device-pixel-content-box" });

  button.addEventListener("click", () => toggle(index));
  seek.addEventListener("input", () => {
    if (Number.isFinite(media.duration)) media.currentTime = Number(seek.value) / 1000 * media.duration;
    draw();
  });
  media.addEventListener("playing", () => {
    setStatus("");
    syncControls();
    if (!frame) frame = requestAnimationFrame(tick);
  });
  for (const event of ["pause", "loadedmetadata", "seeked"]) {
    media.addEventListener(event, () => {
      if (panel.hidden) return;
      syncControls();
      draw();
    });
  }
  media.addEventListener("ended", () => {
    if (index === null || pending || !media.ended) return;
    syncControls();
    draw();
    ended(index);
  });
  media.addEventListener("error", () => {
    if (media.hasAttribute("src")) setStatus("内置播放器无法播放这个格式");
  });

  return { toggle, close, current: () => index, playing: () => !media.paused && !media.ended,
    setLoop: enabled => { media.loop = enabled; } };
}
