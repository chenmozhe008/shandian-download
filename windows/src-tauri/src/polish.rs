// 字幕润色：先修结构（去掉自动字幕的滚动重复、拼成整句），再用所选的 AI 服务校对或翻译。
//
// YouTube 自动字幕是滚动的：每条都重复上一条的后半句，机翻又是按碎片翻的，
// 所以才有行首标点和「我们只是把」「它隐藏在」这种半句。只改文字救不了，结构要先修。
// 模型只碰文字，时间轴和条数由这里决定，模型漏条、串行时这几句保留原文，不会错位。
// 本机模式只连 Ollama，API 模式由字幕服务配置决定。

use crate::subtitle_ai::Model;
use std::path::{Path, PathBuf};
use std::time::Duration;

const OLLAMA: &str = "http://127.0.0.1:11434";
const BATCH: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub start: u64,
    pub end: u64,
    pub zh: String,
    pub en: String,
}

// ---------- 找文件、读写 ----------

/// 这个视频的中文字幕：优先合并好的中英那份，其次只有中文的
pub fn find_sub(video: &Path) -> Option<PathBuf> {
    let stem = video.file_stem()?.to_string_lossy().to_lowercase();
    let prefix = format!("{stem}.");
    let mut hits: Vec<(u8, PathBuf)> = std::fs::read_dir(video.parent()?)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let name = p.file_name()?.to_string_lossy().to_lowercase();
            let rest = name.strip_prefix(&prefix)?.to_string();
            if !crate::video_files::is_subtitle_tail(&rest) { return None; }
            let ext = rest.rsplit('.').next()?.to_string();
            if !["ass", "srt", "vtt"].contains(&ext.as_str()) {
                return None;
            }
            let rank = if rest.starts_with("zh-en.") {
                0
            } else if rest.starts_with("zh") {
                1
            } else if lang_of(&name).is_some() {
                2 // 只有原文（英语、日语、俄语……）：让模型翻译
            } else {
                return None;
            };
            Some((rank, p))
        })
        .collect();
    hits.sort();
    // 翻译出来的中英文件没有留底：底稿是旁边的原文，重新处理要从它开始
    if hits.first().is_some_and(|(_, p)| !backup_of(p).exists() && is_translated(p)) {
        if let Some((_, src)) = hits.iter().find(|(r, _)| *r == 2) {
            return Some(src.clone());
        }
    }
    hits.into_iter().next().map(|(_, p)| p)
}

fn is_translated(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().ends_with(".zh-en.ass"))
}

/// 文件名里的语言段（v.ja.ass → ja）。对比文件这类不是语言代码的不算
fn lang_of(name: &str) -> Option<String> {
    let mut it = name.rsplitn(3, '.');
    let (_ext, lang) = (it.next()?, it.next()?);
    it.next()?;
    let lang = lang.to_ascii_lowercase();
    let head = lang.split('-').next()?;
    let ok = (2..=3).contains(&head.len()) && head.chars().all(|c| c.is_ascii_lowercase());
    (ok && lang.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).then_some(lang)
}

/// 这份是没有中文的原文（不是 zh、zh-Hans、zh-en）
fn is_source_lang(sub: &Path) -> bool {
    let name = sub.file_name().unwrap_or_default().to_string_lossy().to_string();
    lang_of(&name).is_some_and(|l| !l.starts_with("zh"))
}

/// 润色前的原字幕留底：v.zh-en.ass → v.zh-en.ass.bak（srt 就是 .srt.bak），通用的备份写法。
/// 以前叫 .original.bak、.original_bak，旧名字的留底第一次用到时顺手改成新名字
pub fn backup_of(sub: &Path) -> PathBuf {
    let with = |suffix: &str| {
        let mut s = sub.as_os_str().to_os_string();
        s.push(suffix);
        PathBuf::from(s)
    };
    let new = with(".bak");
    if !new.exists() {
        for old in [with(".original_bak"), with(".original.bak")] {
            if old.is_file() {
                let _ = std::fs::rename(&old, &new);
                break;
            }
        }
    }
    new
}

