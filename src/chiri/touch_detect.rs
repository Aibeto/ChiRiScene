//! touch_detect.rs: [consts] [monitor] [poll_wait]

use log::{debug, error, info, warn};
use std::fs;
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::sync::mpsc::SyncSender;
use std::time::Duration;

use crate::fluent_args;
use crate::i18n::{t, t_with_args};

// [consts]
// evdev 事件常量（linux/input-event-codes.h）
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const BTN_TOUCH: u16 = 0x14a; // 330
const ABS_MT_TRACKING_ID: u16 = 0x39; // 57
/// 64 位 Android 下 struct input_event 长度：timeval(16) + type(2) + code(2) + value(4)
const INPUT_EVENT_SIZE: usize = 24;

// [exit_guard]
/// 探测线程退出留痕哨兵：`monitor_touch` 外层是无限 `loop`，唯一退出方式是 panic（遍历设备/解析事件越界、sysfs 异常等），而 panic 只打到 stderr，结构化日志里一行都没有
/// 用 Drop 保证任何退出路径（含 panic 展开）都落一条 error；纯观测，不改控制流、不吞 panic
struct ExitGuard;

impl Drop for ExitGuard {
    fn drop(&mut self) {
        // 功能性失效、不可自愈（线程已死，无人拉起）→ error，符合级别契约
        error!("{}", t("touch-detect-thread-exit"));
    }
}

