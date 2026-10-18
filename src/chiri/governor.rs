//! governor.rs: [types] [activate] [release] [residue]
//! performance 调速器接管（FAS / contingency）：激活时快照各 policy 的 `scaling_governor` 原值并写 `performance`，release 逐 policy 恢复；对象归调度线程独占、无内部锁，SIGKILL 残留由 [residue] 启动清理兜底
use log::{debug, error, info, warn};
use std::fs;

use crate::fluent_args;
use crate::i18n::t_with_args;

/// 调速器写入（区分成败）：不用 `try_write_file`，其内部吞错恒返回 Ok，且收尾会把文件 chmod 0444，对之后还要恢复的 sysfs 节点是毒药
/// 写前 `enable_perm` + 失败读回校验，走 `utils::write_sysfs`（与 FAS 引擎同款，不做收尾 chmod）；读回等于目标值即按成功计，只记 debug
fn write_governor(path: &str, value: &str) -> bool {
    match crate::utils::write_sysfs(path, value) {
        Ok(()) => true,
        Err(e) => {
            let already = fs::read_to_string(path)
                .map(|s| s.trim() == value)
                .unwrap_or(false);
            if already {
                debug!("governor write rejected but readback is already '{value}': {path}");
                return true;
            }
            warn!(
                "{}",
                t_with_args(
                    "sysfs-write-failed",
                    &fluent_args!("path" => path, "error" => e.to_string()),
                )
            );
            false
        }
    }
}

// [types]
/// 单个 policy 的调速器快照，governor 为 None 的 policy 不接管
struct GovernorSnapshot {
    policy_id: i32,
    original: String,
}

pub struct GovernorGuard {
    snapshots: Vec<GovernorSnapshot>,
    active: bool,
    /// [no-op] activate 全军覆没 error 已报标记（翻转沿去重）：governor 节点只读的机型上 ok==0 属常态（performance 写不进，靠 min=max 锁频窗口等效）
    /// 首次 error、之后降 debug，接管成功或 release 时复位，状态真变了允许再报
    noop_reported: bool,
}

