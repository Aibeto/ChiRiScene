//! config.rs: [helpers] [rules]

use crate::common;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// [helpers]
pub fn get_rules_path() -> PathBuf {
    common::get_module_root().join("rules.yaml")
}

/// global_mode 缺省时使用模块模板中的默认 default 模式
// 静默失败兜底点：字段缺失（被删/键名拼错，serde 对未知字段不报错）时静默按 default 兜底，与用户确实配了 default 在 current_mode.chr / 通知 / 日志上同形，故补一条告警
fn default_global_mode() -> String {
    log::warn!(
        "{}",
        crate::i18n::t_with_args(
            "rule-global-mode-defaulted",
            &crate::fluent_args!("mode" => "default")
        )
    );
    "default".to_string()
}

/// app_modes 缺失或为 null 时按空表处理，WebUI 会把空表写成 "app_modes: null"，serde_yaml 无法反序列化为 HashMap
fn deserialize_app_modes<'de, D>(deserializer: D) -> Result<HashMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum MapOrNull {
        Map(HashMap<String, String>),
        Null,
    }
    Ok(match MapOrNull::deserialize(deserializer)? {
        // **预处理**：规则键允许写子进程名（com.xx:push），加载时归一到主包名，与前台名（set_current_package 同样归一）口径一致，查表即精确匹配；冲突时后者保留并告警，不静默丢规则
        MapOrNull::Map(m) => {
            let mut out: HashMap<String, String> = HashMap::with_capacity(m.len());
            for (k, v) in m {
                match k.split_once(':') {
                    Some((base, _suffix)) => {
                        // 写子进程名是文档明确支持的写法，归一属于预期内的加载期预处理不是异常，异常只在于归一冲突丢规则
                        // 只在启动/热重载时按规则键各打一条，归 info 而非 debug（debug 留给每 tick/每帧打点）
                        log::info!(
                            "{}",
                            crate::i18n::t_with_args(
                                "rule-key-normalized",
                                &crate::fluent_args!("key" => k.as_str(), "base" => base)
                            )
                        );
                        if out.insert(base.to_string(), v).is_some() {
                            // 归一冲突意味着前一条规则被整条丢弃（后者胜），该应用的模式映射能力永久丢失、无法自愈、只能改 rules.yaml 才能恢复，按 error!（功能性失效）
                            log::error!(
                                "{}",
                                crate::i18n::t_with_args(
                                    "rule-key-conflict",
                                    &crate::fluent_args!("key" => k.as_str(), "base" => base)
                                )
                            );
                        }
                    }
                    None => {
                        out.insert(k, v);
                    }
                }
            }
            // 静默失败兜底点：空表（`app_modes: {}`）解析成功但一条规则没有，与 null 归空在查表结果上同形
            // 查表侧无法区分规则整段丢失与用户确实一条没配，按下方 Null 分支同款口径（debug + 门控，复用既有 key）补留痕
            if out.is_empty() && log::log_enabled!(log::Level::Debug) {
                log::debug!("{}", crate::i18n::t("rule-app-modes-empty"));
            }
            out
        }
        // 静默失败兜底点：null / 空表都收敛成空 HashMap，与用户确实一条规则都没配在查表结果上同形（都恒不命中、模式恒取全局模式）
        // 旧 WebUI 会把空表写成 "app_modes: null"，此分支因此是正常路径之一，只在 debug 级留痕，不占 warn 噪声
        MapOrNull::Null => {
            if log::log_enabled!(log::Level::Debug) {
                log::debug!("{}", crate::i18n::t("rule-app-modes-empty"));
            }
            HashMap::new()
        }
    })
}

