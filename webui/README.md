# chiri-webui

ChiRi 内核调度模块的 WebUI（KernelSU / Magisk 模块管理界面）。
Svelte 5（runes）+ TypeScript + Vite，**玻璃拟态 / Nocturne Glassmorphism** 观感，无 UI 组件框架依赖。
设计规范见 `DESIGN.md`（token、原子类、组件契约、红线）；风格原始资源见 `stylekit/glassmorphism/`。

## 分层

```
ksu.exec → kernel/shell（可注入） → contract（契约层） → data（数据层） → views（界面）
                  ↘ dev/mock-shell：无设备时的替身（浏览器预览 / 自动化走查）
```

| 目录 | 职责 |
| --- | --- |
| `src/kernel/` | KernelSU 桥（`ksu.ts`）与可注入的 `ShellRunner`（`shell.ts`） |
| `src/contract/` | 与守护进程的接口：路径与安全校验、三分类读取、meta 读写、实验室状态读写、存活探测、关闭调度 |
| `src/data/` | 纯解析：status.csv（27 列，`daemon_utime_ms`/`daemon_stime_ms` 为 daemon 自测量基线，末两列 `cpu_dyn_w`/`resid_w` 为功耗分解）、daemon.log、白名单、rules、模式派生、应用标签、实验室状态 |
| `src/app.css` | 玻璃拟态设计系统：`[fonts] [scene] [tokens] [base] [glass] [primitives] [signal-map] [utilities] [fallbacks]`。夜景场景/玻璃三层结构/原子类全在这里，色值只在此处定义 |
| `src/views/` | 四主屏：状态总览 / 配置 / 应用与规则 / 日志；二级页：实验室 `#/lab`、电量 `#/battery`、高级 `#/advanced` |
| `src/components/` | 面板、存活芯片、状态占位、开关、数值输入、分段切换、确认弹层（原生 `<dialog>`） |
| `src/i18n/` | 界面文案（界面语言与 daemon 日志语言互不影响） |
| `src/stylekit/` | 上游 StyleKit 导出的风格包留档（全局 CSS / tokens / Tailwind preset / SKILL.md） |
| `tests/` | 契约层与数据层的纯逻辑断言（vitest，可脱离真机运行） |

> `WebUI-old/` 是重构前的 ak-ui 版本留档（2026-10-15 起停更，仅供对照）。

## 脚本

```sh
npm install          # 安装依赖
npm run dev          # 本地预览（无 ksu 时自动装载设备替身）
npm run build        # 产出 dist（xtask 拷进模块 webroot/）
npm run type-check   # svelte-check（--tsgo）
npm run style-check  # 玻璃拟态风格闸门（扫禁止项/必须项，防风格漂移）
npm test             # vitest
npm run verify       # style-check + type-check + test + build 一把过
```

`dev` / `build` 前会自动执行 `scripts/fetch-fonts.mjs`（npm 的 `predev` / `prebuild`）：
从 jsdelivr 拉取字体并落地到 `src/assets/fonts/`（二进制不入库，已在 `.gitignore` 忽略）。
字体分工见 `src/app.css` 的 `[fonts]`：拉丁/数字走 Poppins（几何主义）、中文走 Noto Sans SC
（人文主义）子集、等宽走内置 JetBrains Mono。脚本按仓库实际用字裁中文（只留 CJK 码位，
拉丁交给 Poppins，故无需 `unicode-range`），新增中文文案后重跑构建即可覆盖新字。

## 与守护进程的契约要点

- **存活判据**：心跳文件 `LiveTime.chr`（daemon 每 15s 写一次本地时间 `MM:SS`），与本机时间比对，
  超容差（20s）判「已停止」，文件缺失同判；读取失败/内容非法才报「无法判定」。
  差值按 1 小时取模。daemon.lock 仍由 daemon 自持（单实例），WebUI 不再读写它。
- **只读接触点**：`active_config.chr`、`current_mode.chr`、`pf.chr`（CLG 是否启用帕累托前沿落点：
  `1` = 启用，仅 8550；家族名据此显示 `CLG-PF`）、`rhine-back.chr`、`PowerAVG.chr`、`LiveTime.chr`、
  `rules.yaml`、`special_tuned.yaml`、`fas_whitelist.yaml`、`module.prop`、`logs/**`。
- **可写接触点**：`meta.yaml`（写入口 = `contract/meta.ts::WRITABLE_FIELDS`，当前 16 项：language /
  loglevel / dev_record / fas_enabled / scenemode_enabled / powerbase_enabled / power_avg / notify /
  oplus_chg / oplus_dual_cell / voltage_double / current_double / voltage_divisor / current_divisor /
  power_max_w / screen_off_value），`rhine.chr`（实验室状态，写模式 key、空内容或保留字
  `off` 强制关闭），以及 `down.chr`（DOWN 停摆，写 `down` 开、写空内容关）。
  都做「tmp→rename」的原子写；meta 还需顶层行替换以保住注释，且都要写后回读——守护进程会在
  文件非法时整体重置。
- **实验室（`#/lab`）**：状态文件 `rhine.chr` 不存在或只有注释都算未启用（不是错误）；
  关闭实验室写空内容而不是删文件；锁定期间关闭要写保留字 `off`（强制关闭，走确认弹层）。
  模式列表在 `data/lab.ts` 里硬编码，英文名取明日方舟官方译名
  模式名一律用英文（中英两套 locale 同值、不分语言）：Vector Breakthrough / Contingency
  Contract / Babel / Frozen；中文只出现在各模式的 `.detail` 描述里。
