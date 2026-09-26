// daemon.ts: [liveness] [watchdog] [stop] [recover]
// 守护进程存活判据与生命周期操作硬规则：
// - 存活判据 = 心跳文件 LiveTime.chr（daemon 每 15s 写一次本地时间 MM:SS），与本机
// 时间比对超容差（20s）判「已停止」；旧 flock 探测已弃用——daemon.lock 由 daemon
// 自持，WebUI 不探测也绝不删除（flock 是 inode 级，删除会让新实例在旧 inode 外加锁）；
// - 关闭调度必须先杀看门狗再杀主进程，否则看门狗会把 daemon 拉回来
import { absOf, shQuote } from './paths'
import { exists, readText } from './read'
import {
  LIVE_TIME_FILE,
  isLiveTimeFresh,
  nowSecondsOfHour,
  parseLiveTimeSeconds
} from '@/data/live-time'
import { isLive, run } from '@/kernel/shell'
import { absent, failed, ok, shellError, type ReadResult } from './errors'

/** 常驻状态通知的 tag（与 daemon 侧 src/notify.rs::TAG 对齐）：更新与取消都靠它定位 */
const NOTIFY_TAG = 'chiri-status'

// [liveness]
export type DaemonState =
  /** 心跳新鲜 = 有活跃实例 */
  | 'running'
  /** 心跳过期或文件缺失 = 没有实例在跑 */
  | 'stopped'
  /** 无法判定（环境不支持 / 读失败 / 内容非法）——界面如实显示，不猜 */
  | 'unknown'

/** 心跳文件极小（`MM:SS\n`，64 字节足够） */
const LIVE_TIME_READ_BYTES = 64

/**
 * [liveness] 存活纯判定：从心跳原文出发、不发任何 IO，loadOverview 批量读与
 * probeLiveness 共用此口径：raw null（文件缺失）→ stopped；内容非法 → unknown +
 * 错误详情（读到了但不可用，不谎报「已停止」）；其余按心跳新鲜度二分
 */
export function judgeLiveness(raw: string | null): {
  state: DaemonState
  error: string
} {
  if (raw === null) return { state: 'stopped', error: '' }
  const fileSeconds = parseLiveTimeSeconds(raw)
  if (fileSeconds === null) {
    return {
      state: 'unknown',
      error: `${LIVE_TIME_FILE} 内容非法：${raw.trim() || '(空)'}`
    }
  }
  const fresh = isLiveTimeFresh(fileSeconds, nowSecondsOfHour())
  return { state: fresh ? 'running' : 'stopped', error: '' }
}

/**
 * 探测存活：读心跳文件并与本机时间比对（判定复用 judgeLiveness）秒级轮询已走
 * readMany 批量读，此独立入口保留给单次探测调用方（如停止调度后的复核）
 */
export async function probeLiveness(): Promise<ReadResult<DaemonState>> {
  if (!isLive()) return absent<DaemonState>('unsupported-env')
  const r = await readText(absOf('liveTime'), 'not-created', LIVE_TIME_READ_BYTES)
  if (r.kind === 'absent') return ok<DaemonState>('stopped')
  if (r.kind !== 'ok') return r
  const j = judgeLiveness(r.value)
  if (j.error !== '') return failed<DaemonState>(j.error)
  return ok<DaemonState>(j.state)
}

// [watchdog]
/**
 * 看门狗 PID（契约保留 API：关闭调度在 shell 内联读取，此函数供界面展示/诊断用）
 * 两种写入格式并存（sh 看门狗带换行、daemon 自愈不带）→ 读取必须 trim
 */
export async function readWatchdogPid(): Promise<ReadResult<number>> {
  const r = await readText(absOf('watchdogPid'), 'not-created', 256)
  if (r.kind !== 'ok') return r
  const raw = r.value.trim()
  if (!/^\d+$/.test(raw)) return failed<number>(`watchdog.pid 内容非法：${raw || '(空)'}`)
  return ok(Number(raw))
}

// [stop]
/**
 * 关闭调度：先按 pidfile 杀看门狗 → 再杀 chiri → 删 pidfile → 撤销常驻通知
 * 副作用（由界面负责提示）：daemon 被信号杀死、没有还原逻辑，fast 模式锁频会残留，
 * 恢复只能点模块 Action 或重启设备
 */
export async function stopScheduler(): Promise<ReadResult<true>> {
  if (!isLive()) return absent<true>('unsupported-env')
  const pidFile = shQuote(absOf('watchdogPid'))
  const cmd =
    `p=$(cat ${pidFile} 2>/dev/null); ` +
    `case "$p" in ''|0|*[!0-9]*) ;; *) kill "$p" 2>/dev/null ;; esac; ` +
    `killall -9 chiri 2>/dev/null || pkill -9 chiri 2>/dev/null; ` +
    `rm -f ${pidFile}; ` +
    // daemon 被 -9 杀死无清理时机，常驻通知由这里撤销（失败静默，部分 ROM 无 -d 旗标）
    `cmd notification post -d ${NOTIFY_TAG} >/dev/null 2>&1; ` +
    `sleep 1; echo done`
  try {
    const { errno, stderr } = await run(cmd)
    if (errno !== 0) return failed<true>(shellError(errno, stderr))
    return ok(true)
  } catch (e) {
    return failed<true>(e instanceof Error ? e.message : String(e))
  }
}

// [recover]
/** 恢复入口只是模块 Action（WebUI 内不能自己 nohup 拉起：ksu.exec 会清理进程组） */
export async function hasActionScript(): Promise<boolean> {
  return exists(absOf('actionSh'))
}
