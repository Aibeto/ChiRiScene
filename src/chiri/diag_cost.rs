//! diag_cost.rs: [clock] [buckets] [stats] [summary]
//! @S 快照同步成本的线程局部累计器：墙钟与线程 CPU 时钟、固定直方图、会话窗口摘要。
//!
//! **归因边界（写清便于审查）**：三个 `Block` 是**互不重叠**的分段，各自独立记账：
//! - `Build`  = build 段（`before_build` → `after_build`）；
//! - `Write`  = write 段（`after_build` → `after_write`）；
//! - `Summary`= 摘要写入段（`after_write` → `after_summary`，独立记账，不与前两段重叠）。
//!
//! **CPU 口径**：`thread_cpu_ns` 读的是**当前线程** `CLOCK_THREAD_CPUTIME_ID` 的绝对读数，
//! 调用方在段两端各取一次做差分。读取失败返回 `None`——**不兜底为 0 或进程时间**；
//! `record` 收到 `cpu_ns == None` 时仍计 `count`/墙钟/桶，只是**不计入 CPU 分母**
//! （CPU 均值 = `cpu_ns_sum / cpu_valid_count`，`cpu_valid_count == 0` 时该块 CPU 不可得）。
//!
//! **并发语义**：本结构**无内部锁**，由调用方（同步采集线程）以 `&mut self` **独占**持有。
//! 将来若需跨线程，另设计——本版不引入全局锁累计。
//!
//! **读端口径提示**：`summary_line` 在 `cpu_valid_count == 0` 时仍输出 `cpu_ns_sum=0`
//! （字段位置/数量固定，不因缺失而增删），读端须按 `cpu_valid_count=0 → CPU unavailable` 判读，
//! **不可**把 `cpu_ns_sum=0` 当成「CPU 耗时为 0」。

pub const SCHEMA: &str = "selfcost/1";
/// 墙钟直方图桶上界（ms，含上界）；另有第 9 个「超界」桶
pub const WALL_BUCKETS_MS: [u64; 8] = [1, 2, 5, 10, 20, 40, 50, 100];
pub const WALL_BUCKET_COUNT: usize = 9;
/// 慢调用阈值：只用于节流告警，不影响入账
pub const SLOW_NS: u64 = 50_000_000;

/// 同步成本分块标识（见模块头：三段互不重叠）
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Block {
    Build,
    Write,
    Summary,
}

impl Block {
    pub const ALL: [Block; 3] = [Block::Build, Block::Write, Block::Summary];

    /// 小写字段名，用于 `summary_line` 的 `block=` 取值
    const fn as_str(self) -> &'static str {
        match self {
            Block::Build => "build",
            Block::Write => "write",
            Block::Summary => "summary",
        }
    }

    /// 在本结构三个槽位中的下标（`Block::ALL` 同序）
    const fn slot(self) -> usize {
        match self {
            Block::Build => 0,
            Block::Write => 1,
            Block::Summary => 2,
        }
    }
}

// [stats]
/// 单个 `Block` 的累计量：次数、CPU 有效分母、墙钟和/最大值、CPU 和、
/// ≥`SLOW_NS` 次数，以及固定桶直方图（长度 = [`WALL_BUCKET_COUNT`]）
#[derive(Clone, Debug, Default)]
pub struct BlockStats {
    pub count: u64,
    pub cpu_valid_count: u64,
    pub wall_ns_sum: u64,
    pub wall_ns_max: u64,
    pub cpu_ns_sum: u64,
    pub slow_count: u64,
    pub wall_buckets: [u64; WALL_BUCKET_COUNT],
}

// [stats]
/// `@S` 快照同步成本的线程局部累计器：三个 [`BlockStats`] + 两类错误计数。
/// 由调用方独占持有（`&mut self`），无内部锁。
#[derive(Debug, Default)]
pub struct CostStats {
    blocks: [BlockStats; 3],
    clock_errors: u64,
    write_errors: u64,
}