fn is_ass(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ass"))
}

fn tag_of(text: &str) -> String {
    if text.starts_with('{') {
        text.find('}').map_or(String::new(), |i| text[..=i].to_string())
    } else {
        String::new()
    }
}

/// 读字幕。ass 还要带回头部和中英两段的样式标签，重写时沿用着色和字号
fn read_sub(path: &Path, text: &str) -> (Vec<Line>, String, String, String) {
    if !is_ass(path) {
        let cues = crate::engine::parse_subs(path);
        let lines = cues.into_iter().map(|c| Line { start: c.start, end: c.end, zh: c.text.replace('\n', " "), en: String::new() }).collect();
        return (lines, String::new(), String::new(), String::new());
    }
    let mut head = String::new();
    let (mut tag_zh, mut tag_en) = (String::new(), String::new());
    let mut lines = Vec::new();
    for l in text.lines() {
        if !l.trim_start().to_ascii_lowercase().starts_with("dialogue:") {
            // 黑底那一层的样式不带过去：润色写回后按任务的样式重新加
            if lines.is_empty() && !l.trim_start().to_ascii_lowercase().starts_with("style: box,") {
                head.push_str(l);
                head.push('\n');
            }
            continue;
        }
        if crate::engine::is_box_line(l) {
            continue;
        }
        let mut it = l.splitn(10, ',');
        let fields: Vec<&str> = it.by_ref().take(9).collect();
        let body = it.next().unwrap_or("");
        let start = fields.get(1).and_then(|t| crate::engine::ts_ms(t)).unwrap_or(0);
        let end = fields.get(2).and_then(|t| crate::engine::ts_ms(t)).unwrap_or(0);
        let (a, b) = body.split_once("\\N").unwrap_or((body, ""));
        if lines.is_empty() {
            tag_zh = tag_of(a);
            tag_en = tag_of(b);
        }
        lines.push(Line {
            start,
            end,
            zh: crate::engine::strip_tags(a).trim().to_string(),
            en: crate::engine::strip_tags(b).trim().to_string(),
        });
    }
    (lines, head, tag_zh, tag_en)
}

fn srt_time(ms: u64, sep: char) -> String {
    format!("{:02}:{:02}:{:02}{sep}{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

fn write_sub(path: &Path, lines: &[Line], head: &str, tag_zh: &str, tag_en: &str) -> std::io::Result<()> {
    let mut out = String::new();
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "ass" => {
            out.push_str(head);
            for l in lines {
                let mut body = format!("{tag_zh}{}", l.zh);
                if !l.en.is_empty() {
                    body.push_str(&format!("\\N{tag_en}{}", l.en));
                }
                out.push_str(&format!(
                    "Dialogue: 0,{},{},Default,,0,0,0,,{body}\n",
                    crate::engine::ass_time(l.start),
                    crate::engine::ass_time(l.end)
                ));
            }
        }
        "vtt" => {
            out.push_str("WEBVTT\n\n");
            for l in lines {
                out.push_str(&format!("{} --> {}\n{}\n\n", srt_time(l.start, '.'), srt_time(l.end, '.'), l.zh));
            }
        }
        _ => {
            for (i, l) in lines.iter().enumerate() {
                out.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, srt_time(l.start, ','), srt_time(l.end, ','), l.zh));
            }
        }
    }
    // 先写临时文件再换：中途失败不能把字幕弄没
    let tmp = path.with_extension("polish.tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, path)
}

// ---------- 修结构 ----------

fn is_cjk(c: char) -> bool {
    ('\u{3400}'..='\u{9fff}').contains(&c)
}

/// 词之间有没有空格：日语、中文没有，要按字比较重复
fn spaced(lines: &[Line]) -> bool {
    let (mut spaces, mut total) = (0usize, 0usize);
    for l in lines {
        spaces += l.en.chars().filter(|c| *c == ' ').count();
        total += l.en.chars().count();
    }
    total > 0 && spaces * 12 >= total
}

