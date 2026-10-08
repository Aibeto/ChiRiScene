//! diag_build_cost.rs - 区块索引: [blocks] [stats] [tests]
//! build 子账只分解父 build，不能与 selfcost/1 相加。
//! clock_errors 累计子块失败的读钟端点与逆行区间；父端点错误仍在父账。
//! residual_errors 累计不可得的残差维度，wall/CPU 各计一次，不伪造有效零。

// [blocks]
#[derive(Clone, Copy, Debug)]
pub enum BuildBlock {
    ThreadTids,
    TidStat,
    ProcessMap,
    SchedAffinity,
    Residual,
}

impl BuildBlock {
    pub const ALL: [Self; 5] = [
        Self::ThreadTids,
        Self::TidStat,
        Self::ProcessMap,
        Self::SchedAffinity,
        Self::Residual,
    ];

    fn slot(self) -> usize {
        self as usize
    }

    fn name(self) -> &'static str {
        match self {
            Self::ThreadTids => "get_thread_tids",
            Self::TidStat => "tid_stat",
            Self::ProcessMap => "snapshot_procs",
            Self::SchedAffinity => "sched_affinity",
            Self::Residual => "residual",
        }
    }
}

// [stats]
#[derive(Clone, Debug, Default)]
pub struct BuildBlockStats {
    pub count: u64,
    pub wall_valid_count: u64,
    pub wall_ns_sum: u64,
    pub cpu_valid_count: u64,
    pub cpu_ns_sum: u64,
}

#[derive(Debug, Default)]
pub struct BuildCostStats {
    blocks: [BuildBlockStats; 5],
    clock_errors: u64,
    residual_errors: u64,
    write_errors: u64,
    wall_overflow: bool,
    cpu_overflow: bool,
}

impl BuildCostStats {
    pub fn record(
        &mut self,
        block: BuildBlock,
        wall_ns: u64,
        start: Option<u64>,
        end: Option<u64>,
    ) {
        self.clock_errors += u64::from(start.is_none()) + u64::from(end.is_none());
        let cpu_ns = start.zip(end).and_then(|(start, end)| end.checked_sub(start));
        if start.is_some() && end.is_some() && cpu_ns.is_none() {
            self.clock_errors += 1;
        }
        let stats = &mut self.blocks[block.slot()];
        stats.count += 1;
        stats.wall_valid_count += 1;
        match stats.wall_ns_sum.checked_add(wall_ns) {
            Some(total) => stats.wall_ns_sum = total,
            None => self.wall_overflow = true,
        }
        if let Some(cpu_ns) = cpu_ns {
            stats.cpu_valid_count += 1;
            match stats.cpu_ns_sum.checked_add(cpu_ns) {
                Some(total) => stats.cpu_ns_sum = total,
                None => self.cpu_overflow = true,
            }
        }
    }

    /// 每帧仅结算一次；缺失时钟、溢出和负残差均不产生有效零值。
    pub fn finish(&mut self, wall_ns: u64, cpu_ns: Option<u64>) {
        let children = &self.blocks[..BuildBlock::Residual.slot()];
        let wall_sum = children.iter().try_fold(0u64, |total, block| {
            total.checked_add(block.wall_ns_sum)
        });
        let cpu_sum = children.iter().try_fold(0u64, |total, block| {
            (block.count == block.cpu_valid_count)
                .then_some(())
                .and_then(|_| total.checked_add(block.cpu_ns_sum))
        });
        let residual_wall = wall_sum
            .filter(|_| !self.wall_overflow)
            .and_then(|sum| wall_ns.checked_sub(sum));
        let residual_cpu = cpu_ns.zip(cpu_sum)
            .filter(|_| !self.cpu_overflow)
            .and_then(|(total, sum)| total.checked_sub(sum));
        let stats = &mut self.blocks[BuildBlock::Residual.slot()];
        stats.count += 1;
        if let Some(residual_wall) = residual_wall {
            stats.wall_valid_count += 1;
            stats.wall_ns_sum += residual_wall;
        } else {
            self.residual_errors += 1;
        }
        if let Some(residual_cpu) = residual_cpu {
            stats.cpu_valid_count += 1;
            stats.cpu_ns_sum += residual_cpu;
        } else {
            self.residual_errors += 1;
        }
    }

    pub fn absorb(&mut self, other: &Self) {
        for (target, source) in self.blocks.iter_mut().zip(&other.blocks) {
            target.count = target.count.saturating_add(source.count);
            target.wall_valid_count = target.wall_valid_count.saturating_add(source.wall_valid_count);
            target.wall_ns_sum = target.wall_ns_sum.saturating_add(source.wall_ns_sum);
            target.cpu_valid_count = target.cpu_valid_count.saturating_add(source.cpu_valid_count);
            target.cpu_ns_sum = target.cpu_ns_sum.saturating_add(source.cpu_ns_sum);
        }
        self.clock_errors = self.clock_errors.saturating_add(other.clock_errors);
        self.residual_errors = self.residual_errors.saturating_add(other.residual_errors);
    }

    pub fn note_write_error(&mut self) {
        self.write_errors += 1;
    }

    pub fn reset(&mut self) {
        self.blocks = Default::default();
        self.wall_overflow = false;
        self.cpu_overflow = false;
    }

    pub fn reset_session(&mut self) {
        *self = Self::default();
    }

    pub fn stats(&self, block: BuildBlock) -> &BuildBlockStats {
        &self.blocks[block.slot()]
    }

