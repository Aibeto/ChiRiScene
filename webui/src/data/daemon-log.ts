// daemon-log.ts: [types] [parse]
// logs/daemon.log 行格式（src/logger.rs 的 LineEncoder，[init] 块）：
//   [2026-09-13 12:00:00] [INFO] [chiri] 消息
// 时间是**设备本地时间**；级别取自 log crate 的 LevelFilter；模块是 Rust 模块路径
// 且已剥掉 crate 名前缀（`chiri::chiri::config` → `chiri::config`，包名与子模块
// 同名产生的重复段，daemon 侧编码时裁剪）。多行消息（含换行）会以不带前缀的
// 续行出现，解析时并入上一条。

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

/**
 * 显示用时间戳：隐藏年份（窗口通常只覆盖几天，年份是冗余信息）。
 * 解析正则保持完整格式不动，这里只裁剪展示层。
 */
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
 * 解析日志文本为结构化行。窗口从行中间开始时，首行往往残缺——
 * 若首行不匹配格式则直接丢弃（它无法对应任何完整事件）。
 * 返回数组保持文件顺序（旧 → 新）。
 *
 * [logs] continuation：增量解析的「上一条」锚点（无参调用方零影响）。有值时
 * 视为解析已开始（started=true），chunk 首部不带前缀的续行（daemon 多行
 * message）会**原地并入该条目的 message** 而不是被丢弃；返回数组首元素就是
 * continuation 本体，调用方拼接新行时需自行把它从结果里去掉。
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
    if (!started) continue // 窗口开头被截断的残行
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
