---
description: 解析与判定 ChiRi 设备的 devimp 日志包（devimpbin 下 logd_*.tar.gz），做功耗/调度回归分析、FAS 与热限频验证、场景画像（**不做 A/B 对照实验，2026-09-28 口径**）
argument-hint: [日志包/解压目录，或要分析的问题]
---

> 维护位置：`.cursor/commands/devimp-log-analysis.md`（本文件，唯一副本）；脚本套件在 `scripts/devimp/`
> （入口 `dvrun.py` 与 `scripts\devimp-run.cmd`；聚合表仍为 `scripts/devimp-analyze.py`）。
> **改动任何脚本后同步下方「预设脚本」节**。
> 由 skill 迁移而来（2026-09-23），`.agents/skills/devimp-log-analysis/` 已删除，勿再建 skill 副本。

本次用户输入：$ARGUMENTS
（为空则先问用户：要分析哪个 logd 包（`devimpbin/` 下哪个 `logd_*.tar.gz` 或已解压目录）？要回答什么问题？）

# devimp 日志包分析（ChiRi 专用）

设备端 Magisk 模块在「导出日志」时打包 `logd_<MMDD-HHMMSS>.tar.gz`，内含
`devimp_<包名>_<MMDD-HHMMSS>.log`、`status.csv`、`daemon.log`（可能还有内层 tar）。
本命令固化的是已重复验证过的分析流程与**判读口径**——数字很容易读错，先读「判定要点」再下结论。

## 一、五步流程

### 1. 解压（含内层 tar）

**解压/输出目录一律放项目内**（`devimpbin/<MMDD-HHMMSS>/`，`dvextract.py --out-root` 默认），不用 `%TEMP%`——临时产物不出项目树，
分析完可直接删（`devimpbin/` 已被忽略）。

```powershell
cd e:\code\ChiRi
# 单命令全流程（推荐）：解压 + 聚合表 + 四个探针 + report.md
scripts\devimp-run.cmd devimpbin\logd_XXXX-XXXXXX.tar.gz
#   → 产出在 devimpbin\<MMDD-HHMMSS>\ ：inventory.txt / analyze.txt / main.txt /
#     aff.txt / status.txt / report.md（输出目录就是解压目录）

# 也可分步跑（--only 选择阶段：extract,analyze,main,aff,status,power,energy）
python scripts\devimp\dvextract.py devimpbin\logd_XXXX-XXXXXX.tar.gz
python scripts\devimp\dvmain.py    devimpbin\<tag> [--since MMDD-HHMMSS] [--min-n 30]
python scripts\devimp\dvaff.py     devimpbin\<tag>
python scripts\devimp\dvstatus.py  devimpbin\<tag>
python scripts\devimp\dvpower.py   devimpbin\<tag> [--since MMDD-HHMMSS]
python scripts\devimp\dvenergy.py  devimpbin\<tag>
```

**内层归档格式 2026-09-25 起多了一种（重要）**：`module/scripts/pack.sh` 的 `archive`
分支现在把内层归档压成 **`.tar.lz4`**（`lz4 -f` 帧格式）；设备上没有 lz4 时**回落保留 `.tar`**。
所以外层包里的内层成员可能是 `<ts>.tar` / `<ts>.tar.lz4` / `devimp_<ts>.tar` /
`devimp_<ts>.tar.lz4` 四种，**扩展名还可能骗人**（实测出现过 LZ4 负载却叫 `.tar`）。
`dvextract.py` 一律按**前 4 字节内容嗅探**（`0x184D2204` 帧 / `0x184C2102` 遗留 / 否则当 raw tar）
并在 LZ4 解出后校验结果确实像 tar；**不要按文件名判断，手写解压脚本时同理**。本机未装
`lz4` python 模块，走 `scripts/devimp/dvlz4.py` 的纯 python 解码器。

解压与校验都交给 `dvextract.py`：它会写 `inventory.txt`（每个文件 + 大小 + 嵌套层 + 检出压缩），并核对每个 devimp 批次至少解出 `main_*`/`aff_*`、对缺 logd 侧的批次告警（老包降级口径见「五、已知坑」12）。

### 2. 现场判定（**目录名不可信**；先定版再比数据）

| 判什么        | 怎么看                                                                                                                                                                                                                                        |
| ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **模块版本**  | devimp 文件头三行：`# module=ChiRi Canary <ver> (versionCode N)`；`daemon.log` 的 `[Main] 模块版本:` 行（A06 起有）。**同一 tar 内出现多值 = 混版本，必须报警**                                                                               |
| **机型/系统** | 同处文件头：`# soc=… board=… model=…` 与 `# android= kernel=`。**机型或系统不同 → 功耗绝对值不可比**（实例：logd_0922-042407 = Canary92 / PHB110 / A16；logd_0922-130643 = Canary88 / MEIZU 21 Note / A15，同目录同天但不可比）               |
| 设备族        | devimp 文件名的包名：`miui.*`/`quicksearchbox` = 小米系；`coloros.*`/`heytap.*`/`salt.music` = OPlus 系                                                                                                                                       |
| schema        | devimp 首行表头：无 `cpu_cur_khz` = **40 列旧**（Canary92 前）；含 `cpu_cur_khz` 含 `from_core` = **48 列**（Canary92 期）；含 `cpu_cur_khz` 不含 `from_core` = **44 列新**（拆分版）；**44 列表头后还有一行 `# ts-column=local format_now`** |
| 版本行为特征  | `daemon.log` 启动段：`已导出 N 个 FAS 白名单条目`、`PowerBase`/`meta.yaml fields invalid` 等                                                                                                                                                  |

