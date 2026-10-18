//! i18n.rs: [bundle] [t] [macro]

use fluent::bundle::FluentBundle;
use fluent::{FluentArgs, FluentResource};
use intl_memoizer::concurrent::IntlLangMemoizer;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::RwLock;

// [bundle]
static BUNDLE: LazyLock<RwLock<FluentBundle<FluentResource, IntlLangMemoizer>>> =
    LazyLock::new(|| {
        let bundle = FluentBundle::new_concurrent(vec!["en".parse().unwrap()]);
        RwLock::new(bundle)
    });

/// 无参消息（[`t`]）的预格式化缓存：键 → 已格式化文本，命中即省掉 fluent 的 `format_pattern` 与中间分配
/// 切换语言时必须整体清空重建，否则残留上一语言文本；锁序固定「先 BUNDLE 后 T_CACHE」
static T_CACHE: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// 加载指定语言的翻译资源：语言包编译期嵌入（common::embedded_ftl_str），不读磁盘 i18n 目录
fn load_bundle(
    lang: &str,
) -> Result<FluentBundle<FluentResource, IntlLangMemoizer>, anyhow::Error> {
    let ftl_string = crate::common::embedded_ftl_str(lang).to_string();
    // [level] 语言切换的边界日志（发起/成功/失败）在 load_language，本行只是解析细节，故降为 debug
    log::debug!("[i18n] 加载内嵌语言: {}", lang);

    let resource = FluentResource::try_new(ftl_string)
        .map_err(|e| anyhow::anyhow!("Failed to parse embedded FTL resource: {:?}", e))?;

    let langid = match lang.parse() {
        Ok(id) => id,
        Err(_) => {
            log::warn!(
                "[i18n] 非法语言标签 '{}'，回退 'en'", // 避免 parse().unwrap() panic
                lang
            );
            "en".parse().unwrap()
        }
    };
    let mut bundle = FluentBundle::new_concurrent(vec![langid]);

    bundle
        .add_resource(resource)
        .map_err(|e| anyhow::anyhow!("Failed to add FTL resource: {:?}", e))?;

    Ok(bundle)
}

/// 切换语言
pub fn load_language(lang: &str) {
    log::info!("[i18n] 切换语言: '{}'", lang);

    match load_bundle(lang) {
        Ok(new_bundle) => {
            // 毒化防御：锁持有者 panic 后仍可用（与 logger/core_ctl 同口径）
            let mut bundle_lock = BUNDLE.write().unwrap_or_else(|p| p.into_inner());
            *bundle_lock = new_bundle;
            drop(bundle_lock);
            // 语言已切换：无参消息缓存整体失效，必须清空重建
            T_CACHE.write().unwrap_or_else(|p| p.into_inner()).clear();
            log::info!(
                "[i18n] 已切换到语言: {}", // 随后清空 T_CACHE
                lang
            );
        }
        Err(e) => {
            log::error!(
                "[i18n] 加载语言 '{}' 失败，保持原语言: {}", // 保留原语言包
                lang,
                e
            );
        }
    }
}

// [t]
/// 获取翻译文本
pub fn t(key: &str) -> String {
    // 先查预格式化缓存：命中即返回（读锁在 if-let 结束时释放）
    if let Some(hit) = T_CACHE.read().unwrap_or_else(|p| p.into_inner()).get(key) {
        return hit.clone();
    }
    let bundle = BUNDLE.read().unwrap_or_else(|p| p.into_inner());
    // [level] 回退裸 key 补 debug 打点：t() 是热路径，故 log_enabled 门控 + 字面量文案
    // 绝不在这里调 t()/t_with_args()，否则 key 缺失时打日志会无限递归
    let msg = match bundle.get_message(key) {
        Some(msg) => msg,
        None => {
            if log::log_enabled!(log::Level::Debug) {
                log::debug!(
                    "[i18n] key '{}' 缺失，回退裸 key", // 热路径，仅 debug
                    key
                );
            }
            return key.to_string();
        }
    };
    let pattern = match msg.value() {
        Some(pattern) => pattern,
        None => {
            if log::log_enabled!(log::Level::Debug) {
                log::debug!(
                    "[i18n] key '{}' 无值，回退裸 key", // 热路径，仅 debug
                    key
                );
            }
            return key.to_string();
        }
    };

    let mut errors = Vec::new();
    let value = bundle.format_pattern(pattern, None, &mut errors);

    if errors.is_empty() {
        let out = value.to_string();
        // 回填缓存（未命中才写，键为静态字面量，规模有界）；必须在持有 BUNDLE 读锁时回填
        // 否则与 load_language「替换 BUNDLE → 清 T_CACHE」并发时会把旧语言文本写回缓存
        T_CACHE
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.to_string(), out.clone());
        out
    } else {
        log::warn!(
            "[i18n] 格式化 key '{}' 失败: {:?}", // 回退裸 key
            key,
            errors
        );
        key.to_string()
    }
}

/// 获取带参数的翻译文本
pub fn t_with_args(key: &str, args: &FluentArgs) -> String {
    let bundle = BUNDLE.read().unwrap_or_else(|p| p.into_inner());
    // [level] 同 t()：回退裸 key 补 debug 打点，字面量文案 + log_enabled 门控，不调 t() 防递归
    let msg = match bundle.get_message(key) {
        Some(msg) => msg,
        None => {
            if log::log_enabled!(log::Level::Debug) {
                log::debug!(
                    "[i18n] key '{}' 缺失，回退裸 key", // 热路径，仅 debug
                    key
                );
            }
            return key.to_string();
        }
    };
    let pattern = match msg.value() {
        Some(pattern) => pattern,
        None => {
            if log::log_enabled!(log::Level::Debug) {
                log::debug!(
                    "[i18n] key '{}' 无值，回退裸 key", // 热路径，仅 debug
                    key
                );
            }
            return key.to_string();
        }
    };

    let mut errors = Vec::new();
    let value = bundle.format_pattern(pattern, Some(args), &mut errors);

    if errors.is_empty() {
        value.to_string()
    } else {
        log::warn!(
            "[i18n] 格式化 key '{}' 失败: {:?}", // 回退裸 key
            key,
            errors
        );
        key.to_string()
    }
}

// [macro]
#[macro_export]
macro_rules! fluent_args {
    ($($key:expr => $value:expr),* $(,)?) => {{
        let mut args = fluent::FluentArgs::new();
        $(
            args.set($key, fluent::FluentValue::from($value));
        )*
        args
    }};
}
