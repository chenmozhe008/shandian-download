fn main() {
    std::fs::copy("../browser-extension/feedback.js", "../ui/feedback.js").expect("copy shared feedback helper");
    tauri_build::build();
    // 开发版标记：打包时 Windows 版或扩展还有没提交的改动，「关于」的版本号后面标「开发版」（Mac 版同款）
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no", "--", "..", "../browser-extension"])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);
    println!("cargo:rustc-env=SHANDIAN_DEV_BUILD={}", if dirty { "1" } else { "0" });
    for p in ["src", "../ui", "../browser-extension", "../../.git/index"] {
        println!("cargo:rerun-if-changed={p}");
    }
}
