//! scripts.rs: [embed] [release] [call]
// 模块脚本的**唯一执行入口**：`scripts/notify.sh` 与 `scripts/pack.sh` 在编译期用 `include_str!`
// 嵌进二进制，运行时不读磁盘上的同名文件——磁盘副本只是给人看的副本，改了也不生效（防篡改）。
//
// [embed] 为什么嵌入而不是直接跑 `<模块根>/scripts/*.sh`：
//   ① **防篡改**：脚本以 root 执行（notify.sh 里还会 `su` 提权），磁盘上的 `.sh` 任何有写权限的
//      进程都能改，改完下次调用即生效。嵌入后生效内容与已发布的二进制绑死，改磁盘无效。
//   与 `rules.yaml`/`special_tuned.yaml`/`down.chr` 同口径（那几处也是 `include_str!` 编译期打底、
//   磁盘副本仅展示）。② 剥离注释：`xtask` 的 `strip_comments` **豁免** `module/scripts/`（那里是
//   对外稳定接口，源码注释要保留），但嵌入用的是仓库源码，注释随 `.rodata` 进了二进制——既不是
//   安全问题（二进制本来就不可读），也不会因为剥离与否改变行为。
//
// [release] 每次调用前**重新写入并覆盖**临时文件，再做 `chmod 0700`：
//   ① 「每次覆盖」而不是「存在即跳过」——否则先落地一份伪造的同名文件就能长期劫持；
//      每次都 `fs::write`（内含 truncate）保证内容必然是本次二进制里的那份。
//   ② 释放到**私有临时目录**（`$TMPDIR` 或 `/data/local/tmp`，见 [`release_dir`]）而不是模块目录：
//      模块目录对其他进程可写，且用户可能期望那里是「可改的副本」；临时目录里的是进程产物，
//      用完即弃，语义清楚。文件名带 pid，避免并发调用互相覆盖。
//   ③ `chmod 0700`：`pack.sh` 会调 `chiri zstd`（内置压缩），`notify.sh` 会 `su` 提权，
//      权限过宽等于给其他 uid 一个可替换的执行点。
//
// [call] 统一用 `/system/bin/sh <释放路径> <args...>` 直接 spawn argv，不经 `sh -c`：
//   参数按位置传，天然免二次转义（标题/正文里出现引号也不会破壳）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 通知投递脚本（源码随二进制走；磁盘副本改了不生效）
pub const NOTIFY_SH: &str = include_str!("../module/scripts/notify.sh");
/// 日志打包脚本（同上）
pub const PACK_SH: &str = include_str!("../module/scripts/pack.sh");

/// 释放到磁盘的脚本名（带内容判别，便于人工核对是哪个版本）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Notify,
    Pack,
}

impl Script {
    /// 源文件名（用于释放后的临时文件名与日志）
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

/// 释放目录：优先 `$TMPDIR`（Android 上由 init 设成应用私有目录），回退 `/data/local/tmp`，
/// 再回退 `/dev/shm`。**不落在模块目录**：模块目录对其他进程可写，且应保持「用户可见副本」语义
fn release_dir() -> PathBuf {
    for cand in ["TMPDIR", "TMP", "TEMP"] {
        if let Ok(v) = std::env::var(cand) {
            let p = PathBuf::from(&v);
            if p.is_dir() {
                return p.join("chiri-scripts");
            }
        }
    }
    for p in ["/data/local/tmp", "/dev/shm", "/tmp"] {
        let p = PathBuf::from(p);
        if p.is_dir() {
            return p.join("chiri-scripts");
        }
    }
    // 都不行时退到模块根下的临时子目录（仍比直接跑模块目录里的脚本安全：名字不同且每次覆盖）
    crate::common::get_module_root().join(".tmp")
}

/// 把脚本**写入并覆盖**到 `dir`，返回可执行路径。失败时返回 `None`（调用方按「脚本不可用」留痕）
fn release_into(script: Script, dir: &Path) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    // 文件名带 pid：并发调用（daemon 与 WebUI 同时点测试）不会互相覆盖到半截
    let path = dir.join(format!("{}.{}", script.file_name(), std::process::id()));
    // 每次都写：内含 truncate，保证内容必然是本次二进制里的那份（「存在即跳过」会被抢先落地劫持）
    std::fs::write(&path, script.body()).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 0700：owner 可读写执行，其他 uid 无任何权限——`su` 提权与 `chiri zstd` 都经这里
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).ok()?;
    }
    Some(path)
}

/// 释放脚本并返回其绝对路径（供需要自行 spawn 的调用方，如 `pack_dir_tar`）
pub fn release(script: Script) -> Option<PathBuf> {
    release_into(script, &release_dir())
}

/// 执行脚本并收集输出。`args` 按位置传给脚本（不经 `sh -c`，免二次转义）
pub fn run(script: Script, args: &[&str]) -> Option<std::process::Output> {
    let path = release(script)?;
    let mut cmd = Command::new("/system/bin/sh");
    cmd.arg(&path).args(args).stdin(Stdio::null());
    // 释放出的副本用完即删：临时目录里不留可被二次利用的执行点
    let out = cmd.output().ok();
    let _ = std::fs::remove_file(&path);
    out
}

/// 仅供测试：把脚本释放到指定目录（不删），用于断言正文与权限
#[cfg(test)]
fn release_for_test(script: Script, dir: &Path) -> Option<PathBuf> {
    release_into(script, dir)
}

// ─────────────────────────── 模块目录副本：启动覆盖 + 运行期监听还原 ───────────────────────────

