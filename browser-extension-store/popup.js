const messages = {
  sent: "已发送，请在应用中查看。 / Sent; check the desktop app.",
  unsupported: "不支持此链接。 / This link is not supported.",
  unavailable: "无法连接应用，请检查应用和扩展版本。 / Cannot connect; check the app and extension."
};
chrome.storage.local.get("lastResult").then(({ lastResult }) => {
  document.getElementById("result").textContent = messages[lastResult] || "";
  chrome.action.setBadgeText({ text: "" });
});
const takeover = document.getElementById("takeover");
chrome.storage.local.get("takeover").then(({ takeover: on = true }) => { takeover.checked = on; });
takeover.addEventListener("change", () => chrome.storage.local.set({ takeover: takeover.checked }));