/// 这段文字里新出现的部分：去掉和前文结尾重复的那段。容差只在首尾，中间不猜
fn new_tail(prev: &[String], cur: &[String]) -> usize {
    let max = prev.len().min(cur.len());
    (1..=max).rev().find(|n| prev[prev.len() - n..] == cur[..*n]).unwrap_or(0)
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(String::from).collect()
}

/// 合并字幕时重叠的句子会接在一起（A B A B）：连续重复的一段只留一份
fn collapse_repeats(mut t: Vec<String>) -> Vec<String> {
    let mut i = 0;
    while i < t.len() {
        let max = (t.len() - i) / 2;
        match (3..=max).rev().find(|n| t[i..i + n] == t[i + n..i + 2 * n]) {
            Some(n) => {
                t.drain(i..i + n);
            }
            None => i += 1,
        }
    }
    t
}

fn chars(s: &str) -> Vec<String> {
    s.chars().filter(|c| !c.is_whitespace()).map(|c| c.to_string()).collect()
}

const LEADING: &[char] = &['，', '。', '、', '；', '：', '！', '？', ',', '.', ';', ':', '!', '?', '）', ')', '」', '』', '】', '》'];
const SENTENCE_END: &[char] = &['.', '?', '!', '。', '？', '！', '…'];

fn strip_speaker_marker(text: &str) -> &str {
    let text = text.trim_start();
    for marker in [">", "＞", "&gt;"] {
        let mut rest = text;
        let mut count = 0;
        while let Some(next) = rest.strip_prefix(marker) {
            count += 1;
            rest = next;
        }
        if count >= 2 && (rest.is_empty() || rest.starts_with(char::is_whitespace)) {
            return rest.trim_start();
        }
    }
    text
}

/// 整理：去滚动重复 → 拼成整句 → 行首标点归位 → 时间不重叠
pub fn restructure(src: &[Line]) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    let (mut seen_en, mut seen_zh): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let mut cur: Option<Line> = None;
    let has_en = src.iter().any(|l| !l.en.is_empty());
    let by_word = spaced(src);
    for l in src {
        if l.end <= l.start {
            continue;
        }
        let en = strip_speaker_marker(&l.en);
        let en_tokens = collapse_repeats(if by_word { words(en) } else { chars(en) });
        let n = new_tail(&seen_en, &en_tokens);
        let en_new = en_tokens[n..].join(if by_word { " " } else { "" });
        seen_en.extend_from_slice(&en_tokens[n..]);
        let zh_tokens = collapse_repeats(chars(strip_speaker_marker(&l.zh)));
        let n = new_tail(&seen_zh, &zh_tokens);
        let zh_new: String = zh_tokens[n..].concat();
        seen_zh.extend_from_slice(&zh_tokens[n..]);
        if en_new.is_empty() && zh_new.is_empty() {
            // 全是重复：只把上一句的显示时间延到这里
            if let Some(c) = cur.as_mut() {
                c.end = c.end.max(l.end);
            }
            continue;
        }
        // 隔得太久就不是同一句
        if let Some(c) = cur.take() {
            if l.start > c.end + 1500 {
                out.push(c);
            } else {
                cur = Some(c);
            }
        }
        let c = cur.get_or_insert(Line { start: l.start, end: l.end, zh: String::new(), en: String::new() });
        c.end = c.end.max(l.end);
        if !en_new.is_empty() {
            if !c.en.is_empty() && by_word {
                c.en.push(' ');
            }
            c.en.push_str(&en_new);
        }
        c.zh.push_str(&zh_new);
        let basis = if has_en { &c.en } else { &c.zh };
        let long = if has_en { c.en.len() >= 90 } else { c.zh.chars().count() >= 34 };
        if long || basis.trim_end().ends_with(SENTENCE_END) {
            out.push(cur.take().unwrap());
        }
    }
    out.extend(cur);
    // 行首的标点还给上一句
    for i in 0..out.len() {
        let lead: String = out[i].zh.chars().take_while(|c| LEADING.contains(c)).collect();
        if !lead.is_empty() {
            out[i].zh = out[i].zh[lead.len()..].to_string();
            if i > 0 && !out[i - 1].zh.ends_with(LEADING) {
                let first = lead.chars().next().unwrap();
                out[i - 1].zh.push(first);
            }
        }
    }
    out.retain(|l| !l.zh.is_empty() || !l.en.is_empty());
    for i in 1..out.len() {
        if out[i - 1].end > out[i].start {
            out[i - 1].end = out[i].start;
        }
    }
    out
}

