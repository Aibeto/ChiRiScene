//! max_freq_pin.rs: [types] [pin] [unpin] [sysfs]
//! 释放接管后的频率失效保护（2026-10-31）：CLG 看门狗判定负载源失效时 daemon 会 `release()` 把 CPU 交还系统原生调频，
//! 而系统快照里的 max 通常就是低档——实测 2026-10-31 16:35 释放后核心一直跑在 cpu_min 档直到内核崩溃。
//! 故释放后统一把所有 policy 钉到硬件最高频（min = max = cpuinfo_max_freq），直到下一次接管
//! （CLG `init_policies`/`reload_config`、`FastLock::init`、`TunedGovernor::init_policies`）先 `unpin()` 还原接管前的
//! min/max——新快照必须拍到系统原值，否则快照被钉频值污染，之后每次 release 都会把机器永久钉在最高频。
use std::sync::Mutex;

/// 单个 policy 的钉频前快照（kHz）；读不到的节点记 None，还原时跳过
#[derive(Debug, Clone, Copy)]
struct PinnedPolicy {
    id: i32,
    min: Option<u32>,
    max: Option<u32>,
}

/// 钉频快照：Some = 当前处于「释放后钉最高频」态；重复钉频保留首份快照（那才是系统原值）
static PINNED: Mutex<Option<Vec<PinnedPolicy>>> = Mutex::new(None);

// [sysfs]
fn node_path(id: i32, node: &str) -> String {
    format!("/sys/devices/system/cpu/cpufreq/policy{id}/{node}")
}

fn read_u32(path: &str) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
}

/// 硬件最高频：优先 `cpuinfo_max_freq`；部分内核不导出该节点，退到 `scaling_available_frequencies` 的末项（升序表）
fn hw_max_freq(id: i32) -> Option<u32> {
    read_u32(&node_path(id, "cpuinfo_max_freq")).or_else(|| {
        std::fs::read_to_string(node_path(id, "scaling_available_frequencies"))
            .ok()
            .and_then(|s| {
                s.split_whitespace()
                    .filter_map(|v| v.parse::<u32>().ok())
                    .max()
            })
    })
}

// [pin]
/// 把所有 policy 钉到硬件最高频（写序：先抬 `scaling_max_freq` 再抬 `scaling_min_freq`，否则中间态 min > max 被内核拒绝）
/// 已处于钉频态时保留首份快照、只重写频率；返回是否至少写入成功一个 policy
pub fn pin_all_max(reason: &str) -> bool {
    let mut guard = PINNED.lock().unwrap_or_else(|e| e.into_inner());
    let policies = crate::chiri::get_cpu_policies();
    if policies.is_empty() {
        log::warn!("[release] max-freq pin skipped: no cpufreq policy (reason={reason})");
        return false;
    }

    let mut snap: Vec<PinnedPolicy> = Vec::with_capacity(policies.len());
    let mut pinned: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for p in &policies {
        let id = p.id;
        let Some(hw_max) = hw_max_freq(id) else {
            log::warn!("[release] max-freq pin: policy{id} hw_max unknown, skipped");
            continue;
        };
        // 首份快照优先：重复钉频时沿用上一轮的 min/max，不把钉频值当成系统原值
        let prev = guard
            .as_ref()
            .and_then(|v| v.iter().find(|x| x.id == id).copied());
        let rec = match prev {
            Some(r) => r,
            None => PinnedPolicy {
                id,
                min: read_u32(&node_path(id, "scaling_min_freq")),
                max: read_u32(&node_path(id, "scaling_max_freq")),
            },
        };
        snap.push(rec);

        let w_max =
            crate::utils::write_sysfs(node_path(id, "scaling_max_freq"), hw_max.to_string());
        let w_min =
            crate::utils::write_sysfs(node_path(id, "scaling_min_freq"), hw_max.to_string());
        match (w_max, w_min) {
            (Ok(()), Ok(())) => pinned.push(format!("policy{id}:{hw_max}")),
            (a, b) => failed.push(format!(
                "policy{id}: max={:?} min={:?}",
                a.err().map(|e| e.to_string()),
                b.err().map(|e| e.to_string())
            )),
        }
    }

    if !snap.is_empty() {
        *guard = Some(snap);
    }
    if !pinned.is_empty() {
        log::info!(
            "[release] max-freq pin (reason={}): {}",
            reason,
            pinned.join(";")
        );
    }
    if !failed.is_empty() {
        log::warn!(
            "[release] max-freq pin partial failure (reason={}): {}",
            reason,
            failed.join(";")
        );
    }
    !pinned.is_empty()
}

// [unpin]
/// 还原钉频前的系统 min/max（写序：先降 `scaling_min_freq` 再降 `scaling_max_freq`，保证中间态 min <= max）
/// 由各接管入口在拍接管快照之前调用：CLG `init_policies`/`reload_config`、`FastLock::init`、`TunedGovernor::init_policies`
/// 返回是否真的做过还原（未处于钉频态返回 false，调用方无需区分）
pub fn unpin(reason: &str) -> bool {
    let Some(snap) = PINNED.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return false;
    };
    let mut restored: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for p in &snap {
        if let Some(min) = p.min {
            if let Err(e) =
                crate::utils::write_sysfs(node_path(p.id, "scaling_min_freq"), min.to_string())
            {
                failed.push(format!("policy{} min={min}: {e}", p.id));
            }
        }
        if let Some(max) = p.max {
            if let Err(e) =
                crate::utils::write_sysfs(node_path(p.id, "scaling_max_freq"), max.to_string())
            {
                failed.push(format!("policy{} max={max}: {e}", p.id));
            }
        }
        restored.push(format!(
            "policy{}:min={}:max={}",
            p.id,
            p.min
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string()),
            p.max
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string())
        ));
    }
    log::info!(
        "[release] max-freq unpin (reason={}): {}",
        reason,
        restored.join(";")
    );
    if !failed.is_empty() {
        log::warn!(
            "[release] max-freq unpin partial failure (reason={}): {}",
            reason,
            failed.join(";")
        );
    }
    true
}