impl GovernorGuard {
    pub fn new() -> Self {
        Self {
            snapshots: Vec::new(),
            active: false,
            noop_reported: false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 快照各 policy 原调速器并切到 performance，重复激活是 no-op
    /// 部分 policy 读不到原值属常态（离线簇 / 无 cpufreq policy），跳过不打点；一个都拿不到则按 error 记账
    pub fn activate(&mut self) {
        if self.active {
            return;
        }
        let mut ok = 0usize;
        let mut total = 0usize;
        for policy in crate::chiri::get_cpu_policies() {
            total += 1;
            let pid = policy.id;
            let path = format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
                pid
            );
            let Some(original) = fs::read_to_string(&path)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
            else {
                // 读不到原值的 policy 不接管（宁可不管，也不留下恢复不回去的残留）：单 policy 跳过属常态（离线簇/无该节点），
                // 只打 debug 留痕——全军覆没由下方 activate 末尾的 error 覆盖
                debug!(
                    "governor policy{pid} original governor unreadable, skipping takeover: {path}"
                );
                continue;
            };
            if write_governor(&path, "performance") {
                // 逐 policy 打「由 -> 到」：原值各 policy 可能不同，聚合会丢信息
                info!(
                    "{}",
                    t_with_args(
                        "governor-switched",
                        &fluent_args!("pid" => pid.to_string(), "from" => original.as_str()),
                    )
                );
                self.snapshots.retain(|s| s.policy_id != pid);
                self.snapshots.push(GovernorSnapshot {
                    policy_id: pid,
                    original,
                });
                ok += 1;
            } else {
                warn!(
                    "{}",
                    t_with_args(
                        "governor-switch-failed",
                        &fluent_args!("pid" => pid.to_string()),
                    )
                );
            }
        }
        // [no-op] 一个 policy 都没接管到：performance 语义整条未生效，与「activate 从未被调用」在日志上同形（都无 governor-switched 行），按功能性失效记账
        // policy 列表本身为空（cpufreq 不可见）也算；[no-op] 去重（翻转沿）首次报 error、此后降 debug，接管成功或 release 复位
        if ok == 0 {
            if self.noop_reported {
                debug!(
                    "{}",
                    t_with_args(
                        "governor-activate-noop",
                        &fluent_args!("total" => total.to_string()),
                    )
                );
            } else {
                error!(
                    "{}",
                    t_with_args(
                        "governor-activate-noop",
                        &fluent_args!("total" => total.to_string()),
                    )
                );
                self.noop_reported = true;
            }
        }
        if ok > 0 {
            self.active = true;
            // [no-op] 从全军覆没恢复成功（如临时性读失败后节点恢复）：状态翻转，复位允许下次失败再报
            self.noop_reported = false;
        }
    }

    // [release]
    /// 恢复全部快照的原调速器，未接管时为 no-op
    /// 快照为空会静默跳过还原，与「还原成功」在日志上同形（都没有 governor-restore-failed），故显式打点
    pub fn release(&mut self) {
        if !self.active {
            return;
        }
        if self.snapshots.is_empty() {
            warn!(
                "[Governor] empty snapshot: nothing to restore, performance may persist until next start"
            );
        }
        for snap in self.snapshots.drain(..) {
            let path = format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
                snap.policy_id
            );
            if !write_governor(&path, &snap.original) {
                warn!(
                    "{}",
                    t_with_args(
                        "governor-restore-failed",
                        &fluent_args!(
                            "pid" => snap.policy_id.to_string(),
                            "governor" => snap.original.as_str(),
                        ),
                    )
                );
            } else {
                info!(
                    "{}",
                    t_with_args(
                        "governor-restored",
                        &fluent_args!(
                            "pid" => snap.policy_id.to_string(),
                            "to" => snap.original.as_str(),
                        ),
                    )
                );
            }
        }
        self.active = false;
        // [no-op] release 完成一次完整生命周期：复位去重位，下次 activate 若再全军覆没按首报 error 记账
        self.noop_reported = false;
    }

    // [residue]
    /// 逐个 policy 体检：只回调「当前是 performance」的那些（pid, 节点路径）；读不到的 policy 直接跳过（与接管口径一致）
    fn each_residue_policy(f: &mut dyn FnMut(i32, &str)) {
        for policy in crate::chiri::get_cpu_policies() {
            let path = format!(
                "/sys/devices/system/cpu/cpufreq/policy{}/scaling_governor",
                policy.id
            );
            let Ok(cur) = fs::read_to_string(&path) else {
                continue;
            };
            if cur.trim() == "performance" {
                f(policy.id, &path);
            }
        }
    }

    /// 启动残留清理：SIGKILL 等异常退出会把 `performance` 留在节点上（收尾 release 不执行），它不是任何厂商默认调速器，启动时见到必是残留
    /// 一律写回该 policy 的默认调速器（common.rs [governor] 选型），正常值不动
    pub fn cleanup_residue() {
        Self::each_residue_policy(&mut |pid, path| {
            let target = crate::common::cpu_governor_for_policy(pid);
            if write_governor(path, &target) {
                warn!(
                    "{}",
                    t_with_args(
                        "governor-residue-cleanup",
                        &fluent_args!("pid" => pid.to_string())
                    )
                );
            }
        });
    }

    /// 停摆期的同款体检：只报不写，停摆语义是把系统交回原状，残留来源无从区分，写了就把基线盖成 ChiRi 认为的样子
    /// 等于停摆期还在替用户调度（上一次修的就是这类洞）只留日志让用户判断
    pub fn report_residue() {
        Self::each_residue_policy(&mut |pid, _path| {
            warn!(
                "{}",
                t_with_args(
                    "governor-residue-down",
                    &fluent_args!("pid" => pid.to_string())
                )
            );
        });
    }
}
