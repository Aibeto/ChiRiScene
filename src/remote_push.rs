//! remote_push.rs: [schedule] [fetch] [match] [notify] 每天 08~24 点每 2 小时拉 remindPush.json，命中内嵌 versionCode 就发远程通知
//! 槽位 = 本地偶数整点 08..22，按 (年,月,日,时) 去重；主源失败退 gitee，都失败不重试，未命中静默，tag ChiRi-Remote

use std::time::Duration;

/// 主源：GitHub raw
const PRIMARY_URL: &str = "https://raw.githubusercontent.com/Aibeto/ChiRiScene/refs/heads/main/updateInformation/remindPush.json";
/// 备用源：gitee raw，主源不通时用，同样只试一次
const FALLBACK_URL: &str =
    "https://raw.giteeusercontent.com/Aibeto/ChiRiScene/raw/main/updateInformation/remindPush.json";
/// 通知 tag（无拦截器）
pub const REMOTE_TAG: &str = "ChiRi-Remote";
/// 通知标题固定前缀：远程内容一律带官方标识，降低伪造「官方通知」的钓鱼面
const TITLE_PREFIX: &str = "[ChiRi] ";
/// 远程标题字符上限（按字符截断，不切坏 UTF-8）
const MAX_TITLE_CHARS: usize = 120;
/// 远程正文字符上限
const MAX_BODY_CHARS: usize = 800;
/// 单次 HTTP 总超时（连接+收发）
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// 响应体上限：正常几百字节，64KB 能挡住异常大响应
const MAX_BODY_BYTES: u64 = 64 * 1024;
/// 槽位巡检间隔：一分钟一次
const TICK: Duration = Duration::from_secs(60);
/// 允许拉取的小时（本地时间，08~24 点每 2 小时）
const SLOT_HOURS: [i32; 8] = [8, 10, 12, 14, 16, 18, 20, 22];

/// 本机 versionCode：只认编译期内嵌的 `module.prop`（`include_str!`），不读磁盘
/// 磁盘那份可被篡改去命中伪造条目；读不到返回空串，`find_entry` 直接判不命中，绝不退化成匹配任意一条
pub fn current_version_code() -> String {
    const EMBEDDED: &str = include_str!("../module/module.prop");
    EMBEDDED
        .lines()
        .find_map(|l| l.trim().strip_prefix("versionCode="))
        .map(|v| v.trim().to_string())
        .unwrap_or_default()
}

/// 从一条记录里取 versionCode（字符串或数字都认）
fn entry_code(item: &serde_json::Value) -> Option<String> {
    match item.get("versionCode")? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// 在 JSON 数组里找与本机 versionCode 完全相同的一条，返回 (title, content)
/// 只做 trim 级清洗，不做数值归一（否则 `030000` 会命中 `30000`）；解析失败/无命中一律 None
fn find_entry(json: &str, current_code: &str) -> Option<(String, String)> {
    let want = current_code.trim();
    if want.is_empty() {
        // 内嵌 module.prop 缺 versionCode 时绝不匹配：空值不是通配符
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(json).ok()?;
    for item in parsed.as_array()? {
        let Some(code) = entry_code(item) else {
            continue;
        };
        if code.trim() != want {
            continue;
        }
        // title/content 必须都是非空字符串，否则发了也是空通知
        let title = item.get("title")?.as_str()?.trim();
        let content = item.get("content")?.as_str()?.trim();
        if title.is_empty() || content.is_empty() {
            return None;
        }
        return Some((title.to_string(), content.to_string()));
    }
    None
}

/// GET 一段文本：超时 + 体积上限，错误折叠成可读字符串供 info 留痕
fn fetch_text(url: &str) -> Result<String, String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .https_only(true)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    agent
        .get(url)
        .call()
        .map_err(|e| format!("请求失败: {e}"))?
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_string()
        .map_err(|e| format!("读取响应失败: {e}"))
}

/// 拉一次：主源失败退备用源，都失败返回 None，不重试
fn fetch_any() -> Option<(String, &'static str)> {
    match fetch_text(PRIMARY_URL) {
        Ok(body) => Some((body, "primary")),
        Err(e) => {
            // [level] 连接失败不重试，只留一条 info
            log::info!(
                "{}",
                crate::i18n::t_with_args(
                    "remote-push-primary-failed",
                    &crate::fluent_args! {"error" => e},
                )
            );
            match fetch_text(FALLBACK_URL) {
                Ok(body) => Some((body, "fallback")),
                Err(e2) => {
                    log::info!(
                        "{}",
                        crate::i18n::t_with_args(
                            "remote-push-fallback-failed",
                            &crate::fluent_args! {"error" => e2},
                        )
                    );
                    None
                }
            }
        }
    }
}

/// 按字符（非字节）截断，超限时以 … 结尾，避免切坏多字节字符
fn clamp_chars(s: &str, max: usize) -> String {
    let mut it = s.chars();
    let mut out: String = it.by_ref().take(max.saturating_sub(1)).collect();
    if it.next().is_some() {
        out.push('…');
    }
    out
}

/// 远程内容进通知前的收敛：加官方前缀并截断到上限
/// 只做标识与长度处理，不改写正文（正文可能含换行/emoji，保留原样）
fn shape_notification(title: &str, body: &str) -> (String, String) {
    let mut title = clamp_chars(title.trim(), MAX_TITLE_CHARS);
    title.insert_str(0, TITLE_PREFIX);
    (title, clamp_chars(body.trim(), MAX_BODY_CHARS))
}

/// 跑一轮：拉取 → 匹配 → 命中就发通知，返回是否真的发出
pub fn run_once() -> bool {
    let Some((body, _src)) = fetch_any() else {
        return false;
    };
    let code = current_version_code();
    match find_entry(&body, &code) {
        Some((title, content)) => {
            let (title, content) = shape_notification(&title, &content);
            let sent = crate::notify::post_notification(&title, REMOTE_TAG, &content);
            log::info!(
                "{}",
                crate::i18n::t_with_args(
                    "remote-push-notified",
                    &crate::fluent_args! {"code" => code, "title" => title},
                )
            );
            sent
        }
        None => {
            // [level] 没命中是常态，只留 debug
            if log::log_enabled!(log::Level::Debug) {
                log::debug!(
                    "{}",
                    crate::i18n::t_with_args(
                        "remote-push-no-match",
                        &crate::fluent_args! {"code" => code},
                    )
                );
            }
            false
        }
    }
}

/// 本地时间的小时 + 日期键（`localtime_r` 走 libc，与 logger 时间戳同口径；失败返回 None）
fn local_now() -> Option<(i32, i32, i32, i32)> {
    // SAFETY: `time` 写入本地 `now`；`localtime_r` 线程安全，结果写进 `tm`，二者都只在栈上操作
    unsafe {
        let mut now: libc::time_t = 0;
        if libc::time(&mut now) == -1 {
            return None;
        }
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return None;
        }
        Some((tm.tm_year, tm.tm_mon, tm.tm_mday, tm.tm_hour))
    }
}

