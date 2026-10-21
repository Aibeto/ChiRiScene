# WebUI 设计规范（Fluent Design / 流利设计）

> 2026-10-22 重构。风格源：StyleKit `fluent-design`，资源与完整规则集见 `stylekit/fluent/`。
> 本文是**唯一权威**的界面实现口径。写视图/组件前先读本文件，再读 `src/app.css` 的类定义。

## 0. 最高优先级：六条禁止（违反即风格报废）

| # | 禁止 | 原因 |
| --- | --- | --- |
| 1 | 亚克力铺满全场（输入框、标签、列表行、每个小控件都加 `backdrop-filter`） | Fluent 的材质是有选择的：只有浮在场景上的顶栏/底栏/卡片/弹层用 Acrylic，嵌在里面的块用实心面 `g-surface` |
| 2 | 硬边阴影 / 单层扁平阴影 | Fluent 的深度是「近距离小阴影 + 远距离扩散」两层柔和叠加（`--shadow-1..4`） |
| 3 | 过渡超过 200ms、弹簧/回弹缓动 | Fluent 是利落的：交互过渡只有 150ms / 200ms 两档，缓动固定 `--ease` |
| 4 | 品牌蓝之外的第二强调色（自造紫/青/渐变） | 全站只有一个强调色 `--accent #0078d4`，语义色只标状态且必配文字 |
| 5 | 直角（`rounded-none`）与 16px+ 大圆角 | Fluent 的圆角是克制的 4 / 6 / 8px（`--radius-s/m/l`） |
| 6 | 单侧粗边框信号条、渐变文字、紫粉渐变、`Inter/Roboto/Geist` 之外的替换字体 | 风格身份红线；状态用圆点 + 文字标签表达 |

## 1. 场景与材质

页面由 `App.svelte` 的 `.g-scene` 提供 **Mica 底**：中性 `--bg (#f3f2f1)` + 顶部一层极淡的品牌蓝光晕
（`radial-gradient` 单色，不是彩色渐变）；`.g-noise` 再叠 2% `feTurbulence` 颗粒 —— 这正是 Fluent
Acrylic 自带的那层噪点，用来消除大面积白面的塑料感。

三层材质，从上到下：

| 材质 | 类 | 用在哪 | 关键属性 |
| --- | --- | --- | --- |
| Acrylic（强） | `g-acrylic--strong` | 顶栏、底栏、确认弹层 | `rgb(255 255 255 / 90%)` + `blur(--blur-strong 28px)` |
| Acrylic | `g-acrylic` / `g-card` | 面板卡片、列表容器 | `rgb(255 255 255 / 78%)` + `blur(--blur 20px)` |
| 实心面 | `g-surface` | 卡片内部的列表行、终端、快照表 | `--surface-alt` + 1px `--surface-border`，**无模糊** |

**视图不要自己画背景。** 你的根节点只需 `.u-stack`；卡片直接放上去即可。

### 1.1 自定义背景（backend.chr）

与 `index.html` **同目录**放置 `backend.chr`（任意浏览器可解码的图片）即启用该图作为页面背景；
文件不存在或不是图片时静默回落到 Mica 底，不报错、不出现破图。

- 实现：`App.svelte` 用 `<img class="shell__bg">` 探测并加载（`Image` 探测而非 `fetch`——WebView 的
  `file://` 下 fetch 常被同源策略拒绝；`src` 是运行时字符串，Vite 不参与解析，故相对**文档**解析）。
- 铺满方式：`object-fit: cover` —— 始终填满视口、允许出界裁切，宽高等比，**禁止单边拉伸**。
- 层级：`position: fixed` + `z-index: -1`（定位元素若为 0 会盖住非定位正文）。
- 启用时 `.g-scene--custom-bg` 关掉 Mica 底与光晕：背景图自己就是场景（同一时刻只应有一个场景）。

## 2. Token（全部已定义在 `src/app.css` `:root`，直接用，不要硬编码色值）

```
品牌  --accent #0078D4  --accent-hover #106EBE  --accent-pressed #005A9E  --accent-tint #EFF6FC
      --accent-yellow #FFB900（只做填充）  --danger #E81123  --danger-tint #FDE7E9  --success #107C10
表面  --bg #F3F2F1  --surface #FFF  --surface-alt #FAF9F8  --surface-sunken #EDEBE9
      --surface-border #E1E1E1  --border-strong #8A8886  --divider #EDEBE9
材质  --acrylic 78%  --acrylic-strong 90%  --acrylic-hover 92%  --acrylic-border 60%
      --blur 20px  --blur-strong 28px
文本  --text-primary #201F1E  --text-secondary #323130  --text-muted #605E5C  --text-faint #A19F9D
阴影  --shadow-1（静置）/ --shadow-2（hover 控件）/ --shadow-3（hover 卡片）/ --shadow-4
      --shadow-dialog（弹层）  --shadow-inset（亚克力顶边）  --focus-ring
几何  --radius-s 4px  --radius-m 6px  --radius-l 8px  --space-1..6  --control-h 44px  --hairline 1px
动效  --ease cubic-bezier(.1,.9,.2,1)  --motion 200ms  --motion-fast 150ms  --safe-b
字体  --font-body（Segoe UI 优先）  --font-mono（Cascadia Code / JetBrains Mono）
信号  --sig-info #0078D4  --sig-action #9D5D00  --sig-danger #D13438
      --sig-success #107C10  --sig-accent #8764B8  --sig-muted #8A8886
```