    /// residual 含组装/差分/渲染和父区间内未计入子块的观察者开销。
    /// 子块 CPU 起点读钟尾部与终点读钟前部归子块，其余读钟与 record 归 residual。
    pub fn summary_line(&self, session: &str, start: &str, end: &str, block: BuildBlock) -> String {
        let stats = self.stats(block);
        format!(
            "schema=selfcost-build/1 session={session} window_start={start} window_end={end} \
             parent=build block={} count={} wall_valid_count={} wall_ns_sum={} \
             cpu_valid_count={} cpu_ns_sum={} clock_errors={} residual_errors={} write_errors={}",
            block.name(), stats.count, stats.wall_valid_count, stats.wall_ns_sum,
            stats.cpu_valid_count, stats.cpu_ns_sum, self.clock_errors,
            self.residual_errors, self.write_errors,
        )
    }
}

// [tests]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_operations_and_clock_endpoints_still_count() {
        let mut stats = BuildCostStats::default();
        stats.record(BuildBlock::TidStat, 8, None, None);
        stats.record(BuildBlock::TidStat, 7, Some(9), Some(11));
        stats.record(BuildBlock::TidStat, 6, Some(20), Some(19));
        let block = stats.stats(BuildBlock::TidStat);
        assert_eq!(block.count, 3);
        assert_eq!(block.wall_ns_sum, 21);
        assert_eq!(block.cpu_valid_count, 1);
        assert_eq!(block.cpu_ns_sum, 2);
        assert_eq!(stats.clock_errors, 3);
    }

    #[test]
    fn residual_requires_complete_cpu_and_checked_subtraction() {
        let mut stats = BuildCostStats::default();
        stats.record(BuildBlock::ThreadTids, 10, Some(1), Some(5));
        stats.record(BuildBlock::TidStat, 20, Some(5), Some(11));
        stats.finish(50, Some(15));
        let residual = stats.stats(BuildBlock::Residual);
        assert_eq!(residual.count, 1);
        assert_eq!(residual.wall_ns_sum, 20);
        assert_eq!(residual.cpu_ns_sum, 5);
        assert_eq!(residual.cpu_valid_count, 1);

        let mut incomplete = BuildCostStats::default();
        incomplete.record(BuildBlock::TidStat, 10, None, Some(3));
        incomplete.finish(20, Some(10));
        assert_eq!(incomplete.stats(BuildBlock::Residual).cpu_valid_count, 0);
        assert_eq!(incomplete.stats(BuildBlock::Residual).wall_valid_count, 1);

        let mut inconsistent = BuildCostStats::default();
        inconsistent.record(BuildBlock::TidStat, 30, Some(0), Some(30));
        inconsistent.finish(20, Some(20));
        assert_eq!(inconsistent.stats(BuildBlock::Residual).cpu_valid_count, 0);
        assert_eq!(inconsistent.stats(BuildBlock::Residual).wall_valid_count, 0);
        assert_eq!(inconsistent.residual_errors, 2);
    }

    #[test]
    fn frame_merge_window_reset_and_session_reset_are_separate() {
        let mut frame = BuildCostStats::default();
        frame.record(BuildBlock::SchedAffinity, 12, None, Some(8));
        frame.finish(20, Some(15));
        let mut window = BuildCostStats::default();
        window.absorb(&frame);
        window.absorb(&frame);
        assert_eq!(window.stats(BuildBlock::SchedAffinity).count, 2);
        assert_eq!(window.stats(BuildBlock::Residual).count, 2);
        assert_eq!(window.clock_errors, 2);
        window.reset();
        assert_eq!(window.stats(BuildBlock::SchedAffinity).count, 0);
        assert_eq!(window.clock_errors, 2);
        window.reset_session();
        assert_eq!(window.clock_errors, 0);
    }

    #[test]
    fn invalid_parent_or_overflow_never_becomes_valid_zero_residual() {
        let mut frame = BuildCostStats::default();
        frame.finish(0, None);
        assert_eq!(frame.stats(BuildBlock::Residual).cpu_valid_count, 0);
        let mut overflow = BuildCostStats::default();
        overflow.record(BuildBlock::TidStat, u64::MAX, Some(0), Some(u64::MAX));
        overflow.record(BuildBlock::ThreadTids, 1, Some(0), Some(1));
        overflow.finish(u64::MAX, Some(u64::MAX));
        assert_eq!(overflow.stats(BuildBlock::Residual).cpu_valid_count, 0);
        assert_eq!(overflow.stats(BuildBlock::Residual).wall_valid_count, 0);

        let mut same_block_overflow = BuildCostStats::default();
        same_block_overflow.record(BuildBlock::TidStat, u64::MAX, Some(0), Some(u64::MAX));
        same_block_overflow.record(BuildBlock::TidStat, 1, Some(0), Some(1));
        same_block_overflow.finish(u64::MAX, Some(u64::MAX));
        assert_eq!(same_block_overflow.stats(BuildBlock::Residual).cpu_valid_count, 0);
        assert_eq!(same_block_overflow.stats(BuildBlock::Residual).wall_valid_count, 0);
    }

    #[test]
    fn summary_is_a_separate_schema_and_preserves_unavailable_denominator() {
        let mut frame = BuildCostStats::default();
        frame.record(BuildBlock::ProcessMap, 8, None, None);
        frame.finish(10, None);
        let line = frame.summary_line("session", "1", "2", BuildBlock::ProcessMap);
        assert!(line.starts_with("schema=selfcost-build/1 "));
        assert!(line.contains("parent=build block=snapshot_procs count=1"));
        assert!(line.contains("cpu_valid_count=0 cpu_ns_sum=0 clock_errors=2"));
        frame.note_write_error();
        frame.reset();
        assert!(frame.summary_line("session", "2", "3", BuildBlock::Residual)
            .ends_with("write_errors=1"));
        frame.reset_session();
        assert!(frame.summary_line("session", "3", "4", BuildBlock::Residual)
            .ends_with("write_errors=0"));
    }
}