### 3. 选组（包内常混入多段旧日志）

按文件名 `_MMDD-HHMMSS` 排序取**最新一簇**；`status.csv` 尾行时间戳佐证。
daemon 可能中途重启过（启动序列日志为界）→ 只取重启后的连续段。
**44 列拆分版里「一簇」= 同一次导出的 main*\* + aff*\* 全套**；一个包里可能有 5~6 簇、
每簇覆盖一个前台包（本次 35 分钟包里 = kernelsu/launcher/bili/QQ/王者/skland/mt 共 7 包）。

### 4. 跑聚合脚本

```powershell
python scripts\devimp-analyze.py <解压目录> [--since MMDD-HHMMSS] [--min-n 30]
# 脚本只此一份（scripts\devimp-analyze.py）；本机 python 3.14 可直接跑
```

输出：按 `mode×package` 的 n / P_avg / p50 / p95 / battT / cpuT / cap / gpu / psi / mig

- little/big/prime 的 cap(avg/p95)+util + tgtop 线程占比 + 行类型分布。
  充放电方向自动判定（双向试探取合理功耗侧），不确定时会显式提示。

**按包功耗归因不要读这里的 `P_avg`**（它取 devimp 的 `batt_p` 列，本机 `batt_i` 整数化
→ 排除 `batt_i==0` 会删掉 <0.5A 轻载秒、系统性高估）：功耗归因一律走
`python scripts\devimp\dvpower.py <解压目录>`，权威源是 `status.csv` 的 `charge` + `batt_power_w`
（按 `mode×package` 与按包两张表 + 累计 Wh + 热档秒数），devimp 侧值只作交叉验证。

**脚本输出必须配合语境读**（v2 修正）：

- `fas` 行的 `little/big/prime` 恒为 `-`：FAS 不由 CLG/tuned 写 tick，**没有 tick 数据 ≠ 上限为 0**
- 44 列包没有 `tgtop` 行 → 「tgtop 线程占比」段恒空；线程 top 要去 aff 的 `@S` 帧看
- `mig` 列是 **2s 增量原始值**（脚本未除 2），换算每秒要 ÷2

### 5. 判定与对比

- 先答「与什么比」：**同场景、同版本、同 dev_record 状态、同亮度/音量**；跨包必须先核对版本与设备
- **不做对照实验（2026-09-28 口径）**：本项目**永不再新增 A/B**——允许的是**观察**（同版本、同场景、不并行对照、不控制变量，只记录现象与指标）；不允许的是**对照实验**（并行/交错对比两版本或两配置、以差值做判据，含"关/开某配置比功耗"）。替代手段：离线重放、模型折算、构造性自检（详见 `agentsdocs/04-hard-lessons.md` 的 `[hard]` 首条）
- **先看温度带**：batt ≥40℃ / cpu ≥60℃ 的段属热态，功耗与凉机不可直接比
- **FAS 验证要交叉两处**：`daemon.log` 的 `fas-gear-switch`/`gear_state`/`policy_controller`，与
  `status.csv` 的 `fps` 列（**只有 FAS 激活的秒有值**，其余是 `-`）
- 结论模板：场景表 → 与基线差值 → 归因（调度侧/热/GPU/环境）→ 是否可动 ChiRi 参数

## 预设脚本（首选入口）

反复手写的临时探针已固化为 `scripts/devimp/` 下的常驻脚本；本节是唯一入口说明。
（本节为新增，不影响后续「二、列索引速查」等既有编号。）

| 脚本           | 作用                                                                                                                             | 命令                                                                                  |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| `dvrun.py`     | **单命令全流程**：解压 → 聚合表 → 四探针（main/aff/status/power）→ `report.md`                                                   | `python scripts\devimp\dvrun.py <logd_*.tar.gz 或已解压目录>`                         |
| `dvextract.py` | 解压（递归容忍 + 内容嗅探 + 完整性闸门 + `inventory.txt`）                                                                       | `python scripts\devimp\dvextract.py <logd_*.tar.gz> [--tag T] [--out-root devimpbin]` |
| `dvmain.py`    | `main_*.log`：定版 / mode×package / decide-vs-actual / 热压制带 / snap 侧 / **播放态帧间隔直方图（`[7]`，活跃窗口 + 长尾率）** | `python scripts\devimp\dvmain.py <解压目录> [--since MMDD-HHMMSS] [--min-n 30]`       |
| `dvaff.py`     | `aff_*.log`：`@A` 动作与 bulk、`@S` 差分帧累积（按 `full=1` 收敛存活集合、tid 复用作废）、绑定轨迹、`t` 行核分布与存活集合合并态（`--groups` 按机型分簇） | `python scripts\devimp\dvaff.py <解压目录> [--since MMDD-HHMMSS] [--groups "0-2,3-6,7"] [--core-top 8]` |
| `dvstatus.py`  | `status.csv` + `daemon.log`：charge / 放电功率 / fps / FAS 证据 / 重启界标                                                       | `python scripts\devimp\dvstatus.py <解压目录>`                                        |
| `dvpower.py`   | **按包功耗归因**：`status.csv` 权威（按 mode×package / 按包 / **按 mode 能量汇总 `[7]`**）+ 热档秒数 `[4]` + **档位迁移时间线 `[5]` / 档位温度区间 `[6]`** + devimp 侧交叉验证 `[8]` | `python scripts\devimp\dvpower.py <解压目录> [--since MMDD-HHMMSS]`                   |
| `dvenergy.py`  | **功耗三层分解 + 外围基线剥离**（2026-10-05 新增）：`[1]` 总口/CPU 动态/残差三层能量账、`[2]` 同场景低负载分位的**外围基线**、`[3]` CPU 增量能量（调度改动该看这个）、`[4]` `batt_power_w ~ a + b·cpu_dyn_w` 回归（a=外围地板、R² 低说明总口被外围淹没）。依赖 status.csv 末两列 `cpu_dyn_w`/`resid_w`，老包（2026-10-05 前的 daemon）只报提示不产表 | `python scripts\devimp\dvenergy.py <解压目录>`（`--selftest` 自检）                  |
| `dvlz4.py`     | 纯 python LZ4 解码（供 dvextract 用；有 `--selftest`）                                                                           | `python scripts\devimp\dvlz4.py --selftest`                                           |
| `dvcommon.py`  | 共享工具（列定义、文件头解析、切行、统计、UTF-8 输出）                                                                           | （库，不直接跑）                                                                      |