// ---------- Ollama ----------

fn client(timeout: u64) -> Option<reqwest::blocking::Client> {
    // 本机服务，别走系统代理
    reqwest::blocking::Client::builder().no_proxy().timeout(Duration::from_secs(timeout)).build().ok()
}

/// 本机装的模型（从大到小）。没装或没开就是空的
pub fn models() -> Vec<String> {
    let Some(c) = client(3) else { return Vec::new() };
    let Ok(v) = c.get(format!("{OLLAMA}/api/tags")).send().and_then(|r| r.json::<serde_json::Value>()) else {
        return Vec::new();
    };
    let mut list: Vec<(u64, String)> = v["models"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|m| Some((m["size"].as_u64().unwrap_or(0), m["name"].as_str()?.to_string())))
        .collect();
    list.sort_by(|a, b| b.0.cmp(&a.0));
    list.into_iter().map(|x| x.1).collect()
}

/// 英文要不要交给模型纠错：只纠拉丁字母写的，其他语言意思通就行，原文不动
fn is_latin(s: &str) -> bool {
    let total = s.chars().filter(|c| c.is_alphabetic()).count();
    total > 0 && s.chars().filter(|c| c.is_ascii_alphabetic()).count() * 10 >= total * 9
}

/// 模型改过的英文只接受小修：词数差太多就是改写或串行了，留原文
fn fix_en(old: &str, new: Option<&str>) -> String {
    let Some(new) = new.map(str::trim).filter(|n| !n.is_empty()) else { return old.to_string() };
    let (a, b) = (old.split_whitespace().count(), new.split_whitespace().count());
    if !is_latin(old) || !is_latin(new) || a.abs_diff(b) > 2 + a / 5 {
        return old.to_string();
    }
    new.to_string()
}

/// 模型返回的这批能不能用：条数、序号都要对，每条都得有中文。返回（中文，改过的英文）
pub fn accept(reply: &str, want: usize) -> Option<Vec<(String, Option<String>)>> {
    let v: serde_json::Value = serde_json::from_str(reply).ok()?;
    let arr = v["lines"].as_array()?;
    if arr.len() != want {
        return None;
    }
    let mut out = vec![(String::new(), None); want];
    for o in arr {
        let i = o["i"].as_u64()? as usize;
        let zh = o["zh"].as_str()?.trim();
        if i >= want || !out[i].0.is_empty() || !zh.chars().any(is_cjk) {
            return None;
        }
        out[i] = (zh.trim_start_matches(LEADING).to_string(), o["en"].as_str().map(String::from));
    }
    Some(out)
}