// [rules]

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct RulesConfig {
    // 字段缺省时必须以 null/省略安全反序列化：若无 #[serde(default)]，用户精简 rules.yaml（删任一字段）会报 missing field，read_config 回退 Default（dynamic_enabled=false）导致 dynamic 失效、CLG 不接管
    // 缺省值需与随附 rules.yaml 模板一致：dynamic_enabled 缺省 true、global_mode 缺省 "default"
    #[serde(default = "crate::utils::default_true")]
    pub dynamic_enabled: bool,
    #[serde(default = "default_global_mode")]
    pub global_mode: String,
    #[serde(default, deserialize_with = "deserialize_app_modes")]
    pub app_modes: HashMap<String, String>,
    /// Activity 级规则总开关（缺省 true）：开启后 `app_modes` 里形如 `pkg/.MainActivity` 的键会优先于同包名的键命中，未命中/关闭时完全回退包名级
    /// activity 由全局采样器（`ActivitySampler`，屏幕亮且前台有效时按节流采样，不限 FAS）与 FAS 窗口探针共同刷新，未采到（空）时自动回退
    #[serde(default = "crate::utils::default_true")]
    pub activity_aware: bool,
    /// [activity_params] Activity 级**参数**覆盖（键与 `app_modes` 同一套 activity 键写法）：在模式之上再叠一层参数微调，
    /// 与 `app_modes` 的模式选择**正交**——`app_modes` 决定「用哪套参数」，本段决定「在这套参数上再改哪几个字段」。
    /// 未命中/`activity_aware` 关闭/值为空时完全不介入（与加该字段前逐位一致），故现有规则无需改动。
    /// 限 CLG 与特调两条链路共用的字段（见 `ActivityParamOverride`），不支持 per_cluster 这类整表替换。
    #[serde(default)]
    pub activity_params: HashMap<String, ActivityParamOverride>,
    #[serde(default)]
    pub ignored_apps: Vec<String>,
}

/// [activity_params] 单个 activity 的参数覆盖：**全部字段可选**，None = 不覆盖（沿用该模式原参数）
/// 只收「跨 CLG/特调同义」的四个调节量，让同一份配置对两条链路都成立：
/// - `headroom`：CLG 的 `headroom_factor` / 特调的 `headroom`（目标 = util × headroom，>1 留余量、<1 收紧）
/// - `util_smoothing`：CLG 的 `util_smoothing` / 特调的 `util_smoothing`（EMA 系数，1.0 = 不平滑）
/// - `hysteresis`：特调 `hysteresis`（CLG 同义量是 `write_deadzone`，写频死区，比例口径一致）
/// - `down_hold_ms`：特调 `down_hold_ms`（CLG 同义量是 `down_rate_limit_ticks × tick`，这里直接给毫秒，按 tick 折算）
/// 刻意**不收** `perf_ceil/perf_floor`：天花板已在 2026-09-28 全局拆除，「除特殊模式外都能跑最高频」是硬约束，
/// activity 级再开天花板会绕过该约束且难审计；要压频请用 headroom < 1 表达（软收紧，仍可被负载释放）。
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct ActivityParamOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headroom: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub util_smoothing: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hysteresis: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down_hold_ms: Option<u64>,
}

impl ActivityParamOverride {
    /// 是否一个字段都没写（空覆盖 = 不介入，避免「配了空表」被当成有效覆盖去覆盖掉模式参数）
    pub fn is_empty(&self) -> bool {
        self.headroom.is_none()
            && self.util_smoothing.is_none()
            && self.hysteresis.is_none()
            && self.down_hold_ms.is_none()
    }

    /// 校验并钳制（与 CLG/特调的对应字段同口径）：非有限值丢弃、越界收进合理范围
    pub fn normalize(&mut self) {
        if let Some(v) = self.headroom {
            self.headroom = v.is_finite().then(|| v.clamp(0.5, 2.0));
        }
        if let Some(v) = self.util_smoothing {
            self.util_smoothing = v.is_finite().then(|| v.clamp(0.05, 1.0));
        }
        if let Some(v) = self.hysteresis {
            self.hysteresis = v.is_finite().then(|| v.clamp(0.0, 0.2));
        }
        if let Some(v) = self.down_hold_ms {
            self.down_hold_ms = Some(v.min(5_000));
        }
    }
}