- **Windows 一条命令入口**：`scripts\devimp-run.cmd devimpbin\logd_0925-045336.tar.gz`
  （等价于 `python scripts\devimp\dvrun.py …`，`%*` 透传全部参数）。
- `dvrun.py` 全程在一个 python 进程内跑（`scripts/devimp-analyze.py` 文件名带连字符不能 import，
  用 subprocess 捕获其 stdout 到 `analyze.txt`）；**单阶段失败不中断全链**，`report.md` 标注失败阶段。
- 输出（写进 `devimpbin/<tag>/`）：`inventory.txt` / `analyze.txt` / `main.txt` / `aff.txt` /
  `status.txt` / `power.txt` / `report.md`；`report.md` 末尾给出单独重跑任一阶段的完整命令。
- **每个脚本自己写 UTF-8 结果文件，stdout 只打几行摘要 + 路径**——PowerShell 重定向会把
  python stdout 按控制台代码页重编码、中文必乱码（已知坑 16），**禁止依赖 shell 重定向取全文**。
- `dvaff.py` 的「`t` 行核分布」段回答「某个核/簇上活跃的是谁」：**行数是采样行数不是并发线程数**
  ——2026-09-28 起 `t` 行改为槽级差分，行数口径变成「**有变化的行 + 每 30 帧的刷新行**」，
  **不可跨新旧格式直接比**；`u` 均值里长尾行是 ≤30s 窗口均值（短促占用被抹平）。要「某一刻谁在哪核」
  看「**存活集合合并态**」段（逐槽合并结果，不受省略影响）。簇分界不猜、按机型 DT 显式给
  （8550 `--groups "0-2,3-6,7"`，8475 分界不同，见 `06-kernel.md` 平台基准）；`--core-top` 限 1..64。
- **存活集合与 tid 复用口径**（与写入端契约对齐）：按帧头 `full=1` 的刷新帧收敛——连续两次刷新帧
  都没出现的 tid 视为已退出并移出统计（旧包无 `full=1` 则不收敛，行为与旧版一致）；`t` 行 **`pid`
  变化 = tid 被别的进程复用**，该 tid 的逐槽合并态整条作废重建，不沿用旧进程的 comm/core/pin；
  **无法解析的 `t` 行**计入报告与摘要的「无法解析丢弃 N」（槽名/槽序与写入端不一致时整行丢弃，
  N 突然变大先查帧格式，不要解读数据）。`--groups` 非法（非数字 / 端点倒置）与 `--core-top` 越界
  直接报错退出 2，不再静默产出空簇或反向切片。
- 判读口径一律以本文档「判定要点」为准，脚本只做读数与统计，不另立解释。

## 二、列索引速查

### 44 列（2026-09-23 起重写版）

| 列    | 名字                                            | 列    | 名字                                                      |
| ----- | ----------------------------------------------- | ----- | --------------------------------------------------------- |
| 0-3   | ts,type,mode,screen_on                          | 22-26 | touch,psi_cpu,psi_io,psi_mem,gpu_busy                     |
| 4-9   | pid,package,tid,comm,cluster,core               | 27-29 | batt_v,batt_i,batt_p                                      |
| 10-12 | **max_util**,over_cores,under_cores             | 30-32 | wakeups,migrations,freq_trans                             |
| 13-16 | cur_perf,tgt_perf,**cur_freq_khz,max_freq_khz** | 33-35 | batt_temp,cpu_temp,clg_active                             |
| 17-21 | decision,deb_up,deb_down,reason,thermal_cap_pct | 36-43 | cpu*cur_khz,cpu_max_khz,cpu_min_khz,cpu_governor,gpu*\*×4 |

- **`ts` 没有日期**，形如 `01:59:25.714`（`# ts-column=local format_now`）→ 过滤行用
  `^\d{2}:\d{2}:\d{2}\.\d{3},`，**不要用 `startswith("09")` 之类按日期判断**（会一行都匹配不到）
- 36-38 列是多 policy 串：`policy0:787200;policy3:1785600;policy7:595200`（0=little,3=big,7=prime）
- `snap` 行的 `wakeups/migrations/freq_trans` 才有值，`tick` 行是 `-`