impl CostStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一次分块调用：`wall_ns` 必给；`cpu_ns == None` 表示 CPU 读取无效（仍计 wall/count，
    /// cpu 分母不计）。墙钟和用 `saturating_add`，不因极端输入 panic 或回绕。
    pub fn record(&mut self, block: Block, wall_ns: u64, cpu_ns: Option<u64>) {
        let stats = &mut self.blocks[block.slot()];
        stats.count += 1;
        stats.wall_ns_sum = stats.wall_ns_sum.saturating_add(wall_ns);
        if wall_ns > stats.wall_ns_max {
            stats.wall_ns_max = wall_ns;
        }
        if wall_ns >= SLOW_NS {
            stats.slow_count += 1;
        }
        stats.wall_buckets[bucket_index(wall_ns)] += 1;
        if let Some(cpu_ns) = cpu_ns {
            stats.cpu_ns_sum = stats.cpu_ns_sum.saturating_add(cpu_ns);
            stats.cpu_valid_count += 1;
        }
    }

    /// 把一个已聚合的 [`BlockStats`] 并入指定 block（逐字段相加；`wall_ns_max` 取较大者；
    /// 直方图桶逐桶相加）。供「延后报出的 summary 成本按其自身窗口归属并入」使用。
    pub fn absorb(&mut self, block: Block, other: &BlockStats) {
        let stats = &mut self.blocks[block.slot()];
        stats.count = stats.count.saturating_add(other.count);
        stats.cpu_valid_count = stats.cpu_valid_count.saturating_add(other.cpu_valid_count);
        stats.wall_ns_sum = stats.wall_ns_sum.saturating_add(other.wall_ns_sum);
        stats.cpu_ns_sum = stats.cpu_ns_sum.saturating_add(other.cpu_ns_sum);
        stats.slow_count = stats.slow_count.saturating_add(other.slow_count);
        if other.wall_ns_max > stats.wall_ns_max {
            stats.wall_ns_max = other.wall_ns_max;
        }
        for (slot, &value) in stats.wall_buckets.iter_mut().zip(other.wall_buckets.iter()) {
            *slot = slot.saturating_add(value);
        }
    }

    /// 记一次 CPU 时钟读取失败（读端按 `clock_errors` 判定 CPU 分母是否被误差污染）
    pub fn note_clock_error(&mut self) {
        self.clock_errors += 1;
    }

    /// 记一次摘要写入失败（读端按 `write_errors` 判定该窗口测量是否不完整）
    pub fn note_write_error(&mut self) {
        self.write_errors += 1;
    }

    pub fn stats(&self, block: Block) -> &BlockStats {
        &self.blocks[block.slot()]
    }

    pub fn clock_errors(&self) -> u64 {
        self.clock_errors
    }

    pub fn write_errors(&self) -> u64 {
        self.write_errors
    }

    pub fn slow_count(&self, block: Block) -> u64 {
        self.stats(block).slow_count
    }

    // [stats]
    /// 发送端：窗口级复位——只清三个 [`BlockStats`]（`count` / `cpu_valid_count` /
    /// `wall_ns_sum` / `wall_ns_max` / `cpu_ns_sum` / `slow_count` / `wall_buckets` 全归零），
    /// **保留** `clock_errors` 与 `write_errors`。
    ///
    /// 保留错误计数是为了让「写失败」的窗口仍能在**下一个窗口**的结构化摘要行里以非零
    /// `write_errors` / `clock_errors` 报出——失败事实不因本窗口复位而丢失，可离线复算。
    /// 需要连错误计数一并清零（整会话清空）请用 [`reset_session`](Self::reset_session)。
    pub fn reset(&mut self) {
        for stats in &mut self.blocks {
            *stats = BlockStats::default();
        }
    }

    /// 会话级复位：清零一切，**含** `clock_errors` 与 `write_errors`。用于新会话开始。
    pub fn reset_session(&mut self) {
        *self = Self::default();
    }

    /// 是否还有待结算的窗口数据：任一 block 的 `count > 0` 即为 `true`。
    /// 供会话结束的尾窗排空判断使用。
    pub fn has_pending(&self) -> bool {
        self.blocks.iter().any(|stats| stats.count > 0)
    }

    // [summary]
    /// 一行摘要（每个 block 一行）：固定字段顺序
    /// `schema session window_start window_end block count cpu_valid_count
    /// wall_ns_sum wall_ns_max cpu_ns_sum wall_buckets slow_count clock_errors write_errors`，
    /// 空格分隔 `key=value`，`block` 为小写；`wall_buckets` 为 9 个逗号分隔计数。
    /// `cpu_valid_count == 0` 时 `cpu_ns_sum` 仍写 0——读端须按 `cpu_valid_count=0 → unavailable` 判读。
    pub fn summary_line(
        &self,
        session: &str,
        window_start: &str,
        window_end: &str,
        block: Block,
    ) -> String {
        let stats = self.stats(block);
        let buckets = stats
            .wall_buckets
            .iter()
            .map(|count| count.to_string())
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "schema={} session={} window_start={} window_end={} block={} count={} \
             cpu_valid_count={} wall_ns_sum={} wall_ns_max={} cpu_ns_sum={} wall_buckets={} \
             slow_count={} clock_errors={} write_errors={}",
            SCHEMA,
            session,
            window_start,
            window_end,
            block.as_str(),
            stats.count,
            stats.cpu_valid_count,
            stats.wall_ns_sum,
            stats.wall_ns_max,
            stats.cpu_ns_sum,
            buckets,
            stats.slow_count,
            self.clock_errors,
            self.write_errors,
        )
    }
}

