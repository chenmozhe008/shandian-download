// Decode only the selected resource, with bounded input size.
async function readWaveform(url, byteSize, points, signal, credentials = "include", duration = 0) {
  // This function can also run in the source tab when the CDN requires that referrer.
  const maxBytes = 12 * 1024 * 1024;
  if (byteSize > maxBytes) return null;
  const timeout = AbortSignal.timeout(12000);
  const requestSignal = signal ? AbortSignal.any([signal, timeout]) : timeout;
  const options = {
    credentials, priority: "low",
    signal: requestSignal
  };
  const range = start => fetch(url, { ...options, headers: { Range: "bytes=" + start + "-" } });
  let response = await fetch(url, options);
  if ([400, 403, 416].includes(response.status)) {
    await response.body?.cancel();
    response = await range(0);
  }
  const chunks = [];
  let length = 0;
  let total = 0;
  const etag = response.headers.get("etag");
  while (true) {
    if (!response.ok) throw new Error("Audio request failed");
    const partial = response.status === 206;
    if (!response.body || Number(response.headers.get("content-length")) > maxBytes) {
      await response.body?.cancel();
      return null;
    }
    let end = null;
    if (partial) {
      const limits = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(response.headers.get("content-range") || "");
      const [, start, last, size] = limits?.map(Number) || [];
      const nextTag = response.headers.get("etag");
      if (start !== length || last < start || last >= size || size > maxBytes ||
          (total && total !== size) || (etag && nextTag && etag !== nextTag)) {
        await response.body.cancel();
        return null;
      }
      total = size;
      end = last + 1;
    } else if (length > 0) {
      await response.body.cancel();
      return null;
    }
    const reader = response.body.getReader();
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        length += value.byteLength;
        if (length > maxBytes || (end !== null && length > end)) {
          await reader.cancel();
          return null;
        }
        chunks.push(value);
      }
    } finally {
      reader.releaseLock();
    }
    if (!partial) break;
    if (length !== end) return null;
    if (length === total) break;
    response = await range(length);
  }
  if (!length || requestSignal.aborted) return null;
  const data = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    data.set(chunk, offset);
    offset += chunk.byteLength;
  }
  const decoder = new OfflineAudioContext(1, 1, 8000);
  const audio = await decoder.decodeAudioData(data.buffer);
  if (requestSignal.aborted || (duration > 0 && audio.duration + Math.max(0.15, duration * 0.02) < duration)) return null;
  const peaks = new Float32Array(points);
  for (let channel = 0; channel < audio.numberOfChannels; channel++) {
    const samples = audio.getChannelData(channel);
    for (let bar = 0; bar < peaks.length; bar++) {
      const start = Math.floor(bar * samples.length / peaks.length);
      const end = Math.floor((bar + 1) * samples.length / peaks.length);
      const step = Math.max(1, Math.floor((end - start) / 1024));
      let peak = 0;
      for (let i = start; i < end; i += step) peak = Math.max(peak, Math.abs(samples[i]));
      peaks[bar] = Math.max(peaks[bar], peak);
    }
  }
  const max = Math.max(0.02, ...peaks);
  return Array.from(peaks, peak => peak / max);
}
