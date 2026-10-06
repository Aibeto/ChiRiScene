//! diag_worker.rs: [task] [worker]
//! 诊断渲染/写入的去阻塞 worker：单线程 + 有界 FIFO，任务保序，停机排空。
//!
//! **职责边界（阶段 D）**：本模块**只**做「已渲染好的 @S 行 → 单次原子落盘」。
//! 采样/差分判定仍留在调度线程原时点完成，worker 只消费不可变的 `FrameTask`，
//! 不访问可变 `AffinityManager`、控制器或任何 sysfs 调度节点，**不**改调度策略、
//! **不**绑定 CPU、**不**改线程优先级。
//!
//! **单写者**：worker 线程内唯一写入调用是
//! `crate::logger::aff_snapshot_drain(&rows, full, ts)`——该函数把整帧
//! （帧头 + 所有行）拼成一个 block 并单次 `write_all` 原子落盘，帧头计数由 logger
//! 反推，本模块**不拼帧头、不写 @A 行、不自己算 ntop/nfg**。`ts` 取调用方在**采样时点**
//! 已格式化的 `MMDD-HHmmss`（`logger::aff_frame_ts()`），保证异步写出时 `@S ts=` 仍记采样
//! 时刻而非写出时刻，维持 @S 与 status.csv 的时序配对。
//! **排空写入不再受 `diag_active()` 门控**（仅要求 devimp 目录存在），因此诊断关闭后
//! 已提交进队列的「已采集」帧仍会被写出，不会被门控直接丢弃。
//!
//! **失败处理（非「永不丢帧」，分三层勿混淆）**：
//! - **(a) `submit` 失败**：shutdown / 通道断开 / 阻塞 `send` 失败时以 `Err(task)`
//!   **把 `FrameTask` 所有权交还调用方**（`ts`/`rows`/`full` 与身份字段完好），
//!   调用方据此**回退同步写**（`logger::aff_snapshot`），禁止静默丢弃整帧。
//!   **顺序不变式（先旧后新）**：worker 因写入 panic 进入失败态后**仍在**排空旧帧，
//!   故此态下 `submit` **先等排空完成再交还任务**（无超时，见 `submit` 文档），
//!   保证调用方随后同步写出的新帧必然排在旧帧之后。
//! - **(b) panic 在飞帧**：worker 因写入 panic 时，**正在处理**的那一帧随栈展开被销毁，
//!   **无回滚可能**——把它的 `FrameId` 计入 `lost_frames`/`last_lost` 并 `log::error!`
//!   明确标注 **missing/partial 风险（可能已部分写入，无法回滚）**。
//! - **(c) panic 排队帧**：catch_unwind 后对队列中已入队帧**逐帧**再包一层
//!   `catch_unwind` 改走统一写出口 `write`（生产即 `aff_snapshot_drain`，不受
//!   `diag_active()` 门控、用各自 `ts`）：写出成功计入 `recovered_frames`；失败
//!   （返回 `false`）或**写入器二次 panic** 均计入 `lost_frames`/`last_lost`，且**继续**
//!   排空剩余帧（二次 panic 只损失当帧记账，不让其余已入队帧失去记账）。**不声称零丢帧**。
//! - **写出门控/IO 失败（普通路径）**：`write` 返回 `false` 表示该帧未写出——**不推进**
//!   `last_written`，累加 `lost_frames`、记录 `last_lost` 并 `log::error!`。
//!
//! **身份对账**：`FrameTask` 携带前台 `fg_pid` 与 `config_identity`，供 worker 侧比对
//! 「同包换 PID / 配置热重载」造成的跨身份混写；`@S` 帧格式不变。
//!
//! **保序**：`sync_channel` 是 FIFO，任务自带 `FrameId`；worker 只按**收到顺序**写，
//! 不重排、不合并。worker 侧记录「最后已写出帧」`last_written` 仅供诊断/对账，
//! 不作为消费侧可写状态。
//!
//! **回压口径**：容量 `CAPACITY=2` 帧（完整帧任务），`submit` 在队满时**阻塞**交付，
//! 不覆盖旧任务、不丢帧——这是**保真优先的回压，不宣称永不阻塞**。队满次数用
//! `AtomicU64` 记入 `backpressure_count()`，供「普遍回压则停止扩大异步范围」判读。
//!
//! **停机**：`shutdown` 取走发送端并 drop（std mpsc `Sender` 无显式 close），
//! worker 在 `recv` 收到通道关闭后把**通道内剩余任务全部处理完**再退出，然后 join；
//! 幂等——重复调用不 panic、不等第二次 join。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

// [task]
/// 帧标识：`generation` 为诊断代际（前台切换/诊断重开/配置代际变化时递增），
/// `sequence` 为帧序号（调用方在采样时点取号，保证序号与采样顺序一致）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FrameId {
    pub generation: u64,
    pub sequence: u64,
}