// [buckets]
/// 墙钟纳秒 → 桶下标：落到第一个 `wall_ns <= 上界` 的桶（**上界含**）。
/// 比较在 **ns 精度**完成（上界 ms × 1e6 = 上界 ns），**不做 ms 向下取整**——
/// 故 1.999ms 落 ≤2ms 桶、100.999ms 落超界桶；全部超过则落第 9 个超界桶。
fn bucket_index(wall_ns: u64) -> usize {
    for (index, &upper_ms) in WALL_BUCKETS_MS.iter().enumerate() {
        if wall_ns <= upper_ms.saturating_mul(1_000_000) {
            return index;
        }
    }
    WALL_BUCKET_COUNT - 1
}

// [clock]
/// 当前线程 CPU 时钟绝对读数（ns）：`CLOCK_THREAD_CPUTIME_ID` 的 `tv_sec * 1e9 + tv_nsec`。
/// `clock_gettime` 返回非 0 时返回 `None`——**不兜底为 0 或进程时间**；
/// 数值转换用 `saturating`/`try_from` 防溢出与负值，读端用两端差分。
pub fn thread_cpu_ns() -> Option<u64> {
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    if ret != 0 {
        return None;
    }
    let secs = u64::try_from(ts.tv_sec).ok()?;
    let nsecs = u64::try_from(ts.tv_nsec).ok()?;
    Some(secs.saturating_mul(1_000_000_000).saturating_add(nsecs))
}

// [tests]
// 本仓库无 CI 执行 `cargo test`，以下测试只保证编译通过（真机/CI 恢复后可作为聚焦用例）。
#[cfg(test)]
mod tests {
    use super::*;

    /// ms → ns 便捷构造
    fn ms(value: u64) -> u64 {
        value * 1_000_000
    }

    #[test]
    fn bucket_boundaries_include_upper_and_overflow() {
        // 0/1 落桶 0；2/5/10/20/40/50/100 各落对应桶；101 落第 9（超界）桶
        let cases: [(u64, usize); 10] = [
            (0, 0),
            (1, 0),
            (2, 1),
            (5, 2),
            (10, 3),
            (20, 4),
            (40, 5),
            (50, 6),
            (100, 7),
            (101, 8),
        ];
        for (value_ms, want) in cases {
            let mut cost = CostStats::new();
            cost.record(Block::Build, ms(value_ms), None);
            let buckets = &cost.stats(Block::Build).wall_buckets;
            assert_eq!(buckets[want], 1, "ms={value_ms} 应落桶 {want}");
            assert_eq!(buckets.iter().sum::<u64>(), 1);
        }
    }

