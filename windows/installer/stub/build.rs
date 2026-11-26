// 给外壳嵌图标和清单（asInvoker 的说明见 ../app.manifest）。用 MinGW 自带的 windres，不引依赖。
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    // windres 的路径分隔符要用正斜杠
    let slash = |p: PathBuf| p.display().to_string().replace('\\', "/");
    let icon = slash(here.join("../../src-tauri/icons/icon.ico"));
    let manifest = slash(here.join("../app.manifest"));
    let rc = out.join("stub.rc");
    std::fs::write(&rc, format!("1 ICON \"{icon}\"\n1 24 \"{manifest}\"\n")).unwrap();
    let obj = out.join("stub-res.o");
    let st = Command::new("windres")
        .arg(&rc)
        .arg("-O")
        .arg("coff")
        .arg("-o")
        .arg(&obj)
        .status()
        .expect("找不到 windres（MinGW 要在 PATH 里）");
    assert!(st.success(), "windres 失败");
    println!("cargo:rustc-link-arg-bins={}", obj.display());
    println!("cargo:rerun-if-changed=../app.manifest");
    println!("cargo:rerun-if-changed=embed");
}