### 48 列（旧，仍会遇到）

列 10-12 = `from_core,to_core,util_pct`，13 = `max_util`，18/19 = cur/max_freq，25 = `pinned`，
`thermal_cap_pct` 在 25/列为 21（44 列）——**其余列在 44 列版统一左移 3，末 8 列左移 4**。

### 行与文件

- 行类型：`tick`（决策轨迹，按签名变化写）/ `snap`（1s 环境）/ `tgtop`（30s，列 7=comm、12=占用率、13=累计 ms）/ `event`
- **44 列版起**：`place`/`aff`/`core` 三类行迁出到同目录独立 aff 文件（`AFF_HEADER` 1 列）；main 文件里没有它们。
  实测 44 列包的主文件**只有 `tick`/`snap`/`event`，没有 `tgtop`**
- **FAS 模式段没有 tick 行**（FAS 接管期间 CLG/tuned 都不写 tick），该段只有 `snap` + `event`
- `event` 行：`mode_change` / `thermal_change`（带 `batt=… cpu=… cap=… free=…`）/`activate`/
  `overload_hold`（FAS 亲和候选，带 `tid=… home=… cand=… score_…`）
- aff 文件（`AFF_HEADER`）：`@A` 动作帧（`act=pin|restore|move_group|cpuset_cpus|uclamp|corectl|bind_release|self_pin|self_unpin|elf32`，
  `result=ok|e{errno}`）、`@S` 每秒快照帧（帧头 `ntop`=进程行数、`nfg`=线程行数，刷新帧另带 `full=1`，
  后跟 `p`/`t` 行块）、`t` 行 `pid=0` 表示归属未知、`uclamp` 槽本版恒不变（预留字段）。
  **`t` 行 2026-09-28 起是槽级差分**（`t <pid> <tid>[ comm][ u=][ core=][ home=][ pin=][ uclamp=]`，
  未变槽写 `-`、尾部未变槽省略、全未变不落行）：**读时必须逐槽合并，缺省槽 = 沿用上次值**

## 三、判定要点（最易读错的地方）

- **`cur_freq_khz`/`max_freq_khz` 是调度器写入的 scaling_max 决策值，不是实际频率**；实际值看 snap 的 `cpu_cur_khz`。比率 = cap%
- **`deb_up`/`deb_down` = `up_wait`/`down_wait` 连续方向 tick 计数**（2026-09-24 源码确认）：
  升/降频速率限制计数器，非错误计数；「decision=down 但 cur_freq 不动」不要读成写频失败或假 down。
  **归因五层**（2026-09-26 真机内核验证，按概率排序）：
  ① `scaling_cur_freq` 是**票值**（qcom-cpufreq-hw 的 get 读 EPSS `reg_perf_state` 寄存器
  = 软件最后写入的票），硬件实际频率被压低时票也不动；
  ② LMh/DCVS 热压抬底：只注入调度容量热压 + 暴露 `dcvsh_freq_limit`/`lmh_freq_limit`，
  **不碰 policy->max**，票值与 scaling_max 读数都不变而实际频率被压低；
  ③ 实际 governor 是 waltgov（busy-hold + rate_limit）：down 决策只改 next_freq，
  落地要等下一次 util 更新且不被 hold；
  ④ FREQ_QOS 钳制（thermal cooling / msm_performance / input-boost）：此时
  `scaling_max_freq` 读数也会被拉低，可与 ②③ 区分；
  ⑤ 触摸地板（flush 里 `current_perf.max(floor)` 在决策之后）。
  排障指令：读 `dcvsh_freq_limit` / `lmh_freq_limit` 区分热压，
  `/sys/kernel/qcom-cpufreq-hw/print_cpufreq_debug_regs` 区分「票被改」vs「硬件压频」
  【临时旁证】devimp 新增 `clamp_change` 事件行（reason 内嵌 smax/dcvsh/corectl/msmp/horae 快照，
  变化才落行）自动采集上述旁证节点——**临时 instrumentation**，真机验证（device-todo T4/T5/T8/T10）
  完成后评估去留，验证结论沉淀回本节。

  **「QoS 钳制 vs 硬件热压 vs 决策未落地」三态对照**：

  | 状态                  | scaling_max 读数 | cur_freq 票值    | 实际频率   | 排障动作                                                                                   |
  | --------------------- | ---------------- | ---------------- | ---------- | ------------------------------------------------------------------------------------------ |
  | FREQ_QOS 钳制         | **被拉低**       | 跟随被拉低的 max | 与票一致   | 查 thermal cooling / `msm_performance` cpu_max_freq / input-boost（MIN 侧抬 FREQ_QOS_MIN） |
  | 硬件热压（LMh/DCVS）  | 正常             | 不变（仍高）     | **被压低** | 读 `dcvsh_freq_limit` / `lmh_freq_limit`；`print_cpufreq_debug_regs` 区分票改 vs 硬压      |
  | 决策未落地（waltgov） | 正常             | down 未写入      | 暂未跟上   | busy-hold + rate_limit 属正常，等下一次 util 更新；连续多帧不动再按 ②④ 排查                |

- `max_util`：CLG 行 = 平滑前原始 util；tuned（akmode/playback）行 = 平滑后决策负载——离线二次平滑前先区分来源
- `migrations`/`wakeups` = BpfStats **2s 增量**（源码 `src/common.rs` `DaemonEvent::BpfStats`，
  `cpu_monitor` 每 2s 发一次差分）：**÷2 = 每秒**