    #[test]
    fn bucket_compares_upper_bound_at_ns_precision() {
        // 上界比较在 ns 精度完成（不做 ms 向下取整）：亚毫秒不落入更低的桶，
        // 恰好等于上界的入该桶（含上界），略超上界的落下一个桶（或超界桶）。
        let cases: [(u64, usize); 11] = [
            (0, 0),             // 0ns → ≤1ms
            (999_999, 0),       // 0.999ms → ≤1ms
            (1_000_000, 0),     // 1ms（含上界）→ ≤1ms
            (1_000_001, 1),     // 1.000001ms → ≤2ms（不再误入桶 0）
            (1_999_000, 1),     // 1.999ms → ≤2ms（不再误入桶 0）
            (2_000_000, 1),     // 2ms（含上界）→ ≤2ms
            (5_000_000, 2),     // 5ms（含上界）→ ≤5ms
            (5_000_001, 3),     // 略超 5ms → ≤10ms（不再误入桶 2）
            (100_000_000, 7),   // 100ms（含上界）→ ≤100ms
            (100_999_000, 8),   // 100.999ms → 超界（不再误入桶 7）
            (101_000_000, 8),   // 101ms → 超界
        ];
        for (wall_ns, want) in cases {
            assert_eq!(bucket_index(wall_ns), want, "wall_ns={wall_ns} 应落桶 {want}");
        }
    }

    #[test]
    fn slow_threshold_is_inclusive() {
        let mut cost = CostStats::new();
        cost.record(Block::Write, SLOW_NS - 1, None);
        assert_eq!(cost.slow_count(Block::Write), 0);
        cost.record(Block::Write, SLOW_NS, None);
        assert_eq!(cost.slow_count(Block::Write), 1);
    }

    #[test]
    fn cpu_none_counts_wall_only() {
        let mut cost = CostStats::new();
        cost.record(Block::Build, ms(3), None);
        cost.record(Block::Build, ms(4), Some(ms(1)));
        let stats = cost.stats(Block::Build);
        assert_eq!(stats.count, 2);
        assert_eq!(stats.cpu_valid_count, 1);
        assert_eq!(stats.cpu_ns_sum, ms(1));
        assert_eq!(stats.wall_ns_sum, ms(7));
    }

    #[test]
    fn wall_sum_and_max_tracked() {
        let mut cost = CostStats::new();
        cost.record(Block::Summary, ms(2), None);
        cost.record(Block::Summary, ms(9), None);
        cost.record(Block::Summary, ms(1), None);
        let stats = cost.stats(Block::Summary);
        assert_eq!(stats.wall_ns_sum, ms(12));
        assert_eq!(stats.wall_ns_max, ms(9));
    }

    #[test]
    fn reset_session_clears_all_counters() {
        let mut cost = CostStats::new();
        cost.record(Block::Build, ms(5), Some(ms(2)));
        cost.note_clock_error();
        cost.note_write_error();
        cost.note_write_error();
        cost.reset_session();
        let stats = cost.stats(Block::Build);
        assert_eq!(stats.count, 0);
        assert_eq!(stats.cpu_valid_count, 0);
        assert_eq!(stats.wall_ns_sum, 0);
        assert_eq!(stats.wall_ns_max, 0);
        assert_eq!(stats.cpu_ns_sum, 0);
        assert_eq!(stats.slow_count, 0);
        assert_eq!(stats.wall_buckets.iter().sum::<u64>(), 0);
        assert_eq!(cost.clock_errors(), 0);
        assert_eq!(cost.write_errors(), 0);
    }

