# DSH 环境长期事实（DeepSeek Harness）

> 与 `.codebuddy/memory/MEMORY.md`（历史统一口径，只读）并存；本文件只记 DSH 会话需要长期带走的事实。
> 文档权威：`agentsdocs/01~07` 是活文档（`07-todos.md` = TODO 唯一权威，`06-kernel.md` = 内核机制权威摘要）；
> 文件名含日期的 md 是历史快照，不回改，靠新增订正条处理。

## 协作契约
- **未经用户明确授权，禁止 `git commit` / `git push`**；改完只落工作区并汇报。
- 任何文件不得落在项目文件夹（`/home/aibeto/code/ChiRi`）之外；历史记录性 md 删除前必须先在 `.archive/` 留镜像。
- 本环境记忆目录 = `.dsh/`：`memory/YYYY-MM-DD.md` 按日追加，`memory/MEMORY.md`（本文件）就地更新；`docs/` 放计划与报告。
- 编译校验口径：`YUMI_SKIP_EBPF=1 cargo check`（agentsdocs/03-chiri.md:253）；WebUI 校验在 `webui/` 下 `npm run type-check` / `npm test` / `npm run style-check`（`npm run verify` 会因 `npx` 拉 node 在离线沙箱 EROFS 失败，属既有问题）。

## 2026-10-31 三条改动后的现行口径
- **亮屏判定**：`screen_on_value`（默认 **2**，等于即亮屏；其余取值含 3/4 doze 子态一律息屏；属性缺失按亮屏兜底）；
  取值不设范围、不再 0/1 夹取；旧键 `screen_off_value` 仍被解析并按 0→1、其余→2 迁移。原子量 `common::SCREEN_ON_VALUE`，
  `Config::load` 同步。41 份 `module/config/**/meta.yaml` 已换新键；`customize.sh [screen-detect]` 只在安装期实测 1 时改写。
- **释放兜底**：`src/chiri/max_freq_pin.rs` 在 CLG 看门狗 stale-release 后钉 `min=max=hw_max`；四个接管入口先 `unpin()`
  （CLG init/reload、FastLock::init、Tuned::init_policies），防止把钉频值拍进系统快照导致永久钉最高频。
- **SceneMode**：与 reduce 同口径，不再下线核心、不独占大核、CPU0 留在正常迁移集；只抑制 boost，频率压制由 CLG
  scenemode 档承担。`CoreCtlConfig.scenemode_offline` 已废弃（`#[allow(dead_code)]`），core_ctl 下线机器保留待删
  （见 `agentsdocs/07-todos.md`「清理与回退」）。

## 历史取证档案（结论可直接引用）
- minidump 异常重启：`devimpbin/1031-kmsg/FINDINGS.md`（vWdog 206 tick ≈6.39 s 真超时；s2idle 未 power collapse）。
- AirPortal 对照：`devimpbin/1031-portal/FINDINGS_chiriscene_crosscheck.md`（屏幕轴判错 16:18:08–16:38:34；CLG 释放后 221 s 无接管）。