- 迁移率量级（2026-09-23 实测 Canary Alpha06-04 / 8550）：
  - bili 播放态 ≈4500/s（wakeup ≈7000/s），王者 FAS ≈9000/s（wakeup ≈16000/s）
  - 亮屏 UI（launcher/kernelsu）≈3000~6000/s
  - 早期记录的「视频稳态 ≈300/s」只在**无弹幕/网页面板、无滚动**的纯视频稳态下出现；
    本次包内 `@S` 帧显示 `tv.danmaku.bili:web` 与 `surfaceflinger` 同时活跃 → 属 UI 量级。
    **判定异常前先确认是否有弹幕/网页/滚动**，禁止拿一个数跨场景下结论
- `thermal_cap_pct`：对照 feature.yaml 的 `soft_perf_cap`（8550=0.85、8475/8998=0.70）；电池软限 41℃ 触发 / 38℃ 解除（`hysteresis_c=3`），中限 43℃ 触发 / **8550 41.5℃ 解除**（`hysteresis_mid_c=1.5`，其余机型 2）、硬限 45℃ 触发 / 43℃ 退回中档（`hysteresis_hard_c=2`）。**判读「热是否常态」时同时看档位迁移时间线**：中档与软档在 normalize 里受不变量「本级解除点不得低于下一档跳闸点」约束（`中限 − hysteresis_mid_c >= 软限`），2026-09-28 前中档解除点 40℃ < 软限 41℃，会造成「中档咬住后整段跳过 0.85 档」——见到 cap=60 长时间贴在中限以下 1~3℃ 的桶里，先核对当时的 `hysteresis_mid_c`
  - **`free_above` 是性能豁免档、不是温度带**：压制只落在 `(soft_perf_cap, free_above)` 区间，`>= free_above` 不钳制。**`cap=85` ≠ 已压制**——`free_above == soft_perf_cap` 时压制带为空、软限档完全空转（8550 曾如此，2026-09-23 修为 0.95），日志表现 = `cap=85` 期间写频仍可到 hw_max；热压制只作用于 CLG，tuned 段 cap 列 `-`
  - **实测**：batt 42.1℃ 触发 → `cap=85`（`thermal_change batt=42.1 cpu=56.4 cap=85 free=85`）
  - **daemon 重启会把热状态重置回 100**（新会话从零升温）→ 跨重启的 cap 序列不可直接连读。
    **一个包里 daemon 可能重启多次**（2026-09-27 实测：`devimp/` 约 4.6 h 内 3 次触顶 128MB 强制重启，
    包内 4 个批次各一段；daemon.log 的 `统一启动中` 是重启界标）→ 跨批次只取「各档累计秒数」，
    不要拼连续 cap 轨迹；重启后那段的 100 档占比偏高是重启产物，**不代表该段真没受过热压制**
- **热是常态还是偶发：看 `thermal_cap_pct` 各档的累计占用秒数**（分母 = 全部行），
  不要凭 `thermal_change` 事件条数或 cap 均值判断。2026-09-27 实测（44 列 / 8550 / 4.6 h 会话）：
  85 档 10422 s（64%）、100 档 5312 s（33%）、40 档 530 s（3.3%）、55/70 各 12 s（升档斜坡产物）
  → 2/3 时间在软限档 = **热是本会话的常态背景，不是偶发**。`dvpower.py` 的 `[4]` 段直接产出该表
  （含按批次明细；`thermal_cap_pct` 的浮点残差如 `70.00001` 已归一到整数档）
- 充放电：**不要用电流符号判定**（方向随内核/机型而异）；status.csv 有 `charge` 列（权威）。
  devimp snap 无 charge 列时用脚本的双向启发式，并在结论里注明。
  脚本口径：**严格排除 `batt_i == 0`**（无电流/数据缺失视为非放电样本，会略抬 P_avg）；
  输出 `[!] 两侧功率都合理` 时说明该机型方向需人工确认——结合 status.csv 的 charge 列人工核对一次
- **44 列 devimp 的 `batt_i` 疑似整数化**（2026-09-24 实测，值域仅 0/1/2/3；status.csv 同秒为
  0.106/0.889 等浮点，`batt_p` 列正常）：铁律「排除 batt_i==0」会误删约 58% 样本且恰是
  <0.5A 轻载秒 → P_avg 系统性高估（aweme 实测 3.35W vs 真值 2.14W）。此类包功耗**以
  status.csv 的 `charge`+`batt_power_w` 为准**，devimp 功耗列只做交叉验证
- **`batt_power_w` 是电池端总功率，含屏幕 / modem / GPU / 静态漏电这些调度层碰不到的外围**（2026-10-05
  立此口径）：按包归因时「这个包耗电高」常常只是「这个包亮屏久」。要判定调度改动的效果，走
  `scripts\devimp\dvenergy.py` 的三层分解——`cpu_dyn_w`（能效表折算的 CPU 动态项，只含动态、
  是下界）与 `resid_w` = `batt_power_w − cpu_dyn_w`（残差含外围 + CPU 静态 + GPU，**不是纯外围**）。
  读法：先看 `[1]` 的 CPU 占比（低 = 外围主导，别拿总口归因），再用 `[2]` 的同场景低负载分位得到
  外围基线，`[3]` 的增量能量才是调度该背的那份；`[4]` 回归的 R² 低即说明总口被外围噪声淹没。
  两列只在有功耗表的 SoC（当前仅 8550）有值，其余恒 `-`；充电行的 `resid_w` 无意义
