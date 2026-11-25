// 系统代理路由。照 Mac 版 LocalProxy.swift：YouTube、X 这些境外站走代理（住宅 IP 绕过
// bot 检测），国内站直连少绕一道。代理跟浏览器一样读系统设置，写死端口会被 Clash 换端口坑到。
//
// 什么走代理（Mac 版 10-28 定的规则）：
// - 磁力 / 种子：一律直连（BT 协议走内核，这里管不着）
// - HTTP 直链、视频网站：按域名判断，国内站直连，其余（YouTube、X、GitHub 等）走代理

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 国内站点后缀：命中就直连。没列到的境外站交给 Clash 规则再判断一次
const DOMESTIC_SUFFIXES: &[&str] = &[
    ".cn", "baidu.com", "bdstatic.com", "bcebos.com", "baidupcs.com",
    "aliyun.com", "aliyuncs.com", "alicdn.com", "aliyundrive.com", "alipan.com",
    "quark.cn", "qq.com", "gtimg.com", "myqcloud.com", "tencent-cloud.com", "weiyun.com",
    "163.com", "126.net", "netease.com",
    "bilibili.com", "bilivideo.com", "bilivideo.cn", "hdslb.com", "b23.tv",
    "weibo.com", "sinaimg.cn", "sina.com",
    "douyin.com", "douyinvod.com", "douyincdn.com", "zjcdn.com", "amemv.com", "snssdk.com",
    "ixigua.com", "bytedance.com", "byteimg.com", "volces.com",
    "xiaohongshu.com", "xhscdn.com",
    "kuaishou.com", "kwaicdn.com", "kwimgs.com", "yximgs.com",
    "iqiyi.com", "youku.com", "mgtv.com", "sohu.com",
    "xunlei.com", "115.com", "123pan.com", "lanzou.com", "lanzoui.com",
    "huaweicloud.com", "myhuaweicloud.com", "ctyun.cn",
    "jd.com", "taobao.com", "tmall.com", "zhihu.com", "csdn.net", "gitee.com",
];

/// 读到的系统代理（host:port）
#[derive(Clone)]
struct SysProxy {
    url: String,
    up: bool,
}

/// 系统代理随时会开关（用户下到一半才开 Clash 很常见），不能只读一次：
/// 以前读一次用到退出，启动时没开，之后所有任务和封面都一直直连到超时。
/// 短暂缓存只是因为列表每秒都在问。
const PROXY_TTL: Duration = Duration::from_secs(2);

fn system_proxy() -> SysProxy {
    static CACHE: Mutex<Option<(Instant, SysProxy)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    if let Some((at, p)) = cache.as_ref() {
        if at.elapsed() < PROXY_TTL {
            return p.clone();
        }
    }
    // 读 Windows 注册表的系统代理设置（IE/WinINET 那套，Chrome/Clash 都写这里）
    let (host, port) = read_registry_proxy();
    let up = !host.is_empty() && port_open(&host, port);
    let p = SysProxy {
        url: if up { format!("http://{host}:{port}") } else { String::new() },
        up,
    };
    *cache = Some((Instant::now(), p.clone()));
    p
}

fn read_registry_proxy() -> (String, u16) {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hk = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(k) = hk.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings") else {
        return (String::new(), 0);
    };
    let enabled: u32 = k.get_value("ProxyEnable").unwrap_or(0);
    if enabled != 1 {
        return (String::new(), 0);
    }
    let server: String = k.get_value("ProxyServer").unwrap_or_default();
    let pick = |key: &str| -> Option<(String, u16)> {
        for part in server.split(';') {
            let part = part.trim();
            if let Some((k2, v)) = part.split_once('=') {
                if k2.eq_ignore_ascii_case(key) {
                    return parse_hostport(v);
                }
            }
        }
        None
    };
    pick("https")
        .or_else(|| pick("http"))
        .or_else(|| if server.contains('=') { None } else { parse_hostport(&server) })
        .unwrap_or((String::new(), 0))
}

fn parse_hostport(s: &str) -> Option<(String, u16)> {
    let s = s.trim().trim_start_matches("http://").trim_start_matches("https://");
    let (h, p) = s.rsplit_once(':')?;
    let port: u16 = p.parse().ok()?;
    if h.is_empty() {
        return None;
    }
    Some((h.to_string(), port))
}

/// 代理开着没有：端口连得上就算。以前是经代理访问外网探测、只等 1.5 秒，
/// 节点一慢就判成没开，同样的环境时好时坏
fn port_open(host: &str, port: u16) -> bool {
    use std::net::ToSocketAddrs;
    let Ok(mut addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    addrs.next().is_some_and(|a| {
        std::net::TcpStream::connect_timeout(&a, Duration::from_millis(500)).is_ok()
    })
}

/// 这个链接要不要走代理
pub fn needed(link: &str) -> bool {
    if !system_proxy().up {
        return false;
    }
    let Some(host) = host_of(link) else { return false };
    let host = host.to_lowercase();
    // 本机 / 内网地址不走
    if host == "localhost" || host.starts_with("127.") || host.starts_with("192.168.")
        || host.starts_with("10.") || host.starts_with("172.16.")
    {
        return false;
    }
    !DOMESTIC_SUFFIXES.iter().any(|s| {
        let s = s.trim_start_matches('.');
        host == s || host.ends_with(&format!(".{s}"))
    })
}

/// 给 reqwest 一类客户端用的代理地址：需要走代理就返回 Some(代理 URL)。
/// reqwest 不读 Windows 注册表的系统代理，境外资源（YouTube 封面等）必须显式挂上。
pub fn url_for(link: &str) -> Option<String> {
    let sp = system_proxy();
    if sp.up && needed(link) {
        Some(sp.url.clone())
    } else {
        None
    }
}

/// 给 yt-dlp 的代理参数：需要就返回 ["--proxy", "http://..."]
pub fn ytdlp_args(link: &str) -> Vec<String> {
    let sp = system_proxy();
    if sp.up && needed(link) {
        vec!["--proxy".to_string(), sp.url.clone()]
    } else {
        vec![]
    }
}

/// 系统代理的地址（给 gateway 或别的地方用）
pub fn url() -> Option<String> {
    let sp = system_proxy();
    if sp.up {
        Some(sp.url.clone())
    } else {
        None
    }
}

fn host_of(link: &str) -> Option<&str> {
    // 简单解析：去掉 scheme，取到第一个 / 或 ? 之前
    let s = link
        .strip_prefix("https://")
        .or_else(|| link.strip_prefix("http://"))
        .unwrap_or(link);
    let end = s.find(['/', '?', '#']).unwrap_or(s.len());
    let hostport = &s[..end];
    Some(hostport.split('@').next_back()?.split(':').next()?)
}

#[cfg(test)]
mod tests {
    use super::port_open;

    #[test]
    fn port_open_follows_listener() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(port_open("127.0.0.1", port));
        // 代理关了：端口立刻连不上，下一次刷新就会改走直连
        drop(l);
        assert!(!port_open("127.0.0.1", port));
    }
}
