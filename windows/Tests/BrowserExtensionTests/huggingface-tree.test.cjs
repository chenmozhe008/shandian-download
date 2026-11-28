const assert = require("node:assert/strict");
const fs = require("node:fs");
const test = require("node:test");
const vm = require("node:vm");

const page = "https://huggingface.co/lab/example/tree/main";
const sha = "a".repeat(40);
const model = "https://huggingface.co/api/models/lab/example";

test("Hugging Face hover reads only the matching file size from the current page", () => {
  const source = fs.readFileSync("browser-extension/huggingface.js", "utf8");
  const read = vm.runInNewContext(source + "\nhuggingFaceHoverSize", { URL });
  const root = props => ({ querySelectorAll: () => [{ getAttribute: () => JSON.stringify(props) }] });
  const href = "https://huggingface.co/lab/example/resolve/main/images/example%20image.png?download=true";
  const tree = root({ context: { repo: { name: "lab/example" }, rev: "main" }, entries: [
    { type: "file", path: "images/example image.png", size: 268417 },
    { type: "file", path: "images/other.png", size: null },
  ] });
  assert.equal(read(href, tree).sizeBytes, 268417);
  assert.equal(read(href, tree).sizeLabel, "268.42 KB");
  assert.equal(read(href.replace("/main/", "/other/"), tree), null);
  assert.equal(read(href.replace("example%20image.png", "other.png"), tree).sizeLabel, "");
  const blob = root({ repoId: { name: "lab/example" }, revision: "main", filePath: "images/example image.png", size: "268.42 kB" });
  assert.equal(read(href, blob).sizeLabel, "268.42 KB");
  assert.equal(read(href, blob).sizeBytes, undefined);
  assert.equal(read(href.replace("huggingface.co", "example.com"), blob), null);
});

test("Hugging Face batch uses filenames and preserves the selected directory tree", async () => {
  const source = fs.readFileSync("browser-extension/huggingface.js", "utf8");
  const calls = [];
  const context = vm.createContext({
    URL, AbortSignal,
    fetch: async (url, options) => {
      calls.push(url);
      assert.equal(options.credentials, "omit");
      const json = url.includes("/revision/")
        ? { sha, private: false, gated: false }
        : [
            { type: "directory", path: "images" },
            { type: "file", path: "model.gguf", size: 6753954304 },
            { type: "file", path: "images/example image.png", size: 1024 },
            { type: "file", path: "NOTICE", size: null },
          ];
      return { ok: true, json: async () => json, headers: { get: () => null } };
    },
  });
  const read = vm.runInContext(source + "\nreadHuggingFaceFiles", context);
  const result = await read(page);
  assert.equal(result.folderName, "example");
  assert.deepEqual(Array.from(result.items, item => item.title), ["model.gguf", "images/example image.png", "NOTICE"]);
  assert.equal(result.items[0].size, "6.75 GB");
  assert.equal(result.items[1].size, "1.02 KB");
  assert.equal(result.items[1].sizeBytes, 1024);
  assert.equal(result.items[2].size, "");
  assert.equal(result.items[2].sizeBytes, null);
  assert.equal(result.items[1].url, "https://huggingface.co/lab/example/resolve/" + sha + "/images/example%20image.png");
  const selected = await read(page, [result.items[1].url]);
  assert.deepEqual(Array.from(selected.items, item => item.title), ["images/example image.png"]);
  assert(calls.includes(model + "/revision/" + sha));
  const subfolder = await read(page + "/images");
  assert.equal(subfolder.folderName, "images");
  assert.deepEqual(Array.from(subfolder.items, item => item.title), ["example image.png"]);
  await assert.rejects(read("https://huggingface.co.example/lab/example/tree/main"));
});

test("Hugging Face popup submits only the checked folder's files", async () => {
  const element = () => ({
    children: [], classList: { add() {} },
    append(...children) {
      this.children.push(...children);
      for (const child of children) child.parent = this;
    },
    after(child) { this.parent.children.splice(this.parent.children.indexOf(this) + 1, 0, child); },
    replaceChildren() { this.children = []; }, setAttribute() {},
  });
  const nodes = new Map();
  const get = id => {
    if (!nodes.has(id)) nodes.set(id, element());
    return nodes.get(id);
  };
  const items = ["model.gguf", "images/frame.png", "images/nested/example.png", "images-old/other.png"]
    .map(title => ({ title, url: "https://huggingface.co/lab/example/resolve/" + sha + "/" + title }));
  let sent;
  const chrome = {
    tabs: { query: async () => [{ id: 1, url: page }] },
    runtime: { sendMessage: async message => {
      if (message.type === "scanLinks") return { items };
      sent = message;
      return { ok: true };
    } },
  };
  const source = ["source-labels.js", "huggingface.js", "batch-ui.js"]
    .map(name => fs.readFileSync("browser-extension/" + name, "utf8")).join("\n");
  await vm.runInNewContext(source, { chrome, URL, document: {
    getElementById: get, createElement: element, createElementNS: element, body: { classList: { add() {} } },
  } });
  await get("batch").onclick();
  const [, images] = get("results").children;
  const [header, files] = images.children;
  const [toggle, folder] = header.children;
  const check = folder.children[0];
  get("select-all").checked = false;
  get("select-all").onchange();
  check.checked = true;
  check.onchange();
  const first = files.children[0].children[0];
  first.checked = false;
  first.onchange();
  assert.equal(check.indeterminate, true);
  check.checked = true;
  check.onchange();
  toggle.onclick();
  assert.equal(files.hidden, true);
  assert.equal(get("download-selected").textContent, "下载所选（2）");
  await get("download-selected").onclick();
  assert.deepEqual(Array.from(sent.urls), items.slice(1, 3).map(item => item.url));
});

for (const root of ["."]) {
  test(root + " keeps the subfolder when only one Hugging Face file is selected", async () => {
    const source = fs.readFileSync(root + "/ui/app.js", "utf8");
    const start = source.indexOf("function isHuggingFaceFilePage(");
    const end = source.indexOf("/// 投一条链接", start);
    const url = "https://huggingface.co/lab/example/resolve/" + sha + "/images/example.png";
    const calls = [];
    const context = vm.createContext({
      extInfo: new Map([[url, { name: "images/example.png" }]]),
      invoke: async (name, args) => { calls.push({ name, args }); return "download"; },
      settleMulti: async () => {}, switchView: () => {}, refreshTasks: () => {}, toast: () => {},
    });
    vm.runInContext(source.slice(start, end), context);
    await context.runBatch([{ url }], root === "." ? "C:/Downloads" : "/video", null, null, "example", { sourceUrl: page });
    assert.equal(calls.length, 1);
    assert.equal(calls[0].name, "add_multi");
    assert.equal(calls[0].args.name, "example");
    assert.equal(calls[0].args.files[0].name, "images/example.png");
  });
}