**信号色 ≠ 强调色。** 信号色只标*状态*（且每条状态都必须另有文字，不能只靠颜色）；
`--accent` 是唯一用于*强调*的颜色（主 CTA / 当前项 / 关键数字）。`--sig-action` 用加深的黄褐而不是
`#ffb900`，是因为它经常作为正文色出现，纯黄在浅底上达不到 AA。

## 3. 可用原子类（都在 `src/app.css`，**不要在视图里重定义**）

| 类别 | 类名 |
| --- | --- |
| 场景 | `g-scene` `g-noise`（仅 App.svelte 用） |
| 材质 | `g-acrylic`、`g-acrylic--strong`、`g-surface`（实心面，卡片内部用）、`g-reveal`（Reveal 扫光，放可点击按钮内） |
| 卡片 | `g-card`（含 hover 上浮 4px + 阴影扩张）、`g-card__header` `g-card__titles` `g-card__title` `g-card__sig`（信号圆点）`g-card__desc` `g-card__actions` |
| 图标 | `g-icon`（36px 圆角图标容器，宿主 hover 时填充品牌蓝并放大 1.05） |
| 按钮 | `g-btn`、`g-btn--primary`（品牌蓝主 CTA）、`g-btn--danger`、`g-btn--ghost`、`g-btn--block`、`g-btn--sm`、`g-btn--icon` |
| 输入 | `g-input`（`aria-invalid="true"` 自动红边） |
| 表单 | `g-field` `g-label` `g-hint` `g-form-stack` |
| 标签 | `g-tag`、`g-tag--neutral` `g-tag--accent` `g-tag--danger`、`g-tag-group` |
| 状态 | `g-status` `g-status--running` `g-status__signal` `g-status__label` `g-status__detail` |
| 分段 | `g-segmented`（`data-scroll` 变体）、`g-segmented__item` |
| 开关 | `g-switch` `g-switch__input` `g-switch__control` `g-switch__track` `g-switch__thumb` `g-switch__text` `g-switch__name` |
| 弹层 | `g-dialog` `g-dialog__header` `g-dialog__eyebrow` `g-dialog__title` `g-dialog__body` `g-dialog__footer` |
| 通知 | `g-notice`（按 `data-kind` 自动取信号色）、`g-notice__signal` `g-notice__body` `g-notice__title` `g-notice__message` |
| 进度 | `g-progress` `g-progress__header` `g-progress__track` `g-progress__fill` `g-progress__value` |
| 读数 | `g-readout`（品牌蓝大号数字） |
| 待提交 | `g-pending`（警示黄小点） |
| 动效 | `g-animate-in` |
| 布局/排版 | `u-row` `u-row-tight` `u-between` `u-col` `u-grow` `u-stack` `u-actions` `u-mono` `u-muted` `u-danger` `u-truncate` `u-note` `u-mt-2` `u-mt-3` `u-scroll` `u-list-reset` `kv` `kv__key` `kv__value` `u-notice` |

**`data-signal="info|action|danger|success|accent|muted"`** 挂任意元素即可让 `var(--signal)` 生效
（`Panel` 会据此在标题前画信号圆点，`.g-notice` 据此给圆点上色）。

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
6. **不加依赖**。样式全部手写 CSS 变量，不引入 Tailwind / 组件库。
7. **字体二进制不入库**，由 `scripts/fetch-fonts.mjs` 构建期拉取；`app.css` 里用相对路径 `./assets/fonts/`。
8. **滚动容器**用 `.u-scroll`；「回到底部」按钮必须 `absolute` 锚在滚动容器内（禁 `position: fixed`）。
9. 触控目标 ≥ `--control-h`(44px)；所有交互元素要有可见焦点（全局 `:focus-visible` 2px 品牌蓝 + 2px 偏移）
   与可访问名称。
10. **`prefers-reduced-motion`** 已在 app.css 全局降级，视图不要写死动画。
11. **环境动效例外**：`g-pulse`（存活呼吸）与 `state-pulse`（加载脉冲）是持续动画，不受 200ms 上限约束，
    但必须尊重 reduced-motion；交互过渡一律 ≤200ms。

## 6. 交付前自检

- [ ] 背景是 Mica 底（中性面 + 单色光晕）+ `.g-noise` 颗粒，没有重新引入彩色渐变
- [ ] 亚克力只出现在顶栏/底栏/卡片/弹层；卡片内部的小块用 `g-surface`（无模糊）
- [ ] 阴影全部走 `--shadow-1..4`（两层柔和叠加），没有硬边、没有单层扁平
- [ ] 圆角只出现 4 / 6 / 8px；没有 `rounded-none`、没有 16px+ 大圆角
- [ ] 品牌蓝 `--accent` 只出现在主 CTA / 当前项 / 关键数字 / 焦点环
- [ ] 所有按钮 `:active` 是 `scale(0.97)`；hover 上浮的按钮/卡片阴影同步增强
- [ ] 过渡 ≤200ms（`--motion` / `--motion-fast`），缓动是 `--ease`，没有弹簧
- [ ] 正文用 `--text-muted` 或更深；无硬编码色值（全走 token）
- [ ] 每个可聚焦元素都有可见焦点环（`outline` 2px + offset 2px，或 track 上的 `--focus-ring`）
- [ ] `t()` 键都存在；无硬编码文案
- [ ] 无横向溢出；手机/平板/桌面均稳定
