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

/// 无参消息（[`t`]）的**预格式化缓存**：键 → 已格式化文本。热路径（25-tick 摘要、状态行）反复取同一批键，
/// 命中即省掉 fluent 的 `format_pattern` 及其中的中间分配。**硬约束**：`load_language` 切换语言时必须
/// 整体清空重建，否则会残留上一语言的文本。锁序固定为「先 BUNDLE 后 T_CACHE」，与切换路径一致
static T_CACHE: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// 加载指定语言的翻译资源：语言包编译期嵌入（common::embedded_ftl_str），磁盘 i18n 目录不再读取
fn load_bundle(
    lang: &str,
) -> Result<FluentBundle<FluentResource, IntlLangMemoizer>, anyhow::Error> {
    let ftl_string = crate::common::embedded_ftl_str(lang).to_string();
    log::info!("[i18n] Loading embedded language '{}' (zh/en)", lang);

    let resource = FluentResource::try_new(ftl_string)
        .map_err(|e| anyhow::anyhow!("Failed to parse embedded FTL resource: {:?}", e))?;

    // 非法语言标签回退 "en"，避免 parse().unwrap() panic（locale 类型由 FluentBundle 推断，无需 unic-langid）
    let langid = match lang.parse() {
        Ok(id) => id,
        Err(_) => {
            log::warn!(
                "[i18n] Invalid language tag '{}', falling back to 'en'",
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

/// 对外接口：切换语言
pub fn load_language(lang: &str) {
    log::info!("[i18n] Request to switch language to: '{}'", lang);

    match load_bundle(lang) {
        Ok(new_bundle) => {
            // 毒化防御：锁持有者 panic 后仍可用（与 logger/core_ctl 同口径），翻译服务不能因锁状态崩溃
            let mut bundle_lock = BUNDLE.write().unwrap_or_else(|p| p.into_inner());
            *bundle_lock = new_bundle;
            drop(bundle_lock);
            // 语言已切换：无参消息缓存整体失效，必须清空重建（否则命中残留的上一语言文本）
            T_CACHE.write().unwrap_or_else(|p| p.into_inner()).clear();
            log::info!(
                "[i18n] Successfully loaded and switched to language: {}",
                lang
            );
        }
        Err(e) => {
            log::error!(
                "[i18n] Failed to load language '{}': {}. Keeping previous language.",
                lang,
                e
            );
        }
    }
}

// [t]
/// 获取翻译文本
pub fn t(key: &str) -> String {
    // 先查预格式化缓存：命中即返回（读锁在 if-let 结束时释放，不跨 fluent 调用持有）
    if let Some(hit) = T_CACHE.read().unwrap_or_else(|p| p.into_inner()).get(key) {
        return hit.clone();
    }
    let bundle = BUNDLE.read().unwrap_or_else(|p| p.into_inner());
    let msg = match bundle.get_message(key) {
        Some(msg) => msg,
        None => return key.to_string(),
    };
    let pattern = match msg.value() {
        Some(pattern) => pattern,
        None => return key.to_string(),
    };

    let mut errors = Vec::new();
    let value = bundle.format_pattern(pattern, None, &mut errors);

    if errors.is_empty() {
        let out = value.to_string();
        // 回填缓存（未命中才写；键为调用方传入的静态字面量，缓存规模有界）。**必须在持有 BUNDLE 读锁时回填**：
        // 否则与 load_language「替换 BUNDLE → 清 T_CACHE」并发时，会在此把旧语言文本写回缓存并残留至下次切语言。
        // 锁序固定「先 BUNDLE 后 T_CACHE」，与切换路径一致，无死锁
        T_CACHE
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.to_string(), out.clone());
        out
    } else {
        log::warn!(
            "[i18n] Failed to format message for key '{}': {:?}",
            key,
            errors
        );
        key.to_string()
    }
}

/// 获取带参数的翻译文本
pub fn t_with_args(key: &str, args: &FluentArgs) -> String {
    let bundle = BUNDLE.read().unwrap_or_else(|p| p.into_inner());
    let msg = match bundle.get_message(key) {
        Some(msg) => msg,
        None => return key.to_string(),
    };
    let pattern = match msg.value() {
        Some(pattern) => pattern,
        None => return key.to_string(),
    };

    let mut errors = Vec::new();
    let value = bundle.format_pattern(pattern, Some(args), &mut errors);

    if errors.is_empty() {
        value.to_string()
    } else {
        log::warn!(
            "[i18n] Failed to format message for key '{}': {:?}",
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
