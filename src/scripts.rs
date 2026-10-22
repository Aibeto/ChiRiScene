//! scripts.rs: [embed] [release] [call]
//! 脚本编译期 include_str! 嵌入二进制，运行时不读磁盘副本（防篡改）；每次调用以 O_EXCL 新建私有临时文件并 chmod 0700，经 /system/bin/sh 按位置传参

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 通知投递脚本（随二进制走，磁盘副本改了不生效）
pub const NOTIFY_SH: &str = include_str!("../module/scripts/notify.sh");
/// 日志打包脚本（同上）
pub const PACK_SH: &str = include_str!("../module/scripts/pack.sh");

/// 释放到磁盘的脚本名（带内容判别，便于核对版本）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Notify,
    Pack,
}

impl Script {
    /// 源文件名（释放后的临时文件名与日志都用它）
    pub fn file_name(self) -> &'static str {
        match self {
            Script::Notify => "notify.sh",
            Script::Pack => "pack.sh",
        }
    }

    /// 嵌入的脚本正文
    pub fn body(self) -> &'static str {
        match self {
            Script::Notify => NOTIFY_SH,
            Script::Pack => PACK_SH,
        }
    }
}

/// 固定的 root-only 释放目录：`/data/adb` 权限 0700，非 root 进程无法进入，
/// 抢跑者既建不了同名文件也换不掉目录
const ROOT_ONLY_DIR: &str = "/data/adb/chiri/tmp";

/// 释放目录：优先固定 root-only 目录；退化时只接受 root 拥有且 other 不可写的临时目录
/// 世界可写目录（1777 的 /tmp、0777 的 /dev/shm）允许任意进程预置符号链接，直接跳过
fn release_dir() -> PathBuf {
    let fixed = PathBuf::from(ROOT_ONLY_DIR);
    if ensure_secure_dir(&fixed) {
        return fixed;
    }
    for cand in ["TMPDIR", "TMP", "TEMP"] {
        if let Ok(v) = std::env::var(cand) {
            let p = PathBuf::from(&v);
            if is_secure_dir(&p) {
                return p.join("chiri-scripts");
            }
        }
    }
    for p in ["/data/local/tmp", "/dev/shm", "/tmp"] {
        let p = PathBuf::from(p);
        if is_secure_dir(&p) {
            return p.join("chiri-scripts");
        }
    }
    // 都不行时退到模块根下的临时子目录：建好后置 0700，挡住非 root 进程进入
    let fallback = crate::common::get_module_root().join(".tmp");
    let _ = std::fs::create_dir_all(&fallback);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o700));
    }
    fallback
}

/// 创建并校验 root-only 目录：owner 必须是 root；已存在且权限过宽说明不可信，直接放弃
fn ensure_secure_dir(p: &Path) -> bool {
    if std::fs::create_dir_all(p).is_err() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let Ok(md) = std::fs::metadata(p) else {
            return false;
        };
        if !md.is_dir() || md.uid() != 0 {
            return false;
        }
        if md.mode() & 0o077 != 0 {
            // 只收紧 root 自己建的目录；收紧后仍不合规就放弃
            if std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).is_err() {
                return false;
            }
            let Ok(md) = std::fs::metadata(p) else {
                return false;
            };
            return md.mode() & 0o077 == 0;
        }
        true
    }
    #[cfg(not(unix))]
    {
        p.is_dir()
    }
}

/// 目录是否可作为脚本落点：必须是目录、owner 为 root、other 无写权限
fn is_secure_dir(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(md) = std::fs::metadata(p) else {
            return false;
        };
        md.is_dir() && md.uid() == 0 && (md.mode() & 0o002) == 0
    }
    #[cfg(not(unix))]
    {
        p.is_dir()
    }
}

/// 把脚本独占写入 `dir`，返回可执行路径；失败返回 `None`
/// 名字带 pid + 序号：并发/重入不互相覆盖；`create_new`（O_EXCL）拒绝已存在路径，
/// 预置的同名文件或符号链接只会让我们换名重试，不会被跟随写入
fn release_into(script: Script, dir: &Path) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    for attempt in 0..16u32 {
        let path = dir.join(format!(
            "{}.{}.{}",
            script.file_name(),
            std::process::id(),
            attempt
        ));
        if write_script(script, &path) {
            return Some(path);
        }
    }
    None
}

