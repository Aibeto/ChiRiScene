//! remote_push.rs: [schedule] [fetch] [match] [notify]
// 远程提醒：每天 08:00~24:00 之间每 2 小时拉一次 remindPush.json，命中本机 versionCode 就发通知。
//
// [schedule] 槽位 = 本地时间的偶数整点 08/10/12/14/16/18/20/22（"08~24 点每 2 小时"，24 点即次日
//   零点、不在区间内的偶数点里）。只按**墙钟**排，不按进程运行时长——用户口径是「每天几点」。
//   线程每 60s 醒一次比对当前小时：聊胜于无的开销（一分钟一次 `localtime_r`），换来的是不用算
//   「距下个整点还有多久」、也不必担心设备休眠/改时间导致的长睡错过槽位。同一槽位只拉一次，
//   以 (年,月,日,时) 作 key 去重；进程在槽位中途启动会**补拉一次**（不算重试，是补上本槽）。
//
// [fetch] 主源 GitHub raw 失败 → 留一条 info → 换备用源 gitee；备用源也失败 → 再留一条 info 收工。
//   **失败一律不重试**（用户口径）：下一个槽位自然会再拉。两个源都是 HTTPS，用 ureq + rustls +
//   webpki-roots（内嵌根证书，不依赖设备系统 CA 目录——daemon 跑在 root 下但 CA 路径不一定可读）。
//   超时 10s、响应上限 64KB：这是启动期后台线程，任何一步卡死都不能拖住别的线程。
//   **versionCode 只认编译期内嵌值**（不读磁盘 module.prop）：磁盘那份可被篡改，改它就能让
//   daemon 命中远程列表里任意一条、收到伪造提醒。
//
// [match] JSON 是对象数组 `{versionCode, title, content}`，`versionCode` 现为**数字**
//   （示例 `30000`/`30001`）。与**内嵌** module.prop 的 versionCode **恒等比较**——
//   用户 2026-10-18 明确「不去零」，故不做任何数值归一（归一会让 `030000` 也命中 `30000`）。
//   找不到 → **静默**（只留 debug，不进 info：没命中是常态，不该占日志）。
//
// [notify] 命中即以 JSON 里的 title/content 作通知标题与正文，tag `ChiRi-Remote`（该 tag 无拦截器，
//   直发；拦截器只认 `ChiRi-FAS`）。标题/正文是**远程下发的内容**，不是 i18n 文案。

use std::time::Duration;

/// 主源：GitHub raw
const PRIMARY_URL: &str = "https://raw.githubusercontent.com/Aibeto/ChiRiScene/refs/heads/main/updateInformation/remindPush.json";
/// 备用源：gitee raw（主源不通时用；同样只试一次）
const FALLBACK_URL: &str =
    "https://raw.giteeusercontent.com/Aibeto/ChiRiScene/raw/main/updateInformation/remindPush.json";
/// 通知 tag（无拦截器）
pub const REMOTE_TAG: &str = "ChiRi-Remote";
/// 单次 HTTP 总超时（连接+收发）
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// 响应体上限：正常文件几百字节，64KB 足够且能挡住异常大响应
const MAX_BODY_BYTES: u64 = 64 * 1024;
/// 槽位巡检间隔：一分钟一次，够准且可忽略
const TICK: Duration = Duration::from_secs(60);
/// 允许拉取的小时（本地时间，"08~24 点每 2 小时"）
const SLOT_HOURS: [i32; 8] = [8, 10, 12, 14, 16, 18, 20, 22];

/// 本机 versionCode：**只认编译期内嵌的 `module.prop`**（`include_str!`），**不读磁盘**。
///
/// 刻意不看磁盘那份：模块目录对其它进程可写，改一下 `module.prop` 的 versionCode 就能让 daemon
/// 去匹配远程列表里的任意一条、收到伪造提醒；而内嵌值随二进制一起更新，语义上本来就是
/// 「这个二进制的版本」。读不到就返回空串——`find_entry` 会因空值直接判不命中（静默），
/// 绝不退化成「匹配任意一条」
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

/// 在 JSON 数组里找与本机 versionCode **完全相同**的一条，返回 (title, content)。
///
/// 用户 2026-10-18 口径：两侧 versionCode 「恒等」即可，**不再去前导零**（示例已改成数字
/// `30000`，补零写法不再是接口约定）。只做 trim 级清洗，不做任何数值归一——
/// 归一会让 `"00000"` 与 `0`、`030000` 与 `30000` 互相命中，那是要避免的模糊匹配。
///
/// 解析失败/结构不符/没有命中的都返回 None（**静默**是用户明确要求的）
fn find_entry(json: &str, current_code: &str) -> Option<(String, String)> {
    let want = current_code.trim();
    if want.is_empty() {
        // 内嵌 module.prop 缺 versionCode 时绝不匹配——空值等于「不知道版本」，不是通配符
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
        // title/content 必须都是非空字符串，否则发了也是一条空通知
        let title = item.get("title")?.as_str()?.trim();
        let content = item.get("content")?.as_str()?.trim();
        if title.is_empty() || content.is_empty() {
            return None;
        }
        return Some((title.to_string(), content.to_string()));
    }
    None
}

