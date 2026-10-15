// power.ts: [read]PowerAVG.chr 只读契约（模块根）：由 daemon 写（1s 采样，仅电池放电时计入）、WebUI 只读展示不存在与空文件统一归一为「无值」（watt = null），
// 不是错误
import { absOf } from './paths'
import { readText } from './read'
import { ok, type ReadResult } from './errors'
import { parsePowerAvgWatt } from '@/data/power-avg'

/** 单行数字，4KB 足够 */
const POWER_AVG_READ_BYTES = 4 * 1024

export interface PowerAvgSnapshot {
  /** PowerAVG.chr 绝对路径 */
  path: string
  /** 当前留存值（W）；文件缺失/为空/内容非法时为 null */
  watt: number | null
  /** 无值（文件缺失/为空/内容非法）：界面据此红字提示，而不是静默显示 — */
  missing: boolean
}

export async function readPowerAvg(): Promise<ReadResult<PowerAvgSnapshot>> {
  const path = absOf('powerAvg')
  const text = await readText(path, 'not-created', POWER_AVG_READ_BYTES)
  if (text.kind === 'failed') return text
  // 缺失与空内容一律「无值 + missing」：界面对缺失给出可解释提示，而非静默 —
  const watt = parsePowerAvgWatt(text.kind === 'ok' ? text.value : '')
  return ok({ path, watt, missing: watt === null })
}