fn ask(c: &reqwest::blocking::Client, model: &Model, title: &str, lines: &[Line], before: &[Line]) -> Result<Option<Vec<(String, Option<String>)>>, String> {
    let has_en = lines.iter().any(|l| !l.en.is_empty());
    let items: Vec<serde_json::Value> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if l.zh.is_empty() {
                serde_json::json!({"i": i, "en": l.en})
            } else if has_en {
                serde_json::json!({"i": i, "en": l.en, "zh": l.zh})
            } else {
                serde_json::json!({"i": i, "zh": l.zh})
            }
        })
        .collect();
    let context: Vec<&str> = before.iter().map(|l| l.zh.as_str()).collect();
    let task = if lines.iter().all(|l| l.zh.is_empty()) {
        "下面是视频字幕的原文（字段 en，可能是任何语言，也可能有自动识别的错字），把每条翻译成自然的中文，专有名词和软件术语可保留原文。"
    } else if has_en {
        "下面是视频字幕，中文多为机器直译。参考英文把每条中文改通顺、意思准确，专业术语和软件节点名保留英文。"
    } else {
        "下面是视频字幕，中文多为机器翻译。把每条中文改通顺，看不出原意的地方不要猜，保持原样。"
    };
    // 英文多是语音识别出来的，术语常被听错（SOP 听成 soop）。标题能告诉模型这是什么领域
    let latin = lines.iter().any(|l| is_latin(&l.en));
    let fix = if latin {
        "英文是语音自动识别的，专业术语可能被听错（如把 SOP 听成 soop）：结合视频标题判断，只改明显听错的词，不要改写句子，改后放进 en 字段，中文也按改对的意思写。"
    } else {
        ""
    };
    let shape = if latin { r#"{"i":序号,"en":"纠错后的英文","zh":"校对后的中文"}"# } else { r#"{"i":序号,"zh":"校对后的中文"}"# };
    let title = if matches!(model, Model::Api(_)) { "" } else { title };
    let prompt = format!(
        "你是字幕校对。视频标题：{title}\n{task}{fix}每条保持简短，不要解释。条目数和序号必须不变，不要合并或拆分。\n前文（只作参考，不要输出）：{}\n输出 {{\"lines\":[{shape}]}}，必须包含全部 {} 条。\n{}",
        context.join(" "),
        lines.len(),
        serde_json::Value::Array(items)
    );
    if let Model::Api(api) = model { return Ok(accept(&api.generate(c, &prompt)?, lines.len())); }
    let Model::Ollama(model) = model else { unreachable!() };
    let schema = serde_json::json!({"type": "object", "properties": {"lines": {"type": "array", "items": {"type": "object", "properties": {"i": {"type": "integer"}, "en": {"type": "string"}, "zh": {"type": "string"}}, "required": ["i", "zh"]}}}, "required": ["lines"]});
    let body = serde_json::json!({"model": model, "prompt": prompt, "stream": false, "think": false, "format": schema, "options": {"temperature": 0.2}});
    let value = c.post(format!("{OLLAMA}/api/generate")).json(&body).send().and_then(|r| r.json::<serde_json::Value>()).ok();
    Ok(value.as_ref().and_then(|v| v["response"].as_str()).and_then(|text| accept(text, lines.len())))
}

/// 改一批，返回没改成、保留原文的条数。整批不行就对半拆开再试，只有单条也不行才放弃
fn refine(c: &reqwest::blocking::Client, model: &Model, title: &str, lines: &mut [Line], before: &[Line]) -> Result<usize, String> {
    // 只有 [音乐] 这种的不用问模型
    if !lines.iter().any(|l| (l.zh.is_empty() && !l.en.is_empty()) || l.zh.chars().filter(|c| is_cjk(*c)).count() > 2) {
        return Ok(0);
    }
    let remote = matches!(model, Model::Api(_));
    for _ in 0..if remote { 1 } else { 2 } {
        if let Some(got) = ask(c, model, title, lines, before)? {
            for (l, (z, e)) in lines.iter_mut().zip(got) {
                l.en = fix_en(&l.en, e.as_deref());
                l.zh = z;
            }
            return Ok(0);
        }
    }
    if remote || lines.len() == 1 {
        return Ok(lines.len());
    }
    let mid = lines.len() / 2;
    let (a, b) = lines.split_at_mut(mid);
    Ok(refine(c, model, title, a, before)? + refine(c, model, title, b, before)?)
}

