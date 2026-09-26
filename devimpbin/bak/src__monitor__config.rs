//! config.rs: [helpers] [rules]

use crate::common;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// [helpers]
pub fn get_rules_path() -> PathBuf {
    common::get_module_root().join("rules.yaml")
}

/// global_mode 缺省时使用模块模板中的默认（default）模式
fn default_global_mode() -> String {
    "default".to_string()
}

/// app_modes 缺失或为 null 时按空表处理：WebUI 旧版本会把空 app_modes 写成 "app_modes: null"，
/// 而 serde_yaml 无法把 null 反序列化为 HashMap（#[serde(default)] 只对缺失字段生效），显式兼容保证解析不失败。
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
        // **预处理**：规则键允许写子进程名（com.xx:push），加载时归一到主包名——与前台名
        // （set_current_package 同样归一）口径一致，查表即精确匹配；冲突时后者保留并告警，不静默丢规则。
        MapOrNull::Map(m) => {
            let mut out: HashMap<String, String> = HashMap::with_capacity(m.len());
            for (k, v) in m {
                match k.split_once(':') {
                    Some((base, _suffix)) => {
                        log::warn!(
                            "{}",
                            crate::i18n::t_with_args(
                                "rule-key-normalized",
                                &crate::fluent_args!("key" => k.as_str(), "base" => base)
                            )
                        );
                        if out.insert(base.to_string(), v).is_some() {
                            log::warn!(
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
            out
        }
        MapOrNull::Null => HashMap::new(),
    })
}

// [rules]

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct RulesConfig {
    // 字段缺省时必须以 null/省略安全反序列化：若无 #[serde(default)]，用户精简 rules.yaml（删任一
    // 字段）会报 missing field，read_config 回退 Default（dynamic_enabled=false）导致 dynamic 失效、
    // CLG 不接管。缺省值需与随附 rules.yaml 模板一致：dynamic_enabled 缺省 true、global_mode 缺省 "default"。
    #[serde(default = "crate::utils::default_true")]
    pub dynamic_enabled: bool,
    #[serde(default = "default_global_mode")]
    pub global_mode: String,
    #[serde(default, deserialize_with = "deserialize_app_modes")]
    pub app_modes: HashMap<String, String>,
    #[serde(default)]
    pub ignored_apps: Vec<String>,
}
