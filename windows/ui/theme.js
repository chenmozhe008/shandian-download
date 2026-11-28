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

  const restoreAccent = () => {
    let accent = "amber";
    let vars;
    try {
      const saved = localStorage.getItem("sd.accent") || "amber";
      const replacement = { teal: "tongqing", green: "shilv", purple: "qinglian", rose: "qinglian" }[saved];
      accent = typeof replacement === "string" ? replacement : saved;
      if (accent !== saved) localStorage.setItem("sd.accent", accent);
      if (accent === "custom") vars = JSON.parse(localStorage.getItem("sd.accentVars"));
    } catch {}
    root.dataset.accent = accent === "custom" && !vars ? "amber" : accent;
    for (const [property, value] of [
      ["--accent-h", vars?.h],
      ["--accent-s", vars?.s],
      ["--on-dark", vars?.onDark],
      ["--accent-color", vars?.hex],
    ]) {
      if (value) root.style.setProperty(property, value);
      else root.style.removeProperty(property);
    }
  };

  restoreAccent();
  window.addEventListener("storage", (event) => {
    if (event.key === "sd.accent" || event.key === "sd.accentVars" || event.key === null) restoreAccent();
  });
})();
