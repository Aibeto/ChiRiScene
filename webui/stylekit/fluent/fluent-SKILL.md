# Fluent Design（流利设计）— 本项目落地的规则集

> 来源：StyleKit `fluent-design` 风格参考（2026-10-22 WebUI 重构时按上游规则转录）。
> StyleKit 的导出没有服务端接口（网页上「导出风格包」由浏览器端现算 Blob），故这里只保留**规则口径 + token 值**，
> 实际实现以 `src/app.css` 为准，令牌快照见 `fluent-tokens.json`。

## 五要素

| 要素 | 在本项目里的体现 |
| --- | --- |
| Light（光） | Reveal 扫光 `.g-reveal`；焦点环 2px 品牌蓝 + 2px 偏移；hover 时表面提亮 |
| Depth（深度） | 卡片 hover 上浮 4px + 阴影扩张到 `--shadow-3`；按钮 hover 上浮 2px |
| Motion（动效） | 只有 150ms / 200ms 两档，缓动固定 `cubic-bezier(.1,.9,.2,1)` |
| Material（材质） | Mica 底 + Acrylic 浮层 + 实心面三档，亚克力**有选择地**使用 |
| Scale（缩放） | 图标容器 `scale(1.05)`；按钮按压 `scale(0.97)` |

## 绝对禁止

- 亚克力铺满全场（输入框、标签、列表行、每个小控件都加 `backdrop-filter`）
- 硬边阴影 / 单层扁平阴影
- 过渡超过 200ms、弹簧或回弹缓动（`cubic-bezier(.16,1,.3,1)` 这类过冲曲线也不行）
- 品牌蓝之外的第二强调色、紫粉 AI 渐变、渐变文字
- 直角（`rounded-none`）与 16px+ 大圆角
- 单侧粗边框信号条（`border-left: 3px solid …`）——状态改用圆点 + 文字
- 忽略焦点状态（任何可聚焦元素都必须有可见环）

## 必须遵守

1. **唯一品牌蓝** `#0078D4`：主 CTA、当前项、关键数字、焦点环。
2. **亚克力有选择**：顶栏 / 底栏 / 卡片 / 弹层用 `bg white/78~90% + blur 20~28px`；卡片内部小元素用实心面。
3. **阴影两层柔和**：`--shadow-1..4`，永远「近距离小阴影 + 远距离扩散」。
4. **圆角克制**：4 / 6 / 8px（`--radius-s/m/l`）。
5. **按压确认**：按钮 `:active` 必须 `scale(0.97)`，同时回到表面（`translateY(0)`、阴影收起）。
6. **焦点可见**：`:focus-visible` → `outline: 2px solid #0078D4; outline-offset: 2px`。
7. **字体**：Segoe UI 一族优先，中文回落思源黑体子集；等宽用 Cascadia Code / JetBrains Mono。
8. **对比度**：正文 ≥ `#605E5C`（白底约 6.3:1）；状态不允许只靠颜色传达。

## 交互配方

```
卡片    .g-card            hover: translateY(-4px) + shadow-1 → shadow-3 + 表面提亮 + 边框变亮
按钮    .g-btn             hover: translateY(-2px) + shadow-1 → shadow-2
                           active: scale(0.97) translateY(0) + shadow: none
主 CTA  .g-btn--primary    #0078D4 → hover #106EBE → active #005A9E
图标    .g-icon            宿主 hover: 品牌蓝填充 + scale(1.05)
输入    .g-input           1px #8A8886 → focus 边框 + 1px 内描边变蓝
开关    .g-switch          关: 白底空心; 开: 品牌蓝实心; 焦点环画在 track 上
分段    .g-segmented       灰底容器, 选中项浮成白色小块 + 阴影
```

## 材质分层（为什么不是全站亚克力）

`backdrop-filter` 在 Android WebView 上很贵，层数一多就掉帧；而且**平面上模糊等于没模糊**。
所以本项目只让「浮在场景上的表面」消费模糊：Mica 场景（中性底 + 单色光晕 + 2% 噪点）→
Acrylic 浮层（顶栏/底栏/卡片/弹层）→ 实心面（列表行/终端/快照表）。
`scripts/stylecheck.mjs` 会统计 `backdrop-filter` 次数，超过 10 处直接判违规。
