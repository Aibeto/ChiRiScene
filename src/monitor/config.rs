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
    #[serde(default)]
    pub ignored_apps: Vec<String>,
}