- 模式语义：`clg_active=1` = CLG 在接管；`mode=fas` = FAS 接管；特调模式（playback/akmode）= tuned 接管；
  PowerBase 开启时替换 CLG（日志有 powerbase-activated）
- `freq_trans` 恒 0 而非 `-` = 内核无 cpufreq tracepoint（探针没挂上），不是「频率从未切换」
- **FAS「频率不匹配」判据**（`scheduler/fas/policy_controller.rs`）：只校验下沿——
  `actual < 可用频表中 <= 预期 的最大值` 才告警。**2026-09-23 起该告警多为自噪声**：
  旧实现写完立刻读 `scaling_cur_freq`，内核调频异步，读到的是写入前的旧档（实测 55 次的实际值
  全部 = 旧档、近旁 snap 的 `cur==max`、帧率达标）；已改为写后延迟 200ms、下一次写入前抽查。
  **判读顺序**：先看同一时刻 snap 的 `cur==max` 与 fps，再看告警——三者一致且 fps 达标时，
  不要把告警解释成「被 thermal/QoS 压制」（修复前的老包仍会看到 6%~26% 的『偏低』）
- **`scaling_governor` 写入失败（`os error 13`）已于 2026-09-28 定案并修复**：原判读「内核本就是
  schedutil、意图已满足」**不成立于 FAS 路径**——2026-09-28 包实测 FAS 段 snap 的 `cpu_governor`
  就是 `performance`（`cur==max` 100%），而 `GovernorGuard` 的成功行 `governor-switched` 在**全部已收
  daemon.log 里 0 次**。根因：FAS 引擎用 `utils::try_write_file` 写同一节点，它会先 chmod 0664、
  **写完 chmod 0444**，而 guard 侧是裸 `fs::write` → 引擎写过一次后 guard 必然 EACCES（FAS 期的
  performance 全靠引擎兜住）。现统一走 `utils::write_sysfs`（写前 `enable_perm`、不收尾改权限、失败读回
  校验），引擎侧 3 处写入一并换用。判读：改造后应看到 `P{pid} 调速器 schedutil -> performance` 成功行；
  若仍失败，日志会带出「读回已是目标值」的 debug 而不是 warn
- FAS 验证看 `daemon.log` 的 `fas-gear-switch` / `fas-low-perf-upgrade` 与 snap 的 `mode=fas`；
  `status.csv` 的 `fps` 列是 eBPF uprobe（`Surface::queueBuffer`）帧间隔，只在 FAS 段有值
  （播放态不填该列，改走下面的 `playback_fps` 行）
- **播放态帧信号（2026-09-27 起）：看 `event` 行的 `decision=playback_fps`**
  （只在该包里特调 `playback` 接管、且 `dev_record` 开启时才有；FAS 会话不产出——那一路走 FAS 自己的 fps）。口径：
  - `reason` = `n=<帧数>;b<档号>=<计数>;…`：档宽 **4ms**、档 0 = <4ms，只落非零档
    （档号稀疏，按 `bin × 4ms` 还原帧间隔）。**实际覆盖上限只有 200ms**（`fps_monitor.rs` 的
    `MAX_FRAME_NS`）：>200ms 的帧间隔被**整帧丢弃**、既不进直方图也不进 `n`（2026-09-28 审查发现
    写入端档表虽到 1020ms，但过滤在更前面）→ 档号 51+ 恒空，长尾率的**分子分母都不含 >200ms 的
    严重卡顿**；故「无长尾」只能读作「无 52~200ms 级卡顿」
  - **1 秒一行，窗口内无帧则整行不落**（暂停/静态页 = 无帧）→ 行数 ≈ 播放秒数，可直接当「在播时长」用
  - **同一行是混流的**：探针挂在进程级 `Surface::queueBuffer`，视频层（24/30fps）+ 弹幕层（120Hz）
    的帧间隔落在同一个直方图里。判读先找**占优簇**反推内容基线（bilibili ≈ 档 10 一堆 + 档 0/1 一堆），
    再看是否出现超出基线的长尾档（真掉帧）。**不要对 n 求均值当 fps**
  - 本质是离线判据（在线不判 jank、不写 fps 列）：`n` 只用于交叉验证「这一秒确实在出帧」
  - **汇总看 `dvmain.py` 的 `[7]`**（2026-09-28 起）：只统计 `n >= 15` 的**活跃窗口**，给占优簇 +
    长尾率（>52ms / >100ms）；伪影窗（暂停/静态页/切场景）只计数、不进判据。判「热压制是否伤播放」
    必须**按时间窗切分再比占优簇**（`[7]` 是整包聚合，跨内容不可比）
  - **本探针的存续条件（2026-09-28 评估）**：它的唯一产物是这份离线直方图。立项理由「热压制是否伤
    播放」已在 `logd_0928-170828` 上答完（中/硬档窗口的长尾率与未压制基线同量级：0.0~0.4% vs 0.0%）。
    之后只有**要复核播放态参数**（`tuned_profiles.yaml` 的 hysteresis/down_hold）时才需要它
    （`tuned_thermal_floor` 已于 2026-09-28 定案**不下探**：该地板只在硬档生效，而硬档窗口的有效上限本就是它，
    「无长尾」不能推出「更低也行」）；该条台账关闭后按仓库惯例把旁路入口注释掉（`// [PAUSED]`，勿直接删代码）。
    已知数据质量瑕疵：同 app 多进程会交替占坑（实测同秒内 `9913→18144→9913` 反复 detach/attach，
    伴随 `PID 切换失败: perf_event_open failed`）→ 直方图实际只覆盖当前挂着的那个进程

