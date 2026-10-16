【铁律】ChiRi 仓库开工前必先读其他人留下的工作记录：AGENTS.md（索引，勿通读正文）→ agentsdocs/ 对应子块（Grep [tag] 定位）→ 各 IDE 环境记忆 .cursor/memory/MEMORY.md、.codebuddy/memory/MEMORY.md（2026-09-13~09-23 历史长期事实，各环境互读不互写）、.trae/memory/MEMORY.md，再加最近几天的 YYYY-MM-DD.md 日志。用户 2026-10-11 明确要求永久记住这一习惯。
§
ChiRi 协作铁律：① 任何文件不得落在项目文件夹之外（2026-09-26 用户严令，零容忍）；AI 产出按环境分目录（Cursor：计划类→.cursor/plans/、报告类→.cursor/docs/；.codebuddy/.trae 同构），其他环境目录（.codebuddy/ .trae/ .workbuddy/ agentsdocs/）只读不写，跨环境只做对账。② 未经用户明确授权禁止创建 git 提交/推送（2026-10-07 铁律，曾因越权提交 a2ce07a 被撤销）。③ 永不再新增 A/B 对照实验（判据=逻辑必然+模型折算+功能回归观察；同版本同场景观察可以，并行/交错对照一律不做）。④ 性能优化不降采样质量红线：凡让数据变少/变粗的改动一律不做，行为延迟不算。⑤ 用户明确提出的目标不得用 AI 自选评估口径否决（2026-10-11 教训：误删「取消二段式息屏」被纠正）；评估负面时保留条目附意见，决定权交回用户。
§
ChiRi 工具与落盘口径：一次性分析脚本只要放在 devimpbin/ 文件夹内即可，无需指定子文件夹（2026-10-11 用户更正；devimpbin/ 被 .gitignore:41 忽略，Grep 目录级会跳过需显式给路径），禁落项目根；devimp 日志包分析走 `python scripts/devimp/dvrun.py <logd_*.tar.gz>`（产出落 devimpbin/<MMDD-HHMMSS>/），自耗分析用 scripts/devimp/dvselfcost.py（feas/diag/pair 三子命令，需 --manifest；dvrun.py 的 STAGES 不含它），一律从仓库根执行。Rust 验证：`cargo check -p chiri --target aarch64-linux-android`（host target 因缺 libc 符号失败；本机 build.rs eBPF -Z 仅 nightly 允许，stable 需 YUMI_SKIP_EBPF=1；日志「✅ 编译成功」可能是 eBPF stub 假象，「⚠ 跳过 eBPF 编译」才是真相）。脚本改过的文件必须重新 read 再 edit；整节删除不要用 edit 猜边界。
§
ChiRi 收口与汇报要求（2026-10-11 用户定）：收口/汇报前必做三重核查 list_agents + job_list + ps -eo pid,etimes,args，确认无活跃子代理与未收后台任务；派子代理的 prompt 必须写明终端调用上限、scratch 单文件路径、收尾清理要求与互斥写范围（曾有子代理覆盖主线程文档：.cursor/rules/parallel-agent-file-ownership.mdc）。汇报结构最低要求：问题→原因（含源码路径）→后果→影响（量化）→快速判定方法（用哪个 dv*.py 看哪一段）+改哪一行，缺一不可；结论数字必须标注口径与可复现脚本（例：main tick 行 screen_on 恒为 '-' 无法按屏筛选；fas 无 tick 行；同一指标多口径必须并列）。
§
ChiRi P1 落地状态翻转（critical）：P1 取消二段式息屏已落地（2026-10-12 c48a06c）：doze 段 4 行专用覆盖已删（src/chiri/mod.rs 原 :3725/:3728/:3729/:3730 及 :3321-3329、:4222-4233 同口径），息屏后 CLG 即 reduce 同参；五机型 feature.yaml scene_mode_delay_secs 300→3000，config.rs 缺省同步 3000（含 2026-10-05 7e14783 的预活）。回退 = 恢复 doze 覆盖构造 + YAML 改回 300。净收益口径沿用 logd_1011 定稿（保守 +0.36 / 中枢 +1.59 / 乐观 +3.08 Wh）。
§
审查口径（2026-10-13 用户定）：① 审查时必须避让他人正在进行的修改——只检查本会话自己改的部分（git diff 中自己负责的文件/行），不碰不评判他人 WIP 文件（如 src/chiri/config.rs、cpu_load_governor.rs、feature.yaml、clg_replay.rs 等并行改动）；② 审查默认不包含构建测试（cargo check / svelte-check / vitest 不属于审查动作，除非用户点名要求），审查 = 静态读 diff + 逻辑核对 + 必要的最小功能验证。
§
【铁律】ChiRi 代码改动后必须立即格式化改动的文件（cargo fmt / rustfmt），再收口或提交；CI 用 nightly 工具链跑 `cargo fmt --all --check`（.github/workflows/build.yml:100，且 fmt 前置在所有重编译之前），本机默认是 stable，改动后应两套工具链都跑一遍确认干净（2026-10-13 用户明确要求写入记忆）。
§
ChiRi 温控安全关闭（2026-10-16 用户要求，勿当回归）：全部软件温控已关闭并标注「安全关闭」，共三处需同关才叫温控全关——① module/config/{8475,8550,8650,8998,zumapro}/feature.yaml 的 Thermal.enabled=false；② 同段 tuned_resp_enabled=false（tuned 通道）；③ module/config/normal/fas/{dfm,endfield,lolm,sgame,speedmobile}.yaml 的 core_temp_threshold=0.0（FAS 通道，0=禁用是 d_temp_thresh() 既有缺省）。安全依据：内核 thermal 独立于 ChiRi（~95°C 起控 + LMh/DCVS），ChiRi 只做提前轻压，关掉不新增过热风险。恢复 = 三处改回 true/45.0，阈值与回滞参数原样保留。FAS 活跃时 src/chiri/mod.rs:3651 会整体跳过 ChiRi 热保护，故漏关第三处则游戏场景仍有温控。
§
ChiRi 窗口解析真机教训（2026-10-16，值得复用）：合成 fixture 的「字段布局」会掩盖真实 ROM 形态，新增窗口/dumpsys 解析逻辑一律用真机原文做夹具。实测（8550/PHB110 ColorOS）三条必须兼容：① dumpsys window 与 dumpsys -t 1 window windows 两条命令互补——前者有 mCurrentFocus/mFocusedApp 但无 mObscured/mViewVisibility，后者反之，只用后者则 Focused 恒不可达，只用前者则连 VisibleUnfocused/Hidden 都判不出；② mSession=Session{...} 是行内出现（mDisplayId=0 taskId=851 mSession=... mClient=...），按行首前缀匹配必然失败；③ AID 形式 u0aNNNNN 与同块 mOwnerUid=NNNNN 必须归一到同一 uid（appId 在文本里已是最终值，不得再加 10000，否则恒不相等 → 全落 unknown_owned → 恒 Unknown）。取证脚本 devimpbin/1016-001727/probe3.sh。
§
ChiRi 包1 fps 失效根因定案（2026-10-16 用户裁决）：= ChirunoNeko 自编内核问题（假设 B 实锤，假设 A「符号不在逐帧路径」撤销）。决定性证据=同机对照：同台小米 2210132C/同 A17，stock 5.15.194（devimpbin/1006-123730）fps 非空率 25.9~45.3%，ChirunoNeko 5.15.220（devimpbin/1016-151808）8 会话 0~0.08%，唯一实质变量=内核。修法方向（未实施）：内核侧 uprobe/BPF config 对 stock diff、帧静默 WARN+frame_src=silent 落标、FAS 开环档位死锁权衡。
§
ChiRi fps 探针验证环境坑（2026-10-16 一加/高通 5.15 实测，复验时必读）：①adb shell su -c 必须三层引号 adb shell "su -c '...'"，否则 su 未提权（id=2000）且报错被 2>/dev/null 吞；②高通 5.15 内核 uprobe 事件目录扁平化在 events/qb/（无 events/uprobes/ 前缀）；③per-event count 文件被裁（只有 enable/filter/format/hist/id/trigger），count 读 0 不可信，计数用 hist trigger（echo 'hist:keys=common_pid' > trigger 后读 hist 的 hitcount 列）；④手动验证读数通道坏 ≠ ChiRi 病症（两层「卡」勿混淆）。
