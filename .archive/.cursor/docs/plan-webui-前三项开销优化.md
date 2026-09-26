---
name: WebUI 前三项开销优化
overview: ① 存活探测并入 readMany（稳态 exec 2→1）；② daemon.log 尾部增量解析（记住已解析字节偏移，只解析新增行）；③ status.csv 增量解析（按行边界切增量，电池读数/日志页共用）。全程真机契约不变、界面行为不变。
todos:
  - id: liveness-batch
    content: ① probeLiveness 判定抽纯函数 + 并入 readMany（exec 2→1）
    status: completed
  - id: log-incremental
    content: ② daemon.log 尾部 endsWith 锚定增量解析
    status: completed
  - id: status-incremental
    content: ③ status.csv 增量解析 + appendStatusRows 裁剪
    status: completed
  - id: verify-batch
    content: 验证 type-check + vitest + 当日日志追加
    status: completed
isProject: false
---

# WebUI 前三项开销优化

## 现状

- [state.svelte.ts](webui/src/state.svelte.ts) `loadOverview` 每 tick 2 次 exec：`probeLiveness()` 独立 1 次 + `readMany` 1 次。存活判据 = 读 `LiveTime.chr`（`MM:SS\n`，64B）与本机时间比对（[daemon.ts:44-62](webui/src/contract/daemon.ts)）——它本来就是「读文件」，完全可以并进批量读。
- `loadLogs` 每 tick 读 daemon.log 尾部 64KB 后**整段** `parseDaemonLog`（[state.svelte.ts:952-959](webui/src/state.svelte.ts)）；status 视图每 tick 读 status.csv 尾部后**整段** `parseStatusCsv`（:977-980）。

## 改动（全部在 webui/，单代理顺序实施）

### ① probeLiveness 并入批量读（exec 2→1）

- [sources.ts](webui/src/contract/sources.ts)：无改动，`readMany` 已支持。
- [state.svelte.ts](webui/src/state.svelte.ts) `loadOverview`：
  - `readMany` 增加 `{ key: 'liveTime', path: absOf('liveTime'), tailBytes: 64 }`；
  - 删除 `probeLiveness()` 独立调用，把 [daemon.ts](webui/src/contract/daemon.ts) 的判定逻辑抽成纯函数 `judgeLiveness(raw: string | null): DaemonState`（含内容非法 → error 信息），放在 [data/live-time.ts](webui/src/data/live-time.ts) 旁或 daemon.ts 导出，`probeLiveness` 保留但改为内部复用（其他调用方不破坏）；
  - 映射：`liveTime` 为 null → `stopped`；批量读整体 failed → `unknown` + daemonError=batchError（与现 probeLiveness failed 口径一致）；内容非法 → `unknown` + 非法详情。
- 验收：稳态每秒 exec = **1**。

### ② daemon.log 增量解析

- [state.svelte.ts](webui/src/state.svelte.ts) 增加私有状态：`logTailCache`（上次 64KB 尾部原文）、`logByteTotal`（上次读到的总字节估算不可得——改用**内容锚定**方案：记上次原文与解析结果，新读到的尾部若 `new.endsWith(old)` 则只解析新增前缀行再拼接；否则（日志轮转/截断/尾部窗口滑出）整段重解析）。
- `loadLogs('daemon')`：ok 时先 `endsWith` 锚定判断；命中则解析 `new.slice(0, new.length - old.length)` 的新行并 `concat` 旧结果（保序：tail 从旧到新）；未命中或 absent/failed 走原整段路径。logState/logError 逻辑不变。
- 注意尾部窗口滑动场景：64KB 窗口内旧行会从头部滚出，`endsWith` 不命中即触发整段重解析，**正确性优先**——日志持续增长但每秒仅新增几行时（常态），新增前缀远小于 64KB，`endsWith` 大概率命中；命中失败率随写入量上升，退化为整段解析（同现状），无正确性风险。
- [data/daemon-log.ts](webui/src/data/daemon-log.ts) 的 `parseDaemonLog` 不改（纯函数复用）。

### ③ status.csv 增量解析

- 同锚定方案：`statusTailCache` + 新读尾部 `endsWith` 旧尾部 → 只解析新增行、`setStatusRows` 改为**追加**语义（新增方法 `appendStatusRows(rows)`：push 后同步维护倒序副本；行数超上限（如 720 行 ≈ 12 分钟）从头部裁剪，与现有展示一致）。
- `loadOverview` 里的 status 尾部读取（仅取末行 battPower）**不做**增量——4096B 解析本就 <1ms，保持简单；仅日志页/电池页的 `loadLogs('status')` 路径接入。
- [data/status-csv.ts](webui/src/data/status-csv.ts) 纯函数不改。

### 收尾

- 验证：`npm run type-check` + `npm run test`（终端 ≤2 次，PowerShell 原生 + UTF-8 落盘，单 scratch 用后删——沿用已验证的调用方式）。
- 追加 `.cursor/memory/2026-09-25.md` 当日日志：稳态 exec 2→1、两项增量解析、endsWith 锚定与退化路径说明。

## 不做的事

- 不改 mock-shell（dev 走查时 readMany 整体 failed 的已知缺口不变）。
- 不做导出轮询改阻塞 exec（需真机验证长命令回调，另行安排）。
- 不动 1s 节奏与界面表现。