/// 一个完整帧任务：自带采样时点语义与已渲染好的行，worker 只负责写出。
pub struct FrameTask {
    pub id: FrameId,
    /// 采样时点（由调度线程在原本该采集的时点取），保留时点语义（排队/延迟对账用，非墙钟）
    pub sampled_at: Instant,
    /// 采样时点已格式化的墙钟 `MMDD-HHmmss`（调用方用 `logger::aff_frame_ts()` 取）：
    /// 异步写出时作帧头 `@S ts=`，保证记录的是采样时刻而非写出时刻
    pub ts: String,
    /// 已渲染好的 @S 行（帧头由 logger::aff_snapshot_drain 反推计数，勿在此拼帧头）
    pub rows: Vec<String>,
    pub full: bool,
    /// 采样时点的前台 PID（身份对账用：防止同包换 PID 时跨身份混写；`@S` 帧格式不变）
    pub fg_pid: i32,
    /// 采样时点前台进程的 `starttime`（`/proc/<pid>/stat` 第 22 字段，tick；0 = 读不到）：
    /// 与 `fg_pid` 合起来才能识别**同包同 PID 的进程重启 / PID 回收**（身份对账用，`@S` 帧格式不变）
    pub fg_starttime: u64,
    /// 配置身份（配置代际/指纹，身份对账用：防止配置热重载时跨身份混写；`@S` 帧格式不变）
    pub config_identity: String,
}

// [worker]
/// 可注入写入器类型：给定帧任务，返回**是否真正落盘**（供测试与真实 logger 解耦）。
/// **唯一写出口**——普通处理与 panic 后排空都走它；生产默认指向
/// `crate::logger::aff_snapshot_drain`（不受 `diag_active()` 门控，用帧自带 `ts` 作帧头）。
type WriteFn = Box<dyn Fn(&FrameTask) -> bool + Send>;

/// 诊断渲染/写入 worker：单线程 + 有界 FIFO。
///
/// 默认关闭——**只在调用方 `start()` 时才创建线程**，不做全局单例 lazy；
/// 生命周期由 `mod.rs` 决定（前台切换/诊断关闭/停机时由调用方 `shutdown`）。
pub struct DiagWorker {
    /// 有界发送端：`Option` 以便 `shutdown` 取走并 drop 关闭通道（std mpsc 无显式 close）
    tx: Arc<Mutex<Option<SyncSender<FrameTask>>>>,
    /// 线程句柄：`Option` 以便 `shutdown` 仅 join 一次（幂等）
    handle: Mutex<Option<JoinHandle<()>>>,
    /// 当前代际（前台切换/诊断重开/配置代际变化时递增）
    generation: AtomicU64,
    /// 下一个待分配序号（调用方在采样时点取号）
    sequence: AtomicU64,
    /// 累计回压次数（submit 时队列已满而阻塞的次数）
    backpressure: AtomicU64,
    /// worker 最后**成功写出**的帧标识（诊断/对账用；未写出时为 None，仅成功才推进）
    last_written: Arc<Mutex<Option<FrameId>>>,
    /// 累计「已接收但未能写出」的帧数（普通写入失败 + panic 在飞帧 + panic 排空写入失败；
    /// **不含**排空成功的帧——那些记在 `recovered_frames`）
    lost_frames: Arc<AtomicU64>,
    /// 累计「已接收、未经普通路径写出，但经 panic 排空成功补写」的帧数
    recovered_frames: Arc<AtomicU64>,
    /// 最近一次写出失败的帧标识（供缺帧定位）
    last_lost: Arc<Mutex<Option<FrameId>>>,
    /// worker 是否已进入失败态（panic 后为 true；此后 submit 一律返回 Err(task)）
    failed: Arc<AtomicBool>,
    /// 排空是否已完成（worker panic 后旧帧是否已全部写出 / 已逐帧记账完毕）；
    /// 供失败态下 `submit` 等待（顺序不变式）与验收对账
    drain_finished: Arc<AtomicBool>,
}

impl DiagWorker {
    /// 队列容量（完整帧任务数）。供报告字节容量/峰值内存用。
    pub const CAPACITY: usize = 2;

    /// 启动 worker：单线程 + 容量 `CAPACITY` 的有界 FIFO（`mpsc::sync_channel(2)`），
    /// 线程名 `diag_render`。写入器默认指向真实 logger 的 drain 出口；线程体用 `catch_unwind`
    /// 自愈并 `log::error!` 上报。
    pub fn start() -> Self {
        // 唯一写出口：排空写入不受 diag_active() 门控，用帧自带 ts 作帧头，返回是否真正落盘
        Self::start_with(Box::new(|task: &FrameTask| {
            crate::logger::aff_snapshot_drain(&task.rows, task.full, &task.ts)
        }))
    }

