// All native surfaces read the same preference before their first paint.
(() => {
  const root = document.documentElement;
  const system = window.matchMedia("(prefers-color-scheme: dark)");
  const render = () => {
    const pref = window.__sdTheme;
    const dark = pref === "dark" || (pref === "system" && system.matches);
    root.dataset.theme = dark ? "dark" : "light";
    root.dataset.dark = dark ? "1" : "0";
  };

  window.__sdSetTheme = (pref) => {
    window.__sdTheme = pref === "light" || pref === "dark" ? pref : "system";
    render();
  };

  let cached;
  try { cached = localStorage.getItem("sd.theme"); } catch {}
  window.__sdSetTheme(window.__sdTheme || cached);
  system.addEventListener("change", render);
})();