/// 两个脚本在模块内的相对路径——**WebUI 走这两份磁盘副本**（WebView 取不到二进制内嵌内容），
/// daemon 自己则一律用内嵌正文，故这份副本只为 WebUI 与人工阅读存在
pub const MODULE_REL: [(&str, Script); 2] = [
    ("scripts/notify.sh", Script::Notify),
    ("scripts/pack.sh", Script::Pack),
];

/// 把脚本写入 `root/scripts/<name>` 并置 0755（WebUI 要 `sh` 执行它，故不能是 0700）。
/// 返回是否写入成功。**每次都写**（不比对内容）：调用方语义是「无条件覆盖一次」，
/// 比对再跳过会让「内容碰巧相同」与「已还原」两种状态在日志里不可区分
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

/// 启动时把两个脚本**无条件覆盖**回 `root/scripts/`，返回成功写入的数量（0/1/2）。
/// 失败逐个留痕：脚本被篡改却没还原成功时，只报一个数字看不出是哪个
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
            "[Scripts] {} 个脚本写入模块目录失败（WebUI 侧可能拿到旧副本）: {}",
            failed.len(),
            failed.join(", ")
        );
    }
    ok
}

// [watch]
/// 运行期监听 `root/scripts/`，发现两个脚本被改动就立刻用内嵌正文覆盖还原。
///
/// 三个要点（照搬 `monitor::app_detect::watch_config_file` 的经验）：
/// ① **监听目录而非文件**：WebUI / 用户在外部用「临时文件 + 原子 mv」替换时，挂在旧 inode 上的
///    文件级 watch 会永久失效（之后再也收不到事件，等于监听悄悄死了）。目录级 watch 能收到 MOVED_TO。
/// ② **还原后清空事件队列**：我们自己写入也会触发事件，不清队列会在「还原 → 事件 → 再还原」之间
///    空转刷日志。写入后 sleep 一小段再把积压事件读完。
/// ③ 本线程是**常驻**的：`read_events_blocking` 阻塞、出错即 return（由调用方决定是否重启）；
///    任何情况下都不 panic——这是防护线程，它死了也不该拖垮 daemon。
pub fn watch_module_scripts(root: &Path, stop: &std::sync::atomic::AtomicBool) {
    use inotify::{Inotify, WatchMask};

    let dir = root.join("scripts");
    let mut inotify = match Inotify::init() {
        Ok(i) => i,
        Err(e) => {
            log::warn!("[Scripts] 无法初始化 inotify，脚本篡改监听不可用: {}", e);
            return;
        }
    };
    // 监视目录本身：MODIFY/CLOSE_WRITE 覆盖「原地改写」，MOVED_TO 覆盖「原子替换」，
    // DELETE 覆盖「直接删除」（删掉后 WebUI 会找不到脚本，必须补回）
    if let Err(e) = inotify.watches().add(
        &dir,
        WatchMask::MODIFY | WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO | WatchMask::DELETE,
    ) {
        log::warn!(
            "[Scripts] 无法监听 {}，脚本篡改监听不可用: {}",
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
        // 只关心这两个脚本名；目录里其它文件（pack.sh.bak 之类）不动
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
        // ② 先把可能积压的后续事件读掉，避免同一轮改动触发多次还原
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
                    "[Scripts] 检测到 {} 被改动，但覆盖还原失败（下次改动或重启会再试）",
                    script.file_name()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 嵌入正文必须与仓库源码一致（改了源码却忘了重新编译会在这里露出来）
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

    /// 嵌入的必须是**未剥离**的源码：注释还在（`strip_comments` 豁免 scripts/，故两者本应一致）
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

    /// 释放后文件正文与嵌入内容逐字节相同，且权限为 0700
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

    /// **防篡改核心断言**：先放一份伪造的同名文件，释放必须把它覆盖回嵌入内容
    #[test]
    fn release_overwrites_a_planted_tampered_file() {
        let dir = std::env::temp_dir().join(format!("chiri-scripts-tamper-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("notify.sh.{}", std::process::id()));
        std::fs::write(&path, "#!/system/bin/sh\necho TAMPERED\n").unwrap();
        // 覆盖前确认伪造文件确实在
        assert!(std::fs::read_to_string(&path).unwrap().contains("TAMPERED"));

        let released = release_for_test(Script::Notify, &dir).expect("release ok");

        let got = std::fs::read_to_string(&released).unwrap();
        assert_eq!(got, NOTIFY_SH, "释放必须覆盖掉被篡改的同名文件");
        assert!(!got.contains("TAMPERED"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 每次调用都重新覆盖：第二次释放后内容仍是嵌入正文（覆盖是幂等的，且不以「已存在」为由跳过）
    #[test]
    fn release_repeats_and_rewrites_every_call() {
        let dir = std::env::temp_dir().join(format!("chiri-scripts-repeat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p1 = release_for_test(Script::Pack, &dir).expect("release ok");
        std::fs::write(&p1, "clobbered").unwrap();
        let p2 = release_for_test(Script::Pack, &dir).expect("release ok");
        assert_eq!(p1, p2, "同一进程内路径稳定（按 pid 命名）");
        assert_eq!(std::fs::read_to_string(&p2).unwrap(), PACK_SH);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 脚本名映射（`run` 与日志都用它，写错会让释放路径与实际调用不一致）
    #[test]
    fn script_file_names_are_stable() {
        assert_eq!(Script::Notify.file_name(), "notify.sh");
        assert_eq!(Script::Pack.file_name(), "pack.sh");
        assert!(Script::Notify.body().contains("notify.sh"));
        assert!(Script::Pack.body().contains("pack.sh"));
    }
}
