// 下载来源：扩展从页面里取到的视频地址是 CDN（f.video.weibocdn.com、xpccdn…），
// 光看地址认不出是哪个站。扩展交链接时带着所在页面，这里记下「这个地址来自哪个站」，
// 列表上的类型标签优先用它。存在本机数据目录，最多留 500 条。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAX: usize = 500;

struct Store {
    file: PathBuf,
    map: HashMap<String, String>,
    order: Vec<String>,
}

fn store() -> &'static Mutex<Option<Store>> {
    static S: OnceLock<Mutex<Option<Store>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

pub fn init(local: &Path) {
    let file = crate::migrate::local_path(local, "sources.json");
    let order: Vec<(String, String)> = std::fs::read_to_string(&file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let map = order.iter().cloned().collect();
    *store().lock().unwrap() = Some(Store { file, map, order: order.into_iter().map(|(k, _)| k).collect() });
}

/// 地址本身认得出站点就不用记
pub fn remember(url: &str, page: &str) {
    if url.is_empty() || page.is_empty() || crate::api::material_site_label(url).is_some() || crate::links::LinkKind::of(url) == Some(crate::links::LinkKind::VideoSite) {
        return;
    }
    let site = crate::api::material_site_label(page).or_else(|| crate::api::netdisk_label(page)).map(String::from)
        .unwrap_or_else(|| crate::videos::site_name(page));
    if !is_known(&site) {
        return;
    }
    let mut guard = store().lock().unwrap();
    let Some(s) = guard.as_mut() else { return };
    if s.map.get(url) == Some(&site) {
        return;
    }
    if s.map.insert(url.to_string(), site).is_none() {
        s.order.push(url.to_string());
    }
    while s.order.len() > MAX {
        let old = s.order.remove(0);
        s.map.remove(&old);
    }
    let list: Vec<(&String, &String)> = s.order.iter().filter_map(|k| s.map.get(k).map(|v| (k, v))).collect();
    if let Ok(text) = serde_json::to_string(&list) {
        let _ = std::fs::write(&s.file, text);
    }
}

pub fn lookup(url: &str) -> Option<String> {
    store().lock().unwrap().as_ref()?.map.get(url).cloned()
}

/// site_name 认不出时返回的是域名，只有认得出的站名才记
fn is_known(site: &str) -> bool {
    !site.is_empty() && !site.contains('.')
}
