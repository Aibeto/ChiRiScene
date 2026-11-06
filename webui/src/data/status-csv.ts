// status-csv.ts: [columns] [types] [parse] logs/status.csv 契约（src/logger.rs STATUS_HEADER / status_log_snapshot）：28 列、首行表头、type 恒为 snap，timestamp 是设备本地时间 HH:MM:SS.mmm（无日期），缺测值统一 -，screen_on/clg_active 用 0/1，screen_prop 是 debug.tracing.screen_state 原始值，fps 仅 FAS 激活且帧窗口有样本时有效
// cpu_dyn_w（CPU 动态功率估计，W，无功耗表的 SoC 恒 -）/ resid_w = batt_power_w − cpu_dyn_w（残差 = 屏幕/modem/GPU/静态），充电行 resid_w 无意义，只看放电行
// 末列 daemon_rss_kb 是 daemon 自身 VmRSS（kB）——只用于跨会话看增长趋势，单点值受分配器影响不可作判据；旧 27 列文件兼容读取

// [columns]
export const STATUS_COLUMNS = [
  'timestamp',
  'type',
  'mode',
  'package',
  'charge',
  'screen_on',
  'batt_temp',
  'cpu_temp',
  'thermal_cap_pct',
  'thermal_free_pct',
  'clg_active',
  'psi_cpu_some',
  'psi_io_some',
  'psi_mem_some',
  'gpu_busy_pct',
  'batt_voltage_v',
  'batt_current_ma',
  'batt_power_w',
  'wakeups',
  'migrations',
  'freq_trans',
  'fps',
  'screen_prop',
  'daemon_utime_ms',
  'daemon_stime_ms',
  'cpu_dyn_w',
  'resid_w',
  'daemon_rss_kb'
] as const

export const STATUS_COLUMN_COUNT = STATUS_COLUMNS.length

// [types]
export interface StatusRow {
  /** 设备本地时间 HH:MM:SS.mmm（无日期，跨天需靠归档名/行序推断） */
  timestamp: string
  mode: string
  /** 前台包名，启动初期可能为空串 */
  pkg: string
  /** charging / discharging / full / not_charging / '-' */
  charge: string
  screenOn: boolean
  battTemp: number | null
  cpuTemp: number | null
  /** 热保护上限百分比 */
  thermalCap: number | null
  thermalFree: number | null
  clgActive: boolean
  psiCpu: number | null
  psiIo: number | null
  psiMem: number | null
  gpuBusy: number | null
  battVoltage: number | null
  battCurrent: number | null
  battPower: number | null
  wakeups: number | null
  migrations: number | null
  freqTrans: number | null
/** FAS 帧率；未启动 FAS 时为 null（CSV 里是 '-'） */
  fps: number | null
  /** debug.tracing.screen_state 原始值；属性缺失为空串（CSV 里是 '-'） */
  screenProp: string
  /** daemon 自身累计用户态 CPU 时间（ms，自测量基线；读 /proc/self/stat 失败为 null） */
  daemonUtimeMs: number | null
  /** daemon 自身累计内核态 CPU 时间（ms，自测量基线；读 /proc/self/stat 失败为 null） */
  daemonStimeMs: number | null
  /** daemon 自身 VmRSS（kB）；旧 27 列文件或读失败为 null */
  daemonRssKb: number | null
}

// [parse]
const NA = '-'

function num(raw: string | undefined): number | null {
  if (raw === undefined) return null
  const v = raw.trim()
  if (!v || v === NA) return null
  const n = Number(v)
  return Number.isFinite(n) ? n : null
}

function bool(raw: string | undefined): boolean {
  return raw?.trim() === '1'
}

function str(raw: string | undefined): string {
  const v = (raw ?? '').trim()
  return v === NA ? '' : v
}

/** 解析 CSV 为快照行：窗口可能从行中间开始（tail -c），按「字段数必须是 27 或 28」过滤残缺行（28 = 现役含 daemon_rss_kb，27 = 旧归档兼容），表头按首列名识别；按文件顺序返回，展示方向由调用方定 */
export function parseStatusCsv(text: string, maxRows = 300): StatusRow[] {
  const rows: StatusRow[] = []
  const lines = text.split('\n')
  for (const line of lines) {
    const raw = line.trim()
    if (!raw) continue
    const cells = raw.split(',')
    // 27 = 2026-11-07 之前的归档（无 daemon_rss_kb），28 = 现役表头；其余一律按残缺行丢弃
    if (cells.length !== STATUS_COLUMN_COUNT && cells.length !== STATUS_COLUMN_COUNT - 1) continue
    if (cells[0] === 'timestamp' || cells[1] !== 'snap') continue
    rows.push({
      timestamp: str(cells[0]),
      mode: str(cells[2]),
      pkg: str(cells[3]),
      charge: str(cells[4]),
      screenOn: bool(cells[5]),
      battTemp: num(cells[6]),
      cpuTemp: num(cells[7]),
      thermalCap: num(cells[8]),
      thermalFree: num(cells[9]),
      clgActive: bool(cells[10]),
      psiCpu: num(cells[11]),
      psiIo: num(cells[12]),
      psiMem: num(cells[13]),
      gpuBusy: num(cells[14]),
      battVoltage: num(cells[15]),
      battCurrent: num(cells[16]),
      battPower: num(cells[17]),
      wakeups: num(cells[18]),
      migrations: num(cells[19]),
      freqTrans: num(cells[20]),
      fps: num(cells[21]),
      screenProp: str(cells[22]),
      daemonUtimeMs: num(cells[23]),
      daemonStimeMs: num(cells[24]),
      daemonRssKb: num(cells[27])
    })
  }
  return rows.length > maxRows ? rows.slice(rows.length - maxRows) : rows
}