    #[test]
    fn reset_clears_block_stats_but_keeps_error_counters() {
        // 窗口级 reset：块统计全清，clock/write 错误计数作为会话级累计保留
        let mut cost = CostStats::new();
        cost.record(Block::Build, ms(5), Some(ms(2)));
        cost.record(Block::Write, SLOW_NS, None);
        cost.note_clock_error();
        cost.note_write_error();
        cost.reset();
        for block in Block::ALL {
            let stats = cost.stats(block);
            assert_eq!(stats.count, 0, "{block:?} count");
            assert_eq!(stats.cpu_valid_count, 0, "{block:?} cpu_valid_count");
            assert_eq!(stats.wall_ns_sum, 0, "{block:?} wall_ns_sum");
            assert_eq!(stats.wall_ns_max, 0, "{block:?} wall_ns_max");
            assert_eq!(stats.cpu_ns_sum, 0, "{block:?} cpu_ns_sum");
            assert_eq!(stats.slow_count, 0, "{block:?} slow_count");
            assert_eq!(
                stats.wall_buckets.iter().sum::<u64>(),
                0,
                "{block:?} wall_buckets"
            );
        }
        assert_eq!(cost.clock_errors(), 1);
        assert_eq!(cost.write_errors(), 1);
    }

    #[test]
    fn has_pending_reflects_any_block_count() {
        let mut cost = CostStats::new();
        assert!(!cost.has_pending(), "空累计器不应有待结算数据");
        cost.record(Block::Summary, ms(1), None);
        assert!(cost.has_pending(), "任一 block 有入账即应为 true");
        cost.reset();
        assert!(!cost.has_pending(), "reset 清块统计后应回到无待结算");
    }

    #[test]
    fn reset_after_write_error_keeps_failure_for_next_window() {
        // 组合场景：写失败窗口复位后，write_errors 仍为 1，块统计归零——
        // 失败事实留待下一个窗口的摘要行报出
        let mut cost = CostStats::new();
        cost.record(Block::Build, ms(3), Some(ms(1)));
        cost.record(Block::Build, ms(4), None);
        cost.record(Block::Write, ms(2), None);
        cost.note_write_error();
        cost.reset();
        assert_eq!(cost.write_errors(), 1, "写失败计数不应被窗口 reset 清掉");
        assert_eq!(cost.stats(Block::Build).count, 0);
        assert_eq!(cost.stats(Block::Write).count, 0);
        // 下一窗口即便零入账，摘要行也应带上失败计数
        let line = cost.summary_line("s", "a", "b", Block::Build);
        assert!(line.ends_with("clock_errors=0 write_errors=1"));
    }

    #[test]
    fn error_counters_accumulate() {
        let mut cost = CostStats::new();
        cost.note_clock_error();
        cost.note_clock_error();
        cost.note_write_error();
        assert_eq!(cost.clock_errors(), 2);
        assert_eq!(cost.write_errors(), 1);
    }

    #[test]
    fn summary_line_field_order_and_shape() {
        let mut cost = CostStats::new();
        cost.record(Block::Write, ms(3), Some(ms(1)));
        cost.note_clock_error();
        let line = cost.summary_line("sess", "10:00:00", "10:01:00", Block::Write);
        let keys: Vec<&str> = line
            .split(' ')
            .map(|field| field.split('=').next().unwrap())
            .collect();
        assert_eq!(
            keys,
            [
                "schema",
                "session",
                "window_start",
                "window_end",
                "block",
                "count",
                "cpu_valid_count",
                "wall_ns_sum",
                "wall_ns_max",
                "cpu_ns_sum",
                "wall_buckets",
                "slow_count",
                "clock_errors",
                "write_errors",
            ]
        );
        assert!(line.starts_with("schema=selfcost/1 session=sess"));
        assert!(line.contains(" block=write "));
        let buckets_field = line
            .split(' ')
            .find(|field| field.starts_with("wall_buckets="))
            .unwrap();
        assert_eq!(
            buckets_field
                .trim_start_matches("wall_buckets=")
                .split(',')
                .count(),
            WALL_BUCKET_COUNT
        );
        assert!(line.ends_with("clock_errors=1 write_errors=0"));
    }

    #[test]
    fn block_names_are_lowercase_and_all_is_ordered() {
        let cost = CostStats::new();
        assert!(cost.summary_line("s", "a", "b", Block::Build).contains("block=build"));
        assert!(cost.summary_line("s", "a", "b", Block::Write).contains("block=write"));
        assert!(cost.summary_line("s", "a", "b", Block::Summary).contains("block=summary"));
        assert_eq!(Block::ALL, [Block::Build, Block::Write, Block::Summary]);
    }

