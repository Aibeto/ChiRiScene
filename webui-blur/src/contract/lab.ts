// lab.ts: [read] [write]
// 实验室（rhine.chr）契约：文件对外暴露、支持手改，读写的唯一权威是它本身，界面不缓存也不推测守护进程有没有套用成功；文件不存在 = 合法未启用（daemon 启动时补建），关闭写空内容而非删文件，锁定期间关闭会被 daemon 挡回，写空 ≠ 强制关闭（off）
import { absOf, shQuote } from './paths'
import { isLive, run } from '@/kernel/shell'
import { absent, failed, ok, shellError, type ReadResult } from './errors'
import { exists, readText } from './read'
import { utf8ToBase64 } from './meta'
import {
  LAB_LOCK_DIRS,
  LAB_LOCK_NAME,
  labFileContent,
  parseLabLock,
  parseLabState,
  type LabLock,
  type LabState,
  type LabWriteTarget
} from '@/data/lab'

/** rhine.chr 单行内容，4KB 足够，防止读到异常大的文件 */
const LAB_READ_BYTES = 4 * 1024

export interface LabSnapshot {
  /** rhine.chr 绝对路径 */
  path: string
  state: LabState
  /** 锁定标记：存在就表示「本次开机后启用过」，运行时关不掉 */
  lock: LabLock
  /** rhine-back.chr 是否存在（原值快照 = 还原能力） */
  hasBackup: boolean
}

// [read]
/** 按候选目录顺序找锁定标记，都没有就是未锁定（顺序与守护进程一致） */
async function readLock(): Promise<LabLock> {
  for (const dir of LAB_LOCK_DIRS) {
    const r = await readText(`${dir}/${LAB_LOCK_NAME}`, 'not-created', LAB_READ_BYTES)
    if (r.kind === 'ok') return parseLabLock(r.value, true)
  }
  return parseLabLock('', false)
}

export async function readLab(): Promise<ReadResult<LabSnapshot>> {
  const path = absOf('rhine')
  const [text, lock, hasBackup] = await Promise.all([
    readText(path, 'not-created', LAB_READ_BYTES),
    readLock(),
    exists(absOf('rhineBack'))
  ])
  if (text.kind === 'failed') return text
  const raw = text.kind === 'ok' ? text.value : ''
  return ok({ path, state: parseLabState(raw), lock, hasBackup })
}

// [write]
/** 写实验室状态：mode 为 null = 关闭（写空内容）、为保留字 off = 强制关闭、否则是模式 key；先落同目录临时文件再原子替换（与配置页同款，后缀同为 .webui.tmp），避免守护进程读到半截内容 */
export async function writeLabMode(mode: LabWriteTarget): Promise<ReadResult<LabSnapshot>> {
  if (!isLive()) return absent<LabSnapshot>('unsupported-env')
  const path = absOf('rhine')
  const tmp = `${path}.webui.tmp`
  const b64 = utf8ToBase64(labFileContent(mode))
  const cmd =
    `printf '%s' ${shQuote(b64)} | base64 -d > ${shQuote(tmp)} && ` +
    `mv -f ${shQuote(tmp)} ${shQuote(path)} || { rm -f ${shQuote(tmp)}; exit 1; }`
  try {
    const { errno, stderr } = await run(cmd)
    if (errno !== 0) return failed<LabSnapshot>(shellError(errno, stderr))
  } catch (e) {
    return failed<LabSnapshot>(e instanceof Error ? e.message : String(e))
  }
  // 写后回读：以文件实际内容为准
  return readLab()
}