## 四、基线参考（会随固件变化，仅作量级对照）

| 场景                               | 量级                      |
| ---------------------------------- | ------------------------- |
| 待机（息屏/无前台）                | 0.4~0.8 W                 |
| 视频播放（bili，playback 特调后）  | 2.1~2.8 W                 |
| 音乐播放（本地/在线的 media 场景） | 2.2~3.4 W（GPU 高则偏上） |
| 亮屏 UI（抖音/搜索/桌面）          | 1.9~2.4 W                 |
| 聊天（QQ/微信）                    | 2.0~4.0 W（热态更高）     |
| MOBA（王者，FAS 接管后）           | 3.0~4.7 W                 |
| 重负载游戏                         | 7 W+                      |

2026-09-23 实测（Canary Alpha06-04 / 8550 / PHB110 / A16，热态 batt 37→42℃、cpu 46→78℃）：

| 场景                  | 时长            | P_avg                            | 帧率                                             | 备注                                              |
| --------------------- | --------------- | -------------------------------- | ------------------------------------------------ | ------------------------------------------------- |
| 王者 FAS（120 档位）  | 10.75 min       | **4.32~4.34 W**                  | **121.3 avg / p50 122.2 / p95 124.3 / min 43.6** | cpuT 均值 62.4℃、峰值 78.1℃；命中 55 次频率不匹配 |
| bili playback         | ≈21 min（4 段） | **2.78 W**（cap85 段 2.5~2.7 W） | 无（非 FAS）                                     | cpuT 52.7℃；playback 把 prime 上限压到 ≈648 MHz   |
| kernelsu（default）   | ≈7 min          | 2.8~3.1 W                        | 无                                               | 管理器界面，GPU 空转多                            |
| launcher/QQ/mt/skland | 各 0.5~1.5 min  | 1.0（桌面）~6.3 W（QQ 尖峰）     | 无                                               | 样本 20~30 行，只作参考                           |

## 五、已知坑

1. **目录名不可信**——`devimpbin/8550/` 里出现过 MIUI 8475 设备的包；用包名特征 + SoC 参数形态判
2. **包内混装旧日志**——永远按时间戳筛最新组再下结论
3. **status.csv 可能只有几十行**（daemon 刚重启）——此时只用 devimp；daemon.log 的启动序列是重启界标
4. **统计输出勿截断后计数**——`Select-Object -Last N` 会让「文件数/总改动」失真（踩过）
5. PowerShell 内嵌 python 易被引号/`$` 转义吃掉——**一律写成 .py 文件再跑**；
   pyyaml 在部分环境损坏 → 校验 yaml 用 `node` + `webui/node_modules/js-yaml`
6. 热态日志（battT 长期 40+℃）会整体抬高功耗——**只作状态描述**；不做凉机/热机对照实验（2026-09-28 口径：不新增 A/B）
7. 对功耗结论保持怀疑：先确认「是不是同一台设备、同一个版本、同一类场景」
8. **PowerShell 会吞掉 python 的长 stdout**（本次 200 行以上被截断）→ 让脚本把结果写进
   `<tmp>/probe*.txt` 再用 read_file 分段读；控制台里 `Get-Content` 中文会显示成乱码，但文件本身是好的。
   **管道重定向也不保险（2026-09-24 两度截断）——用 `cmd /c "python ... > file"` 才完整**
9. **`search_content`/ripgrep 搜不到 `daemon.log`**：本仓库 `.gitignore` 忽略 `*.log`，rg 默认尊重 gitignore
   → 分析日志一律用 python 读文件或 `Select-String`（`-Encoding UTF8`）
10. **daemon.log 文本里有 U+2068/U+2069 隔离符**（`P⁨0⁩`、`⁨playback⁩`）：写正则前先
    `re.sub(r"[\u2068\u2069]", "", s)`，否则 `P(\d+)`、`mode=⁨…⁩` 之类匹配全部失败
11. **`@A result=e0` 不是「成功」**：`affinity::io_result_tag` 用 `raw_os_error().unwrap_or(0)`，
    拿不到 errno 时写 `e0`；`e3`=ESRCH（线程已退出，正常）、`e22`=EINVAL（偶发）。
    **`e22`/`e3` 还有 walt_halt 来源**（2026-09-26 内核验证）：WALT core_ctl 可 pause CPU
    （`walt_halt_cpus`），被 halt 的核被所有选核路径避开、setaffinity 到它会失败——
    ChiRi 绑核到被 halt 的核是 `e22`/`e3` 的候选来源，归因前先看 core_ctl 状态；
    另外 `getaffinity` 返回掩码会**扣掉 halted 核**，观测掩码 ≠ 真实 allowed
12. **外层包只有 `devimp_*.tar`、没有 `<ts>.tar`（daemon.log/status.csv）**：2026-09-24 之前的
    logd 预算清理 bug 产物（超大 devimp 归档把同批 logs 侧小 tar 挤掉，实例 logd_0924-173105），
    已按批次原子清理修复；老包按「缺 logd 侧」降级口径（无 fps/FAS/charge/PowerAVG）判读