pub struct Options<'a> {
    pub model: Option<&'a Model>,
    /// 用户只要中文：英文只作参考，结果里不留
    pub drop_en: bool,
    /// 视频标题：判断术语靠它
    pub title: String,
    /// 只有英文字幕时生成新文件用的（头部、中文标签、英文标签）
    pub style: (String, String, String),
}

pub struct Outcome {
    pub before: usize,
    pub after: usize,
    pub kept: usize,
    pub refined: bool,
}

/// 处理这份字幕。第一次留底，以后总从留底开始，重复润色不会越改越歪。
/// model 为 None 时只修结构
pub fn run(sub: &Path, opts: &Options, progress: &dyn Fn(usize, usize)) -> Result<Outcome, String> {
    let model = opts.model;
    let backup = backup_of(sub);
    let source_only = is_source_lang(sub);
    let source = if backup.is_file() { backup.clone() } else { sub.to_path_buf() };
    let text = std::fs::read_to_string(&source).map_err(|e| format!("读不了字幕：{e}"))?;
    if source_only && model.is_none() {
        return Err("这个视频没有中文字幕，翻译需要可用的 AI 字幕服务".into());
    }
    // 原文文件自己就是底稿，不用另存。parse_subs 按扩展名认格式，留底要读原文件名那份的内容
    if !source_only && !backup.is_file() {
        std::fs::copy(sub, &backup).map_err(|e| format!("备份字幕失败：{e}"))?;
    }
    let parse_from = if is_ass(sub) {
        sub.to_path_buf()
    } else {
        let tmp = sub.with_extension(format!("orig.{}", sub.extension().unwrap_or_default().to_string_lossy()));
        std::fs::copy(&source, &tmp).map_err(|e| e.to_string())?;
        tmp
    };
    let (mut src, mut head, mut tag_zh, mut tag_en) = read_sub(&parse_from, &text);
    if parse_from != sub {
        let _ = std::fs::remove_file(&parse_from);
    }
    if src.is_empty() {
        return Err("字幕里没有内容".into());
    }
    // 没有中文：原文放进第二行，翻译结果写到新的中英文件，原文件不动
    let mut target = sub.to_path_buf();
    if source_only {
        for l in &mut src {
            l.en = std::mem::take(&mut l.zh);
        }
        (head, tag_zh, tag_en) = opts.style.clone();
        let name = sub.file_name().unwrap_or_default().to_string_lossy().to_string();
        let base = name.rsplitn(3, '.').nth(2).unwrap_or(&name).to_string();
        target = sub.with_file_name(format!("{base}.zh-en.ass"));
    }
    let mut lines = restructure(&src);
    let mut kept = 0;
    if let Some(m) = model {
        let c = match m {
            Model::Ollama(_) => client(300).ok_or("连不上 Ollama")?,
            Model::Api(api) => api.client(120)?,
        };
        let total = lines.len().div_ceil(BATCH);
        for n in 0..total {
            progress(n, total);
            let at = n * BATCH;
            let end = (at + BATCH).min(lines.len());
            let before = lines[at.saturating_sub(2)..at].to_vec();
            kept += refine(&c, m, &opts.title, &mut lines[at..end], &before)?;
        }
        progress(total, total);
        // 用完就卸载，别一直占着显存
        if let Model::Ollama(name) = m {
            let _ = c.post(format!("{OLLAMA}/api/generate")).json(&serde_json::json!({"model": name, "keep_alive": 0})).send();
        }
    }
    if source_only && !lines.iter().any(|line| line.zh.chars().any(is_cjk)) {
        return Err("模型没有生成中文字幕，已保留原字幕".into());
    }
    if opts.drop_en {
        for l in &mut lines {
            // 翻译失败的句子只有英文，总比空着强
            if l.zh.is_empty() {
                l.zh = std::mem::take(&mut l.en);
            } else {
                l.en.clear();
            }
        }
    }
    write_sub(&target, &lines, &head, &tag_zh, &tag_en).map_err(|e| format!("写字幕失败：{e}"))?;
    Ok(Outcome { before: src.len(), after: lines.len(), kept, refined: model.is_some() })
}

