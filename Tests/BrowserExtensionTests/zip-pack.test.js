const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const vm = require('node:vm');

const code = fs.readFileSync('browser-extension/zip-pack.js', 'utf8');
const ctx = { TextEncoder, Uint8Array, Uint32Array, DataView, Date, Blob };
ctx.globalThis = ctx;
vm.runInNewContext(code, ctx);

(async () => {
  const files = [
    { name: '雨声雷声.mp3', data: Buffer.from('ID3-fake-mp3-data-123456') },
    { name: '清晨鸟鸣 (2).wav', data: Buffer.from('RIFF-fake-wav-data-abcdef') }
  ];
  const blob = ctx.buildZipBlob(files);
  const bytes = Buffer.from(await blob.arrayBuffer());
  const tmpZip = path.join(os.tmpdir(), `shandian-zip-test-${process.pid}.zip`);
  const tmpDir = path.join(os.tmpdir(), `shandian-zip-out-${process.pid}`);
  fs.writeFileSync(tmpZip, bytes);
  try {
    const out = execFileSync('unzip', ['-t', tmpZip], { encoding: 'utf8' });
    assert.match(out, /No errors detected/);
    execFileSync('ditto', ['-x', '-k', tmpZip, tmpDir]);
    const extracted = fs.readdirSync(tmpDir);
    assert.ok(extracted.includes('雨声雷声.mp3'));
    assert.ok(extracted.includes('清晨鸟鸣 (2).wav'));
    assert.equal(fs.readFileSync(path.join(tmpDir, '雨声雷声.mp3'), 'utf8'), 'ID3-fake-mp3-data-123456');
    console.log('ZIP 打包测试通过：中文文件名与标准压缩包校验');
  } finally {
    try { fs.unlinkSync(tmpZip); } catch (_) {}
    try { fs.rmSync(tmpDir, { recursive: true, force: true }); } catch (_) {}
  }
})().catch((e) => { console.error(e); process.exitCode = 1; });