    /// 以注入的写入器启动 worker（供测试解耦真实 logger）；生产经 [`start`] 使用真实 logger。
    /// 注入点语义为 `fn(&FrameTask) -> bool`（即 drain 的替身），普通处理与 panic 排空共用。
    fn start_with(write: WriteFn) -> Self {
        let (tx, rx) = mpsc::sync_channel::<FrameTask>(Self::CAPACITY);
        let tx = Arc::new(Mutex::new(Some(tx)));
        let worker_tx = Arc::downgrade(&tx);
        let last_written: Arc<Mutex<Option<FrameId>>> = Arc::new(Mutex::new(None));
        let lost_frames: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
        let recovered_frames: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
        let last_lost: Arc<Mutex<Option<FrameId>>> = Arc::new(Mutex::new(None));
        let failed: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let drain_finished: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

        let lw = Arc::clone(&last_written);
        let lf = Arc::clone(&lost_frames);
        let rf = Arc::clone(&recovered_frames);
        let ll = Arc::clone(&last_lost);
        let fl = Arc::clone(&failed);
        let df = Arc::clone(&drain_finished);
        // 在飞帧登记：worker 取到任务后、写出前写入 Some(id)，写出返回后置回 None。
        // 若 write panic，则不会置回 None，panic 分支据此取出该 FrameId 记为缺帧。
        let in_flight: Arc<Mutex<Option<FrameId>>> = Arc::new(Mutex::new(None));
        let if_arc = Arc::clone(&in_flight);

        // 线程创建失败（极端资源耗尽）时不 panic：置空句柄，随后 submit 因通道已关闭返回 Err(task)
        let handle = match thread::Builder::new()
            .name("diag_render".to_string())
            .spawn(move || {
                // 关键：闭包**只借用 `&rx`**（`Receiver::recv`/`try_recv` 均取 `&self`），
                // 而非把 rx move 进去——这样 catch_unwind 抛出 panic 后仍返回、rx 仍归本线程所有，
                // 可继续排空「已入队但未处理」的帧并改走统一写出口写出。
                let rx_ref = &rx;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // 单写者：唯一写入调用；保序：只按收到顺序写，不重排。
                    // 取到帧先登记「在飞」，写完/失败后再清；write panic 时保持登记供 panic 分支记账。
                    // 只有写入成功(true)才推进 last_written；失败(false)记缺帧，不谎报已写出。
                    let process = |task: FrameTask| {
                        let id = task.id;
                        *if_arc.lock().unwrap_or_else(|p| p.into_inner()) = Some(id);
                        let ok = write(&task);
                        // write 已正常返回（未 panic）：先清在飞登记，再按结果记账
                        *if_arc.lock().unwrap_or_else(|p| p.into_inner()) = None;
                        if ok {
                            *lw.lock().unwrap_or_else(|p| p.into_inner()) = Some(id);
                        } else {
                            lf.fetch_add(1, Ordering::Relaxed);
                            *ll.lock().unwrap_or_else(|p| p.into_inner()) = Some(id);
                            log::error!(
                                "diag render worker: frame {id:?} 未写出（write 返回 false）"
                            );
                        }
                    };
                    // 通道关闭（全部发送端 drop）前逐条消费，FIFO 保序
                    while let Ok(task) = rx_ref.recv() {
                        process(task);
                    }
                    // 排空兜底：std recv 已排空缓冲，这里再清一次确保无残留
                    while let Ok(task) = rx_ref.try_recv() {
                        process(task);
                    }
                }));
                if result.is_err() {
                    // panic：进入失败态（此后 submit 在等排空完成后交还任务）。
                    fl.store(true, Ordering::SeqCst);
                    // 关闭新发送端入口；已克隆的发送端仍可交付，排空须等它们全部释放。
                    if let Some(sender_slot) = worker_tx.upgrade() {
                        sender_slot
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .take();
                    }
                    // panic 在飞帧：随栈展开被销毁，无法回滚——取出登记并记为缺帧/partial 风险
                    let inflight = if_arc
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .take();
                    if let Some(id) = inflight {
                        lf.fetch_add(1, Ordering::Relaxed);
                        *ll.lock().unwrap_or_else(|p| p.into_inner()) = Some(id);
                        log::error!(
                            "diag render worker panicked: 在飞帧 {id:?} 可能已部分写入\
                             （missing/partial 风险），无法回滚，记入缺帧"
                        );
                    }
                    log::error!(
                        "diag render worker panicked: 进入失败态；现对队列中已入队帧改走统一\
                         写出口（drain，不受 diag_active 门控）排空"
                    );
                    // panic 排队帧：统一写出口排空；成功记 recovered，失败/二次 panic 记 lost。
                    // **逐帧 catch_unwind**：写入器在排空途中**二次 panic** 时，仅当帧按 lost
                    // 记账，**不中断**排空、不让其余已入队帧失去记账（若整段排空被展开，
                    // 剩余已入队帧会被静默丢弃，既不进 lost 也不进 recovered）。
                    while let Ok(task) = rx.recv() {
                        let id = task.id;
                        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write(&task)))
                        {
                            Ok(true) => {
                                rf.fetch_add(1, Ordering::Relaxed);
                            }
                            Ok(false) => {
                                lf.fetch_add(1, Ordering::Relaxed);
                                *ll.lock().unwrap_or_else(|p| p.into_inner()) = Some(id);
                                log::error!(
                                    "diag render worker: frame {id:?} 排空写入失败（write 返回 false），记入缺帧"
                                );
                            }
                            Err(_) => {
                                // 写入器二次 panic：该帧无法落盘，按 lost 记账后**继续**排空剩余队列
                                lf.fetch_add(1, Ordering::Relaxed);
                                *ll.lock().unwrap_or_else(|p| p.into_inner()) = Some(id);
                                log::error!(
                                    "diag render worker: frame {id:?} 排空写入二次 panic，记入缺帧，继续排空剩余队列"
                                );
                            }
                        }
                    }
                }
                // 排空已完成（正常路径：通道关闭后退出；panic 路径：旧帧已逐帧记账后退出）。
                // 置真供失败态下的 `submit` 等待，保证「旧帧先于新帧落盘」。
                df.store(true, Ordering::SeqCst);
            }) {
            Ok(h) => Some(h),
            Err(e) => {
                log::error!("diag render worker spawn failed: {e}");
                None
            }
        };

        Self {
            tx,
            handle: Mutex::new(handle),
            generation: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            backpressure: AtomicU64::new(0),
            last_written,
            lost_frames,
            recovered_frames,
            last_lost,
            failed,
            drain_finished,
        }
    }

    /// 阻塞交付一个完整帧任务（保真优先回压，不覆盖旧任务、不丢帧）。
    ///
    /// **先查失败态**：worker 已 panic（`failed==true`）时**不立即**交还任务，而是
    /// **先等待 worker 排空完成**，再 `return Err(task)`（顺序不变式，见下）。
    /// 成功入队返回 `Ok(())`；worker 已退出（通道关闭）/ 已 `shutdown` / 阻塞 `send` 失败时
    /// 返回 `Err(task)`，**把 `FrameTask` 所有权交还调用方**——调用方必须据此回退同步写
    /// （`logger::aff_snapshot`），**不得静默丢弃**（丢整帧会破坏 `full=1`、帧序号与
    /// 完整快照契约）。队满仍先记回压再阻塞交付，保真优先。
    ///
    /// **顺序不变式（先旧后新）**：worker 因写入 panic 进入失败态后，**仍在**对队列中
    /// 已入队的旧帧逐帧排空写出。若此刻**立即**把任务交还，调用方会**立即**同步写出新帧，
    /// 而 worker 还在写旧帧 —— `@S` 是**槽级差分**帧，顺序错会破坏离线按序重建。
    /// 文件锁只保证**单帧**原子写入（`write_all` 一次落盘），**并不保证**跨帧「先旧后新」。
    /// 故失败态下本方法**必须等排空完成再交还**：调用方拿到 `Err(task)` 时旧帧已落盘，
    /// 其随后同步写出的新帧必然排在旧帧之后。
    ///
    /// 所有失败路径均等待线程退出，不持有发送端锁；写入器卡住时等待无超时。
    pub fn submit(&self, task: FrameTask) -> Result<(), FrameTask> {
        if self.failed.load(Ordering::SeqCst) {
            self.await_drain();
            return Err(task);
        }
        // 先克隆发送端再释放锁：避免 shutdown 取走发送端时被阻塞中的 send 卡住
        let tx = {
            let guard = self.tx.lock().unwrap_or_else(|p| p.into_inner());
            guard.as_ref().cloned()
        };
        let Some(tx) = tx else {
            self.await_drain();
            return Err(task);
        };
        let result = match tx.try_send(task) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(task)) => {
                // 队满：记一次回压，随后阻塞交付（不覆盖、不丢帧）
                self.backpressure.fetch_add(1, Ordering::Relaxed);
                // 阻塞 send 失败同样交还任务（worker 在阻塞期间退出）
                tx.send(task).map_err(|e| e.0)
            }
            // worker 已退出（接收端 drop）：交还任务供回退
            Err(TrySendError::Disconnected(task)) => Err(task),
        };
        // 排空等待通道关闭，等待前必须释放本次提交的发送端。
        drop(tx);
        if result.is_err() {
            self.await_drain();
        }
        result
    }

    /// 等待排空并仅 join 一次；句柄锁串行化所有等待者，worker 不使用此锁。
    fn await_drain(&self) {
        let mut handle = self.handle.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(worker_handle) = handle.take() {
            let _ = worker_handle.join();
        }
    }

    /// 取下一个序号（调用方在采样时点取号，保证序号与采样顺序一致）。
    pub fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }

    /// 递增代际（前台切换/诊断重开/配置代际变化时调用），返回新代际。
    pub fn bump_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// 当前代际。
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// 排空已提交任务后停机；幂等。
    pub fn shutdown(&self) {
        // 关闭发送端：取走并 drop（std mpsc Sender 无显式 close）
        {
            let mut guard = self.tx.lock().unwrap_or_else(|p| p.into_inner());
            *guard = None;
        }
        self.await_drain();
    }

    /// 累计回压次数（submit 时队列已满而阻塞的次数）。
    pub fn backpressure_count(&self) -> u64 {
        self.backpressure.load(Ordering::Relaxed)
    }

    /// worker 最后**成功写出**的帧标识（诊断/对账用；尚未成功写出任何帧时为 None）。
    pub fn last_written(&self) -> Option<FrameId> {
        *self.last_written.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// 累计「已接收但未能写出」的帧数（普通写入失败 + panic 在飞帧 + panic 排空写入失败）；
    /// **不含** panic 排空成功的帧（那些记在 `recovered_frames`）。
    pub fn lost_frames(&self) -> u64 {
        self.lost_frames.load(Ordering::Relaxed)
    }

    /// 累计「已接收、未经普通路径写出，但经 panic 排空成功补写」的帧数。
    pub fn recovered_frames(&self) -> u64 {
        self.recovered_frames.load(Ordering::Relaxed)
    }

    /// 最近一次写出失败的帧标识（供缺帧定位）。
    pub fn last_lost(&self) -> Option<FrameId> {
        *self.last_lost.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// worker 是否已进入失败态（panic 后为 true；此后 submit 在等待排空完成后返回 Err(task)）。
    pub fn is_failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    /// 排空是否已完成（panic 后旧帧是否已全部写出）；供验收对账。
    pub fn drain_finished(&self) -> bool {
        self.drain_finished.load(Ordering::SeqCst)
    }
}

// [tests]
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    // 测试通过注入写入器与真实 logger 解耦：`aff_snapshot_drain` 在测试环境
    // （devimp 目录通常不存在）会返回 false，故默认 `start()` 不适合断言 last_written。

    fn task(generation: u64, sequence: u64) -> FrameTask {
        FrameTask {
            id: FrameId {
                generation,
                sequence,
            },
            sampled_at: Instant::now(),
            ts: "0101-000000".to_string(),
            rows: vec!["p1 - - -".to_string()],
            full: false,
            fg_pid: 1234,
            fg_starttime: 987654,
            config_identity: "cfg-test".to_string(),
        }
    }

    /// 恒成功的注入写入器。
    fn true_writer() -> WriteFn {
        Box::new(|_t: &FrameTask| true)
    }

    #[test]
    fn sequence_is_monotonic() {
        let w = DiagWorker::start();
        let a = w.next_sequence();
        let b = w.next_sequence();
        let c = w.next_sequence();
        assert!(a < b && b < c, "序号必须单调递增: {a} {b} {c}");
        w.shutdown();
    }

    #[test]
    fn generation_increments() {
        let w = DiagWorker::start();
        let initial = w.generation();
        let g1 = w.bump_generation();
        let g2 = w.bump_generation();
        assert_eq!(g1, initial + 1);
        assert_eq!(g2, initial + 2);
        assert_eq!(w.generation(), initial + 2);
        w.shutdown();
    }

    #[test]
    fn submit_then_shutdown_is_idempotent() {
        let w = DiagWorker::start();
        assert!(w.submit(task(0, 0)).is_ok());
        // 排空后停机不 panic
        w.shutdown();
        // 幂等：重复调用不 panic、不等第二次 join
        w.shutdown();
    }

    #[test]
    fn submit_success_returns_ok() {
        let w = DiagWorker::start();
        // 队列未满且 worker 存活：成功入队返回 Ok(())
        assert!(w.submit(task(0, 0)).is_ok());
        w.shutdown();
    }

    #[test]
    fn submit_after_channel_closed_returns_err_task() {
        let w = DiagWorker::start();
        w.shutdown();
        // 通道关闭后提交失败：Err 把任务所有权交还调用方（不丢帧）
        let t = task(0, 0);
        match w.submit(t) {
            Ok(()) => panic!("shutdown 后 submit 必须返回 Err(task)"),
            Err(back) => {
                // 内容完好，调用方可据此回退同步写：id/rows/ts/full/身份字段均未被丢弃
                assert_eq!(
                    back.id,
                    FrameId {
                        generation: 0,
                        sequence: 0
                    }
                );
                assert_eq!(back.rows, vec!["p1 - - -".to_string()]);
                assert_eq!(back.ts, "0101-000000");
                assert!(!back.full);
                assert_eq!(back.fg_pid, 1234);
                assert_eq!(back.config_identity, "cfg-test");
            }
        }
    }

    #[test]
    fn fifo_preserves_order_and_drains_on_shutdown() {
        // 注入恒成功写入器，使 last_written 推进与真实 logger 解耦
        let w = DiagWorker::start_with(true_writer());
        let ids: Vec<FrameId> = (0..5)
            .map(|i| FrameId {
                generation: 0,
                sequence: i,
            })
            .collect();
        for id in &ids {
            assert!(w
                .submit(FrameTask {
                    id: *id,
                    sampled_at: Instant::now(),
                    ts: "0101-000000".to_string(),
                    rows: Vec::new(),
                    full: id.sequence == 0,
                    fg_pid: 1234,
                    fg_starttime: 987654,
                    config_identity: "cfg-test".to_string(),
                })
                .is_ok());
        }
        // shutdown 排空已提交任务并 join：最后写出帧即最后提交帧（保序 + 无丢帧）
        w.shutdown();
        assert_eq!(w.last_written(), Some(ids[4]));
        assert_eq!(w.lost_frames(), 0);
    }

    #[test]
    fn ts_carried_and_last_written_order_consistent() {
        let w = DiagWorker::start_with(true_writer());
        let ids: Vec<FrameId> = (0..3)
            .map(|i| FrameId {
                generation: 0,
                sequence: i,
            })
            .collect();
        let tss = ["0101-000001", "0101-000002", "0101-000003"];
        for (id, ts) in ids.iter().zip(tss.iter()) {
            // ts 可随任务逐帧携带且各自独立，不影响入队顺序
            assert!(w
                .submit(FrameTask {
                    id: *id,
                    sampled_at: Instant::now(),
                    ts: (*ts).to_string(),
                    rows: Vec::new(),
                    full: false,
                    fg_pid: 1234,
                    fg_starttime: 987654,
                    config_identity: "cfg-test".to_string(),
                })
                .is_ok());
        }
        // 保序：last_written 为最后提交帧，携带的 ts 未扰动写出顺序
        w.shutdown();
        assert_eq!(w.last_written(), Some(ids[2]));
    }

    #[test]
    fn writer_true_advances_last_written() {
        let w = DiagWorker::start_with(true_writer());
        let id = FrameId {
            generation: 0,
            sequence: 0,
        };
        assert!(w.submit(task(0, 0)).is_ok());
        w.shutdown();
        // 写入成功：last_written 推进，无缺帧
        assert_eq!(w.last_written(), Some(id));
        assert_eq!(w.lost_frames(), 0);
        assert_eq!(w.last_lost(), None);
        assert!(!w.is_failed());
    }

    #[test]
    fn writer_false_marks_lost_and_does_not_advance() {
        // 注入恒失败写入器：模拟门控/IO 失败（帧未落盘）
        let w = DiagWorker::start_with(Box::new(|_t: &FrameTask| false));
        let id = FrameId {
            generation: 0,
            sequence: 0,
        };
        assert!(w.submit(task(0, 0)).is_ok());
        w.shutdown();
        // 未写出：last_written 不推进，记缺帧与 last_lost，且不计入 recovered
        assert_eq!(w.last_written(), None);
        assert_eq!(w.lost_frames(), 1);
        assert_eq!(w.recovered_frames(), 0);
        assert_eq!(w.last_lost(), Some(id));
    }

    #[test]
    fn failed_after_panic_blocks_submit() {
        let (sig_tx, sig_rx) = mpsc::channel::<()>();
        // 首帧处理即 panic，触发失败态
        let writer: WriteFn = Box::new(move |_t: &FrameTask| -> bool {
            let _ = sig_tx.send(());
            panic!("injected writer panic");
        });
        let w = DiagWorker::start_with(writer);
        assert!(w.submit(task(0, 0)).is_ok());
        let _ = sig_rx.recv_timeout(Duration::from_secs(2));
        // 等 worker 进入失败态
        let deadline = Instant::now() + Duration::from_secs(2);
        while !w.is_failed() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(w.is_failed(), "panic 后必须进入失败态");
        // panic 在飞帧（seq=0）被记为缺帧；等记账完成（failed 置真在记缺帧之前）
        let deadline = Instant::now() + Duration::from_secs(2);
        while w.lost_frames() == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(w.lost_frames(), 1, "panic 在飞帧必须计入 lost_frames");
        assert_eq!(
            w.last_lost(),
            Some(FrameId {
                generation: 0,
                sequence: 0
            })
        );
        // 失败态下 submit 不再入队，交还任务且字段完好
        match w.submit(task(1, 1)) {
            Ok(()) => panic!("失败态下 submit 必须返回 Err(task)"),
            Err(back) => {
                assert_eq!(
                    back.id,
                    FrameId {
                        generation: 1,
                        sequence: 1
                    }
                );
                assert_eq!(back.rows, vec!["p1 - - -".to_string()]);
                assert_eq!(back.fg_pid, 1234);
                assert_eq!(back.config_identity, "cfg-test");
            }
        }
        w.shutdown();
    }

    #[test]
    fn panic_marks_inflight_lost_and_recovers_drained() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (go_tx, go_rx) = mpsc::channel::<()>();
        // 写入器：首帧阻塞待放行后 panic（触发出现在飞帧丢失）；其后调用一律返回 true
        let writer: WriteFn = {
            let calls = Arc::clone(&calls);
            Box::new(move |_t: &FrameTask| -> bool {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    let _ = go_rx.recv();
                    panic!("injected writer panic on first frame");
                }
                true
            })
        };
        let w = DiagWorker::start_with(writer);
        // 提交 3 帧：第 1 帧被 worker 取走并阻塞在 writer，另 2 帧留在队列缓冲（CAPACITY=2）
        assert!(w.submit(task(0, 0)).is_ok());
        assert!(w.submit(task(0, 1)).is_ok());
        assert!(w.submit(task(0, 2)).is_ok());
        // 放行 writer → panic → catch_unwind 返回后对队列帧排空写入（借用 &rx 才能做到）
        let _ = go_tx.send(());
        w.shutdown();
        assert!(w.is_failed(), "panic 后必须进入失败态");
        // 在飞帧（seq=0）丢失并记录 last_lost
        assert_eq!(w.lost_frames(), 1);
        assert_eq!(
            w.last_lost(),
            Some(FrameId {
                generation: 0,
                sequence: 0
            })
        );
        // 排队 2 帧经统一写出口排空成功补写，计入 recovered 而非 lost
        assert_eq!(w.recovered_frames(), 2);
    }

    #[test]
    fn panic_drain_write_false_counts_lost() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (go_tx, go_rx) = mpsc::channel::<()>();
        // 首帧 panic（在飞帧丢失）；其后调用一律返回 false（排空写入失败）
        let writer: WriteFn = {
            let calls = Arc::clone(&calls);
            Box::new(move |_t: &FrameTask| -> bool {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    let _ = go_rx.recv();
                    panic!("injected writer panic on first frame");
                }
                false
            })
        };
        let w = DiagWorker::start_with(writer);
        assert!(w.submit(task(0, 0)).is_ok());
        assert!(w.submit(task(0, 1)).is_ok());
        assert!(w.submit(task(0, 2)).is_ok());
        let _ = go_tx.send(());
        w.shutdown();
        assert!(w.is_failed(), "panic 后必须进入失败态");
        // 在飞帧 + 2 排队帧排空写入失败 → 全部记 lost，无 recovered
        assert_eq!(w.lost_frames(), 3);
        assert_eq!(
            w.last_lost(),
            Some(FrameId {
                generation: 0,
                sequence: 2
            })
        );
        assert_eq!(w.recovered_frames(), 0);
    }

    /// 改动 1 验证：排空途中写入器**二次 panic** 时**逐帧记账**——每帧各记一次 lost，
    /// recovered 为 0，且 writer 被调用次数 == 队列帧数（无帧被无声丢弃）。
    #[test]
    fn panic_drain_double_panic_counts_each_frame_lost() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (go_tx, go_rx) = mpsc::channel::<()>();
        // 写入器：第 1 次调用阻塞待放行后 panic（在飞帧）；**之后每次调用也 panic**（二次 panic）
        let writer: WriteFn = {
            let calls = Arc::clone(&calls);
            Box::new(move |_t: &FrameTask| -> bool {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    let _ = go_rx.recv();
                }
                panic!("injected writer panic every call");
            })
        };
        let w = DiagWorker::start_with(writer);
        // 1 帧在飞（阻塞在 writer），2 帧留队列（CAPACITY=2）
        assert!(w.submit(task(0, 0)).is_ok());
        assert!(w.submit(task(0, 1)).is_ok());
        assert!(w.submit(task(0, 2)).is_ok());
        let _ = go_tx.send(());
        w.shutdown();
        assert!(w.is_failed(), "panic 后必须进入失败态");
        assert!(w.drain_finished(), "排空必须已完成（即便每帧都 panic）");
        // 在飞 1 帧 + 排队 2 帧各触发一次 write panic，全部逐帧记 lost，无无声丢弃
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "writer 调用次数须等于队列帧数（无帧被无声丢弃）"
        );
        assert_eq!(w.lost_frames(), 3, "二次 panic 的每帧都须逐帧记 lost");
        assert_eq!(w.recovered_frames(), 0);
        assert_eq!(
            w.last_lost(),
            Some(FrameId {
                generation: 0,
                sequence: 2
            })
        );
    }

    /// 改动 2 验证：**先旧后新**——失败态下 `submit` 返回 `Err(task)` 前须等旧帧全部写出；
    /// 用 writer 写出序号的 push 顺序与 `recovered_frames` 共同断言。
    #[test]
    fn submit_after_panic_waits_for_drain_then_returns_err() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (go_tx, go_rx) = mpsc::channel::<()>();
        // 旧帧写出顺序的观测点：writer 每写一帧 push 其 sequence
        let order = Arc::new(Mutex::new(Vec::<u64>::new()));
        // 写入器：第 1 次调用阻塞待放行后 panic（在飞帧）；其后调用返回 true 并按序记录 seq
        let writer: WriteFn = {
            let calls = Arc::clone(&calls);
            let order = Arc::clone(&order);
            Box::new(move |t: &FrameTask| -> bool {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    let _ = go_rx.recv();
                    panic!("injected writer panic on first frame");
                }
                order.lock().unwrap_or_else(|p| p.into_inner()).push(t.id.sequence);
                true
            })
        };
        let w = DiagWorker::start_with(writer);
        assert!(w.submit(task(0, 0)).is_ok());
        assert!(w.submit(task(0, 1)).is_ok());
        assert!(w.submit(task(0, 2)).is_ok());
        let _ = go_tx.send(());
        // 等 worker 进入失败态（此后它开始排空旧帧 seq=1、2）
        let deadline = Instant::now() + Duration::from_secs(2);
        while !w.is_failed() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(w.is_failed(), "panic 后必须进入失败态");
        // 失败态下 submit：必须等旧帧写完才交还任务
        match w.submit(task(0, 9)) {
            Ok(()) => panic!("失败态下 submit 必须返回 Err(task)"),
            Err(back) => {
                assert_eq!(
                    back.id,
                    FrameId {
                        generation: 0,
                        sequence: 9
                    }
                );
                assert_eq!(back.rows, vec!["p1 - - -".to_string()]);
                // submit 返回 Err 时：排空已完成、旧帧（seq 1、2）已按序写出（先旧后新）
                assert!(w.drain_finished(), "submit 返回 Err 时排空须已完成");
                assert_eq!(w.recovered_frames(), 2, "submit 返回 Err 时旧帧须已全部写出");
                assert_eq!(
                    *order.lock().unwrap_or_else(|p| p.into_inner()),
                    vec![1, 2],
                    "旧帧须按序先落盘（先旧后新）"
                );
            }
        }
        w.shutdown();
    }

    /// 排空超过 500ms 仍须等待，多个失败提交只能在旧帧完成后交还。
    #[test]
    fn concurrent_failed_submits_wait_beyond_old_timeout() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let (slow_tx, slow_rx) = mpsc::channel::<()>();
        // 写入器：第 1 次调用阻塞待放行后 panic；其后调用阻塞在 slow_rx（模拟排空耗时超上限）
        let writer: WriteFn = {
            let calls = Arc::clone(&calls);
            Box::new(move |_t: &FrameTask| -> bool {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    let _ = go_rx.recv();
                    panic!("injected writer panic on first frame");
                }
                let _ = slow_rx.recv();
                true
            })
        };
        let w = DiagWorker::start_with(writer);
        assert!(w.submit(task(0, 0)).is_ok());
        assert!(w.submit(task(0, 1)).is_ok());
        assert!(w.submit(task(0, 2)).is_ok());
        let _ = go_tx.send(());
        let deadline = Instant::now() + Duration::from_secs(2);
        while !w.is_failed() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(w.is_failed(), "panic 后必须进入失败态");
        thread::scope(|scope| {
            let (returned_tx, returned_rx) = mpsc::channel();
            for sequence in [9, 10] {
                let worker = &w;
                let returned_tx = returned_tx.clone();
                scope.spawn(move || {
                    let result = worker.submit(task(0, sequence));
                    let _ = returned_tx.send((result, worker.drain_finished()));
                });
            }
            drop(returned_tx);
            let early_return = returned_rx.recv_timeout(Duration::from_millis(650));
            // 先放行再断言，避免测试失败时留下阻塞线程。
            let _ = slow_tx.send(());
            let _ = slow_tx.send(());
            assert!(matches!(early_return, Err(mpsc::RecvTimeoutError::Timeout)));
            let mut returned_sequences = Vec::new();
            for _ in 0..2 {
                let (result, drain_finished) = returned_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                let returned_task = match result {
                    Err(returned_task) => returned_task,
                    Ok(()) => panic!("失败态下 submit 必须交还任务"),
                };
                assert!(drain_finished);
                assert_eq!(returned_task.rows, vec!["p1 - - -".to_string()]);
                assert_eq!(returned_task.ts, "0101-000000");
                returned_sequences.push(returned_task.id.sequence);
            }
            returned_sequences.sort_unstable();
            assert_eq!(returned_sequences, vec![9, 10]);
        });
        w.shutdown();
        assert!(w.drain_finished(), "放行后排空须完成");
    }

    /// 已获准的发送端在排空暂时为空后交付，帧仍须被接收并记账。
    #[test]
    fn panic_drain_waits_for_previously_admitted_sender() {
        let (panic_tx, panic_rx) = mpsc::channel();
        let (recovered_tx, recovered_rx) = mpsc::channel();
        let writer: WriteFn = Box::new(move |frame| {
            if frame.id.sequence == 0 {
                panic_rx.recv().unwrap();
                panic!("injected writer panic");
            }
            recovered_tx.send(frame.id.sequence).unwrap();
            true
        });
        let worker = DiagWorker::start_with(writer);
        // 模拟 submit 已检查失败标志并克隆发送端，尚未实际发送。
        let admitted_sender = worker.tx.lock().unwrap().as_ref().unwrap().clone();
        assert!(worker.submit(task(0, 0)).is_ok());
        assert!(worker.submit(task(0, 1)).is_ok());
        panic_tx.send(()).unwrap();
        assert_eq!(recovered_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        let premature_finish = worker.drain_finished();
        let late_send = admitted_sender.send(task(0, 2));
        drop(admitted_sender);
        worker.shutdown();
        assert!(!premature_finish, "发送端尚未释放，排空不得提前完成");
        assert!(late_send.is_ok());
        assert_eq!(recovered_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 2);
        assert_eq!(worker.recovered_frames(), 2);
        assert_eq!(worker.lost_frames(), 1);
        assert!(worker.drain_finished());
    }
}
