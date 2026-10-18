//! webui_asset.rs: [restore]
//! 启动还原内嵌 WebUI 资产：与 webroot/ 逐文件比对，只覆盖缺失或不一致的文件，meta.yaml `nofix: true` 时跳过
use std::fs;
use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/webui_assets.rs"));

/// 把内嵌资产还原到 `root/webroot/`，返回覆盖（含新建）的文件数；内容一致跳过，单文件失败不中断整体
pub fn restore_webroot(root: &Path) -> usize {
    if !EMBEDDED {
        return 0;
    }
    let dir = root.join("webroot");
    let mut restored = 0usize;
    let mut failed: Vec<&str> = Vec::new();
    for (rel, bytes) in FILES {
        let path = dir.join(rel);
        if fs::read(&path).is_ok_and(|cur| cur == *bytes) {
            continue;
        }
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if crate::common::write_file_no_panic(&path, bytes) {
            restored += 1;
        } else {
            // [log] 单文件还原失败原本静默跳过，日志看不出是哪几个没还原（启动路径，低频）
            failed.push(rel);
        }
    }
    if !failed.is_empty() {
        log::warn!(
            "[WebUI] {} 个资产文件还原失败: {}",
            failed.len(),
            failed.join(", ")
        );
    }
    restored
}
