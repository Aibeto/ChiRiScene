// power-avg.ts: [parse]
// PowerAVG.chr：守护进程 src/logger.rs::power_avg_update 每 1s 采样写一行数字（两位小数），仅电池放电时计入（其它状态保留旧值）。
// 口径由 meta.yaml 的 power_avg 决定：false = 参考值（旧值×10 与新值 10:1 加权递推，偏历史）；true = 累计平均（等权全史，仅亮屏放电）。
// 文件只存值不存口径，界面按 meta 开关标注。

/** 文件名（与守护进程 logger.rs::POWER_AVG_CHR 对齐） */
export const POWER_AVG_FILE = 'PowerAVG.chr'

/** 解析为瓦特值：取首个非空行（忽略空行与注释）转数字；缺失/非法/负数返回 null（界面显示 —）。 */
export function parsePowerAvgWatt(text: string): number | null {
  const first = text
    .split('\n')
    .map(line => line.trim())
    .find(line => line.length > 0 && !line.startsWith('#'))
  if (first === undefined) return null
  const value = Number(first)
  if (!Number.isFinite(value) || value < 0) return null
  return value
}