- **实验室接管**：`data/lab.ts::LAB_TAKEOVER` 列出各模式会写的 meta 开关（与 rhine-init.yaml
  同步，单测有刚性断言），配置页把被接管的开关**置灰不可切换**并在提示里点名原因。
- **导出历史日志**（状态页底部）：设备上后台跑 tar（2026-10-13 起仅打包不压缩——zstd -19 在设备上分钟级太慢），
  前端只轮询产物，不阻塞界面。`/sdcard/Download/logd_<MMDD-HHmmss>.tar`；本次运行正在写的文件会被排除。
- **实验室锁定**：`/tmp/chiri-labs.lock`（回退 `/dev/chiri-labs.lock`，与守护进程同序探测）存在
  即表示本次开机后启用过，运行时关不掉——关闭按钮禁用并提示需重启设备。标记里的 `#` 行是守护
  进程留下的异常痕迹，配合 `labWarnings()` 的一致性检查（rhine.chr 非法 / 与锁定态不一致 /
  `rhine-back.chr` 缺失）在页面顶部出「建议立即重启」的警示条。
- **平台差异**：`status.csv`、`devimp/`（诊断双文件 `main_*` 决策 / `aff_*` 线程）、`special_tuned.yaml`、
  `fas_whitelist.yaml` 仅 ChiRi 机型产生，非 ChiRi 与「无法判定」在界面上是两种不同的表达。
- **日志一律取尾部窗口**（daemon.log 单文件 50MB、status.csv 8MB），禁止整文件读取。

## 无设备预览

浏览器里没有 `ksu` 注入时自动装载 `src/dev/mock-shell.ts`，用构建期嵌入的仓库配置 + 生成数据模拟设备。
URL 参数可切换形态，便于走查空态 / 错误态：

```
?soc=chiri             设备形态（影响白名单与 status.csv 是否存在）
?state=normal|empty|error   正常 / 守护进程从未启动 / 读取真实失败
?daemon=running|stopped     存活探测结果
?pf=0                       未启用帕累托前沿落点（家族名显示 CLG）
```

## 自定义背景（backend.chr）

想让界面用自家壁纸，把图片命名为 `backend.chr` 放在 **`index.html` 同目录**
（设备上即模块的 `webroot/`，与 `index.html` 并列）再刷新页面即可：

```sh
# 设备上（模块目录示例）
cp /sdcard/my-wallpaper.jpg /data/adb/modules/chiri/webroot/backend.chr
```

- 支持浏览器能解码的任意图片格式（jpg/png/webp…），文件名固定为 `backend.chr`。
- 缩放为 **cover**：始终填满页面、允许出界裁切，宽高等比，不会单边拉伸变形。
- 删除该文件即恢复默认夜景背景；文件损坏/非图片时静默回落，不会白屏或破图。
- 该文件是**设备侧用户文件**，不参与构建：`build.rs` 的资产清单与 xtask 的组装都会主动剔除它。
  前者必要——若它被嵌进二进制，`restore_webroot` 会在每次 daemon 启动时发现磁盘上的用户图
  与内嵌副本不一致，按「不一致即覆盖」的防篡改口径把用户图反复改写回去，背景将永远无法生效。
- **升级时能否保住它，取决于安装方式**：热更新走 `cp -r`（只覆盖同名文件，用户文件保留）；
  Magisk 完整安装会整体替换模块目录，放在 `webroot/` 的文件会随之丢失，需重新放置。
  两条路径都没有「专门删除 backend.chr」的代码——丢失只源于「整体替换目录」这一通用行为。
- 为让上方白字保持可读，背景会自动压暗（`brightness(.72)`）；这是调场景亮度，不是给玻璃上色。

## 设计系统（玻璃拟态 / Nocturne Glassmorphism）

风格规则来源：StyleKit `glassmorphism`（完整规则集见 `stylekit/glassmorphism/glassmorphism-SKILL.md`）。
实现口径与组件契约见 `DESIGN.md`；**写任何界面代码前先读 DESIGN.md**。

六条不可违反的红线（`npm run style-check` 会静态拦截）：

1. 玻璃无色——面板只用 `rgba(255,255,255,5%~12%)`，颜色只能来自夜景场景底
2. 必须有场景——深墨夜景纯色底（`.g-scene`，2026-10-16 用户要求去除渐变，原 2~3 个大半径 radial 光源已删）+ 2.5% 颗粒噪点（`.g-grain`）
3. `backdrop-filter` 用具体数值（`--glass-blur` 1.25px / `--glass-blur-heavy` 1.875px，2026-10-16 再次减半；上游 40/60 的 1/16 再半）**且**带 `saturate(180%)`
4. 阴影必须三层：外层深度 + `inset 0 1px 0` 顶边受光 + `inset 0 -1px 0` 底边暗缘
5. 唯一强调色香槟金 `#E4B863`（主 CTA / 关键数字 / 当前项）；禁止第二强调色与紫粉渐变
6. 圆角 ≥16px；过渡一律 `500ms cubic-bezier(0.16,1,0.3,1)`

风格闸门是**可执行**的：`scripts/stylecheck.mjs` 扫描全部 `.svelte`/`.css`，
拦截紫粉渐变、给玻璃上色、低模糊值、旧 `ak-*` / `--md-*` 体系残留、`:has()`、顶层 await 等，
并断言场景/噪点/饱和增强/spring easing/香槟金确实存在。已挂进 `npm run verify`。