/// 独占创建并写入脚本正文，成功后置 0700；任何一步失败都清理并返回 false
fn write_script(script: Script, path: &Path) -> bool {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o700);
    }
    let Ok(mut f) = opts.open(path) else {
        return false;
    };
    if f.write_all(script.body().as_bytes()).is_err() {
        drop(f);
        let _ = std::fs::remove_file(path);
        return false;
    }
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 0700：仅 owner 可读写执行，`su` 提权与 `chiri zstd` 都经这里
        if std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).is_err() {
            let _ = std::fs::remove_file(path);
            return false;
        }
    }
    true
}

/// 释放脚本并返回绝对路径（供自行 spawn 的调用方，如 `pack_dir_tar`）
pub fn release(script: Script) -> Option<PathBuf> {
    release_into(script, &release_dir())
}

/// 执行脚本并收集输出；`args` 按位置传，不经 `sh -c` 免二次转义
pub fn run(script: Script, args: &[&str]) -> Option<std::process::Output> {
    let path = release(script)?;
    let mut cmd = Command::new("/system/bin/sh");
    cmd.arg(&path).args(args).stdin(Stdio::null());
    // 释放出的副本用完即删，不留可被二次利用的执行点
    let out = cmd.output().ok();
    let _ = std::fs::remove_file(&path);
    out
}

/// 仅供测试：把脚本释放到指定目录（不删）
#[cfg(test)]
fn release_for_test(script: Script, dir: &Path) -> Option<PathBuf> {
    release_into(script, dir)
}

// ─────────────────────────── 模块目录副本：启动覆盖 + 运行期监听还原 ───────────────────────────

/// 两个脚本在模块内的相对路径：WebUI 走这两份磁盘副本（WebView 取不到内嵌内容），daemon 一律用内嵌正文
pub const MODULE_REL: [(&str, Script); 2] = [
    ("scripts/notify.sh", Script::Notify),
    ("scripts/pack.sh", Script::Pack),
];

/// 把脚本写入 `root/scripts/<name>` 并置 0755（WebUI 要 `sh` 执行），返回是否成功
/// 每次都写不比对内容：比对再跳过会让「内容相同」与「已还原」在日志里不可区分
fn write_module_copy(root: &Path, script: Script) -> bool {
    let path = root.join("scripts").join(script.file_name());
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    if std::fs::write(&path, script.body()).is_err() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    true
}

/// 启动时把两个脚本无条件覆盖回 `root/scripts/`，返回成功数量（0/1/2）；失败逐个留痕
pub fn restore_module_scripts(root: &Path) -> usize {
    let mut ok = 0usize;
    let mut failed: Vec<&str> = Vec::new();
    for (_, script) in MODULE_REL {
        if write_module_copy(root, script) {
            ok += 1;
        } else {
            failed.push(script.file_name());
        }
    }
    if !failed.is_empty() {
        log::warn!(
            "[Scripts] {} 个脚本写入模块目录失败: {}",
            failed.len(),
            failed.join(", ")
        );
    }
    ok
}

