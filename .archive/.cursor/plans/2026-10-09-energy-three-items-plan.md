# 三项自耗改进实施计划

> 状态：三项代码与聚焦测试已写入，两个独立审查已完成，无剩余阻断发现。测试缺参已修复并复审。测试执行和目标编译仍受阻，不代表验收通过。本文件是既有 daemon 自耗专项的本轮执行补充。

**目标**：减少确定无效的亲和/缺失节点写入，并将 @S build 的线程 CPU 成本拆成可定位的子块；不改变采样集合、周期、差分帧或调频策略。

**架构**：亲和模块独占两项失败抑制；诊断模块扩展独立子块记账。保留 selfcost/1 的 build/write/summary 父账，子账另用名称和 schema，不能与父账相加。未启用诊断 worker，不做开关对照、不提交。

**技术**：现有 Rust Instant/CLOCK_THREAD_CPUTIME_ID、标准库、现有便携回归入口。用户已授权规划后实施及独立审查。

## 文件所有权

- 实施代理 A：`src/chiri/affinity.rs` 及新建 `src/chiri/affinity_retry.rs`（若确需隔离纯状态机）；同时完成 EINVAL 与 restricted 缓存，避免同文件竞争。
- 实施代理 B：`src/chiri/mod.rs`、`src/chiri/diag_cost.rs`；必要的新纯统计模块由 B 独占。不修改 affinity/cpu_monitor/logger 的采样行为。
- 主线：计划、验证整合；代理完成后可接手其文件修复审查问题。
- 文档代理：代码完成后独占指定 agentsdocs 小节和 Cursor memory；主线提供统一口径，不与其他代理同时写。
- 独立审查代理：全部只读，不参与实现。

## 任务 A1：EINVAL 有界退避

- [x] 先补纯状态机测试：同身份/同掩码冷却、到期重试、不同掩码立即放行、成功清零、非 EINVAL 不退避、身份/配置重置。执行受阻，未取得 red/green 证据。
- [x] 仅 normal_busy/normal_press 的组绑定失败进入退避。初次 EINVAL 后 4s，连续失败逐级延长到 8/16/32s，32s 封顶；跳过期间不递增计数、不假报成功。
- [x] 重试状态按线程身份（starttime）及目标掩码隔离，生命周期回收；配置重载/释放清理。成功才改变 group_bind 等已绑定状态。
- [x] 恢复/释放路径不受退避影响。用实写结果 errno 判断，保留真实失败打点；不永久缓存失败。

## 任务 A2：restricted ENOENT 缓存

- [x] 先补测试：只缓存 ENOENT、600s 到期、值变化与恢复请求不误清缓存、配置重载失效、节点恢复成功清零。生产重载接线已静态审查，运行验证待执行。
- [x] 保留 write-first，不做 exists 预查。仅 restricted 的 cpu.uclamp.max 返回 ENOENT 后跳过常规重写，600s 再允许实际写入探测。
- [x] background 与其他 errno 保持原路径，60s 同值再断言仍生效；无真实写入就不增加失败计数、不输出 @A 写入帧。
- [x] 配置重载清缓存，恢复 max 请求继续恢复所有可用节点；缺失节点的缓存不得被每次目标值变化冲掉。

## 任务 B：build 子块 CPU 记账

- [x] 先补统计测试：完整调用计数、无效 CPU 分母、窗口/会话复位、错误计数、子账不进入父账、边界失败不记零。执行受阻。
- [x] 拆分枚举、逐 TID stat 读取（含现有解析）、进程 map 快照、亲和查询；其余组装/差分/渲染用 residual，子块不重叠。
- [x] 子块仅 build 路径启用，不影响其他调用方；每次实际调用包括失败调用均入账。CPU 时钟失败记错误及无效分母，不以 0 替代。
- [x] 复用原 60s/会话末摘要通道，新子账使用 `selfcost-build/1`，保留旧三行 `selfcost/1` 及字段。子账是父 build 的分解，严禁与父账求和。
- [x] 观察者成本保留在父 build 账，注明子块读钟边界开销与 residual 的归属；不在逐线程循环格式化计时日志，不新增逐调用落盘。
- [x] 保持取数顺序、full=1、TID 复用语义；线程化仍关闭。新增读钟的实际采样时延仍待真机观察，不能宣称零观察者开销。

## 验证与审查

- [ ] 针对重试/缓存/统计补聚焦测试，能便携运行的先 red 后 green；Android/eBPF 限制如实记录，便携测试不代替目标编译。
- [ ] 主线运行 `cargo check -p chiri --target aarch64-linux-android`，确认 Checking chiri；若工具链缺失，不自动安装，报告阻塞。
- [ ] ReadLints、git diff --check、核对无越界更改与临时文件。
- [x] 实现完成后派未参与实现的独立只读代理审查身份复用、缓存生命周期、真实 errno、父子账、不降质与测试盲区；测试缺参已修复并复审，panic 错线程疑虑经词法核对撤回。
- [x] 文档按统一口径更新：agentsdocs 契约和 TODO、Cursor 长期记忆和当日日志，注明设备收益未验证。

### 本轮验证结果

- 持久纯模块测试入口：`scripts/tests/rust_regression.rs`，新增两个真实模块，共 9 个聚焦测试；未执行。
- Android check 已尝试：仅见 Compiling chiri，eBPF 构建的 `-Z` 在 stable Cargo 被拒，未见 Checking chiri。
- 纯模块 rustc 测试：项目内 TMPDIR 只读，编译未启动成功；没有运行测试，没有 red/green 证据。
- `git diff --check` 通过；ReadLints 返回 totalFiles=0，不能替代编译。
- 主线终端 4 次，第 4 次用于最终文件范围与临时产物收口；实施 A/B 各 1/3 次，验证代理 3 次，其余代理 0 次。未留下本轮 scratch 或测试二进制，未暂存/提交。

终端每个代理默认最多 3 次；需要长输出时只追加各自一个项目内 scratch，工具删除清理。所有代理不暂存、不提交、不读原始日志、不改 CI/FAS/调频参数。
