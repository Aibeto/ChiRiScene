# Fluent Design — AI 生成规则（速查）

> 写/改本仓库 WebUI 时把这份当硬约束；完整说明见 `fluent-SKILL.md`，实现口径见 `../../DESIGN.md`。

## 必须

- 强调色只用 `var(--accent)`（#0078D4）；语义色只用 `--sig-*`，且状态必须另有文字
- 卡片用 `.g-card`（已含 hover 上浮 4px + `--shadow-3` 扩张）；卡片内部的小块用 `.g-surface`
- 按钮用 `.g-btn` 系列（已含 hover 上浮 2px、`:active` `scale(0.97)` + 阴影收起）
- 交互过渡只用 `var(--motion-fast)` 150ms / `var(--motion)` 200ms + `var(--ease)`
- 圆角只用 `--radius-s` 4px / `--radius-m` 6px / `--radius-l` 8px
- 亚克力只给「浮在场景上的表面」：顶栏、底栏、卡片、弹层；模糊值必须走 `var(--blur)` / `var(--blur-strong)`
- 焦点状态交给全局 `:focus-visible`（2px 品牌蓝 + 2px 偏移），不要自己写 `outline: none` 了事
- 正文色 ≥ `var(--text-muted)`；不要用 `--text-faint` 承载正文
- 文案走 `t()`；路径/数值走 `var(--font-mono)` + `tabular-nums`

## 禁止

- `backdrop-filter` 出现在输入框、标签、列表行、开关等小控件上（亚克力滥用）
- 单层或硬边阴影、`box-shadow: 2px 2px 0`
- 过渡 > 200ms、弹簧/回弹缓动（负值控制点的 `cubic-bezier`）
- 品牌蓝之外的第二强调色、紫粉渐变、`background-clip: text`
- `border-radius: 0` 或 > 8px 的大圆角
- `border-left/right: Npx solid <color>` 这类单侧信号条
- `:has()`、`Intl.Segmenter`、组件顶层 `await`（WebView 兼容红线）
- 在视图 `<style>` 里写死色值（一律用 app.css 的 token）

## 改样式时的自检

```sh
cd webui
npm run style-check   # 静态闸门：禁止项 + 必须项 + 类名定义完整性
npm run verify        # style-check + svelte-check + vitest + build
```