/// 换回原字幕
pub fn restore(sub: &Path) -> Result<(), String> {
    // find_sub 对翻译过的视频返回原文：把翻译结果删掉就回去了
    if is_source_lang(sub) {
        let name = sub.file_name().unwrap_or_default().to_string_lossy().to_string();
        let base = name.rsplitn(3, '.').nth(2).unwrap_or(&name).to_string();
        return std::fs::remove_file(sub.with_file_name(format!("{base}.zh-en.ass"))).map_err(|e| format!("删除翻译结果失败：{e}"));
    }
    let backup = backup_of(sub);
    std::fs::copy(&backup, sub).map_err(|e| format!("没找到原字幕：{e}"))?;
    let _ = std::fs::remove_file(&backup);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(start: u64, end: u64, zh: &str, en: &str) -> Line {
        Line { start, end, zh: zh.into(), en: en.into() }
    }

    #[test]
    fn rolling_captions_become_sentences() {
        let src = [
            l(0, 2000, "它并没有清除任何东西。", "it doesn't get rid of anything."),
            l(2000, 4000, "它并没有清除任何东西。 数据都在那里。 我们只是把", "it doesn't get rid of anything. The data is there. We're just"),
            l(4000, 6000, "数据都在那里。 我们只是把 它隐藏起来。", "The data is there. We're just hiding it."),
        ];
        let out = restructure(&src);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].en, "it doesn't get rid of anything.");
        assert_eq!(out[1].en, "The data is there. We're just hiding it.");
        assert_eq!(out[1].zh, "数据都在那里。我们只是把它隐藏起来。");
        assert!(out[0].end <= out[1].start);
    }

    #[test]
    fn leading_punctuation_goes_back() {
        let src = [l(0, 1000, "第一句", "one."), l(1000, 2000, "。 第二句", "two.")];
        let out = restructure(&src);
        assert_eq!(out[0].zh, "第一句。");
        assert_eq!(out[1].zh, "第二句");
    }

    #[test]
    fn english_fixes_stay_small() {
        let old = "we need to say soop import and then";
        assert_eq!(fix_en(old, Some("we need to say SOP import and then")), "we need to say SOP import and then");
        // 改写、串行、其他语言都不收
        assert_eq!(fix_en(old, Some("import")), old);
        assert_eq!(fix_en(old, None), old);
        assert_eq!(fix_en("今日はいい天気です", Some("today is nice")), "今日はいい天気です");
    }

    #[test]
    fn languages_from_file_names() {
        assert_eq!(lang_of("a [x].ja.ass").as_deref(), Some("ja"));
        assert_eq!(lang_of("a.en-orig.vtt").as_deref(), Some("en-orig"));
        assert!(lang_of("a.英文直译对比.ass").is_none());
        assert!(is_source_lang(Path::new("a.ko.ass")));
        assert!(!is_source_lang(Path::new("a.zh-Hans.ass")));
        assert!(!is_source_lang(Path::new("a.zh-en.ass")));
    }

    #[test]
    fn languages_without_spaces_dedupe_by_char() {
        let src = [l(0, 1000, "", "今日はいい天気です。"), l(1000, 2000, "", "今日はいい天気です。明日は雨です。")];
        let out = restructure(&src);
        assert_eq!(out.iter().map(|x| x.en.as_str()).collect::<Vec<_>>(), ["今日はいい天気です。", "明日は雨です。"]);
    }

    #[test]
    fn repeats_inside_a_line_collapse() {
        let src = [l(0, 1000, "好的", "keep it fifty fifty keep it fifty fifty okay.")];
        assert_eq!(restructure(&src)[0].en, "keep it fifty fifty okay.");
    }

    #[test]
    fn english_only_makes_new_file_with_backup() {
        let dir = std::env::temp_dir().join(format!("sd-polish-en-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sub = dir.join("v.en.ass");
        std::fs::write(&sub, "[Events]\nDialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,hello there.\n").unwrap();
        // 没模型：翻不了，文件不动，也不留底稿
        let none = Options { model: None, drop_en: false, title: String::new(), style: Default::default() };
        assert!(run(&sub, &none, &|_, _| {}).is_err());
        assert!(sub.is_file() && !backup_of(&sub).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chinese_only_still_works() {
        let src = [l(0, 1000, "你好世界。", ""), l(1000, 2000, "你好世界。 再见。", "")];
        let out = restructure(&src);
        assert_eq!(out.iter().map(|x| x.zh.as_str()).collect::<Vec<_>>(), ["你好世界。", "再见。"]);
    }

    #[test]
    fn bad_model_replies_are_rejected() {
        assert_eq!(accept(r#"{"lines":[{"i":0,"zh":"好"},{"i":1,"zh":"的","en":"ok"}]}"#, 2), Some(vec![("好".to_string(), None), ("的".to_string(), Some("ok".to_string()))]));
        // 少条、重复序号、没中文都不要
        assert!(accept(r#"{"lines":[{"i":0,"zh":"好"}]}"#, 2).is_none());
        assert!(accept(r#"{"lines":[{"i":0,"zh":"好"},{"i":0,"zh":"的"}]}"#, 2).is_none());
        assert!(accept(r#"{"lines":[{"i":0,"zh":"ok"}]}"#, 1).is_none());
        assert!(accept("not json", 1).is_none());
    }

    /// 真实字幕跑一遍：SD_POLISH_SRC=文件 SD_POLISH_MODEL=模型 cargo test real_file -- --ignored --nocapture
    /// 只在临时目录里处理一份拷贝
    #[test]
    #[ignore]
    fn real_file() {
        let src = PathBuf::from(std::env::var("SD_POLISH_SRC").unwrap());
        let dir = std::env::temp_dir().join("sd-polish-real");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 文件名决定走润色还是翻译：SD_POLISH_LANG=ja 就当日语原文
        let sub = dir.join(format!("v.{}.ass", std::env::var("SD_POLISH_LANG").unwrap_or("zh-en".into())));
        std::fs::copy(&src, &sub).unwrap();
        let model = std::env::var("SD_POLISH_MODEL").ok();
        let t0 = std::time::Instant::now();
        let style = crate::engine::subtitle_style(1.0, true, "#00FFAC", "#516EFE");
        let title = std::env::var("SD_POLISH_TITLE").unwrap_or_default();
        let model = model.map(Model::Ollama);
        let opts = Options { model: model.as_ref(), drop_en: false, title, style };
        let r = run(&sub, &opts, &|a, b| eprintln!("批次 {a}/{b} {:.0}s", t0.elapsed().as_secs_f32())).unwrap();
        eprintln!("原 {} 行 → {} 句，保留原文 {} 句，耗时 {:.0}s，输出 {}", r.before, r.after, r.kept, t0.elapsed().as_secs_f32(), sub.display());
    }
}

#[cfg(test)]
mod speaker_marker_tests {
    use super::strip_speaker_marker;
    use crate::engine::strip_tags;

    #[test]
    fn strips_only_the_leading_speaker_marker() {
        assert_eq!(strip_speaker_marker(">> Shift with x >> 1"), "Shift with x >> 1");
        assert_eq!(strip_speaker_marker("&gt;&gt; Welcome"), "Welcome");
        assert_eq!(strip_speaker_marker("＞＞ 这是另一位说话人"), "这是另一位说话人");
        assert_eq!(strip_speaker_marker(">>out.log"), ">>out.log");
        assert_eq!(strip_speaker_marker("比较 x > 1"), "比较 x > 1");
        assert_eq!(
            strip_speaker_marker(&strip_tags("<c>&gt;&gt; Shift with x &gt;&gt; 1.</c>")),
            "Shift with x >> 1."
        );
    }
}
