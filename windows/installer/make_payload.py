"""把发布目录打成安装数据。只打主程序要用的文件，说明文件不进安装包。"""
import os, sys, zipfile
src, dst = sys.argv[1], sys.argv[2]
SKIP = {"使用说明.txt"}
with zipfile.ZipFile(dst + ".tmp", "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for root, _, files in os.walk(src):
        for f in files:
            if f in SKIP:
                continue
            p = os.path.join(root, f)
            z.write(p, os.path.relpath(p, src).replace(os.sep, "/"))
os.replace(dst + ".tmp", dst)
print("payload", os.path.getsize(dst))
