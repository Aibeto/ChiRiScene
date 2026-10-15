# WebUI 设计规范（玻璃拟态 / Nocturne Glassmorphism）

> 2026-10-15 重构。风格源：StyleKit `glassmorphism`，资源与完整规则集见 `stylekit/glassmorphism/`。
> 本文是**唯一权威**的界面实现口径。写视图/组件前先读本文件，再读 `src/app.css` 的类定义。

## 0. 最高优先级：六条禁止（违反即风格报废）

| # | 禁止 | 原因 |
| --- | --- | --- |
| 1 | 给玻璃面板上色（`bg-purple-500/20`、`--md-*` 之类） | **玻璃无色**，颜色只能来自场景 |
| 2 | 纯色平面背景上做玻璃 | 没有可模糊之物，玻璃不成立 |
| 3 | 玻璃不透明度 > 15% | 会变实心色块，失去玻璃错觉 |
| 4 | `backdrop-blur` 无具体数值（裸用）/ 省略 `saturate(180%)` | 模糊不足 / 场景光透不过来。本项目模糊按用户要求两次下调（上游 40/60 → 25% → 再四分之一）：`--glass-blur` 2.5px、`--glass-blur-heavy` 3.75px |
| 5 | 单层扁平阴影 | 没有光的方向 = 平面半透明 |
| 6 | 紫粉 AI 渐变、第二强调色、`rounded-none/sm`、`duration-100/150` | 风格身份红线 |

## 1. 场景先于玻璃

背景由 `App.svelte` 的 `.g-scene` 提供：深墨夜景底色（`#060A13 → #0B1322 → #16233A` 165° 渐变）
+ 三个大半径 radial 光源（右上月光 `rgba(124,156,196,.28)`、左下钢蓝 `rgba(51,81,122,.35)`、
中部香槟点缀 `rgba(228,184,99,.10)`），加 `.g-grain` 的 2.5% `feTurbulence` 噪点。

**视图不要自己画背景。** 你的根节点只需 `.u-stack`；玻璃面板直接放上去即可。

### 1.1 自定义背景（backend.chr）

与 `index.html` **同目录**放置 `backend.chr`（任意浏览器可解码的图片）即启用该图作为页面背景；
文件不存在或不是图片时静默回落到夜景底，不报错、不出现破图。

- 实现：`App.svelte` 用 `<img class="shell__bg">` 探测并加载（`Image` 探测而非 `fetch`——WebView 的
  `file://` 下 fetch 常被同源策略拒绝；`src` 是运行时字符串，Vite 不参与解析，故相对**文档**解析）。
- 铺满方式：`object-fit: cover` —— 始终填满视口、允许出界裁切，宽高等比，**禁止单边拉伸**。
- 层级：`position: fixed` + `z-index: -1`（定位元素若为 0 会盖住非定位正文）。
- 启用时 `.g-scene--custom-bg` 关掉夜景底与光源光斑：背景图自己就是"可折射之物"，
  再叠假光源会让画面浑浊（同一时刻只应有一个场景）。

## 2. Token（全部已定义在 `src/app.css` `:root`，直接用，不要硬编码色值）

```
夜景  --night-deep #060A13  --night #0B1322  --night-steel #16233A
      --moon-steel #33517A  --moonlight #7C9CC4
强调  --champagne #E4B863    --champagne-bright #F3DCA8   ← 唯一强调色
玻璃  --glass-bg / -strong / -hover / -weak   --glass-border / -hover
      --glass-blur 2.5px   --glass-blur-heavy 3.75px   --glass-saturate 180%
文本  --text-primary #fff  --text-secondary 78%  --text-muted 58%  --text-faint 40%
      （正文一律 ≥ --text-muted；风格自检要求 white/60 或更亮）
阴影  --glass-shadow-sm / --glass-shadow / --glass-shadow-hover / --glass-shadow-lifted
      --glass-shadow-inset   --focus-ring
几何  --radius-m 16px  --radius-l 24px  --space-1..6  --control-h 44px  --hairline 1px
动效  --spring cubic-bezier(.16,1,.3,1)   --motion 500ms   --safe-b
字体  --font-body  --font-mono
信号  --sig-info #7C9CC4  --sig-action #E4B863  --sig-danger #E0716F
      --sig-success #6FBF9B  --sig-accent #A894D8  --sig-muted
```

**信号色 ≠ 强调色。** 信号色只标*状态*（且每条状态都必须另有文字，不能只靠颜色）；
香槟金是唯一用于*强调*的颜色（主 CTA / 关键数字 / 当前项）。

## 3. 可用原子类（都在 `src/app.css`，**不要在视图里重定义**）

