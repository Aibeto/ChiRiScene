// sources.ts: [device] [files] [logs] [readMany]
// 各接触点的原始读取入口（不解析，解析见 src/data/*）。
// 缺失语义：白名单类文件只有 ChiRi 机型会产生（→ chiri-only）；日志/快照类
// 是「尚未产生」（→ not-created）；真正的失败一律 failed，绝不吞成空值。
import { REL, absOf, shQuote } from './paths'
import { listDir, readTail, readText } from './read'
import { readActiveConfigRel } from './meta'
import type { ReadResult } from './errors'
import { absent, failed, ok, shellError } from './errors'
import { isLive, run } from '@/kernel/shell'

// [device]
/** 设备形态：非 ChiRi 机型与读不到生效配置时都是 unknown */
export type DeviceKind = 'chiri' | 'unknown'

let deviceCache: DeviceKind | null = null

/**
 * 是否 ChiRi 专属机型：以 active_config.chr 是否指向 SoC 子目录（含 '/'）为判据
 * （与 daemon 的 is_chiri_soc 同结果：只有 ChiRi 的生效配置在 config/{soc}/ 下）。
 * 仅用于界面呈现「不适用」等文案，不作为权限或写入判断依据。
 */
export async function deviceKind(): Promise<DeviceKind> {
  if (deviceCache !== null) return deviceCache
  const rel = await readActiveConfigRel()
  deviceCache = rel.kind !== 'ok' || !rel.value.includes('/') ? 'unknown' : 'chiri'
  return deviceCache
}


// [files]
/** current_mode.chr：无换行、无空格；取值由 determine_mode 决定，可能是未注册的模式名 */
export async function readCurrentModeRaw(): Promise<ReadResult<string>> {
  const r = await readText(absOf('currentMode'), 'not-created', 256)
  if (r.kind !== 'ok') return r
  return ok(r.value.trim())
}

/** rules.yaml（磁盘展示副本，只读，无写入口） */
export function readRulesRaw(): Promise<ReadResult<string>> {
  return readText(absOf('rules'), 'not-created', 256 * 1024)
}

/** special_tuned.yaml：仅精确包名条目；只有 ChiRi 机型且 daemon 启动过才有 */
export function readSpecialTunedRaw(): Promise<ReadResult<string>> {
  return readText(absOf('specialTuned'), 'chiri-only', 256 * 1024)
}

/** fas_whitelist.yaml：每行「包名:配置名」 */
export function readFasWhitelistRaw(): Promise<ReadResult<string>> {
  return readText(absOf('fasWhitelist'), 'chiri-only', 256 * 1024)
}

/** module.prop：模块名/版本/作者等 */
export function readModulePropRaw(): Promise<ReadResult<string>> {
  return readText(absOf('moduleProp'), 'not-created', 8 * 1024)
}

// [logs]
/** 本次运行的守护进程日志尾部（单文件上限 50MB，禁止整读；日志页每秒解析，
 * 窗口只留尾部展示所需（64KB），减半自 128KB——窗口越大每秒解析的白付越多 */
export function readDaemonLogTail(bytes = 64 * 1024): Promise<ReadResult<string>> {
  return readTail(absOf('daemonLog'), bytes, 'not-created')
}

/** 每秒状态快照尾部（8MB 上限；仅 ChiRi 产生） */
export function readStatusCsvTail(bytes = 256 * 1024): Promise<ReadResult<string>> {
  return readTail(absOf('statusCsv'), bytes, 'not-created')
}

/** devimp 目录文件列表（惰性创建：`main_open`/`aff_open` 写入前才 `create_dir_all`，dev_record 关闭时不产生任何数据、连空目录都没有；仅 ChiRi 有内容） */
export function listDevimp(): Promise<ReadResult<string[]>> {
  return listDir(absOf('devimpDir'), 'not-created')
}

/** logd 归档包列表（只有发生过归档才存在） */
export function listLogd(): Promise<ReadResult<string[]>> {
  return listDir(absOf('logdDir'), 'not-created')
}

