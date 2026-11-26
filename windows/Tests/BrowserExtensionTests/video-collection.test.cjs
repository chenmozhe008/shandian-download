const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");

const youtube = "https://www.youtube.com/watch?v=6dBlxB1pBa8&list=UUz75RVbH8q2jdBJ4SnwuZZQ";
const playlist = "https://www.youtube.com/playlist?list=UUz75RVbH8q2jdBJ4SnwuZZQ";
const playlistId = new URL(playlist).searchParams.get("list");
const bili = "https://www.bilibili.com/video/BV1bK411W797?p=2";
const source = fs.readFileSync("windows/browser-extension/background.js", "utf8");
const context = vm.createContext({
  URL,
  wechatShareOf: () => null,
  sendToApp: async (...args) => args,
  location: new URL(bili),
  document: { querySelector: () => null },
  window: { __INITIAL_STATE__: { videoData: { bvid: "BV1bK411W797", pages: [{ page: 1 }, { page: 2 }] } } },
  chrome: { scripting: { executeScript: async ({ func, args = [] }) => [{ result: func(...args) }] } },
});
for (const [start, end] of [
  ["const PICK_FROM_PAGE =", "function wechatShareOf("],
  ["function videoPageURL(", "async function appReady("],
]) {
  vm.runInContext(source.slice(source.indexOf(start), source.indexOf(end)), context);
}

(async () => {
  assert.equal((await context.downloadVideoInTab({ id: 1, url: youtube }))[0], youtube);
  assert.equal((await context.videoCollectionAction({ url: youtube })).url, playlist);
  assert.equal((await context.videoCollectionAction({ url: playlist })).explicit, true);
  assert.equal(await context.videoCollectionAction({ url: youtube.replace("UUz75RVbH8q2jdBJ4SnwuZZQ", "RD6dBlxB1pBa8") }), null);
  context.location = new URL(youtube);
  context.window.ytInitialData = { contents: { twoColumnWatchNextResults: { playlist: {
    playlist: { playlistId, totalVideos: 90, contents: [{}, {}] },
  } } } };
  context.document.querySelector = () => ({ data: { playlistId: "old-list", totalVideos: 7 } });
  assert.equal((await context.videoCollectionAction({ id: 1, url: youtube })).label, "下载合集（90）");
  context.window.ytInitialData = {};
  assert.equal((await context.videoCollectionAction({ id: 1, url: youtube })).label, "下载合集");
  context.location = new URL(playlist);
  context.document.querySelector = () => null;
  context.window.ytInitialData = {
    header: { playlistHeaderRenderer: { playlistId, numVideosText: { runs: [{ text: "1,050" }, { text: " videos" }] } } },
  };
  assert.equal((await context.videoCollectionAction({ id: 1, url: playlist })).label, "下载合集（1050）");
  context.location = new URL(bili);
  assert.equal((await context.downloadVideoInTab({ id: 1, url: bili }))[0], bili);
  assert.equal((await context.videoCollectionAction({ id: 1, url: bili })).label, "下载合集（2）");
  context.window.__INITIAL_STATE__.videoData.pages.pop();
  assert.equal(await context.videoCollectionAction({ id: 1, url: bili }), null);
  context.window.__INITIAL_STATE__.videoData.ugc_season = { id: 3662502, ep_count: 80, sections: [{ episodes: [{}] }] };
  context.window.__INITIAL_STATE__.videoData.owner = { mid: 2142762 };
  assert.equal((await context.videoCollectionAction({ id: 1, url: bili })).url, "https://space.bilibili.com/2142762/lists/3662502?type=season");
  assert.equal((await context.videoCollectionAction({ id: 1, url: bili })).label, "下载合集（80）");
})().catch((error) => { console.error(error); process.exitCode = 1; });
