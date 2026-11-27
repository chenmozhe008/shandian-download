(() => {
  let pending = false;

  function mount() {
    const tree = huggingFaceTree(location.href);
    const old = document.getElementById("shandian-hf-download");
    const files = tree && [...document.querySelectorAll('li a[href*="/resolve/"][download], li a[href*="/tree/"]')]
      .find(link => {
        const path = new URL(link.getAttribute("href"), location.href).pathname;
        return path.startsWith("/" + tree.model + "/resolve/") ||
          path.startsWith("/" + tree.model + "/tree/" + encodeURIComponent(tree.revision) + "/");
      })?.closest("ul");
    if (old && files && old.nextElementSibling === files && old.dataset.page === location.pathname) return;
    old?.remove();
    if (!files) return;

    const toolbar = document.createElement("div");
    toolbar.id = "shandian-hf-download";
    toolbar.dataset.page = location.pathname;
    toolbar.className = "mb-2 flex flex-wrap items-center gap-2";
    const button = document.createElement("button");
    button.type = "button";
    button.className = "btn btn-sm";
    const label = tree.directory.length ? "闪电下载此目录" : "闪电下载全部文件";
    button.textContent = label;
    const status = document.createElement("span");
    status.setAttribute("role", "status");
    toolbar.append(button, status);
    files.before(toolbar);

    button.onclick = async () => {
      if (pending) return;
      pending = true;
      button.disabled = true;
      button.textContent = "正在读取文件…";
      status.textContent = "";
      try {
        const result = await chrome.runtime.sendMessage({ type: "downloadHuggingFaceTree" });
        status.textContent = result?.ok ? "已打开闪电新建下载" : result?.error || "未能交给闪电，请确认应用已打开";
      } catch (_) {
        status.textContent = "未能连接闪电，请重试";
      } finally {
        pending = false;
        button.disabled = false;
        button.textContent = label;
      }
    };
  }

  let scheduled = false;
  new MutationObserver(() => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => { scheduled = false; mount(); });
  }).observe(document.body, { childList: true, subtree: true });
  mount();
})();