    #[test]
    fn cpu_unavailable_line_still_reports_zero_sum() {
        let cost = CostStats::new();
        let line = cost.summary_line("s", "a", "b", Block::Build);
        assert!(line.contains("cpu_valid_count=0"));
        assert!(line.contains("cpu_ns_sum=0"));
    }

    #[test]
    fn thread_cpu_clock_is_readable_and_monotonic() {
        // 开发机/真机都应可读；两次读数单调不减（相等允许）
        let first = thread_cpu_ns();
        let second = thread_cpu_ns();
        if let (Some(first), Some(second)) = (first, second) {
            assert!(second >= first);
        } else {
            // 极端环境读不到时钟时只校验返回语义（None 而非 0 兜底）
            assert!(first.is_none() && second.is_none());
        }
    }

    #[test]
    fn absorb_adds_fields_and_keeps_other_blocks() {
        let mut cost = CostStats::new();
        // 目标 block 先有入账：3ms（落桶 2），CPU 有效 1 条
        cost.record(Block::Summary, ms(3), Some(ms(1)));
        // 另一个 block 不应被 absorb 波及
        cost.record(Block::Build, ms(9), None);

        let mut other = BlockStats::default();
        other.count = 2;
        other.cpu_valid_count = 1;
        other.wall_ns_sum = ms(20);
        other.wall_ns_max = ms(12);
        other.cpu_ns_sum = ms(7);
        other.slow_count = 1;
        other.wall_buckets[7] = 2; // 两条落 ≤100ms 桶

        cost.absorb(Block::Summary, &other);

        let stats = cost.stats(Block::Summary);
        assert_eq!(stats.count, 3, "count 逐字段相加");
        assert_eq!(stats.cpu_valid_count, 2);
        assert_eq!(stats.wall_ns_sum, ms(23));
        assert_eq!(stats.wall_ns_max, ms(12), "max 取较大者");
        assert_eq!(stats.cpu_ns_sum, ms(8));
        assert_eq!(stats.slow_count, 1);
        assert_eq!(stats.wall_buckets[2], 1, "原 3ms 桶保持不变");
        assert_eq!(stats.wall_buckets[7], 2, "直方图桶逐桶相加");

        // 其它 block 不受影响
        let build = cost.stats(Block::Build);
        assert_eq!(build.count, 1);
        assert_eq!(build.wall_ns_sum, ms(9));
        assert_eq!(build.wall_buckets.iter().sum::<u64>(), 1);
        assert_eq!(cost.stats(Block::Write).count, 0);
    }

    #[test]
    fn absorb_then_summary_line_reports_merged_fields() {
        let mut cost = CostStats::new();
        let mut other = BlockStats::default();
        other.count = 4;
        other.cpu_valid_count = 3;
        other.wall_ns_sum = ms(40);
        other.wall_ns_max = ms(18);
        other.cpu_ns_sum = ms(9);
        other.wall_buckets[6] = 4; // 4 条落 ≤50ms 桶

        cost.absorb(Block::Summary, &other);

        let line = cost.summary_line("sess", "100", "200", Block::Summary);
        assert!(line.contains(" block=summary "));
        assert!(line.contains(" count=4 "));
        assert!(line.contains(" cpu_valid_count=3 "));
        assert!(line.contains(" wall_ns_sum=40000000 "));
        assert!(line.contains(" wall_ns_max=18000000 "));
        assert!(line.contains(" cpu_ns_sum=9000000 "));
        assert!(line.ends_with("slow_count=0 clock_errors=0 write_errors=0"));

        let buckets_field = line
            .split(' ')
            .find(|field| field.starts_with("wall_buckets="))
            .unwrap();
        let counts: Vec<&str> = buckets_field
            .trim_start_matches("wall_buckets=")
            .split(',')
            .collect();
        assert_eq!(counts.len(), WALL_BUCKET_COUNT);
        assert_eq!(counts[6], "4");
    }
}
