//! i18n.rs: [bundle] [t] [macro]

use fluent::bundle::FluentBundle;
use fluent::{FluentArgs, FluentResource};
use intl_memoizer::concurrent::IntlLangMemoizer;
use std::sync::LazyLock;
use std::sync::RwLock;

// [bundle]
static BUNDLE: LazyLock<RwLock<FluentBundle<FluentResource, IntlLangMemoizer>>> =
    LazyLock::new(|| {
    let bundle = FluentBundle::new_concurrent(vec!["en".parse().unwrap()]);
    RwLock::new(bundle)
});

/// 加载指定语言的翻译资源：语言包编译期嵌入（common::embedded_ftl_str），磁盘 i18n 目录不再读取。
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
