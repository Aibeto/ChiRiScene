#!/usr/bin/env node
// stylecheck.mjs: Fluent Design 风格闸门 —— 静态扫描禁止项 / 必须项，防止风格漂移
//
// 为什么需要它：Fluent 的规则（唯一品牌蓝、亚克力不许滥用、阴影必须分层柔和、过渡 ≤200ms、
// 按钮按压 scale(.97)、焦点环必须可见）靠人眼 review 不可靠，且一旦漂移整站观感报废。
// 本脚本把 StyleKit fluent-design 的规则落成可执行断言，挂在 verify.mjs 最前面。
//
// 用法：node scripts/stylecheck.mjs        （退出码非 0 即违规）
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import { dirname } from 'node:path'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const SRC = join(ROOT, 'src')

function walk(dir, out = []) {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) walk(full, out)
    else if (/\.(svelte|css)$/.test(entry)) out.push(full)
  }
  return out
}

const files = walk(SRC)

// 违规项：{ pattern, why }
const RULES = [
  // —— 风格身份红线 ——
  {
    pattern: /#667eea|#764ba2|#f093fb/i,
    why: '紫粉 AI 渐变是本风格明确拒绝的配色（Fluent 只有品牌蓝 + 中性灰 + 语义色）'
  },
  {
    pattern: /linear-gradient\([^)]*(purple|indigo|pink|fuchsia|violet)/i,
    why: '禁止紫/靛/粉系渐变'
  },
  {
    pattern: /background-clip\s*:\s*text|[-:]bg-clip-text/,
    why: '禁止渐变文字（background-clip: text）'
  },
  {
    pattern: /\brounded-(none|sm|2xl|3xl)\b/,
    why: 'Fluent 的圆角是克制的 4/6/8px（--radius-s/m/l），没有直角也没有 16px+ 大圆角'
  },
  {
    pattern: /shadow-\[(?:\d+px_){2}0px/,
    why: 'Fluent 阴影是柔和分层的两层影子，禁止硬边投影'
  },
  {
    // 自检清单明确禁止「单侧粗边框装饰（border-left/right accent stripe）」。
    // 只抓"只有某一侧加粗/上色"的写法，四边等宽 border 不受影响。
    pattern: /border-(left|right)(-width)?\s*:\s*(?!var\(--hairline\))[2-9]\d*px|border-(left|right)-color\s*:/,
    why: '禁止单侧粗边框信号条（border-left/right accent stripe），状态请用圆点 + 文字标签表达'
  },
  {
    // 裸 blur 值 → 各浮层厚薄不一；必须走 --blur / --blur-strong。
    // 前面限一个空白/分号，避免命中 @supports 的能力探测 `(backdrop-filter: blur(1px))`
    pattern: /[\s;{]backdrop-filter\s*:\s*blur\((?!var\()/,
    why: '背景模糊必须用 var(--blur)/var(--blur-strong)：裸数值会让顶栏、底栏、弹层厚薄不一'
  },
  {
    // Fluent 的动效利落：交互过渡不得超过 200ms（环境 animation 不受此限）
    pattern: /transition[^;{}]*\b([3-9]\d{2}|[1-9]\d{3,})ms/,
    why: '过渡超过 200ms 违反 Fluent 的利落感（用 --motion 200ms / --motion-fast 150ms）'
  },
  {
    pattern: /cubic-bezier\([^)]*-\d/,
    why: '禁止回弹/弹性缓动（负控制点会过冲），Fluent 用 --ease cubic-bezier(0.1,0.9,0.2,1)'
  },
  // —— 旧体系残留（本轮重构必须清零）——
  {
    pattern: /--glass-|\bg-glass|g-sheen|g-grain|--champagne|--night|--moonlight|--spring/,
    why: '玻璃拟态体系已废弃：改用 --acrylic-* / g-acrylic / g-surface / g-reveal / g-noise / --accent / --ease'
  },
  {
    pattern: /\bak-[a-z0-9_-]+/,
    why: 'ak-ui 体系已废弃，改用 g-* / u-* 原子类'
  },
  {
    pattern: /--md-[a-z0-9-]+/,
    why: 'MIUIX/Material token 已废弃，改用 app.css 的 --accent-* / --surface-* / --text-*'
  },
  {
    pattern: /data-ak-ui/,
    why: 'ak-ui 作用域标记已移除'
  },
  // —— 项目红线 ——
  {
    pattern: /:has\(/,
    why: 'WebView 兼容红线：禁用 :has()（开关轨道必须用 input:checked ~ track 联动）'
  },
  {
    // 只抓真正的顶层 await（脚本块内顶格、不在 async 函数体里）。缩进的 await 一律不算
    pattern: /^await\s/,
    why: 'Svelte 组件禁顶层 await（必须包在 async 函数里）'
  },
  {
    pattern: /Intl\.Segmenter/,
    why: '项目红线：禁用 Intl.Segmenter（WebView 缺失）'
  }
]

const ALLOW_FILES = new Set(['stylecheck.mjs'])

const violations = []
for (const file of files) {
  const rel = relative(ROOT, file)
  if (ALLOW_FILES.has(rel.split('/').pop())) continue
  const raw = readFileSync(file, 'utf8')
  // 先整文件剥注释（含跨行 /* */ 与 HTML <!-- -->）：注释里提到禁止项是允许的（正是解释"为什么禁止"的地方），
  // 用等长空格占位保住行号
  const lines = raw.split('\n')
  const masked = raw
    .replace(/<!--[\s\S]*?-->/g, m => m.replace(/[^\n]/g, ' '))
    .replace(/\/\*[\s\S]*?\*\//g, m => m.replace(/[^\n]/g, ' '))
    .replace(/\/\/[^\n]*/g, m => m.replace(/[^\n]/g, ' '))
    .split('\n')
  for (const rule of RULES) {
    masked.forEach((line, i) => {
      if (rule.pattern.test(line)) {
        violations.push({ rel, line: i + 1, text: lines[i].trim(), why: rule.why })
      }
    })
  }
}

const css = readFileSync(join(SRC, 'app.css'), 'utf8')
const app = readFileSync(join(SRC, 'App.svelte'), 'utf8')

// —— 必须项：Fluent 的身份特征必须能在 app.css 里找到 ——
const REQUIRED_CSS = [
  [/--accent:\s*#0078d4/i, 'app.css 缺少唯一品牌蓝 #0078d4'],
  [/--shadow-1:[^;]*,/, 'app.css 缺少两层分层阴影 --shadow-1（Fluent 阴影永远两层柔和叠加）'],
  [/--ease:\s*cubic-bezier\(0\.1,\s*0\.9,\s*0\.2,\s*1\)/, 'app.css 缺少 Fluent 标准缓动 --ease'],
  [/--motion:\s*200ms/, 'app.css 的 --motion 必须是 200ms（Fluent 的交互节奏上限）'],
  [/--motion-fast:\s*150ms/, 'app.css 缺少 --motion-fast: 150ms'],
  [/outline:\s*2px solid var\(--accent\)/, 'app.css 缺少统一焦点环（2px 品牌蓝 outline）'],
  [/outline-offset:\s*2px/, 'app.css 焦点环缺少 outline-offset: 2px（等价 focus:ring-offset-2）'],
  [/backdrop-filter:\s*blur\(var\(--blur\)\)/, 'app.css 缺少亚克力材质（backdrop-filter: blur(var(--blur))）'],
  [/--radius-s:\s*4px/, 'app.css 缺少 --radius-s: 4px（Fluent 输入框/徽标圆角）']
]
for (const [re, why] of REQUIRED_CSS) {
  if (!re.test(css)) violations.push({ rel: 'src/app.css', line: 0, text: '', why })
}

// 必须项：卡片有 Z 轴抬升（hover 上浮 + 阴影扩张）、按钮有触觉按压（scale 0.97）、开关焦点环可见
for (const [re, why] of [
  [/\.g-card:hover\s*\{[^}]*translateY\(-4px\)/, '.g-card:hover 必须上浮 4px（等价 hover:-translate-y-1）'],
  [/\.g-card:hover\s*\{[^}]*box-shadow:\s*var\(--shadow-3\)/, '.g-card:hover 必须把阴影扩张到 --shadow-3'],
  [/\.g-btn:active:not\(:disabled\)\s*\{[^}]*scale\(0\.97\)/, '.g-btn:active 必须有 scale(0.97) 触觉确认'],
  [/\.g-switch input:focus-visible[^{]*\{[^}]*--focus-ring/, '开关的焦点环必须画在 track 上（键盘可达）']
]) {
  if (!re.test(css)) violations.push({ rel: 'src/app.css', line: 0, text: '', why })
}

// 必须项：App.svelte 必须挂 Mica 场景与 Acrylic 颗粒
if (!/g-scene/.test(app)) violations.push({ rel: 'src/App.svelte', line: 0, text: '', why: '缺少 .g-scene Mica 场景底' })
if (!/g-noise/.test(app)) violations.push({ rel: 'src/App.svelte', line: 0, text: '', why: '缺少 .g-noise 颗粒层（Fluent Acrylic 的 2% 噪点）' })

// 必须项：亚克力不许滥用 —— 统计 backdrop-filter 次数，超阈值说明连小控件都铺了模糊
const blurCount = (css.match(/backdrop-filter:/g) || []).length
if (blurCount > 10) {
  violations.push({
    rel: 'src/app.css',
    line: 0,
    text: `backdrop-filter ×${blurCount}`,
    why: '亚克力被滥用：只允许顶栏/底栏/卡片/弹层使用模糊（禁止给小控件、标签、输入框加 blur）'
  })
}

// 必须项：所有 g-* 类必须在样式里有定义（防止重命名后残留死类名）
const definedClasses = new Set()
const styleSources = [css, ...files.filter(f => f.endsWith('.svelte')).map(f => readFileSync(f, 'utf8'))]
for (const src of styleSources) {
  for (const m of src.matchAll(/\.([a-z][\w-]*)/g)) definedClasses.add(m[1])
}
const usedG = new Map()
for (const file of files.filter(f => f.endsWith('.svelte'))) {
  const src = readFileSync(file, 'utf8').replace(/<!--[\s\S]*?-->/g, '')
  for (const m of src.matchAll(/class(?:="([^"]*)"|:([\w-]+))/g)) {
    const tokens = (m[1] || m[2] || '').split(/\s+/).filter(Boolean)
    for (const t of tokens) {
      // 动态片段（{cond ? 'a' : 'b'}）在源码里会被拆成独立 token，只校验完整形态的 g- 类名
      if (!/^g-[a-z][\w-]*$/.test(t)) continue
      usedG.set(t, file)
    }
  }
}
for (const [cls, file] of usedG) {
  if (!definedClasses.has(cls)) {
    violations.push({
      rel: relative(ROOT, file),
      line: 0,
      text: cls,
      why: `类 ${cls} 在标记里使用但样式里没有定义（重命名漏改或拼写错误）`
    })
  }
}

/*
 * 必须项：DESIGN.md 的组件 prop 契约必须与组件真实签名一致。
 * 起因：DESIGN.md 曾把 ConfirmSheet 的关闭回调 prop 误写成 `oncancel`（真名是 `ondismiss`，
 * 组件里那行 <dialog oncancel={...}> 是原生 dialog 事件、与 prop 无关）。文档是后续 agent 的唯一依据，
 * 写错等于指示后来者把能跑的代码改成编译不过的代码——而 svelte-check 只检查代码、不会检查文档，
 * 所以这类"文档漂移"能一路全绿地交付出去。这里逐字段比对，让文档错误直接卡住验收。
 */
const COMPONENTS = join(SRC, 'components')
const contract = new Map()
const design = readFileSync(join(ROOT, 'DESIGN.md'), 'utf8')
for (const line of design.split('\n')) {
  const m = line.match(/^(\w+)\s+\{([^}]*)\}\s*$/)
  if (!m) continue
  const props = m[2]
    .split(',')
    .map(p => p.trim().replace(/\?.*$/, '').replace(/:.*$/, '').trim())
    .filter(Boolean)
  contract.set(m[1], new Set(props))
}

for (const [name, documented] of contract) {
  const file = join(COMPONENTS, `${name}.svelte`)
  let src
  try {
    src = readFileSync(file, 'utf8')
  } catch {
    violations.push({
      rel: `DESIGN.md → ${name}`,
      line: 0,
      text: '',
      why: `DESIGN.md 契约表列了 ${name}，但 src/components/${name}.svelte 不存在`
    })
    continue
  }
  // 取 $props<{ ... }>() 里声明的字段名
  const body = src.match(/\$props<\{([\s\S]*?)\}>\(\)/)
  const actual = new Set(
    (body ? body[1] : '')
      .split('\n')
      .map(l => l.trim())
      .filter(l => /^[A-Za-z_$][\w$]*\??\s*:/.test(l))
      .map(l => l.replace(/\??\s*:.*$/, '').trim())
  )
  for (const p of documented) {
    if (!actual.has(p)) {
      violations.push({
        rel: `DESIGN.md → src/components/${name}.svelte`,
        line: 0,
        text: `${name} { ... ${p} ... }`,
        why: `DESIGN.md 声明的 prop \`${p}\` 在组件里不存在（真实 prop：${[...actual].join(', ')}）`
      })
    }
  }
  for (const p of actual) {
    if (!documented.has(p)) {
      violations.push({
        rel: `DESIGN.md → src/components/${name}.svelte`,
        line: 0,
        text: `${name} 缺少文档 ${p}`,
        why: `组件有 prop \`${p}\` 但 DESIGN.md 契约表未列出`
      })
    }
  }
}

// 回归守卫：给行内元素（标记里是 <span>）设 width/height 的类必须显式 display:block 或
// 处于 flex/grid 容器中（此时子元素被 blockify），否则行内盒忽略宽高 → 元素恒为 0 尺寸。
// 2026-10-15 状态页两个功耗条恒空即此因：.g-progress__fill 漏了 display:block。
//
// 两处坑（本脚本第一版都踩了，故留下说明）：
// ① 必须先在「剥掉注释的副本」上判定——注释里写着 display:block 是在解释规则本身；
// ② 判定 flex item 必须用「元素栈」求真实直接父节点，正则放宽会触发灾难性回溯、整个守卫静默失效。
const cssNoComment = css.replace(/\/\*[\s\S]*?\*\//g, m => m.replace(/[^\n]/g, ' '))
const cssRules = new Map()
for (const m of cssNoComment.matchAll(/\.([A-Za-z][\w-]*)\s*\{([^{}]*)\}/g)) {
  cssRules.set(m[1], m[2])
}
const isContainerCls = cls => {
  const b = cssRules.get(cls)
  return !!b && /display\s*:\s*(inline-)?(flex|grid)/.test(b)
}
const flexItemCls = new Set()
const VOID_TAGS = new Set(['input', 'br', 'hr', 'img', 'meta', 'link', 'source', 'path', 'circle'])
for (const f of files) {
  const src = readFileSync(f, 'utf8').replace(/<!--[\s\S]*?-->/g, '')
  const stack = []
  for (const m of src.matchAll(/<(\/?)([a-zA-Z][\w-]*)([^>]*?)(\/?)>/g)) {
    const [, close, tag, attrs, selfClose] = m
    if (close) { stack.pop(); continue }
    const cls = (attrs.match(/class="([^"]*)"/) || [, ''])[1].split(/\s+/).filter(Boolean)
    const parent = stack[stack.length - 1]
    if (parent && parent.some(isContainerCls)) cls.forEach(c => flexItemCls.add(c))
    if (!selfClose && !VOID_TAGS.has(tag)) stack.push(cls)
  }
}
for (const [cls, body] of cssRules) {
  if (!/(^|[\s;])width\s*:/.test(body) && !/(^|[\s;])height\s*:/.test(body)) continue
  if (/position\s*:\s*(absolute|fixed)|display\s*:\s*(block|flex|grid|inline-block|inline-flex|table)/.test(body)) continue
  const usedInline = files.some(f =>
    new RegExp(`<(span|i|em|b|small)\\b[^>]*class="[^"]*\\b${cls}\\b`).test(readFileSync(f, 'utf8'))
  )
  if (!usedInline || flexItemCls.has(cls)) continue
  violations.push({
    rel: 'src/app.css',
    line: 0,
    text: `.${cls}`,
    why: '在 <span> 等行内元素上设了 width/height，却既没有 display:block 也不在 flex/grid 容器内 —— 行内盒会忽略宽高，元素恒为 0 尺寸（功耗进度条恒空就是这个原因）'
  })
}

if (violations.length === 0) {
  console.log(`[stylecheck] ✓ ${files.length} 个文件，无风格违规`)
  process.exit(0)
}

console.error(`[stylecheck] ✗ ${violations.length} 处风格违规：\n`)
for (const v of violations) {
  console.error(`  ${v.rel}${v.line ? ':' + v.line : ''}`)
  if (v.text) console.error(`    ${v.text}`)
  console.error(`    → ${v.why}`)
}
console.error('')
process.exit(1)
