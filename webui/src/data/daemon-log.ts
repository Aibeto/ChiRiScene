// daemon-log.ts: [types] [parse]
// logs/daemon.log 行格式（src/logger.rs LineEncoder，[init] 块）：[时间] [级别] [模块] 消息。
// 时间是设备本地时间；模块已剥 crate 前缀；多行消息以不带前缀的续行出现，解析时并入上一条。

// [types]
export type LogLevelName = 'OFF' | 'ERROR' | 'WARN' | 'INFO' | 'DEBUG' | 'TRACE' | 'OTHER'

export interface LogLine {
  /** 本地时间字符串，解析失败时为空 */
  time: string
  level: LogLevelName
  /** Rust 模块路径（已剥 crate 前缀），如 chiri、chiri::config */
  module: string
  message: string
}

const LINE_RE = /^\[(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})\]\s*\[([A-Za-z]+)\]\s*\[([^\]]*)\]\s?([\s\S]*)$/

/** 显示用时间戳：隐藏年份（窗口通常只覆盖几天）；解析正则保持完整格式，只裁剪展示层。 */
function hideYear(time: string): string {
  const m = /^\d{4}-(\d{2}-\d{2} \d{2}:\d{2}:\d{2})/.exec(time)
  return m ? m[1] : time
}

function normalizeLevel(raw: string): LogLevelName {
  const up = raw.toUpperCase()
  switch (up) {
    case 'OFF':
    case 'ERROR':
    case 'WARN':
    case 'INFO':
    case 'DEBUG':
    case 'TRACE':
      return up
    default:
      return 'OTHER'
  }
}

// [parse]
/** 展示上限：与增量路径（state.svelte.ts 的 concat 后裁剪）共用同一口径 */
export const LOG_MAX_LINES = 2000

/**
 * 解析日志文本为结构化行，按文件顺序（旧 → 新）返回。窗口从行中间开始时首行往往残缺，不匹配格式则丢弃。
 * [logs] continuation：增量解析的「上一条」锚点（无参调用方零影响）。有值时视为解析已开始，chunk 首部
 * 不带前缀的续行会原地并入该条目；返回数组首元素就是 continuation 本体，调用方拼接新行时需自行去掉。
  */
 export function parseDaemonLog(
  text: string,
  maxLines = LOG_MAX_LINES,
  continuation: LogLine | null = null
): LogLine[] {
  const lines = text.split('\n')
  const out: LogLine[] = continuation !== null ? [continuation] : []
  let started = continuation !== null

  for (const line of lines) {
    const m = LINE_RE.exec(line)
    if (m) {
      started = true
      out.push({
        time: hideYear(m[1]),
        level: normalizeLevel(m[2]),
        module: m[3],
        message: m[4]
      })
      continue
    }
    // 窗口开头被截断的残行，丢弃
    if (!started) continue
    if (!line.trim()) continue
    const last = out[out.length - 1]
    if (last) last.message += `\n${line}`
  }

  return out.length > maxLines ? out.slice(out.length - maxLines) : out
}

/** 按级别过滤（界面上的级别筛选） */
export function filterByLevel(lines: LogLine[], min: LogLevelName): LogLine[] {
  const order: LogLevelName[] = ['TRACE', 'DEBUG', 'INFO', 'WARN', 'ERROR', 'OFF', 'OTHER']
  const threshold = order.indexOf(min)
  if (threshold < 0) return lines
  return lines.filter(l => {
    const idx = order.indexOf(l.level)
    return idx < 0 ? true : idx >= threshold
  })
}