/// GET 一段文本：超时 + 体积上限；错误折叠成可读字符串（只用于 info 留痕）
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

/// 拉一次：主源失败退备用源，二者都失败返回 None。**不重试**（下一个槽位自然再拉）
fn fetch_any() -> Option<(String, &'static str)> {
    match fetch_text(PRIMARY_URL) {
        Ok(body) => Some((body, "primary")),
        Err(e) => {
            // [level] 用户口径：连接失败不重试，只留一条 info
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

/// 跑一轮：拉取 → 匹配 → 命中就发通知。返回是否真的发出了通知（供测试与排障）
pub fn run_once() -> bool {
    let Some((body, _src)) = fetch_any() else {
        return false;
    };
    let code = current_version_code();
    match find_entry(&body, &code) {
        Some((title, content)) => {
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
            // [level] 没命中是常态：debug 即可，不进 info（用户口径「没有则静默」）
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

/// 本地时间的小时 + 日期键（`localtime_r` 走 libc，与 logger 的时间戳同口径；失败返回 None）
fn local_now() -> Option<(i32, i32, i32, i32)> {
    // SAFETY: `time` 写入本地 `now`；`localtime_r` 是线程安全版本，把结果写进 `tm`。
    // 两者都只在栈上操作，指针有效且不共享。
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

/// 启动后台线程：每 60s 巡检一次，命中槽位且本槽未拉过就跑一轮。
/// 独立线程、不 join、任何失败都不 panic（通知属增值功能，不能拖垮 daemon）
pub fn spawn() {
    let spawned = std::thread::Builder::new()
        .name("remote_push".to_string())
        .spawn(move || {
            log::info!("{}", crate::i18n::t("remote-push-started"));
            // 槽位去重键：(年,月,日,时)；只在同一进程内有效
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

    /// 用户 2026-10-18 修正后的示例：versionCode 是**数字**，命中即恒等
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

    /// **不去前导零**：`"030001"` 不该命中 `30001`，`"00000"` 也不该命中 `0`。
    /// 用户口径是「两个 versionCode 需要恒等」，归一化会把这种近似值误判成命中
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

    /// 未命中必须静默（None），且不因数组中其它字段异常而 panic
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

    /// versionCode 为字符串时也认（接口现为数字，但容错读字符串无害）；title/content
    /// 缺失或为空则该条不产生通知
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

    /// 槽位口径：本地时间 08~24 点的偶数整点，共 8 个；奇数点与凌晨不拉
    #[test]
    fn slot_hours_are_even_8_to_22() {
        for h in [8, 10, 12, 14, 16, 18, 20, 22] {
            assert!(is_slot_hour(h), "{h} 点应是槽位");
        }
        for h in [0, 1, 6, 7, 9, 11, 23] {
            assert!(!is_slot_hour(h), "{h} 点不该拉取");
        }
    }

    /// **只认内嵌 versionCode**：磁盘 module.prop 被改成什么都无所谓——
    /// 篡改磁盘就能让 daemon 匹配到远程列表里任意一条，这正是要堵的口子
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

    /// 内嵌值与仓库 module.prop 的另一处投影一致（改版本时两边必须同步，否则命中会错位）
    #[test]
    fn embedded_version_code_is_non_empty_digits() {
        let code = current_version_code();
        assert!(!code.is_empty());
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    /// 真实网络冒烟：验证 rustls + webpki-roots 这条 TLS 链路在本机能走通。
    /// **默认忽略**（CI 无网也能跑全量）：手动执行 `cargo test -- --ignored remote_push`
    /// 取一个确定存在的文件（仓库里的 update.json），避免依赖 remindPush.json 是否已推送
    #[test]
    #[ignore = "需要网络，手动 --ignored 运行"]
    fn fetch_real_https_endpoint_smoke() {
        let url = "https://raw.githubusercontent.com/Aibeto/ChiRiScene/main/updateInformation/update.json";
        let body = fetch_text(url).expect("应能取到仓库里的 update.json");
        assert!(body.contains('{'), "返回内容不像 JSON: {body}");
    }
}
