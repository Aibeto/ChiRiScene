// 构建期拉取 WebUI 字体（二进制不入库，每次构建从 jsdelivr 取）。
//
// 分工：拉丁/数字 → Poppins（'ChiRi Sans'）；中文 → Noto Sans SC（'ChiRi Sans CJK'）；
// 等宽 → JetBrains Mono（已入库 assets/fonts/mono-*.woff2，不经本脚本）。均为 SIL OFL 1.1，
// 许可正文随仓库入库：LICENSE-OFL-{poppins,noto-sans-sc,jetbrains-mono}.txt。
//
// 中文字体须子集化：①只留 CJK 码位——上游 chinese-simplified 子集自带完整 ASCII，原样内嵌
// 会顶掉 Poppins 把英文也变成思源黑体；裁掉拉丁后两字体互不干扰，也无需 unicode-range。
// ②体积：整份 1.14MB/字重 → 按仓库用字裁到约 145KB/字重。
// 字表来源：WebUI 会渲染到的全部中文出处（i18n zh、组件内联文案、daemon 的 zh.ftl、
// rules.yaml 与 config/**/*.yaml）；字表外生僻字回落系统 CJK（同为思源黑体，观感无缝）。
// 产物写入 src/assets/fonts/（已被 .gitignore 忽略）。
import { readFileSync, writeFileSync, readdirSync, existsSync, statSync, mkdirSync, renameSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import subsetFont from 'subset-font'

const HERE = dirname(fileURLToPath(import.meta.url))
const REPO = join(HERE, '..', '..')
const OUT_DIR = join(HERE, '..', 'src', 'assets', 'fonts')
const POPPINS = 'https://cdn.jsdelivr.net/npm/@fontsource/poppins@5.2.5/files'
const NOTO = 'https://cdn.jsdelivr.net/npm/@fontsource/noto-sans-sc@5.2.5/files'

// 拉丁几何字体：fontsource 的 latin 子集已是最小集，无需再裁
const PLAIN = [
  ['poppins-regular.woff2', `${POPPINS}/poppins-latin-400-normal.woff2`],
  ['poppins-semibold.woff2', `${POPPINS}/poppins-latin-600-normal.woff2`],
  ['poppins-bold.woff2', `${POPPINS}/poppins-latin-700-normal.woff2`]
]

// 中文字体：需按仓库用字裁到 CJK 码位
const SUBSET = [
  ['noto-sans-sc-400.woff2', `${NOTO}/noto-sans-sc-chinese-simplified-400-normal.woff2`],
  ['noto-sans-sc-700.woff2', `${NOTO}/noto-sans-sc-chinese-simplified-700-normal.woff2`]
]

async function download(url) {
  const res = await fetch(url)
  if (!res.ok) throw new Error(`[fonts] 拉取失败 HTTP ${res.status}：${url}`)
  const buf = Buffer.from(await res.arrayBuffer())
  if (buf.length === 0) throw new Error(`[fonts] 拉取到空内容：${url}`)
  return buf
}

function save(name, buf) {
  const dest = join(OUT_DIR, name)
  const tmp = `${dest}.tmp`
  writeFileSync(tmp, buf)
  // tmp→rename：避免半截文件被 vite 打进产物
  renameSync(tmp, dest)
  console.log(`[fonts] ${name} → ${(buf.length / 1024).toFixed(0)} KB`)
}

function cached(name) {
  const dest = join(OUT_DIR, name)
  if (existsSync(dest) && statSync(dest).size > 0) {
    console.log(`[fonts] ${name} 已存在（${(statSync(dest).size / 1024).toFixed(0)} KB），跳过`)
    return true
  }
  return false
}

// —— 1. 收集中文子集字表 ——
// CJK 码位范围（自上而下）：部首补充、CJK 标点、笔画、扩展 A、统一表意、兼容表意、
// CJK 兼容形式、全角/半角形式（拉丁/西文标点一律不要）
const isCjk = cp =>
  (cp >= 0x2e80 && cp <= 0x2eff) ||
  (cp >= 0x3000 && cp <= 0x303f) ||
  (cp >= 0x31c0 && cp <= 0x31ef) ||
  (cp >= 0x3400 && cp <= 0x4dbf) ||
  (cp >= 0x4e00 && cp <= 0x9fff) ||
  (cp >= 0xf900 && cp <= 0xfaff) ||
  (cp >= 0xfe30 && cp <= 0xfe4f) ||
  (cp >= 0xff00 && cp <= 0xffef)

const sources = []
function walk(dir, filter) {
  if (!existsSync(dir)) return
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name)
    if (e.isDirectory()) walk(p, filter)
    else if (filter(p)) sources.push(p)
  }
}
walk(join(REPO, 'webui', 'src'), p => /\.(ts|svelte)$/.test(p))
walk(join(REPO, 'module', 'config'), p => /\.ya?ml$/.test(p))
sources.push(
  join(REPO, 'webui', 'index.html'),
  join(REPO, 'module', 'config', 'i18n', 'zh.ftl'),
  join(REPO, 'module', 'rules.yaml')
)

const chars = new Set()
for (const f of sources) {
  if (!existsSync(f)) { console.warn(`[fonts] 跳过缺失文件 ${f}`); continue }
  for (const ch of readFileSync(f, 'utf8')) if (isCjk(ch.codePointAt(0))) chars.add(ch)
}
const text = [...chars].sort().join('')
if (text.length < 100) throw new Error(`[fonts] 字表异常（仅 ${text.length} 字），拒绝产出残缺字体`)

// —— 2. 拉取并落地 ——
mkdirSync(OUT_DIR, { recursive: true })
console.log(`[fonts] 中文子集字表：${text.length} 个 CJK 字符（扫描 ${sources.length} 个源文件）`)

// 拉丁字体内容恒定，按「文件已存在」跳过即可
for (const [name, url] of PLAIN) {
  if (cached(name)) continue
  save(name, await download(url))
}

// 中文字体按当前字表裁出，字表一变产物即作废——只按存在性跳过会让新增文案的字永远进不了
// 子集。故用**字表哈希**当缓存键：哈希一致且文件齐备才跳过，否则重裁。
const digest = createHash('sha256').update(text).digest('hex')
const stampFile = join(OUT_DIR, '.subset-charset')
const stamp = existsSync(stampFile) ? readFileSync(stampFile, 'utf8').trim() : ''
const fresh = stamp === digest && SUBSET.every(([name]) => existsSync(join(OUT_DIR, name)))

if (fresh) {
  console.log(`[fonts] 中文字体子集与当前字表一致（${digest.slice(0, 8)}），跳过`)
} else {
  for (const [name, url] of SUBSET) {
    const out = await subsetFont(await download(url), text, { targetFormat: 'woff2' })
    if (!out || out.length === 0) throw new Error(`[fonts] 子集化为空：${name}`)
    save(name, out)
  }
  // 哈希落盘放在字体之后：中途失败则下次仍会重裁，不会留下「哈希是新的、字体是旧的」
  writeFileSync(stampFile, digest)
}
