var T = globalThis.T || ((text, values) => text.replace(/\{(\w+)\}/g, (_, key) => values?.[key]));  // 测试环境没有 i18n.js
// 批量抓取入口：扫描当前页面后由用户选择，只有点击下载才向应用交接。
(async function () {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  const batch = document.getElementById('batch');
  const actions = document.getElementById('batch-actions');
  const results = document.getElementById('results');
  const selectAll = document.getElementById('select-all');
  const download = document.getElementById('download-selected');
  const hint = document.getElementById('hint');
  let url;
  try { url = new URL(tab?.url || ''); } catch (_) {}
  if (!url || !['http:', 'https:'].includes(url.protocol) || url.hostname === 'mypikpak.com') {
    batch.style.display = 'none'; return;
  }
  batch.onclick = async () => {
    document.body.classList.add('batch-mode');
    batch.disabled = true; batch.textContent = T('正在抓取…'); results.replaceChildren(); actions.hidden = true;
    try {
      const response = await chrome.runtime.sendMessage({ type: 'scanLinks', tabId: tab.id });
      if (response?.error) throw new Error('无法读取网页');
      const items = response?.items || [];
      const known = new Set(response?.known || []);
      const checks = [];
      for (const item of items) {
        const row = document.createElement('label'); row.className = 'batch-row';
        const check = document.createElement('input'); check.type = 'checkbox'; check.value = item.url;
        check.disabled = known.has(item.url); check.checked = !check.disabled;
        const title = document.createElement('span'); title.className = 'name'; title.textContent = item.title; title.title = item.title;
        const kind = document.createElement('small'); kind.textContent = T(item.kind) + (check.disabled ? ' · ' + T('已添加') : '');
        row.append(check, title, kind); results.appendChild(row); checks.push(check);
      }
      const update = () => {
        const available = checks.filter(c => !c.disabled), selected = available.filter(c => c.checked);
        selectAll.checked = available.length > 0 && selected.length === available.length;
        selectAll.indeterminate = selected.length > 0 && selected.length < available.length;
        download.disabled = !selected.length; download.textContent = T('下载所选（{n}）', { n: selected.length });
      };
      for (const check of checks) check.onchange = update;
      selectAll.onchange = () => { for (const check of checks) if (!check.disabled) check.checked = selectAll.checked; update(); };
      download.onclick = async () => {
        const urls = checks.filter(c => c.checked && !c.disabled).map(c => c.value);
        if (!urls.length) return;
        download.disabled = true;
        try {
          const response = await chrome.runtime.sendMessage({ type: 'downloadLinks', tabId: tab.id, urls });
          if (!response?.ok) throw new Error('交接失败');
          hint.textContent = T('已交给闪电处理 {n} 个链接，请在应用中确认任务；取消后可重新选择。', { n: urls.length });
          for (const check of checks) if (check.checked) check.checked = false;
          update();
        } catch (_) { hint.textContent = T('没能交给闪电，请确认应用已打开后重试。'); update(); }
      };
      actions.hidden = items.length === 0; update();
      hint.textContent = items.length ? T('找到 {n} 个链接（最多显示 100 个）。', { n: items.length }) + (response.checked ? '' : T('未能检查已有任务。')) + T('需要登录的直链可能无法下载。') : T('当前页面没有找到磁力、种子或常见文件直链。');
    } catch (_) { hint.textContent = T('无法读取当前页面，请刷新后重试。'); }
    finally { batch.disabled = false; batch.textContent = T('重新抓取下载链接'); }
  };
})();
