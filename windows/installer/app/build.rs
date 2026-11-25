fn main() {
    // Reuse the app's pointer handling instead of maintaining another drag implementation.
    println!("cargo:rerun-if-changed=../../ui/window-frame.js");
    std::fs::copy("../../ui/window-frame.js", "ui/window-frame.js").expect("copy shared window frame");
    // 版本号跟主程序走，只有 src-tauri/tauri.conf.json 这一处
    let conf = "../../src-tauri/tauri.conf.json";
    println!("cargo:rerun-if-changed={conf}");
    let text = std::fs::read_to_string(conf).expect("读不到主程序的 tauri.conf.json");
    let v: serde_json::Value = serde_json::from_str(&text).expect("tauri.conf.json 格式不对");
    println!("cargo:rustc-env=SD_VERSION={}", v["version"].as_str().unwrap_or("0.0.0"));

    // 普通版 / NAS 版：build.sh 通过 SHANDIAN_EDITION=nas 指定
    println!("cargo:rerun-if-env-changed=SHANDIAN_EDITION");
    let edition = if std::env::var("SHANDIAN_EDITION").as_deref() == Ok("nas") { "nas" } else { "plain" };
    println!("cargo:rustc-env=SD_EDITION={edition}");

    println!("cargo:rerun-if-changed=../app.manifest");
    let attrs = tauri_build::Attributes::new().windows_attributes(
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("../app.manifest")),
    );
    tauri_build::try_build(attrs).expect("tauri build 失败");
}