/// 当前小时是否属于拉取槽位
fn is_slot_hour(hour: i32) -> bool {
    SLOT_HOURS.contains(&hour)
}

/// 启动后台线程：每 60s 巡检，命中槽位且本槽未拉过就跑一轮
/// 独立线程、不 join、任何失败都不 panic
pub fn spawn() {
    let spawned = std::thread::Builder::new()
        .name("remote_push".to_string())
        .spawn(move || {
            log::info!("{}", crate::i18n::t("remote-push-started"));
            // 槽位去重键：(年,月,日,时)，仅同一进程内有效
            let mut last_slot: Option<(i32, i32, i32, i32)> = None;
            loop {
                if let Some((y, mon, d, hour)) = local_now()
                    && is_slot_hour(hour)
                {
                    let key = (y, mon, d, hour);
                    if last_slot != Some(key) {
                        last_slot = Some(key);
                        let _ = run_once();
                    }
                }
                std::thread::sleep(TICK);
            }
        });
    if let Err(e) = spawned {
        log::warn!(
            "{}",
            crate::i18n::t_with_args(
                "remote-push-thread-failed",
                &crate::fluent_args! {"error" => e.to_string()},
            )
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// versionCode 是数字时命中即恒等
    #[test]
    fn find_entry_hits_matching_version_code() {
        let json = r#"[
            {"versionCode": 30000, "title": "title0", "content": "content0"},
            {"versionCode": 30001, "title": "title1", "content": "content1"}
        ]"#;
        assert_eq!(
            find_entry(json, "30001"),
            Some(("title1".to_string(), "content1".to_string()))
        );
        assert_eq!(
            find_entry(json, "30000"),
            Some(("title0".to_string(), "content0".to_string()))
        );
    }

    /// 不去前导零：`"030001"` 不该命中 `30001`，归一化会把近似值误判成命中
    #[test]
    fn leading_zeros_do_not_match() {
        let padded = r#"[{"versionCode": "030001", "title": "t", "content": "c"}]"#;
        assert_eq!(find_entry(padded, "30001"), None, "补零写法不得命中");
        let zeros = r#"[{"versionCode": "00000", "title": "t", "content": "c"}]"#;
        assert_eq!(find_entry(zeros, "0"), None, "全零补零写法不得命中");
        // 完全一致时才命中
        let exact = r#"[{"versionCode": "00000", "title": "t", "content": "c"}]"#;
        assert_eq!(
            find_entry(exact, "00000"),
            Some(("t".to_string(), "c".to_string()))
        );
    }

    /// 未命中必须静默（None），且不因其它字段异常而 panic
    #[test]
    fn find_entry_misses_are_silent() {
        let json = r#"[{"versionCode": 30000, "title": "a", "content": "b"}]"#;
        assert_eq!(find_entry(json, "30210"), None);
        // 空/非法 JSON、非数组、空数组一律 None
        assert_eq!(find_entry("", "30000"), None);
        assert_eq!(find_entry("not json", "30000"), None);
        assert_eq!(find_entry(r#"{"versionCode":30000}"#, "30000"), None);
        assert_eq!(find_entry("[]", "30000"), None);
        // 空 versionCode 绝不等于通配符
        assert_eq!(find_entry(json, ""), None);
    }

    /// versionCode 为字符串时也认；title/content 缺失或为空则该条不产生通知
    #[test]
    fn find_entry_accepts_string_code_and_requires_text() {
        let json = r#"[{"versionCode": "30210", "title": "t", "content": "c"}]"#;
        assert_eq!(
            find_entry(json, "30210"),
            Some(("t".to_string(), "c".to_string()))
        );
        for bad in [
            r#"[{"versionCode": 1, "title": "", "content": "c"}]"#,
            r#"[{"versionCode": 1, "title": "t"}]"#,
            r#"[{"versionCode": 1, "content": "c"}]"#,
        ] {
            assert_eq!(find_entry(bad, "1"), None, "空文本不该发通知: {bad}");
        }
    }

    /// 槽位口径：本地时间 08~24 点的偶数整点共 8 个，奇数点与凌晨不拉
    #[test]
    fn slot_hours_are_even_8_to_22() {
        for h in [8, 10, 12, 14, 16, 18, 20, 22] {
            assert!(is_slot_hour(h), "{h} 点应是槽位");
        }
        for h in [0, 1, 6, 7, 9, 11, 23] {
            assert!(!is_slot_hour(h), "{h} 点不该拉取");
        }
    }

    /// 只认内嵌 versionCode，磁盘 module.prop 不作依据
    #[test]
    fn current_version_code_ignores_tampered_disk_prop() {
        let dir = std::env::temp_dir().join("chiri-remote-push-tampered");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("module.prop"),
            "id=chiri\nversionCode=99999\nversion=v9\n",
        )
        .unwrap();
        let code = current_version_code();
        assert_ne!(code, "99999", "磁盘的 versionCode 不得被采用");
        assert!(!code.is_empty(), "内嵌 module.prop 必须能取到 versionCode");
        assert!(
            code.chars().all(|c| c.is_ascii_digit()),
            "内嵌 versionCode 应为纯数字: {code}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 内嵌值与仓库 module.prop 一致（改版本时两边必须同步）
    #[test]
    fn embedded_version_code_is_non_empty_digits() {
        let code = current_version_code();
        assert!(!code.is_empty());
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    /// 真实网络冒烟：验证 rustls + webpki-roots 链路可走通，默认忽略，手动 `cargo test -- --ignored remote_push`
    /// 取仓库里的 update.json，避免依赖 remindPush.json 是否已推送
    #[test]
    #[ignore = "需要网络，手动 --ignored 运行"]
    fn fetch_real_https_endpoint_smoke() {
        let url = "https://raw.githubusercontent.com/Aibeto/ChiRiScene/main/updateInformation/update.json";
        let body = fetch_text(url).expect("应能取到仓库里的 update.json");
        assert!(body.contains('{'), "返回内容不像 JSON: {body}");
    }

    /// 截断按字符计：CJK/emoji 不会被切坏，超限以 … 收尾
    #[test]
    fn clamp_chars_counts_chars_not_bytes() {
        assert_eq!(clamp_chars("abc", 10), "abc");
        // 恰好等于上限时保留 max-1 个字符 + 省略号
        assert_eq!(clamp_chars("abcde", 5), "abcd…");
        let long = "测".repeat(300);
        let cut = clamp_chars(&long, MAX_TITLE_CHARS);
        assert_eq!(cut.chars().count(), MAX_TITLE_CHARS);
        assert!(cut.ends_with('…'));
        assert!(cut.starts_with('测'));
    }

    /// 远程通知一律带 [ChiRi] 前缀，标题/正文超长被截断
    #[test]
    fn shape_notification_prefixes_and_truncates() {
        let (t, b) = shape_notification("  v2.0.3 已发布  ", "  正文  ");
        assert_eq!(t, "[ChiRi] v2.0.3 已发布");
        assert_eq!(b, "正文");

        let (t, b) = shape_notification(&"T".repeat(500), &"B".repeat(5000));
        assert_eq!(
            t.chars().count(),
            TITLE_PREFIX.chars().count() + MAX_TITLE_CHARS,
            "前缀 + 截断后标题长度不对"
        );
        assert_eq!(b.chars().count(), MAX_BODY_CHARS);
    }
}
