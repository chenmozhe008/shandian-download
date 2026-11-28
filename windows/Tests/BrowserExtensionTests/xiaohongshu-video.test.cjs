const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");

const noteId = "6ad2dd96000000001800407d";
const cdn = "https://cdn.example/";
const noteUrl = "https://www.xiaohongshu.com/discovery/item/" + noteId + "?xsec_token=example";
const source = fs.readFileSync("browser-extension/background.js", "utf8");
const h265 = [
  { height: 1080, masterUrl: cdn + "video.m4s" },
  { height: 1080, masterUrl: cdn + "muted.mp4", audioCodec: "none" },
];
const h264 = [
  { height: 720, masterUrl: cdn + "fragment.m4s", backupUrls: [cdn + "with-sound.mp4"], audioCodec: "aac" },
];
const context = vm.createContext({
  URL, URLSearchParams,
  location: new URL(noteUrl),
  window: { __INITIAL_STATE__: { note: { noteDetailMap: { [noteId]: { note: {
    title: "笔记视频", video: { media: { stream: { h264, h265 } } },
  } } } } } },
  document: { title: "小红书", querySelectorAll: () => [{ currentSrc: cdn + "video.m4s" }] },
  wechatShareOf: () => null,
  sendToApp: async (...args) => args,
});
vm.runInContext(source.slice(source.indexOf("function pickPlayingVideo()"), source.indexOf("async function pickInTab(")), context);
vm.runInContext(source.slice(source.indexOf("const PICK_FROM_PAGE ="), source.indexOf("function wechatShareOf(")), context);
context.pickInTab = async () => context.pickPlayingVideo();

(async () => {
  const tab = { id: 1, url: noteUrl };
  assert.equal((await context.downloadVideoInTab(tab))[0], cdn + "with-sound.mp4");
  h264.splice(0);
  assert.equal(context.pickPlayingVideo(), null);
  const sent = await context.downloadVideoInTab(tab);
  assert.equal(sent[0], noteUrl);
  assert.equal(sent[1], noteUrl);
  assert.equal(sent[2], undefined);
  const profile = noteUrl.replace("/discovery/item/", "/user/profile/example/");
  assert.equal((await context.downloadVideoInTab({ ...tab, url: profile }))[0], noteUrl.replace("/discovery/item/", "/explore/"));
  context.location = new URL("https://www.xiaohongshu.com/explore");
  await assert.rejects(() => context.downloadVideoInTab({ ...tab, url: context.location.href }), /请打开这条小红书笔记/);
})().catch((error) => { console.error(error); process.exitCode = 1; });