// [watch]
/// 运行期监听 `root/scripts/`，发现脚本被改动就用内嵌正文覆盖还原
/// 监听目录而非文件（原子 mv 后文件级 watch 会永久失效）；还原后清空事件队列避免自触发空转；常驻线程不 panic
pub fn watch_module_scripts(root: &Path, stop: &std::sync::atomic::AtomicBool) {
    use inotify::{Inotify, WatchMask};

    let dir = root.join("scripts");
    let mut inotify = match Inotify::init() {
        Ok(i) => i,
        Err(e) => {
            log::warn!("[Scripts] inotify 初始化失败，篡改监听不可用: {}", e);
            return;
        }
    };
    // 监视目录：MODIFY/CLOSE_WRITE 覆盖原地改写，MOVED_TO 覆盖原子替换，DELETE 覆盖直接删除
    if let Err(e) = inotify.watches().add(
        &dir,
        WatchMask::MODIFY | WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO | WatchMask::DELETE,
    ) {
        log::warn!(
            "[Scripts] 无法监听 {}，篡改监听不可用: {}",
            dir.display(),
            e
        );
        return;
    }
    log::info!("[Scripts] 脚本篡改监听已启动: {}", dir.display());

    let mut buffer = [0u8; 4096];
    loop {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let events = match inotify.read_events_blocking(&mut buffer) {
            Ok(ev) => ev,
            Err(e) => {
                // 读失败（含 daemon 退出时 fd 被关）就结束本线程，不 panic
                log::warn!("[Scripts] 脚本监听读取失败，线程退出: {}", e);
                return;
            }
        };
        // 只关心这两个脚本名，目录里其它文件不动
        let mut hit: Vec<Script> = Vec::new();
        for ev in events {
            let Some(name) = ev.name.as_deref().and_then(|n| n.to_str()) else {
                continue;
            };
            for (_, script) in MODULE_REL {
                if name == script.file_name() && !hit.contains(&script) {
                    hit.push(script);
                }
            }
        }
        if hit.is_empty() {
            continue;
        }
        // 先把可能积压的后续事件读掉，避免同一轮改动触发多次还原
        std::thread::sleep(std::time::Duration::from_millis(50));
        while let Ok(more) = inotify.read_events(&mut buffer) {
            if more.peekable().peek().is_none() {
                break;
            }
        }
        for script in hit {
            if write_module_copy(root, script) {
                log::warn!(
                    "[Scripts] 检测到 {} 被改动，已用内嵌内容覆盖还原",
                    script.file_name()
                );
            } else {
                log::warn!(
                    "[Scripts] 检测到 {} 被改动，覆盖还原失败",
                    script.file_name()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 嵌入正文必须与仓库源码一致
    #[test]
    fn embedded_body_matches_repo_source() {
        assert_eq!(
            NOTIFY_SH,
            include_str!("../module/scripts/notify.sh"),
            "notify.sh 嵌入正文与仓库源码不一致"
        );
        assert_eq!(
            PACK_SH,
            include_str!("../module/scripts/pack.sh"),
            "pack.sh 嵌入正文与仓库源码不一致"
        );
    }

    /// 嵌入的必须是未剥离源码：注释还在（`strip_comments` 豁免 scripts/）
    #[test]
    fn embedded_bodies_keep_comments() {
        for body in [NOTIFY_SH, PACK_SH] {
            assert!(
                body.lines().any(|l| l.trim_start().starts_with('#')),
                "嵌入正文里没有注释行，说明被剥离过"
            );
            assert!(body.starts_with("#!"), "脚本必须以 shebang 开头");
        }
    }

    /// 释放后正文与嵌入内容逐字节相同，权限为 0700
    #[test]
    fn release_writes_full_body_with_owner_only_mode() {
        let dir = std::env::temp_dir().join(format!("chiri-scripts-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = release_for_test(Script::Notify, &dir).expect("release ok");
        let got = std::fs::read_to_string(&path).unwrap();
        assert_eq!(got, NOTIFY_SH);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "释放出的脚本权限必须是 0700");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 防篡改核心断言：预置同名符号链接时 O_EXCL 拒绝跟随，换名重试，受害者文件不被写穿
    #[cfg(unix)]
    #[test]
    fn release_does_not_follow_a_planted_symlink() {
        let dir =
            std::env::temp_dir().join(format!("chiri-scripts-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let victim = dir.join("victim");
        std::fs::write(&victim, "original").unwrap();
        // 首个候选名可预测（<name>.<pid>.0），抢先放符号链接指向受害者
        let planted = dir.join(format!("notify.sh.{}.0", std::process::id()));
        std::os::unix::fs::symlink(&victim, &planted).unwrap();

        let released = release_for_test(Script::Notify, &dir).expect("release ok");

        assert_ne!(released, planted, "不得复用被抢占的名字");
        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "original",
            "不得写穿符号链接"
        );
        assert_eq!(std::fs::read_to_string(&released).unwrap(), NOTIFY_SH);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 每次调用都独占新建：同进程重复调用换名，既有文件不被覆盖
    #[test]
    fn release_creates_a_fresh_file_each_call() {
        let dir = std::env::temp_dir().join(format!("chiri-scripts-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p1 = release_for_test(Script::Pack, &dir).expect("release ok");
        std::fs::write(&p1, "clobbered").unwrap();
        let p2 = release_for_test(Script::Pack, &dir).expect("release ok");
        assert_ne!(p1, p2, "重复调用必须换名，不覆盖既有文件");
        assert_eq!(std::fs::read_to_string(&p2).unwrap(), PACK_SH);
        assert_eq!(std::fs::read_to_string(&p1).unwrap(), "clobbered");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 世界可写目录不得作为脚本落点
    #[cfg(unix)]
    #[test]
    fn secure_dir_rejects_world_writable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("chiri-scripts-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(!is_secure_dir(&dir), "0777 目录不得通过校验");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 脚本名映射（`run` 与日志都用它）
    #[test]
    fn script_file_names_are_stable() {
        assert_eq!(Script::Notify.file_name(), "notify.sh");
        assert_eq!(Script::Pack.file_name(), "pack.sh");
        assert!(Script::Notify.body().contains("notify.sh"));
        assert!(Script::Pack.body().contains("pack.sh"));
    }
}