// [monitor]
/// 触摸检测线程：读取全部 /dev/input/event* 输入设备检测触摸按下，并把事件通过 `tx` 发给 scheduler_ipc（事件驱动，即时触发 CLG 大核升频）
/// 阻塞运行（poll + 阻塞 read），随守护进程退出消亡
pub fn monitor_touch(tx: SyncSender<()>) {
    // 线程退出留痕（含 panic 展开）：线程一死触摸升频即永久失效，必须留下与「用户没碰屏幕」可区分的痕
    let _exit_guard = ExitGuard;

    info!("{}", t("touch-detect-started"));

    // 外层循环：枚举设备；设备增删/读取异常时重新枚举
    loop {
        let mut devices: Vec<std::fs::File> = Vec::new();
        // [silent-fail] 单个 event 节点打不开会让触摸升频静默失效，与「用户没碰屏幕」同形，仅在重新枚举时走一次
        let mut open_failed: Vec<String> = Vec::new();
        if let Ok(entries) = fs::read_dir("/dev/input") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("event") {
                    let path = format!("/dev/input/{name}");
                    match std::fs::OpenOptions::new().read(true).open(&path) {
                        Ok(f) => devices.push(f),
                        Err(e) => open_failed.push(format!("{name}: {e}")),
                    }
                }
            }
        } else {
            // [silent-fail] 目录本身读不到（selinux 拒绝 / 非标准路径）同样是整条触摸链失效
            warn!("{}", t("touch-detect-input-unreadable"));
        }
        if !open_failed.is_empty() {
            warn!(
                "{}",
                t_with_args(
                    "touch-detect-device-open-failed",
                    &fluent_args!(
                        "count" => open_failed.len().to_string(),
                        "list" => open_failed.join(", ")
                    )
                )
            );
        }

        if devices.is_empty() {
            warn!("{}", t("touch-detect-no-devices"));
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }

        // 内层循环：poll 阻塞等待可读（超时 -1，无事件零唤醒）
        'poll: loop {
            let mut fds: Vec<libc::pollfd> = devices
                .iter()
                .map(|f| libc::pollfd {
                    fd: f.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                })
                .collect();

            // [poll_wait] 超时 -1 = 无限期阻塞，无事件零唤醒；事件处理与重枚举时机不变
            let ret = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
            if ret < 0 {
                // [level] poll 失败意味着本次探测整体停摆（触摸升频不生效），与「用户没碰屏」同形，故按契约报 warn；可自愈（退避后重新枚举、设备恢复即恢复），500ms 退避限制刷屏
                warn!(
                    "{}",
                    t_with_args(
                        "touch-detect-poll-error",
                        &fluent_args!("error" => std::io::Error::last_os_error().to_string())
                    )
                );
                std::thread::sleep(Duration::from_millis(500));
                break; // 重新枚举
            }

            let mut buf = [0u8; INPUT_EVENT_SIZE];
            for (i, pfd) in fds.iter().enumerate() {
                // 挂断/错误/无效描述符（部分内核在设备断开时上报）：与读错误同路重新枚举；阻塞等待后没有超时兜底，这类 revents 必须显式处理，否则 poll 立即返回却无事可做会原地空转
                if pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                    // [poll_backoff] 异常 revents 多为电平触发，重枚举后 poll 会立即返回，退避 500ms（与 poll 出错路径同款）防持续异常时紧循环空转
                    // [silent-fail] 设备挂断导致本轮重枚举：若不留痕，「触摸升频失效」与「用户没碰屏」同形
                    warn!(
                        "{}",
                        t_with_args(
                            "touch-detect-device-error",
                            &fluent_args!(
                                "index" => i.to_string(),
                                "revents" => pfd.revents.to_string()
                            )
                        )
                    );
                    std::thread::sleep(Duration::from_millis(500));
                    break 'poll;
                }
                if pfd.revents & libc::POLLIN == 0 {
                    continue;
                }
                match devices[i].read(&mut buf) {
                    // 读到 0 字节或读取失败 = 设备断开/异常，重新枚举；读失败多为驱动持续异常（POLLIN 电平触发但 read 恒败），与异常 revents 同款退避 500ms 防紧循环
                    Ok(0) | Err(_) => {
                        // [silent-fail] 设备读失败/断开是本轮重枚举的唯一入口，静默时与「一切正常但用户没碰屏」不可分，这里带上设备下标
                        warn!(
                            "{}",
                            t_with_args(
                                "touch-detect-read-failed",
                                &fluent_args!("index" => i.to_string())
                            )
                        );
                        std::thread::sleep(Duration::from_millis(500));
                        break 'poll;
                    }
                    Ok(n) => {
                        // 一次 read 可能包含多个 input_event，逐个解析
                        let mut off = 0;
                        while off + INPUT_EVENT_SIZE <= n {
                            let etype = u16::from_ne_bytes([buf[off + 16], buf[off + 17]]);
                            let code = u16::from_ne_bytes([buf[off + 18], buf[off + 19]]);
                            let value = i32::from_ne_bytes([
                                buf[off + 20],
                                buf[off + 21],
                                buf[off + 22],
                                buf[off + 23],
                            ]);
                            // 触摸按下：BTN_TOUCH 置位，或 ABS_MT_TRACKING_ID 出现有效触点（>= 0）
                            let touched = (etype == EV_KEY && code == BTN_TOUCH && value == 1)
                                || (etype == EV_ABS && code == ABS_MT_TRACKING_ID && value >= 0);
                            if touched {
                                // 事件驱动：向 scheduler_ipc 非阻塞发送触摸事件，通道满时丢弃
                                // [silent-drop] 接收端已退出时 try_send 恒失败且被 `let _ =` 吞掉，触摸事件全部丢失但日志一行不留；触摸是高频路径，故 debug + 门控，Full 是正常背压不报，只报接收端断开
                                if let Err(std::sync::mpsc::TrySendError::Disconnected(_)) =
                                    tx.try_send(())
                                {
                                    if log::log_enabled!(log::Level::Debug) {
                                        debug!(
                                            "[Touch] 触摸事件通道已断开，升频信号不再送达，触摸升频失效"
                                        );
                                    }
                                }
                                debug!(
                                    "{}",
                                    t_with_args(
                                        "touch-detect-down",
                                        &fluent_args!(
                                            "type" => etype.to_string(),
                                            "code" => code.to_string()
                                        )
                                    )
                                );
                            }
                            off += INPUT_EVENT_SIZE;
                        }
                    }
                }
            }
        }
    }
}
