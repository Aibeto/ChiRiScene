//! down.rs: [consts] [state] [flag] [unsupported] [watch]
//! DOWN 模式（调度停摆）：模块根 `down.chr` 写着保留字 `down` 时，调度关停**全部**调度功能，
//! 只留采集与日志（eBPF、status.csv、daemon.log、devimp 照常），便于与接管时对比
//! 与 `rhine.chr` 同款：对外暴露、可手改、内容即状态；文件在磁盘上，**重启后仍保持停摆**、只能改文件解除
//! 判据只认 `down.chr`；`current_mode.chr` 的 `down` 是对外投影——停摆期调度线程不覆盖它，免得外部工具误判「已恢复」
//! 唯一例外：非 ChiRi SoC 不起调度线程、本就不接管任何 CPU，开机直接写一次投影，见 `project_unsupported`
//! 释放/恢复动作在调度循环里（governor 为该线程独占），这里只提供「当前该不该停摆」这一事实

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use inotify::WatchMask;
use log::{info, warn};

use crate::common;
use crate::fluent_args;
use crate::i18n::{t, t_with_args};
use crate::utils;

// [consts]
/// 对外暴露的停摆状态文件（模块根）
pub const DOWN_CHR: &str = "down.chr";
/// 停摆期间写进 `current_mode.chr` 的模式值
pub const DOWN_MODE: &str = "down";
/// 随模块下发的内容模板，同时是「文件缺失」时补建的内容（只有注释 = 正常调度）
pub const DOWN_DEFAULT_CHR: &str = include_str!("../module/down.chr");
/// 监听异常后的重建间隔（与另外两套 watcher 同口径）
const WATCH_RETRY_BACKOFF: Duration = Duration::from_secs(2);

// [state]
/// 解析 `down.chr`：写了保留字 `down`（忽略大小写、允许成对引号）= 停摆；空/注释/其它 = 正常；缺失就地补建模板
fn read_down(root: &Path) -> bool {
    let path = root.join(DOWN_CHR);
    let Ok(text) = fs::read_to_string(&path) else {
        // 读不到按「不停摆」处理——绝不因为一个状态文件异常就让调度自己关掉
        let _ = common::write_file_no_panic(&path, DOWN_DEFAULT_CHR.as_bytes());
        return false;
    };
    let body: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    match body.first() {
        Some(raw) => raw
            .trim_matches(|c| c == '"' || c == '\'')
            .eq_ignore_ascii_case(DOWN_MODE),
        None => false,
    }
}

// [flag]
/// 停摆中。调度循环高频读它，那里不许再去读文件
static DOWN_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 当前是否处于 DOWN 停摆（进程级，读原子量）
pub fn is_down() -> bool {
    DOWN_ACTIVE.load(Ordering::Acquire)
}

// [unsupported]
/// 非 ChiRi SoC：该分支不启动调度、不接管任何 CPU，与停摆本就同态，所以把对外状态对齐——只写一次
/// `current_mode.chr`（外部工具 / WebUI 据此显示 DOWN），**不写 `down.chr`**：那是「用户意图」的
/// 输入文件，本机没有可释放的接管，写它等于伪装成用户设过；进程级标志一并置真，与其它路径同口径。
/// 由 main.rs 在「调度器未启动」分支调用一次（该分支无监听线程，写入后不再变更）
pub fn project_unsupported(root: &Path) {
    DOWN_ACTIVE.store(true, Ordering::Release);
    let _ = utils::try_write_file(root.join("current_mode.chr"), DOWN_MODE.as_bytes());
}

// [watch]
/// 启动期读取一次。必须在调度线程起循环之前调用（ChiRi 专属）
pub fn on_startup(root: &Path) -> bool {
    let v = read_down(root);
    DOWN_ACTIVE.store(v, Ordering::Release);
    v
}

/// 盯模块根的 `down.chr`：写入或删除即生效，与 meta.yaml、rhine.chr 同语义，不用重启调度
pub fn watch_loop(root: PathBuf) {
    let mut watcher: Option<utils::DirWatcher> = None;
    loop {
        if watcher.is_none() {
            // 比默认多一个 DELETE：这个文件的**删除**同样表示解除停摆，默认掩码只认 CLOSE_WRITE / MOVED_TO、删掉不会醒
            match utils::DirWatcher::new_with_mask(
                &root,
                WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO | WatchMask::DELETE,
            ) {
                Ok(w) => watcher = Some(w),
                Err(e) => {
                    warn!(
                        "{}",
                        t_with_args("down-watch-error", &fluent_args!("error" => e.to_string()))
                    );
                    std::thread::sleep(WATCH_RETRY_BACKOFF);
                    continue;
                }
            }
        }
        if let Err(e) = watcher.as_mut().unwrap().wait_change(DOWN_CHR) {
            warn!(
                "{}",
                t_with_args("down-watch-error", &fluent_args!("error" => e.to_string()))
            );
            watcher = None;
            std::thread::sleep(WATCH_RETRY_BACKOFF);
            continue;
        }
        let next = read_down(&root);
        let prev = DOWN_ACTIVE.swap(next, Ordering::AcqRel);
        if next != prev {
            info!(
                "{}",
                t(if next {
                    "down-enabled"
                } else {
                    "down-disabled"
                })
            );
        }
    }
}