| 类别 | 类名 |
| --- | --- |
| 场景 | `g-scene` `g-grain`（仅 App.svelte 用） |
| 玻璃 | `g-glass`、`g-glass--flat`（干玻璃，嵌在玻璃里的小元素用）、`g-glass--heavy`、`g-sheen`（扫光，放卡片/按钮内） |
| 卡片 | `g-card`（含 hover 上浮）、`g-card__header` `g-card__titles` `g-card__title` `g-card__desc` `g-card__actions` |
| 按钮 | `g-btn`、`g-btn--primary`（香槟主 CTA）、`g-btn--danger`、`g-btn--ghost`、`g-btn--block`、`g-btn--sm`、`g-btn--icon` |
| 输入 | `g-input`（`aria-invalid="true"` 自动红边） |
| 表单 | `g-field` `g-label` `g-hint` `g-form-stack` |
| 标签 | `g-tag`、`g-tag--neutral` `g-tag--accent` `g-tag--danger`、`g-tag-group` |
| 状态 | `g-status` `g-status--running` `g-status__signal` `g-status__label` `g-status__detail` |
| 分段 | `g-segmented`（`data-scroll` 变体）、`g-segmented__item` |
| 开关 | `g-switch` `g-switch__input` `g-switch__control` `g-switch__track` `g-switch__thumb` `g-switch__text` `g-switch__name` |
| 弹层 | `g-dialog` `g-dialog__header` `g-dialog__eyebrow` `g-dialog__title` `g-dialog__body` `g-dialog__footer` |
| 通知 | `g-notice`（按 `data-kind` 自动取信号色）、`g-notice__body` `g-notice__title` `g-notice__message` |
| 进度 | `g-progress` `g-progress__header` `g-progress__track` `g-progress__fill` `g-progress__value` |
| 读数 | `g-readout`（香槟金大号数字） |
| 待提交 | `g-pending`（香槟金小点） |
| 动效 | `g-animate-in` |
| 布局/排版 | `u-row` `u-row-tight` `u-between` `u-col` `u-grow` `u-stack` `u-actions` `u-mono` `u-muted` `u-danger` `u-truncate` `u-note` `u-mt-2` `u-mt-3` `u-scroll` `u-list-reset` `kv` `kv__key` `kv__value` `u-notice` |

**`data-signal="info|action|danger|success|accent|muted"`** 挂任意元素即可让 `var(--signal)` 生效
（`.g-card[data-signal]` 会画左侧信号条）。

## 4. 组件 prop 契约（**不得改签名**，调用点已固定）

```
Panel      { title?, desc?, signal?, actions?: Snippet, children?: Snippet }
LiveStatus { state: 'running'|'stopped'|'unknown', label: string, detail?: string }
StateBox   { kind?: 'empty'|'missing'|'error'|'loading', message?, detail?, action?: Snippet }
Segmented  { items: {id,label}[], value: string, label?, scroll?, onselect: (id)=>void }
ToggleField{ label, hint?, checked?, disabled?, pending?, onchange: (next:boolean)=>void }
NumberField{ label, hint?, value: number, unit?, min?, max?, disabled?, pending?, onchange: (next:number)=>void }
ConfirmSheet { open, title, message?, notes?: string[], confirmText, cancelText, danger?, busy?, onconfirm, ondismiss }
```

> `ConfirmSheet` 的关闭回调 prop 名是 **`ondismiss`**（**不是** `oncancel`）。别被组件里那行
> `<dialog oncancel={...}>` 带偏——那是原生 dialog 元素的**事件**（Esc 键），与 prop 无关。
> 另注意正文 prop 是 **`message`**（不是 `body`），按钮文案是 **`confirmText` / `cancelText`**
> （不是 `confirmLabel`）——这两条才是重构中被写错的用法。

## 5. 硬性工程约束（项目红线，破坏即回归）

1. **`.u-stack` 必须保留 `minmax(0,1fr)`**：已在 app.css 里；不要用自定义 grid 覆盖掉列约束——
   日志/路径这类 nowrap 宽内容会把整页撑出屏幕。
2. **禁 `:has()`**（WebView 兼容）。开关轨道是 `input` 的兄弟元素的兄弟，靠 `input:checked ~ ...` 联动。
3. **禁顶层 await、禁 `Intl.Segmenter`**。
4. **i18n 必须用 `t()`**，禁止硬编码中文/英文文案。键必须已存在于 `src/i18n/locales/{zh,en}.ts`
   （`tests/i18n.test.ts` 会校验源码用键都有定义；新增键要中英同步加）。
5. **不改逻辑层**：`src/kernel/` `src/contract/` `src/data/` `src/i18n/` `src/router.svelte.ts`
   `src/state.svelte.ts` 一律只读调用，签名不改。视图只消费 `state.svelte.ts` 暴露的 `app`。
6. **不加依赖**。
7. **字体二进制不入库**，由 `scripts/fetch-fonts.mjs` 构建期拉取；`app.css` 里用相对路径 `./assets/fonts/`。
8. **滚动容器**用 `.u-scroll`；「回到底部」按钮必须 `absolute` 锚在滚动容器内（禁 `position: fixed`）。
9. 触控目标 ≥ `--control-h`(44px)；所有交互元素要有可见焦点与可访问名称。
10. **`prefers-reduced-motion`** 已在 app.css 全局降级，视图不要写死动画。

## 6. 交付前自检

- [ ] 背景是深墨夜景 + 光源光斑（不是平面渐变，更不是紫粉渐变）
- [ ] 面板只用 `g-glass`/`g-card`/`g-btn`/`g-input`/`g-notice` 等原子类，透明度 5%~12%
- [ ] 玻璃处都有具体 blur 值（`--glass-blur` 2.5px / `--glass-blur-heavy` 3.75px）**且** `saturate(180%)`（.g-glass--flat 除外，它是刻意的干玻璃）
- [ ] 阴影有方向性（外层深度 + inset 顶边高光 + inset 底边暗缘）
- [ ] 圆角是 `--radius-m` / `--radius-l`（16/24px）
- [ ] 香槟金 `--champagne` 只出现在主 CTA / 关键数字 / 当前项 / 高亮文字
- [ ] 过渡 `var(--motion) var(--spring)`，无 duration-100/150 之类的快过渡
- [ ] 正文用 `--text-muted` 或更亮；无硬编码色值（全走 token）
- [ ] `t()` 键都存在；无硬编码文案
- [ ] 无横向溢出；手机/平板/桌面均稳定