/**
 * 删除历史归档：只删 logd/（历次重启的归档包，整目录删掉即可——下次归档自动重建）。
 * devimp/ 不是历史归档：它是当前诊断写入现场（main_/aff_ 文件），调度启动归档时才
 * 整体打包进 logd 并清空，在这里删它会毁掉进行中的诊断记录。当前会话的 logs/
 * （daemon.log / status.csv）同样不在此列。
 */
export async function clearArchives(): Promise<ReadResult<true>> {
  if (!isLive()) return absent<true>('unsupported-env')
  const { errno, stderr } = await run(
    `rm -rf ${shQuote(absOf('logdDir'))} && echo done`
  )
  return errno === 0 ? ok(true) : failed<true>(shellError(errno, stderr))
}

// [readMany]
/** 批量读规格：key 是调用方取结果用的键，path 为设备绝对路径 */
export interface ReadManySpec {
  key: string
  path: string
  /** 可选：只取文件末尾 N 字节（大文件防整读，如 status.csv） */
  tailBytes?: number
}

export type ReadManyResult = ReadResult<Record<string, string | null>>

/** BEGIN/END 标记模板：带序号防内容碰撞——解析按序号顺序从上一个 END 之后推进，
 * 前一个文件的内容即使恰好含后一个文件的标记串也不会被误切 */
const RM_BEGIN = (i: number): string => `__CHIRI_RM_${i}_BEGIN__`
const RM_END = (i: number): string => `__CHIRI_RM_${i}_END__`

/**
 * 一次 shell exec 完成多个文件的读取。每个 fork 的 su -c 都很贵（总览页秒级轮询
 * 曾达 9 次 exec/s），把 N 次读拼成一条脚本只付一次 fork，是轮询降开销的原语。
 * 单个文件缺失不算失败（对应项为 null）；只有 exec 本身失败（桥错误/命令整体失败）
 * 才返回 failed——与 readText 的三分类口径一致。
 */
export async function readMany(specs: readonly ReadManySpec[]): Promise<ReadManyResult> {
  if (!isLive()) return absent<Record<string, string | null>>('unsupported-env')
  if (specs.length === 0) return ok({})
  const script = specs
    .map((s, i) => {
      const q = shQuote(s.path)
      const body =
        s.tailBytes !== undefined ? `tail -c ${s.tailBytes} ${q} || cat ${q}` : `cat ${q}`
      return `if [ -f ${q} ]; then echo ${shQuote(RM_BEGIN(i))}; ${body}; echo ${shQuote(RM_END(i))}; fi`
    })
    .join('\n')
  try {
    // 整条脚本经 shQuote 再交给 sh -c：换行与单引号都在转义出口内，禁止裸拼
    const { errno, stdout, stderr } = await run(`sh -c ${shQuote(script)}`)
    if (errno !== 0) return failed<Record<string, string | null>>(shellError(errno, stderr))
    const value: Record<string, string | null> = {}
    for (const s of specs) value[s.key] = null
    let pos = 0
    for (let i = 0; i < specs.length; i++) {
      const beginTag = RM_BEGIN(i)
      const endTag = RM_END(i)
      const bi = stdout.indexOf(`${beginTag}\n`, pos)
      if (bi < 0) continue // 文件缺失（if -f 跳过）：保持 null
      const ei = stdout.indexOf(endTag, bi + beginTag.length + 1)
      if (ei < 0) continue // 标记不完整按缺失处理，不升级为失败
      // BEGIN 换行后到 END 标记前即文件原文；结尾换行观感同 readTail（tail 原样保留）
      value[specs[i].key] = stdout.slice(bi + beginTag.length + 1, ei)
      pos = ei + endTag.length
    }
    return ok(value)
  } catch (e) {
    return failed<Record<string, string | null>>(e instanceof Error ? e.message : String(e))
  }
}

export { REL }