13. **`@A` 清理帧 2026-09-24 起改为汇总（aff 减量）**：逐条 `bind_release` 只代表**真有内核动作**的条目
    （pin/move_group/restore 等）；「只被扫到、从未动手」的条目清理时汇成一条
    `<场景>_bulk`（`stale_bulk`/`gone_bulk`/`departed_bulk`/`release_bulk`/`disabled_bulk`，`value`=本次条数）。
    统计清理规模要读 bulk 帧累加，别再按逐条 `bind_release` 计数
14. **`@S` 帧的 `t` 行是差分的，2026-09-28 起进一步收到「槽级」**（压 devimp 体积，实测 `t` 行
    降到旧格式的 26~32%）。两代口径都要认识，读包先看有没有 `-` 占位 / `full=1`：
    - **2026-09-24 起（行级差分）**：前台线程与被管条目每帧全量，长尾只在变化时落行。
    - **2026-09-28 起（槽级差分）**：`t` 行只写本次变化的槽，未变槽写 `-`、**尾部未变槽整段省略**，
      整行全未变则不落行；`p` 行仍每帧全量。**缺省槽 = 沿用上次值，绝不能把 `-` 当字段值**；
      **槽真值为 `-` 时写入端写 `NaN`**（只有 `comm` 取得到 `-`），读者还原为 `-`，否则
      「comm 变成 `-`」会被当成「comm 没变」而永久吞掉。
      帧头带 `full=1` 的是刷新帧（首帧 + 每 30 帧，t 行全量），据此重建存活集合；连续两次刷新帧
      都不出现的 tid 才算已退出（粒度 = 刷新间隔）。`dvaff.py` 已按此实现（按 `full=1` 收敛 + `pid`
      变化即作废该 tid 的合并态 + 「无法解析丢弃 N」计数）。
    - 共同点：**「行数 ≠ 线程数」**，必须跨帧累积（`dvaff.py` 已按逐槽合并实现，新旧包同一套解析）；
      刷新帧内长尾 `u` 是 ≤30s 均值。另：`t` 行 `pid` 已由 `/proc/<tid>/status` 的 Tgid 补全
      （旧包的 54% `pid=0` 属老版本行为）；诊断开关重开后首帧必为全量（新会话重新建档）。
    - **别从半截开始读**：刷新帧节奏跟着**进程内帧号**走，`aff_` 文件写满 128 MB 会换新文件继续写，
      所以**新文件的开头不保证是刷新帧**（且旧文件可能已被预算清理掉）。从会话中段（或其后的轮转文件）
      开始读，头 ≤30 s 只能拿到残缺状态——`final_bound`/末态直方图不受影响（它们在文件末尾），
      要完整重建就等下一个刷新帧或从会话第一个 `aff_` 文件读起。
15. **内层归档 2026-09-25 起可能是 `.tar.lz4`**（`pack.sh` 的 `archive` 分支 `lz4 -f`；
    设备无 lz4 时回落 `.tar`）：外层包内层成员四种形态都可能出现，且**扩展名可能骗人**
    （实测 LZ4 负载叫 `.tar`）。一律**按前 4 字节内容嗅探**，别按文件名判；本机没装 `lz4`
    python 模块，走 `scripts/devimp/dvlz4.py` 的纯 python 解码器（`dvextract.py` 已封装）。
16. **PowerShell 重定向会毁掉 python stdout 的中文**：按控制台代码页重编码，产出 mojibake
    （2026-09-25 实测）。预设脚本因此**自己写 UTF-8 结果文件**（`main.txt` 等），stdout 只留
    短摘要；分析这些脚本的输出**看文件，不要看终端回显**。
17. **daemon.log 是 fluent 本地化文案，不是 key 字面量**（2026-09-26 实测）：搜 `fas-qos-clamp`、
    `clampev-node-missing`、`corectl` 这类打点 key **恒零命中且是假阴性**——log::warn!/info! 输出的
    是 `i18n/zh.ftl` 翻译后的文本（如 `QoS 钳制`、`档位切换`、`core_ctl` 带下划线）。
    检索前先查 `module/config/i18n/zh.ftl` 拿中文原文再搜
18. **Grep 工具对部分 daemon.log 彻底读不动**（2026-09-26，`x_0926-162614\daemon.log` 13.4MB）：
    显式路径、强制命中串（如必在首行的 `chiri`）也零命中——编码/控制字符导致 rg 匹配失效。
    **对该文件的任何 Grep 零命中一律按「未检索」处理**，检索必须走显式解码脚本
    （PowerShell StreamReader / python），并先用必在串做 positive control
19. **`clamp_change` 事件行（main log，P1-2 旁证）判读口径**：2s 热块采样、**变化才落行**
    （落行密度≈smax 变化密度，不是固定节拍）；reason 字段值域：
    `msmp_min/max=-` = msm_performance 参数节点读失败/空（真机可为常态缺失）；
    `horae=1` = `/proc/horae_qmi` 存在；`dcvsh=policyN:X` = `dcvsh_freq_limit`（簇首核），
    恒等满频即无硬件 DCVS 降档；`corectl=min/max/enable` 三值斜杠分隔（**字段序固定**，
    enable=0 时 min 摆动不生效）；`smax` = scaling_max_freq 读回（P1-1 三级判定的读回源）。
    节点缺失首见会 warn 一次（文案 `佐证节点不可用`），之后静默记 `-`
