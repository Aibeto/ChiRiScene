#!/usr/bin/env node
// stylecheck.mjs: 玻璃拟态风格闸门 —— 静态扫描禁止项 / 必须项，防止风格漂移
//
// 为什么需要它：风格规则（禁止紫粉渐变、禁止给玻璃上色、禁止低模糊值…）靠人眼 review 不可靠，
// 且一旦漂移整站观感报废。本脚本把 SKILL.md 的规则落成可执行的断言，挂进 verify.mjs。
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

// 违规项：{ pattern, why, allow?（相对路径白名单） }
const RULES = [
  // —— 风格身份红线 ——
  {
    pattern: /#667eea|#764ba2|#f093fb/i,
    why: '紫粉 AI 渐变是本风格明确拒绝的配色（见 SKILL.md 绝对禁止）'
  },
  {
    pattern: /linear-gradient\([^)]*(purple|indigo|pink|fuchsia|violet)/i,
    why: '禁止紫/靛/粉系渐变（颜色必须来自夜景光源）'
  },
  {
    pattern: /rounded-(none|sm)\b|\brounded(?![-\w])/,
    why: '直角/小圆角破坏玻璃感，必须 rounded-2xl(16px) 或 rounded-3xl(24px) 量级'
  },
  {
    pattern: /duration-(100|150)\b/,
    why: '过快过渡失去流动感，改用 duration-500 + spring easing'
  },
  {
    pattern: /backdrop-blur(-sm)?\b(?!-\[)/,
    why: 'backdrop-blur-sm / 裸 backdrop-blur 模糊不足，必须用具体数值（本项目 --glass-blur 1.25px / --glass-blur-heavy 1.875px）'
  },
  {
    pattern: /shadow-none\b/,
    why: '必须有三层方向性阴影（外层深度 + 顶边高光 + 底边暗缘）'
  },
  {
    pattern: /(^|\s)bg-(white|black)(?![/\w-])/,
    why: '玻璃必须半透明：bg-white/5~12，禁止不透明 bg-white / bg-black'
  },
  {
    // 自检清单明确禁止「单侧粗边框装饰（border-left/right accent stripe）」。
    // 这条曾被漏掉：LabView 的 .mode 与 ConfirmSheet 的 .note 从旧版基线继承了
    // `border-left-width: 3px` / `border-left: 2px solid …` 信号条，一路全绿地交付。
    // 只抓"只有某一侧加粗/上色"的写法，四边等宽 border 不受影响。
    pattern: /border-(left|right)(-width)?\s*:\s*(?!var\(--hairline\))[2-9]\d*px|border-(left|right)-color\s*:/,
    why: '禁止单侧粗边框信号条（border-left/right accent stripe），状态请用整圈描边 + 文字标签表达'
  },
  {
    pattern: /background-clip\s*:\s*text|[-:]bg-clip-text/,
    why: '禁止渐变文字（background-clip: text）'
  },
  // —— 项目红线 ——
  {
    pattern: /:has\(/,
    why: 'WebView 兼容红线：禁用 :has()（开关轨道必须用 input:checked ~ track 联动）'
  },
  {
    // 只抓真正的顶层 await（脚本块内顶格、不在 async 函数体里）。
    // async 函数体内的 await 完全正常，缩进的 await 一律不算
    pattern: /^await\s/,
    why: 'Svelte 组件禁顶层 await（必须包在 async 函数里）'
  },
  {
    pattern: /Intl\.Segmenter/,
    why: '项目红线：禁用 Intl.Segmenter（WebView 缺失）'
  },
  // —— 旧体系残留（本轮重构必须清零）——
  {
    pattern: /\bak-[a-z0-9_-]+/,
    why: 'ak-ui 体系已废弃，改用 g-* / u-* 原子类'
  },
  {
    pattern: /--ak-[a-z0-9-]+/,
    why: 'ak-ui token 已废弃，改用 app.css 的 --glass-* / --text-* / --space-*'
  },
  {
    pattern: /--md-[a-z0-9-]+/,
    why: 'MIUIX/Material token 已废弃，改用玻璃拟态 token'
  },
  {
    pattern: /data-ak-ui/,
    why: 'ak-ui 作用域标记已移除'
  }
]

// 允许出现的例外文件（如本脚本自身、资源留档）
const ALLOW_FILES = new Set(['stylecheck.mjs'])

const violations = []
for (const file of files) {
  const rel = relative(ROOT, file)
  if (ALLOW_FILES.has(rel.split('/').pop())) continue
  const raw = readFileSync(file, 'utf8')
  // 先整文件剥注释（含跨行 /* */ 与 HTML <!-- -->），再用占位符保号：
  // 注释里提到禁止项是允许的（正是解释"为什么禁止"的地方），但行号要能对上
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

// 必须项：App.svelte 必须挂场景与噪点
const app = readFileSync(join(SRC, 'App.svelte'), 'utf8')
if (!/g-scene/.test(app)) violations.push({ rel: 'src/App.svelte', line: 0, text: '', why: '缺少 .g-scene 夜景场景（玻璃必须有可折射之物）' })
if (!/g-grain/.test(app)) violations.push({ rel: 'src/App.svelte', line: 0, text: '', why: '缺少 .g-grain 颗粒噪点（会显得像塑料）' })

/*
 * 必须项：DESIGN.md 的组件 prop 契约必须与组件真实签名一致。
 * 起因：DESIGN.md 曾把 ConfirmSheet 的关闭回调 prop 误写成 `oncancel`（真名是 `ondismiss`，
 * 组件里那行 <dialog oncancel={...}> 是原生 dialog 事件、与 prop 无关）。文档是后续 agent 的唯一依据，
 * 写错等于指示后来者把能跑的代码改成编译不过的代码——而 svelte-check 只检查代码、不会检查文档，
 * 所以这类"文档漂移"能一路全绿地交付出去。这里逐字段比对，让文档错误直接卡住验收。
 */
const COMPONENTS = join(SRC, 'components')
// 契约表里一行形如：Name { a, b?, ... }（容错 Snippet / 类型注解 / 空行）
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

// 必须项：app.css 必须含 saturate(180%) 与三层阴影
const css = readFileSync(join(SRC, 'app.css'), 'utf8')
for (const [re, why] of [
  [/saturate\(180%\)/, 'app.css 缺少 backdrop-saturate(180%)'],
  [/inset 0 1px 0/, 'app.css 缺少顶边受光 inset 高光（光要有方向）'],
  [/inset 0 -1px 0/, 'app.css 缺少底边暗缘 inset（光要有方向）'],
  [/feTurbulence/, 'app.css 缺少 feTurbulence 颗粒噪点'],
  [/cubic-bezier\(0\.16,\s*1,\s*0\.3,\s*1\)/, 'app.css 缺少 spring easing cubic-bezier(0.16,1,0.3,1)'],
  [/#e4b863/i, 'app.css 缺少唯一强调色香槟金 #E4B863']
]) {
  if (!re.test(css)) violations.push({ rel: 'src/app.css', line: 0, text: '', why })
}

// 回归守卫：给行内元素（标记里是 <span>）设 width/height 的类必须显式 display:block 或
// 处于 flex/grid 容器中（此时子元素被 blockify，宽高有效），否则行内盒忽略宽高 → 元素恒为 0 尺寸。
// 2026-10-15 状态页两个功耗条恒空即此因：.g-progress__fill 漏了上游 ak-ui 的 display:block，
// 而它的父元素 .g-progress 是 grid、但 fill 的**直接**父 .g-progress__track 不是容器，
// 所以它既不是 flex item 也没有 display → 必须靠这条断言兜住。此前被误诊为宽度/变量问题。
//
// 两处坑（本脚本第一版都踩了，故留下说明）：
// ① 必须先在「剥掉注释的副本」上判定——注释里写着 display:block 是在解释规则本身，
//    不剥离则判据被自己的说明文字满足 → 守卫永不触发；
// ② 判定 flex item 必须用「元素栈」求真实直接父节点。用正则放宽成 `(?:,[^{]*)?` 之类的
//    近似写法会触发灾难性回溯（88 条规则只剩 2 条匹配），整个守卫静默失效。
const cssNoComment = css.replace(/\/\*[\s\S]*?\*\//g, m => m.replace(/[^\n]/g, ' '))
const cssRules = new Map()
for (const m of cssNoComment.matchAll(/\.([A-Za-z][\w-]*)\s*\{([^{}]*)\}/g)) {
  cssRules.set(m[1], m[2])
}
const isContainerCls = cls => {
  const b = cssRules.get(cls)
  return !!b && /display\s*:\s*(inline-)?(flex|grid)/.test(b)
}
// 收集「直接父元素是 flex/grid 容器」的类名（用元素栈，svelte 多行标签也能正确配对）
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
