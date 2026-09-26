// live-time.ts: [parse]
// LiveTime.chr：守护进程每 15s 写本地时间 MM:SS（src/logger.rs::write_live_time），
// 与本机时间比差判调度存活：差值 > LIVE_TIME_TOLERANCE_SECONDS 即已关闭（文件缺失同判）。

/** 文件名（与守护进程 logger.rs::LIVE_TIME_CHR 对齐） */
export const LIVE_TIME_FILE = 'LiveTime.chr'

/** 心跳容差（秒）：守护进程 15s 写一次，容差留 5s 余量 */
export const LIVE_TIME_TOLERANCE_SECONDS = 20

/** 解析 `MM:SS` 为小时内秒数（0..3599）：取首个非空行，分/秒须在 00-59；无有效行或格式非法返回 null。 */
export function parseLiveTimeSeconds(text: string): number | null {
  for (const raw of text.split('\n')) {
    const line = raw.trim()
    if (!line) continue
    const m = /^(\d{1,2}):([0-5]\d)$/.exec(line)
    if (!m) return null
    const minutes = Number(m[1])
    if (minutes > 59) return null
    return minutes * 60 + Number(m[2])
  }
  return null
}

/** 心跳年龄（秒）：差值按 1 小时取模（文件只有分秒）；落在 (1800,3600) 表示文件时间超前（时钟回拨或旧值），同样是过期。 */
export function liveTimeAgeSeconds(fileSeconds: number, nowSeconds: number): number {
  return (nowSeconds - fileSeconds + 3600) % 3600
}

/** 心跳是否新鲜（年龄 ≤ 容差）——即守护进程正在写心跳 */
export function isLiveTimeFresh(fileSeconds: number, nowSeconds: number): boolean {
  return liveTimeAgeSeconds(fileSeconds, nowSeconds) <= LIVE_TIME_TOLERANCE_SECONDS
}

/** 本机当前时间的「小时内秒数」（与心跳文件同口径） */
export function nowSecondsOfHour(date: Date = new Date()): number {
  return date.getMinutes() * 60 + date.getSeconds()
}
