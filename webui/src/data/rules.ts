// rules.ts: [types] [parse] [group]
// rules.yaml 磁盘副本是 daemon 启动时复制的展示副本（运行时一律读二进制内嵌值），WebUI 只读展示；fas_rules 段随包文件不含，解析时不能假设存在，字段来源 src/monitor/config.rs::RulesConfig
import { load as loadYaml } from 'js-yaml'
import type { SpecialTunedEntry } from './whitelists'

// [types]
export interface RulesInfo {
  /** 动态模式总开关（false 时前台永远用 global_mode） */
  dynamicEnabled: boolean
  /** 全局模式（dynamic_enabled=false 或应用未命中时的取值） */
  globalMode: string
  /** 应用 → 模式（模块随附维护，WebUI 只读展示）；键可能是包名或 `pkg/.Activity`（activity 级） */
  appModes: Record<string, string>
  /** Activity 级规则总开关（缺省 true；false 时 `pkg/.Activity` 键不参与查表，完全回退包名级） */
  activityAware: boolean
  /** 被跳过的包名（含模糊匹配：pkg.contains(ignored)） */
  ignoredApps: string[]
  /** 解析是否成功 */
  ok: boolean
  /** 解析失败说明 */
  problem?: string
}

export const EMPTY_RULES: RulesInfo = {
  dynamicEnabled: true,
  globalMode: 'default',
  appModes: {},
  activityAware: true,
  ignoredApps: [],
  ok: false
}

// [parse]
function asBool(v: unknown, fallback: boolean): boolean {
  return typeof v === 'boolean' ? v : fallback
}

function asString(v: unknown, fallback: string): string {
  if (typeof v === 'string' && v.trim()) return v.trim()
  if (typeof v === 'number') return String(v)
  return fallback
}

function asStringMap(v: unknown): Record<string, string> {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return {}
  const out: Record<string, string> = {}
  for (const [k, val] of Object.entries(v as Record<string, unknown>)) {
    if (typeof val === 'string' && val.trim()) out[k] = val.trim()
    else if (typeof val === 'number') out[k] = String(val)
  }
  return out
}

function asStringList(v: unknown): string[] {
  if (!Array.isArray(v)) return []
  return v.filter((x): x is string => typeof x === 'string' && x.trim().length > 0)
}

export function parseRules(text: string): RulesInfo {
  try {
    const parsed = loadYaml(text)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return { ...EMPTY_RULES, problem: '文件内容不是 YAML 映射结构' }
    }
    const obj = parsed as Record<string, unknown>
    return {
      dynamicEnabled: asBool(obj.dynamic_enabled, true),
      globalMode: asString(obj.global_mode, 'default'),
      appModes: asStringMap(obj.app_modes),
      activityAware: asBool(obj.activity_aware, true),
      ignoredApps: asStringList(obj.ignored_apps),
      ok: true
    }
  } catch (e) {
    return { ...EMPTY_RULES, problem: e instanceof Error ? e.message : String(e) }
  }
}

// [group]
/** `pkg/.Activity` 规则键拆成包名 + activity；不含 `/` 的键是包名级（daemon 查表键就是完整键，拆分只用于展示分组） */
export function splitRuleKey(key: string): { pkg: string; activity: string } {
  const idx = key.indexOf('/')
  if (idx <= 0) return { pkg: key, activity: '' }
  return { pkg: key.slice(0, idx), activity: key.slice(idx + 1) }
}

export interface RuleActivity {
  /** `/` 之后的 activity 原文（与 daemon 精确查表的键一致，可能以 `.` 开头） */
  activity: string
  mode: string
}

export interface RuleGroup {
  pkg: string
  /** 包名级默认档；空串 = 该包只有 activity 级覆盖或只有特调/FAS */
  mode: string
  /** activity 级覆盖（按 activity 名排序） */
  activities: RuleActivity[]
  /** special_tuned.yaml 精确条目 */
  special?: SpecialTunedEntry
  /** fas_whitelist.yaml 配置名 */
  fasConfig?: string
  /** 命中的 ignored_apps 模糊条目；空串 = 未忽略 */
  ignored: string
}

/** 三份规则合成「按包名分组」的视图模型：app_modes（包名级 + activity 级）、特调、FAS 白名单 */
export function buildRuleGroups(
  rules: RulesInfo,
  special: ReadonlyMap<string, SpecialTunedEntry>,
  fas: ReadonlyMap<string, string>
): RuleGroup[] {
  const groups = new Map<string, RuleGroup>()
  const ensure = (pkg: string): RuleGroup => {
    let group = groups.get(pkg)
    if (!group) {
      group = { pkg, mode: '', activities: [], ignored: '' }
      groups.set(pkg, group)
    }
    return group
  }

  for (const [key, mode] of Object.entries(rules.appModes)) {
    const { pkg, activity } = splitRuleKey(key)
    if (!pkg) continue
    const group = ensure(pkg)
    if (activity) group.activities.push({ activity, mode })
    else group.mode = mode
  }
  for (const [pkg, entry] of special) ensure(pkg).special = entry
  for (const [pkg, cfg] of fas) ensure(pkg).fasConfig = cfg

  const list = [...groups.values()]
  for (const group of list) {
    group.ignored = rules.ignoredApps.find(x => group.pkg.includes(x)) ?? ''
    group.activities.sort((a, b) => a.activity.localeCompare(b.activity))
  }
  return list.sort((a, b) => a.pkg.localeCompare(b.pkg))
}

/** 规则分组过滤：应用名 / 包名 / activity / 模式 / 特调 / FAS 任一命中（大小写不敏感） */
export function filterRuleGroups(
  groups: RuleGroup[],
  keyword: string,
  labels?: Readonly<Record<string, string>>
): RuleGroup[] {
  const kw = keyword.trim().toLowerCase()
  if (!kw) return groups
  return groups.filter(
    g =>
      g.pkg.toLowerCase().includes(kw) ||
      labels?.[g.pkg]?.toLowerCase().includes(kw) === true ||
      g.mode.toLowerCase().includes(kw) ||
      g.activities.some(
        a => a.activity.toLowerCase().includes(kw) || a.mode.toLowerCase().includes(kw)
      ) ||
      g.ignored.toLowerCase().includes(kw) ||
      g.fasConfig?.toLowerCase().includes(kw) === true ||
      g.special?.modes.some(m => m.toLowerCase().includes(kw)) === true ||
      g.special?.fallback.toLowerCase().includes(kw) === true
  )
}
