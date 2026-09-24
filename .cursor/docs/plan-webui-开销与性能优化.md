---
name: WebUI 开销与性能优化
overview: 把总览/日志页每秒约 9 次内核桥 exec 合并为 1~2 次、静态数据退出秒级轮询、WebView 后台时暂停轮询，并做渲染层微优化；全程不改守护进程、不改 1s 刷新节奏与界面表现。
todos:
  - id: batch-read
    content: 批次 A：contract/sources.ts 新增一次 exec 的多文件批量读 readMany（单文件缺失不失败）
    status: completed
  - id: poll-diet
    content: 批次 B：state.svelte.ts 轮询瘦身——活跃项走批量读、静态项移入 loadStatic（进入/写后刷新）、document.hidden 门控、statusRowsNewestFirst 改 derived
    status: completed
  - id: views-poll
    content: 批次 C：OverviewView/LogsView——hidden 跳过 tick、日志尾部窗口 128KB→64KB
    status: completed
  - id: verify
    content: 批次 D（主线）：type-check + vitest + build + 浏览器走查，一个 scratch 文件落盘核对后删除，更新当日日志
    status: completed
isProject: false
---

# WebUI 开销与性能优化

## 现状与结论

- 最大开销在轮询：[OverviewView.svelte](webui/src/views/OverviewView.svelte) 与 [LogsView.svelte](webui/src/views/LogsView.svelte) 各挂一个 1s `setInterval`。每次 `loadOverview()` 经内核桥并发发起 **约 9 次 shell exec**（每次都是 `su -c` fork）：`probeLiveness`、`readCurrentModeRaw`、`readSpecialTunedRaw`、`readFasWhitelistRaw`、`hasActionScript`、`readPowerAvg`、`deviceKind`、`readModulePropRaw`、`readMeta`；[state.svelte.ts](webui/src/state.svelte.ts) 的 `loadOverview` 可见这些调用全在同一个 `Promise.all`。
- 其中大量是**静态/准静态数据**（module.prop、meta、特调白名单、FAS 白名单、action 脚本存在性、设备型号），每秒重读纯属浪费。
- 轮询在 WebView 切后台后照常跑（无 `visibilitychange` 门控）。
- 渲染层：`statusRowsNewestFirst` getter 每次求值都 `[...].reverse()`；LogsView 每秒读 128KB 尾部并整段重解析。

## 改动方案（按文件所有权切分，可并行）

### 批次 A：contract 层批量读（独占 `webui/src/contract/sources.ts`）

- 新增一个批量读入口（如 `readMany(paths): Promise<Record<rel, string|null>>`）：用**一次** `sh -c` 把多个小文件 cat 出来、以约定分隔符区分，单文件缺失不算失败（映射为 null）。参考现有 `readTail`/`run` 的错误三分类（[sources.ts](webui/src/contract/sources.ts)、[errors.ts](webui/src/contract/errors.ts)）。
- 保持现有单读函数不动（别处仍在用），批量入口只服务秒级轮询路径。
- 禁改清单：`state.svelte.ts`、所有 views、`export.ts`、`meta.ts`。

### 批次 B：状态层轮询瘦身（独占 `webui/src/state.svelte.ts`）

- `loadOverview` 拆两类：
  - **秒级活跃项**（走批次 A 批量读，1 次 exec）：liveness 探测、current_mode、powerAvg、status.csv 尾部（4096B）。
  - **静态项**（退出秒级轮询）：module.prop、meta、特调/FAS 白名单、hasActionScript、deviceKind —— 移入新的 `loadStatic()`，仅在首次进入、页面切换回来、以及**任意写操作成功后**调用一次。
- 新增轮询门控：`document.hidden` 时跳过 tick（`visibilitychange` 恢复时立即补一次 `loadOverview`/`loadLogs`）。
- `statusRowsNewestFirst` 改为 `$derived` 缓存，避免每次求值重建数组。
- 契约（A↔B 接口）：批次 A 只交付 `readMany` 的签名与语义，由主线在分派时写死，两批不改对方文件。
- 禁改清单：contract/ 全部、views/ 全部。

### 批次 C：视图层（独占 `webui/src/views/OverviewView.svelte`、`webui/src/views/LogsView.svelte`）

- OverviewView：interval 回调里跳过 `document.hidden`；静态项不再依赖每秒刷新后的重渲染（数据源变化由批次 B 保证）。
- LogsView：daemon.log 尾部窗口从 128KB 降到 64KB（界面本就只展示尾部若干行）；仅 `source`/首次进入时全量读，秒级 tick 沿用在飞守卫。
- 禁改清单：state.svelte.ts、contract/、components/。

### 批次 D（主线收口，不派子代理）

- 按并行所有权规则：三批并行，交付时各自报告改动面与需同步的口径；`docs/agents/`、`.cursor/memory/MEMORY.md` 只由主线统一更新。
- 验证（终端 ≤3 次，一个 scratch 文件，收尾删除）：
  1. `npm run type-check` + `npm run test`（vitest 全量）+ `npm run build`，输出落盘一个 scratch 后 Read 尾部核对；
  2. `npm run dev` 浏览器走查四屏（mock-shell）确认刷新节奏与显示不变；
  3. 修复若需第二轮构建，计入预算。
- 更新当日日志 `.cursor/memory/2026-09-25.md`（追加）。

## 预期收益

- 稳态每秒 exec 数：约 9 次 → **1 次**（前台可见时）；后台 0 次。
- 静态文件 IO 与 YAML 重解析每秒归零，仅在进入/写后发生。
- 日志页每秒解析量减半，渲染层去掉重复数组重建。

## 不做的事

- 不改守护进程、不改 status.csv/mode 文件格式（纯 WebUI 侧）。
- 不动 1s 刷新节奏与任何界面文案/布局。
- 不含 bundle 优化（js-yaml、mock-shell 剥离等维持现状，mock-shell 已仅浏览器 dev 生效）。
