const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");

const load = (root, name) => fs.readFileSync(root + "/windows/browser-extension/" + name, "utf8").replace(/\r\n/g, "\n");
const section = (code, start, end) => code.slice(code.indexOf(start), code.indexOf(end, code.indexOf(start)));
const rect = (width, height) => ({ width, height });

function node(tagName, width, height, children = {}) {
  return {
    tagName,
    matches: () => false,
    getBoundingClientRect: () => rect(width, height),
    querySelector: key => children[key] || null,
    querySelectorAll: key => children[key] || [],
  };
}

class DomNode {
  constructor() { this.children = []; this.dataset = {}; this.style = {}; }
  append(...items) { this.children.push(...items); }
  replaceChildren() { this.children = []; }
  after(...items) { this.inserted = items; }
}

(async () => {
  for (const root of ["."]) {
    const hover = load(root, "hover.js");
    const native = Object.assign(node("AUDIO", 260, 32), {
      currentSrc: "https://media.example/audio",
      src: "https://media.example/old.mp3",
    });
    const hidden = Object.assign(node("AUDIO", 0, 0), { src: "https://media.example/breath.ogg" });
    const row = node("DIV", 320, 48, { audio: [hidden], "button, [role=\"button\"], [role=\"slider\"]": {} });
    const video = node("VIDEO", 480, 288);
    let elements = [native, video];
    const context = vm.createContext({
      host: null, audioOnly: false, inlineShown: () => false, MIN_W: 200, MIN_H: 120, isX: false, isFeishu: false,
      location: { hostname: "example.test" },
      document: { elementsFromPoint: () => elements, elementFromPoint: () => null },
    });
    vm.runInContext(section(hover, "  function audioSource(", "  function place("), context);
    assert.equal(context.pick(1, 2).url, native.currentSrc, "use the hovered audio, not the adjacent video");
    elements = [row, video];
    assert.equal(context.pick(1, 2).url, hidden.src, "compact custom controls can anchor hidden audio");
    const feishu = node("DIV", 820, 57, { audio: [hidden] });
    const block = {};
    feishu.closest = selector => selector === ".docx-audio-v2-container" ? feishu
      : selector === ".file-block.preview-type-audio" ? block : null;
    context.isFeishu = true;
    elements = [feishu, video];
    assert.equal(context.pick(1, 2).feishuBlock, block, "select the actual Feishu attachment");
    assert.equal(context.pick(1, 2).url, undefined, "do not use a playback URL as the original");
    context.isFeishu = false;
    assert.equal(context.pick(1, 2).video, true, "the Feishu fallback stays site-scoped");
    context.audioOnly = true;
    elements = [video];
    assert.equal(context.pick(1, 2), null, "new iframe detection is audio-only");
    elements = [native];
    assert.equal(context.pick(1, 2).url, native.currentSrc);
    native.currentSrc = "blob:https://media.example/object";
    assert.equal(context.pick(1, 2), null, "do not substitute a stale source for blob playback");

    let selected = [block];
    let downloads = 0;
    const control = {
      getClientRects: () => [rect(30, 30)],
      matches: () => false,
      click: () => { downloads++; },
    };
    let pendingSelection = false;
    let activeRequest = true;
    block.isConnected = true;
    const original = vm.createContext({
      document: { querySelectorAll: selector => selector === ".file-block.preview-selected" ? selected : [control] },
      setTimeout: callback => {
        if (pendingSelection) {
          selected = [block];
          pendingSelection = false;
        }
        callback();
      },
    });
    vm.runInContext(section(hover, "  function feishuDownloadControl(", "  function place("), original);
    const downloadAudio = (ready = null) => original.downloadFeishuAudio(block, ready, () => activeRequest);
    assert.equal(await downloadAudio(control), true);
    assert.equal(downloads, 1);
    selected = [{}];
    pendingSelection = true; // The native title handles the same click as the extension button.
    assert.equal(await downloadAudio(), true);
    assert.equal(downloads, 2);
    selected = [{}, block];
    assert.equal(await downloadAudio(), false, "ambiguous selection cannot download a different block");
    assert.equal(downloads, 2);
    selected = [{}];
    pendingSelection = true;
    activeRequest = false;
    assert.equal(await downloadAudio(), false, "do not click a toolbar after the request was superseded");
    assert.equal(downloads, 2);
    activeRequest = true;
    selected = [block];
    block.isConnected = false;
    assert.equal(await downloadAudio(), false, "do not download a detached attachment");
    assert.equal(downloads, 2);

    const position = vm.createContext({ button: { style: {} }, SIZE: 38, GAP: 4, innerWidth: 950, innerHeight: 650 });
    vm.runInContext(section(hover, "  function place(", "  function hide("), position);
    position.place({ getBoundingClientRect: () => ({ left: 100, top: 100, right: 400, bottom: 156, height: 56 }) }, true);
    assert.equal(position.button.style.left, "404px");
    assert.equal(position.button.style.top, "109px");

    const calls = [];
    const pointer = vm.createContext({
      enabled: true, host: {}, target: null, lastMove: 0, isNativeMediaSite: false,
      isChannelPreview: () => false, isPixabay: false,
      document: { elementFromPoint: (x, y) => calls.push(["host", x, y]) },
      pick: (x, y) => { calls.push(["media", x, y]); return null; },
    });
    vm.runInContext(section(hover, "  function onMove(", "  async function onClick("), pointer);
    for (const event of [{}, { clientX: NaN, clientY: 4 }, { clientX: 0, clientY: Infinity }]) {
      pointer.onMove(event);
    }
    assert.deepEqual(calls, []);
    assert.equal(pointer.lastMove, 0, "synthetic events must not consume the hover throttle");
    pointer.onMove({ clientX: 0, clientY: 4 });
    assert.deepEqual(calls, [["host", 0, 4], ["media", 0, 4]]);

    const background = load(root, "background.js");
    const scan = vm.createContext({
      URL, location: { href: "https://example.test/lesson" },
      document: {
        title: "Lesson",
        querySelectorAll: () => [{
          tagName: "AUDIO", currentSrc: "https://cdn.example/voice", src: "",
          querySelectorAll: name => name === "source" ? [{ src: "https://cdn.example/voice", type: "audio/mpeg" }] : [],
        }],
      },
      performance: { getEntriesByType: () => [
        { name: "https://cdn.example/short.ogg", transferSize: 1000 },
        { name: "https://cdn.example/streamFile", initiatorType: "audio" },
        { name: "https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/preview/Example", initiatorType: "audio" },
      ] },
    });
    vm.runInContext(section(background, "function collectPageMedia()", "async function scanPageMedia("), scan);
    const items = scan.collectPageMedia();
    assert.ok(items.some(item => item.kind === "音频" && item.ext === "mp3" && item.url.endsWith("/voice")));
    assert.ok(items.some(item => item.kind === "音频" && item.ext === "ogg"));
    assert.ok(items.some(item => item.kind === "音频" && !item.ext && item.url.endsWith("/streamFile")));
    assert.ok(items.every(item => !item.url.includes("/download/preview/")));

    const save = vm.createContext({
      URL, setTimeout: fn => fn(),
      chrome: { downloads: { search: async () => [{ id: 9, filename: "C:\\Downloads\\呼吸声.wav" }] } },
    });
    vm.runInContext(section(background, "function feishuAttachmentDownload(", "chrome.downloads.onCreated.addListener"), save);
    const attachment = "https://internal-api-drive-stream.feishu.cn/space/api/box/stream/download/all/ExampleId";
    assert.equal(save.feishuAttachmentDownload(attachment), true);
    assert.equal(save.feishuAttachmentDownload(attachment.replace("feishu.cn/", "feishu.cn.evil.test/")), false);
    assert.equal(await save.feishuDownloadName({ id: 9, filename: "ExampleId.mp3" }, attachment), "呼吸声.wav");

    const popup = load(root, "popup.js");
    const commands = [];
    const nodes = Object.fromEntries(["#title", "#dl", "#hint"].map(id => [id,new DomNode()]));
    nodes["#dl"].parentElement = new DomNode();
    const ui = vm.createContext({
      document: { createElement: () => new DomNode() },
      chrome: { runtime: { sendMessage: async command => {
        commands.push(command);
        if (command.type === "scanFeishuMedia") return { ok: true, document: {
          title: "Media", ticket: "fixture", files: [{ name: "一.mp3", size: 3200 }, { name: "二.mp4", size: 4200 }],
        } };
        return { ok: true };
      } } },
    });
    vm.runInContext(section(popup, "function showFeishuDownloads(", "function showDetectedMedia("), ui);
    ui.showFeishuDownloads({ id: 3, title: "Feishu" },{ querySelector: key => nodes[key] });
    await nodes["#dl"].onclick();
    const [list, actions] = nodes["#dl"].parentElement.inserted;
    assert.equal(list.children[0].children[1].textContent, "一.mp3");
    assert.equal(list.children[0].children[2].textContent, "3 KB");
    const button = actions.children[1];
    assert.equal(button.textContent, "下载所选（2）");
    const item = list.children[1].children[0];
    item.checked = false;
    item.onchange();
    assert.equal(button.textContent, "下载所选（1）");
    await button.onclick();
    assert.equal(commands[1].type, "downloadFeishuMedia");
    assert.equal(commands[1].ticket, "fixture");
    assert.equal(JSON.stringify(commands[1].selected), "[0]");
    const errors = [];
    const policy = vm.createContext({
      document: { createElement: () => new DomNode() },
      chrome: {
        tabs: { create: async options => { errors.push(options.url); } },
        runtime: { sendMessage: async () => ({ ok:false, error:{
          message:"飞书数据安全策略阻止了应用授权（21001）",
          permissionUrl:"https://open.feishu.cn/app/cli_example/auth",
          policyBlocked:true,
        }}) },
      },
    });
    vm.runInContext(section(popup, "function showFeishuDownloads(", "function showDetectedMedia("), policy);
    policy.showFeishuDownloads({ id:4, title:"Feishu" }, {querySelector:key=>nodes[key]});
    await nodes["#dl"].onclick();
    const [, permissionButton] = nodes["#hint"].children;
    assert.match(nodes["#hint"].children[0].textContent, /用户身份权限.*查看新版文档.*下载云文档中的图片和附件.*查看知识空间节点信息/);
    assert.match(nodes["#hint"].children[0].textContent, /仍报 21001/);
    await permissionButton.onclick();
    assert.deepEqual(errors, ["https://open.feishu.cn/app/cli_example/auth"]);
  }
  console.log("通用音频悬浮与扫描、飞书官方原名和附件勾选通过");
})().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
