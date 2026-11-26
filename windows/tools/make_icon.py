#!/usr/bin/env python3
"""从一张 PNG 生成 Windows 用的 .ico（以及顺带生成 Linux/网页可用的各尺寸 PNG）。

只用标准库（zlib）。为什么要自己写：这台机器的 pip 装不上 Pillow，
PowerShell 的 Add-Type 又被安全策略拦了，而 tauri-build 生成 Windows 资源文件
必须有 windows/src-tauri/icons/icon.ico。

用法：
    python windows/tools/make_icon.py
    python windows/tools/make_icon.py 源图.png 输出目录
"""
import os
import struct
import sys
import zlib

SIZES = [16, 24, 32, 48, 64, 128, 256]


def _paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    return b if pb <= pc else c


def decode_png(path):
    """解出 (width, height, bytearray RGBA)。只支持 8 位、非隔行的 RGB/RGBA/灰度。"""
    raw = open(path, "rb").read()
    if raw[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("不是 PNG")
    pos = 8
    idat = bytearray()
    w = h = bd = ct = interlace = 0
    while pos < len(raw):
        (length,) = struct.unpack(">I", raw[pos:pos + 4])
        ctype = raw[pos + 4:pos + 8]
        body = raw[pos + 8:pos + 8 + length]
        pos += 12 + length
        if ctype == b"IHDR":
            w, h, bd, ct, _comp, _filt, interlace = struct.unpack(">IIBBBBB", body)
        elif ctype == b"IDAT":
            idat += body
        elif ctype == b"IEND":
            break
    if bd != 8:
        raise ValueError(f"只支持 8 位深度，这是 {bd}")
    if interlace:
        raise ValueError("不支持隔行 PNG")
    channels = {0: 1, 2: 3, 4: 2, 6: 4}.get(ct)
    if channels is None:
        raise ValueError(f"不支持的颜色类型 {ct}（调色板图请先转成 RGBA）")

    data = zlib.decompress(bytes(idat))
    stride = w * channels
    out = bytearray(h * stride)
    prev = bytearray(stride)
    p = 0
    for y in range(h):
        f = data[p]
        p += 1
        line = bytearray(data[p:p + stride])
        p += stride
        if f == 1:      # Sub
            for i in range(channels, stride):
                line[i] = (line[i] + line[i - channels]) & 0xFF
        elif f == 2:    # Up
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif f == 3:    # Average
            for i in range(stride):
                a = line[i - channels] if i >= channels else 0
                line[i] = (line[i] + ((a + prev[i]) >> 1)) & 0xFF
        elif f == 4:    # Paeth
            for i in range(stride):
                a = line[i - channels] if i >= channels else 0
                c = prev[i - channels] if i >= channels else 0
                line[i] = (line[i] + _paeth(a, prev[i], c)) & 0xFF
        out[y * stride:(y + 1) * stride] = line
        prev = line

    # 统一成 RGBA
    if channels == 4:
        return w, h, out
    rgba = bytearray(w * h * 4)
    for i in range(w * h):
        if channels == 3:
            r, g, b = out[i * 3:i * 3 + 3]
            a = 255
        elif channels == 2:
            v, a = out[i * 2:i * 2 + 2]
            r = g = b = v
        else:
            r = g = b = out[i]
            a = 255
        rgba[i * 4:i * 4 + 4] = bytes((r, g, b, a))
    return w, h, rgba


def resize_area(sw, sh, src, tw, th):
    """面积平均缩放。缩小时比最近邻干净得多，图标边缘不会锯齿。"""
    dst = bytearray(tw * th * 4)
    for ty in range(th):
        y0 = ty * sh / th
        y1 = (ty + 1) * sh / th
        for tx in range(tw):
            x0 = tx * sw / tw
            x1 = (tx + 1) * sw / tw
            r = g = b = a = 0.0
            wsum = 0.0
            iy = int(y0)
            while iy < min(sh, int(y1) + (1 if y1 > int(y1) else 0)):
                hy = min(y1, iy + 1) - max(y0, iy)
                if hy <= 0:
                    iy += 1
                    continue
                ix = int(x0)
                while ix < min(sw, int(x1) + (1 if x1 > int(x1) else 0)):
                    wx = min(x1, ix + 1) - max(x0, ix)
                    if wx > 0:
                        wt = wx * hy
                        o = (iy * sw + ix) * 4
                        # 按 alpha 加权，避免透明边缘发黑
                        al = src[o + 3] / 255.0
                        r += src[o] * wt * al
                        g += src[o + 1] * wt * al
                        b += src[o + 2] * wt * al
                        a += src[o + 3] * wt
                        wsum += wt
                    ix += 1
                iy += 1
            if wsum == 0:
                continue
            o = (ty * tw + tx) * 4
            aw = a / wsum
            if aw > 0:
                # 还原回非预乘
                scale = (a / 255.0) if a else 1.0
                dst[o] = min(255, int(r / scale + 0.5))
                dst[o + 1] = min(255, int(g / scale + 0.5))
                dst[o + 2] = min(255, int(b / scale + 0.5))
            dst[o + 3] = min(255, int(aw + 0.5))
    return dst


def encode_png(w, h, rgba):
    raw = bytearray()
    stride = w * 4
    for y in range(h):
        raw.append(0)  # filter None
        raw += rgba[y * stride:(y + 1) * stride]
    comp = zlib.compress(bytes(raw), 9)

    def chunk(tag, body):
        return (struct.pack(">I", len(body)) + tag + body
                + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF))

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", comp) + chunk(b"IEND", b""))


def build_ico(pngs):
    """pngs: [(size, png_bytes)]，按 ICO 规范把 PNG 直接嵌进去（Vista 起支持）。"""
    n = len(pngs)
    header = struct.pack("<HHH", 0, 1, n)
    entries = b""
    offset = 6 + 16 * n
    blobs = b""
    for size, data in pngs:
        dim = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
        blobs += data
    return header + entries + blobs


def main():
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    src = sys.argv[1] if len(sys.argv) > 1 else os.path.join(root, "Resources", "AppIcon-source.png")
    outdir = sys.argv[2] if len(sys.argv) > 2 else os.path.join(root, "windows", "src-tauri", "icons")
    os.makedirs(outdir, exist_ok=True)

    w, h, rgba = decode_png(src)
    print(f"源图 {w}x{h}")

    pngs = []
    for s in SIZES:
        scaled = resize_area(w, h, rgba, s, s)
        data = encode_png(s, s, scaled)
        pngs.append((s, data))
        if s in (32, 256):
            # 顺手留两张 PNG，托盘和网页能用
            with open(os.path.join(outdir, f"{s}x{s}.png"), "wb") as f:
                f.write(data)
        print(f"  {s}x{s}  {len(data)} 字节")

    ico = build_ico(pngs)
    target = os.path.join(outdir, "icon.ico")
    with open(target, "wb") as f:
        f.write(ico)
    print(f"写出 {target}（{len(ico)} 字节，{len(SIZES)} 个尺寸）")


if __name__ == "__main__":
    main()
