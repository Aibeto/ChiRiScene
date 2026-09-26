// state.svelte.ts: [common] [overview] [config] [apps] [logs]
// 应用级状态：把契约层的三分类读取结果映射成界面可直接渲染的状态，
// 并保证「读失败」与「不适用」在界面上是不同的表达。
import {
  hasActionScript,
  judgeLiveness,
  stopScheduler,
  type DaemonState
} from '@/contract/daemon'
import { readDown, writeDown } from '@/contract/down'
import { absOf } from '@/contract/paths'
import { pollExport, startExport } from '@/contract/export'
import { readLab, writeLabMode } from '@/contract/lab'
import { readMeta, writeMetaFields, type MetaSnapshot, type WritableField } from '@/contract/meta'
import {
  clearArchives,
  deviceKind,
  listDevimp,
  listLogd,
  readDaemonLogTail,
  readFasWhitelistRaw,
  readMany,
  readModulePropRaw,
  readRulesRaw,
  readSpecialTunedRaw,
  readStatusCsvTail,
  type DeviceKind
} from '@/contract/sources'
import { fetchInstalledPackages, buildAppEntries, filterApps, type AppEntry } from '@/data/apps'
import { LOG_MAX_LINES, parseDaemonLog, type LogLine } from '@/data/daemon-log'
import {
  LAB_FORCE_OFF,
  LAB_MODE_KEYS,
  LAB_TAKEOVER,
  labWarnings,
  type LabLock,
  type LabModeKey,
  type LabState,
  type LabWarningCode,
  type LabWriteTarget
} from '@/data/lab'
import { describeMode, type ModeInfo } from '@/data/mode'
import { EMPTY_MODULE_PROP, parseModuleProp, type ModuleProp } from '@/data/module-info'
import { EMPTY_RULES, parseRules, type RulesInfo } from '@/data/rules'
import { parseStatusCsv, type StatusRow } from '@/data/status-csv'
import { parsePowerAvgWatt } from '@/data/power-avg'
import {
  parseFasWhitelist,
  parseSpecialTuned,
  specialModeSet,
  type SpecialTunedEntry
} from '@/data/whitelists'
import { t } from '@/i18n/index.svelte'
import { toast } from '@/kernel/ksu'

/** 只读文件的界面态：缺失（不适用/尚未生成）与失败要分开 */
export type FileState = 'ok' | 'missing' | 'failed'

/** 静态项复用窗口：module.prop / meta / 白名单 / 机型变化频率低（天级），退出秒级轮询 */
const STATIC_TTL_MS = 30_000

class AppStore {
  // [common]
  ready = $state(false)
  loading = $state(false)
  /** 当前生效配置相对路径（active_config.chr） */
  configRel = $state('')
  /** 生效配置绝对路径 */
  metaPath = $state('')
  configState = $state<FileState>('ok')
  configError = $state('')
  deviceKind = $state<DeviceKind>('unknown')
  moduleProp = $state<ModuleProp>(EMPTY_MODULE_PROP)

  // [overview]
  daemonState = $state<DaemonState>('unknown')
  daemonError = $state('')
  currentMode = $state('')
  modeInfo = $state<ModeInfo>(describeMode(''))
  modeMissing = $state(false)
  /** current_mode.chr 读取「真实失败」——与「尚未生成」分开，界面上排查方向不同 */
  modeError = $state('')
  /** 特调 / FAS 白名单读取真实失败 */
  whitelistError = $state('')
  actionAvailable = $state(false)
  metaValid = $state(true)
  metaProblems = $state<string[]>([])
  metaSnapshot = $state<MetaSnapshot | null>(null)
  specialCount = $state(0)
  fasCount = $state(0)
  stopping = $state(false)

  // [config]
  saving = $state(false)
  writeError = $state('')
  writeReverted = $state(false)
  /** 待保存的改动（页面上未提交的草稿） */
  draft = $state<Partial<Record<WritableField, string | boolean>>>({})

  // [lab]
  /** rhine.chr 的解析结果（off / on / invalid），未读到前保持 off */
  labState = $state<LabState>({ kind: 'off' })
  /** rhine.chr 绝对路径 */
  labPath = $state('')
  /** 锁定标记（tmpfs 上）：存在就表示本次开机后启用过，运行时关不掉 */
  labLock = $state<LabLock>({ locked: false, mode: '', notes: [] })
  /** rhine-back.chr 是否存在（原值快照 = 还原能力） */
  labHasBackup = $state(false)
  labError = $state('')
  labLoading = $state(false)
  /** 正在写入（启用/关闭）——期间禁用按钮，避免重复提交 */
  labPending = $state(false)

  // [apps]
  apps = $state<AppEntry[]>([])
  appKeyword = $state('')
  scanning = $state(false)
  appsState = $state<FileState>('ok')
  rules = $state<RulesInfo>(EMPTY_RULES)

  // [logs]
  logLines = $state<LogLine[]>([])
  logState = $state<FileState>('missing')
  logError = $state('')
  statusRows = $state<StatusRow[]>([])
  statusState = $state<FileState>('missing')
  statusError = $state('')
  devimpFiles = $state<string[]>([])
  logdFiles = $state<string[]>([])
  /** devimp / logd 目录列举的真实失败（目录不存在是正常缺失，不算失败） */
  dirError = $state('')
  logLoading = $state(false)
  /** 自刷新失败提示的节流标志：失败 toast 一次，成功后复位（每秒轮询不能刷屏） */
  refreshFailNotified = false

  // [logs] 增量解析锚点（endsWith 内容锚定）：
  // - xxxTailCache = 上次读到的尾部窗口原文。新读尾部以它**结尾**（endsWith 命中）
  //   说明文件只在尾部追加（轮转/截断必不命中）→ 前缀部分即纯新增字节
  // - xxxAnchorLen = tailCache 里已完整解析部分的长度（按最后一个 '\n'（含）切）。
  //   尾部半行**不进显示**：写盘是行缓冲，读到无换行结尾是瞬时竞态，该行完整后
  //   下一轮照常解析（最多晚 1s 显示），正确性优先于逐字节行为一致
  /** 上次 daemon.log 尾部窗口原文（null = 无锚点，整段重解析） */
  private logTailCache: string | null = null
  /** tailCache 中已完整解析的长度（tailCache 最后一个 '\n' 之后为未解析半行） */
  private logAnchorLen = 0
  /** 上次 status.csv 尾部窗口原文（null = 无锚点，整段重解析） */
  private statusTailCache: string | null = null
  /** statusTailCache 中已完整解析的长度 */
  private statusAnchorLen = 0

  /** daemon.log 锚定失效（缺失/失败）：清空展示时一并复位，下轮从整段重解析重建 */
  private resetLogAnchor(): void {
    this.logTailCache = null
    this.logAnchorLen = 0
  }

  /** status.csv 锚定失效（缺失/失败）：同上 */
  private resetStatusAnchor(): void {
    this.statusTailCache = null
    this.statusAnchorLen = 0
  }

  /**
   * 窗口尾部的「未解析半行」长度（最后一个 '\n' 之后的字符数；无 '\n' 则整个
   * 窗口都是半行）。> 0 时本轮放弃增量走整段重解析——半行是瞬时竞态（行缓冲
   * 写盘几乎总以 \n 结尾），为它维护跨条目合并语义不值得。
   */
  private static tailPartialLen(text: string): number {
    const lastNl = text.lastIndexOf('\n')
    return lastNl >= 0 ? text.length - (lastNl + 1) : text.length
  }

  /** 已完整解析长度 = 窗口长度 - 尾部半行长度（两处锚点推进共用） */
  private static anchorLenOf(text: string): number {
    return text.length - AppStore.tailPartialLen(text)
  }

  /** 自刷新失败时的 toast（同一次失败串只提示一次） */
  notifyRefreshFail(): void {
    if (this.refreshFailNotified) return
    this.refreshFailNotified = true
    toast(t('state.refreshFailed'))
  }

  // [derived]
  /** 只有明确判定为 ChiRi 机型时才为真（无法判定时不为真，界面据此显示“无法判定”） */
  get isChiri(): boolean {
    return this.deviceKind === 'chiri'
  }

  get filteredApps(): AppEntry[] {
    return filterApps(this.apps, this.appKeyword)
  }

  get statusRowsNewestFirst(): StatusRow[] {
    return this.statusRowsReversed
  }

  /** 倒序副本：在赋值 statusRows 处同步维护（setStatusRows），getter 只返回不重建 */
  private statusRowsReversed = $state<StatusRow[]>([])

  /** statusRows 的唯一赋值入口：同步维护倒序副本，避免每次读取都 [...].reverse() */
  private setStatusRows(rows: StatusRow[]): void {
    this.statusRows = rows
    // 0/1 行时倒序 = 原序，直接共享同一数组省一次拷贝
    this.statusRowsReversed = rows.length > 1 ? [...rows].reverse() : rows
  }

  /** 展示行数上限：720 行 ≈ 12 分钟（每秒 1 行），与既有展示口径一致 */
  private static readonly STATUS_MAX_ROWS = 720

  /**
   * [logs] statusRows 的追加入口（增量解析专用）：新行 concat 到尾部、超上限
   * 从头部裁剪（CSV 每秒 append，倒序副本同步维护交给 setStatusRows）。
   * 空批次直接跳过（避免白付一次 concat + 赋值）。
   */
  private appendStatusRows(rows: StatusRow[]): void {
    if (rows.length === 0) return
    let merged = this.statusRows.concat(rows)
    if (merged.length > AppStore.STATUS_MAX_ROWS) {
      merged = merged.slice(merged.length - AppStore.STATUS_MAX_ROWS)
    }
    this.setStatusRows(merged)
  }

  // [common] → [static]
  /** 静态项上次加载时间（0 = 从未加载）：loadOverview 据此判断是否补拉 */
  private staticLoadedAt = 0
  /** 在飞的静态项请求：并发调用共享同一个 promise（理由同 loadOverview） */
  private staticJob: Promise<void> | null = null
  /** 特调白名单解析出的模式名集合（describeMode 派生用；秒级批量读路径维护） */
  private modeNameSet: Set<string> = new Set()
  /** 上次 tick 批量读到的特调白名单原文（null = 缺失）：原文未变不重解析 */
  private specialRawCache: string | null = null
  /** 上次 tick 批量读到的 FAS 白名单原文（null = 缺失）：原文未变不重解析 */
  private fasRawCache: string | null = null

  /**
   * 静态项加载（原 loadCommon 的内容 + 白名单 + hasActionScript）：module.prop、meta、
   * 特调/FAS 白名单、hasActionScript、deviceKind 这些变化频率低（天级），退出秒级轮询——
   * loadOverview 每 30s 补拉一次，各写路径成功后 force 刷新一次。
   * force=false 且命中 TTL 窗口直接返回，避免每秒 tick 白付这几次 exec。
   */
  async loadStatic(force = false): Promise<void> {
    if (this.staticJob) {
      if (!force) return this.staticJob
      // force 请求（写成功后）不能被在飞的非 force 补拉吞掉：那次 job 读的是
      // 写前数据、结束时会刷新 staticLoadedAt，直接复用会让本次写入的静态项
      // 最长 30s 不更新。先等它落地，再走下方强制重读。
      await this.staticJob
    }
    if (!force && Date.now() - this.staticLoadedAt < STATIC_TTL_MS) return
    this.staticJob = (async () => {
      try {
      const [kind, propRaw, meta, actionOk] = await Promise.all([
        deviceKind(),
        readModulePropRaw(),
        readMeta(),
        hasActionScript()
      ])
      this.deviceKind = kind
      if (propRaw.kind === 'ok') this.moduleProp = parseModuleProp(propRaw.value)

      this.actionAvailable = actionOk

      if (meta.kind === 'ok') {
        this.metaSnapshot = meta.value
        this.metaPath = meta.value.path
        this.configRel = meta.value.rel
        this.metaValid = meta.value.valid
        this.metaProblems = meta.value.problems
        this.configState = 'ok'
        this.configError = ''
        this.refreshFailNotified = false
      } else if (meta.kind === 'absent') {
        this.metaSnapshot = null
        this.configState = 'missing'
        this.configError = ''
        this.configRel = ''
        this.metaPath = ''
      } else {
        this.configState = 'failed'
        this.configError = meta.error
        this.notifyRefreshFail()
      }
      this.staticLoadedAt = Date.now()
      } finally {
        this.staticJob = null
      }
    })()
    try {
      await this.staticJob
    } finally {
      this.staticJob = null
    }
  }

  // [overview]
  /** 在飞的 overview 请求：并发调用共享同一个 promise */
  private overviewJob: Promise<void> | null = null

  async loadOverview(): Promise<void> {
    // 并发调用共享在飞请求（子组件 onMount 先于父组件执行，首次进入会被触发两次）。
    // 不能简单 `if (this.loading) return`：后来者会立刻拿到「还没加载完」的空状态，
    // 例如 isChiri 仍是 false → 依赖它的页面会静默跳过（接管置灰就是这么失效的）
    if (this.overviewJob) return this.overviewJob
    this.loading = true
    this.overviewJob = (async () => {
      try {
      // 静态项（module.prop/meta/白名单/hasActionScript/机型）退出秒级轮询：
      // 陈旧（从未加载或距上次 >30s）时与本 tick 并行补拉一次，不阻塞活跃项读取
      const staticRefresh =
        Date.now() - this.staticLoadedAt >= STATIC_TTL_MS ? this.loadStatic() : null
      // [tag] 秒级开销收敛：活跃项合并为一次 readMany（稳态 = 1 次 exec）——
      // current_mode.chr、PowerAVG.chr、status.csv 尾部、特调/FAS 白名单，以及
      // LiveTime.chr（存活判据）。旧实现存活探测是独立 exec（probeLiveness），
      // 现并入批量读，判定逻辑抽成纯函数 judgeLiveness（contract/daemon.ts）
      const rm = await readMany([
        { key: 'mode', path: absOf('currentMode') },
        { key: 'powerAvg', path: absOf('powerAvg') },
        { key: 'status', path: absOf('statusCsv'), tailBytes: 4096 },
        // 特调/FAS 白名单挂在同一次批量读上（0 次额外 exec）：计数与模式名翻译
        // 均秒级可达；原文未变只付一次字符串比对（见下方处理）
        { key: 'special', path: absOf('specialTuned') },
        { key: 'fas', path: absOf('fasWhitelist') },
        // [liveness] 心跳文件并入批量读（64B）：key 为 null = 文件缺失 = stopped，
        // 与旧 probeLiveness 的 absent→stopped 口径一致
        { key: 'liveTime', path: absOf('liveTime'), tailBytes: 64 }
      ])

      // [liveness] 存活判定改走批量条目，三分类映射与旧独立探测一致：
      // - 批量 ok：liveTime null（缺失）→ stopped；内容非法 → unknown + 详情
      // - 批量整体 absent（非 live 环境）→ unknown + unsupportedEnv
      // - 批量整体 failed → unknown + batchError（旧 failed 同口径）
      const okEntries = rm.kind === 'ok' ? rm.value : null
      const batchError = rm.kind === 'failed' ? rm.error : ''
      if (okEntries !== null) {
        const live = judgeLiveness(okEntries.liveTime ?? null)
        this.daemonState = live.state
        // 空串复位旧错误、非空为「内容非法」详情（judgeLiveness 只在这两态出错）
        this.daemonError = live.error
      } else if (rm.kind === 'absent') {
        this.daemonState = 'unknown'
        this.daemonError = t('state.unsupportedEnv')
      } else {
        this.daemonState = 'unknown'
        this.daemonError = batchError
      }

      // readMany → 单文件三分类的映射：'ok' 里某 key 为 null = 该文件缺失（≈absent），
      // 整体 'absent' = 非 live 环境（全部按缺失），'failed' = exec 本身失败（全部按失败）

      // current_mode.chr：缺失按「尚未生成」（modeMissing），真实失败才报 modeError
      const modeText = okEntries?.mode ?? null
      this.modeMissing = okEntries ? modeText === null : rm.kind === 'absent'
      this.modeError = okEntries ? '' : batchError
      const mode = modeText?.trim() ?? ''

      // PowerAVG.chr：缺失/为空/非法统一「无值」；exec 整体失败沿用旧 readPowerAvg 的
      // 映射（watt=null、missing=false），不把环境瞬时错误误报成「文件缺失」红字
      const watt = okEntries ? parsePowerAvgWatt(okEntries.powerAvg ?? '') : null
      this.powerAvgWatt = watt
      this.powerAvgMissing = okEntries ? watt === null : false

      // status.csv 尾部：powerNowWatt 与「为什么没有均值」共用同一次读取——
      // 旧实现 powerStaleWhy 会再发一次独立 exec，这里一并省掉
      const tailRows = parseStatusCsv(okEntries?.status ?? '')
      const lastStatus = tailRows[tailRows.length - 1]
      this.powerNowWatt = lastStatus?.battPower ?? null

      // 特调白名单（挂在本次批量读上，0 额外 exec）：原文与上次相同只付一次字符串
      // 比对、变了才重解析——模式卡名称翻译与特调计数因此秒级可达；failed 不动上次
      // 解析结果（瞬时故障不抖动清零），缺失按空表处理（与旧 absent 口径一致）
      const specialRaw = okEntries?.special ?? null
      if (specialRaw !== this.specialRawCache) {
        this.specialRawCache = specialRaw
        const special =
          specialRaw === null
            ? new Map<string, SpecialTunedEntry>()
            : parseSpecialTuned(specialRaw)
        this.modeNameSet = specialModeSet(special)
        this.specialCount = special.size
      }
      // FAS 白名单同样挂在批量读上：原文未变只付一次比对；failed 不动上次结果，
      // 缺失按空表（与旧 absent 口径一致）。readMany 整体失败才报 whitelistError
      const fasRaw = okEntries?.fas ?? null
      if (fasRaw !== this.fasRawCache) {
        this.fasRawCache = fasRaw
        const fas =
          fasRaw === null ? new Map<string, string>() : parseFasWhitelist(fasRaw)
        this.fasCount = fas.size
      }
      this.whitelistError = rm.kind === 'failed' ? batchError : ''

      // 依赖静态项的派生（白名单模式名 / meta 快照的口径开关）等静态补拉就位后再算；
      // 静态新鲜时这里是已完成的 promise，await 无额外开销
      if (staticRefresh) await staticRefresh
      this.currentMode = mode
      this.modeInfo = describeMode(mode, this.modeNameSet)
      // 没有取值时把「为什么没有」一并算出来：界面只显示 — 会让人以为是坏的
      this.powerStaleReason = watt === null ? this.powerStaleWhyOf(lastStatus) : ''
      } finally {
        this.loading = false
        this.ready = true
      }
    })()
    try {
      await this.overviewJob
    } finally {
      this.overviewJob = null
    }
  }

  /** 兼容入口（显式刷新静态项，写路径后调用）：绕过 TTL 窗口强制重读 */
  async refreshCommon(): Promise<void> {
    await this.loadStatic(true)
  }

  // [config]
  /** 草稿：合并多个字段后一次提交（每次写入都会触发守护进程全量热重载） */
  setDraft(field: WritableField, value: string | boolean): void {
    this.draft = { ...this.draft, [field]: value }
    this.writeError = ''
  }

  discardDraft(): void {
    this.draft = {}
    this.writeError = ''
    this.writeReverted = false
  }

  get hasDraft(): boolean {
    return Object.keys(this.draft).length > 0
  }

  async commitDraft(): Promise<boolean> {
    if (!this.hasDraft || this.saving) return false
    this.saving = true
    this.writeError = ''
    this.writeReverted = false
    try {
      // 期望值 = 写入前的字段值 + 本次草稿；回读后若有字段不等于期望值，
      // 说明守护进程判定文件非法并整体重置（界面必须如实说“已恢复默认值”）
      const patchKeys = Object.keys(this.draft) as WritableField[]
      const expected: Record<string, unknown> = {}
      for (const key of patchKeys) expected[key] = this.draft[key]

      const result = await writeMetaFields(this.draft)
      if (result.kind !== 'ok') {
        // 失败要分清楚：真实失败、文件尚未生成、当前环境无法访问设备
        this.writeError =
          result.kind === 'failed'
            ? result.error
            : result.reason === 'unsupported-env'
              ? t('state.unsupportedEnv')
              : t('config.missing')
        toast(this.writeError)
        return false
      }
      this.metaSnapshot = result.value
      this.metaValid = result.value.valid
      this.metaProblems = result.value.problems
      this.metaPath = result.value.path
      this.configRel = result.value.rel
      this.draft = {}
      this.writeReverted = patchKeys.some(key => result.value.values[key] !== expected[key])
      // meta 落盘成功：静态项里的 meta 快照刚被改，强制补拉一次（fire-and-forget，不拖慢返回）
      void this.loadStatic(true)
      toast(t('config.saved'))
      return true
    } finally {
      this.saving = false
    }
  }

  // [apps]
  async loadApps(rescan = false): Promise<void> {
    // in-flight 守卫：连点「重新扫描」时避免并发两组扫描互相复位标志
    if (this.scanning) return
    this.scanning = true
    try {
      const [packages, rulesRaw, tunedRaw, fasRaw] = await Promise.all([
        fetchInstalledPackages(),
        readRulesRaw(),
        readSpecialTunedRaw(),
        readFasWhitelistRaw()
      ])
      this.rules =
        rulesRaw.kind === 'ok'
          ? parseRules(rulesRaw.value)
          : rulesRaw.kind === 'failed'
            ? { ...EMPTY_RULES, problem: rulesRaw.error }
            : { ...EMPTY_RULES, problem: t('logs.missing') }
      const special = tunedRaw.kind === 'ok' ? parseSpecialTuned(tunedRaw.value) : new Map()
      const fas = fasRaw.kind === 'ok' ? parseFasWhitelist(fasRaw.value) : new Map()
      this.appsState = packages.length > 0 ? 'ok' : 'missing'
      this.apps = buildAppEntries(packages, {
        specialTuned: special,
        fasWhitelist: fas,
        appModes: this.rules.appModes
      })
      if (rescan) toast(t('apps.scanned'))
    } finally {
      this.scanning = false
    }
  }

  // [lab]
  /** 当前启用的实验室模式（未启用 / 内容非法时为 null） */
  get labMode(): LabModeKey | null {
    return this.labState.kind === 'on' ? this.labState.mode : null
  }

  /** rhine.chr 读到的内容不是合法标量——守护进程会把它重置为未启用 */
  get labInvalid(): boolean {
    return this.labState.kind === 'invalid'
  }

  /** 锁定中：关闭实验室需要重启设备，界面直接禁用关闭入口 */
  get labLocked(): boolean {
    return this.labLock.locked
  }

  /**
   * 当前实验室模式接管的 meta 开关（配置页据此置灰不可切换）；实验室未启用时为空。
   * 优先看 rhine.chr 里的模式；文件读不出模式（内容非法 / 强制关闭中间态 / 读失败）
   * 但**锁定标记里还记着模式**时按标记推导——那一刻守护进程正是按标记里的模式在
   * reassert，置灰一松，用户改的开关马上会被写回去。
   */
  get labTakeover(): readonly string[] {
    const fromFile = this.labMode
    const fromLock = (LAB_MODE_KEYS as readonly string[]).includes(this.labLock.mode)
      ? (this.labLock.mode as LabModeKey)
      : null
    const key = fromFile ?? fromLock
    return key ? LAB_TAKEOVER[key] : []
  }

  /** 运行配置一致性问题的代号（界面映射成文案） */
  get labWarnings(): LabWarningCode[] {
    return labWarnings({
      lock: this.labLock,
      state: this.labState,
      hasBackup: this.labHasBackup
    })
  }

  /** 把契约层读回来的快照写进状态（loadLab 与强制关闭的收敛轮询共用） */
  private applyLabSnapshot(snap: {
    state: LabState
    path: string
    lock: LabLock
    hasBackup: boolean
  }): void {
    this.labState = snap.state
    this.labPath = snap.path
    this.labLock = snap.lock
    this.labHasBackup = snap.hasBackup
  }

  /** 在飞的 lab 读请求：并发调用共享同一个 promise（理由同 loadOverview） */
  private labJob: Promise<void> | null = null

  async loadLab(): Promise<void> {
    if (this.labJob) return this.labJob
    this.labLoading = true
    this.labJob = (async () => {
      try {
        const result = await readLab()
        if (result.kind === 'ok') {
          this.applyLabSnapshot(result.value)
          this.labError = ''
        } else {
          // 文件不存在按未启用处理（契约层已归一），走到这里只剩环境不可用
          this.labError = result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
        }
      } finally {
        this.labLoading = false
      }
    })()
    try {
      await this.labJob
    } finally {
      this.labJob = null
    }
  }

  /** 启用（传 key）或关闭（传 null）实验室。写后回读，状态以文件实际内容为准。 */
  async setLabMode(mode: LabModeKey | null): Promise<void> {
    // 锁定期间关不掉：守护进程会把 rhine.chr 原样写回去，这里先挡住，别让界面假装成功。
    // 确实要在锁定时关掉，走下面的 forceDisableLab（写保留字 off）。
    if (mode === null && this.labLocked) {
      this.labError = t('lab.lock.denied')
      toast(this.labError)
      return
    }
    await this.commitLab(mode, mode ? t('lab.toast.enabled') : t('lab.toast.disabled'))
  }

  /**
   * 强制关闭实验室（无视「关闭需重启」的风险）：写保留字 off。
   * 守护进程收到后会清掉锁定标记、按快照还原原值、把 rhine.chr 写回未启用，
   * 也就是「恢复原地调度」。
   */
  async forceDisableLab(): Promise<void> {
    // 等待窗口交给 commitLab：它必须落在 pending 区间内，否则这段时间按钮会复活、
    // 和守护进程的收敛抢着写 rhine.chr
    await this.commitLab(LAB_FORCE_OFF, t('lab.toast.forceOff'), 400)
  }

  /**
   * 写 rhine.chr 并刷新相关状态（启用/关闭/强制关闭三个入口共用）。
   * `settleMs` > 0 时写后多等一会儿再回读：守护进程处理 off 要走「清标记 → 还原 meta
   * → 写回未启用」，立刻读会停在中间态；等待期间 pending 保持 true。
   */
  async commitLab(mode: LabWriteTarget, done: string, settleMs = 0): Promise<void> {
    if (this.labPending) return
    this.labPending = true
    this.labError = ''
    try {
      const result = await writeLabMode(mode)
      if (result.kind !== 'ok') {
        this.labError = result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
        toast(this.labError)
        return
      }
      this.labState = result.value.state
      this.labPath = result.value.path
      this.labLock = result.value.lock
      this.labHasBackup = result.value.hasBackup
      // 被实验室接管的开关若还留着草稿，保存出去也会被守护进程按定义写回去——
      // 直接清掉，别让「待提交」标记在配置页骗人
      for (const field of this.labTakeover) delete this.draft[field as WritableField]
      // 实验室会改写 meta.yaml 的 fas/scenemode 开关，配置页那份快照已过期
      await this.refreshCommon()
      if (settleMs > 0) {
        // 等守护进程把 off 收敛完（清标记 → 还原 meta → 写回未启用）：固定等一次不够，
        // 慢文件系统上可能还没收敛，界面就会停在「已提交强制关闭」、按钮还会复活去和
        // 守护进程抢写 rhine.chr。这里直接读文件（绕过 labJob 守卫）轮询到不再是
        // force-off 或超时为止。
        const deadline = Date.now() + settleMs * 10
        for (;;) {
          await new Promise(resolve => setTimeout(resolve, settleMs))
          const snap = await readLab()
          if (snap.kind === 'ok') {
            this.applyLabSnapshot(snap.value)
            if (snap.value.state.kind !== 'force-off') break
          }
          if (Date.now() >= deadline) break
        }
        await this.refreshCommon()
      }
      toast(done)
    } finally {
      this.labPending = false
    }
  }

  // [down]
  /** DOWN 停摆：调度关停全部调度功能（CLG/特调/FAS/锁频/线程摆放/core_ctl），只留采集与日志 */
  downActive = $state(false)
  /** down.chr 绝对路径 */
  downPath = $state('')
  downError = $state('')
  /** 正在写入（切换期间禁用开关，避免重复提交） */
  downPending = $state(false)

  // [powerAvg]
  /** 功耗参考/平均值（W）：null = PowerAVG.chr 缺失/为空/非法（显示 —） */
  powerAvgWatt = $state<number | null>(null)
  /** 口径开关写入中（高级设置直写 meta.yaml） */
  /** 文件缺失/为空（daemon 未运行过）：界面红字提示，不静默显示 — */
  powerAvgMissing = $state(false)
  /** 没有取到值时的原因文案（空 = 说不清，界面回退到 overview.power.missing） */
  powerStaleReason = $state('')
  powerAvgPending = $state(false)
  powerAvgError = $state('')
  /** PowerBase 开关写入中的状态（高级设置） */
  powerbasePending = $state(false)
  powerbaseError = $state('')
  /** 息屏判定值写入中的状态（高级设置） */
  screenOffPending = $state(false)
  screenOffError = $state('')
  /** 当前功耗（W，status.csv 最后一行的 batt_power_w，1s 采样）：null = 无行/无读数，显示 — */
  powerNowWatt = $state<number | null>(null)

  /** 功耗口径：是否使用累计平均值（meta.yaml `power_avg`，默认 false = 参考值） */
  get powerAvgUsesAverage(): boolean {
    return this.metaSnapshot?.values?.power_avg === true
  }

  /** PowerBase 是否开启（meta.yaml `powerbase_enabled`，默认 false = 由 CLG 调频） */
  get powerbaseEnabled(): boolean {
    return this.metaSnapshot?.values?.powerbase_enabled === true
  }

  /** 息屏判定值（meta.yaml `screen_off_value`，缺省 1）：debug.tracing.screen_state
   *  等于该值视为息屏；非法值与缺失一律按默认 1 呈现（daemon 侧同口径） */
  get screenOffValue(): number {
    return this.metaSnapshot?.values?.screen_off_value === 0 ? 0 : 1
  }

  // [powerMax]
  /** 耗电仪表盘满量程（W）：meta.yaml 可选字段 `power_max_w`，缺省/越界/非法一律按 12。
   *  只在 meta.yaml 手改（高级设置不提供输入框），此处仅供仪表盘换算读取 */
  get powerMaxWatt(): number {
    const v = this.metaSnapshot?.values?.power_max_w
    return typeof v === 'number' && Number.isFinite(v) && v > 0 && v <= 200 ? v : 12
  }

  /**
   * PowerAVG 没取到值时，说明原因（空串 = 说不清，界面回退到 overview.power.missing）。
   * 判据来自本 tick 批量读取的 status.csv 末行（同步派生，不再单独 exec 重读）：
   *   ① 非放电（充电 / 充满 / 未充电 / 状态不可识别）→ 按口径不取样；
   *   ② 平均模式且息屏 → 不取样（平均口径＝亮屏放电）；
   *   ③ 放电且屏幕条件满足却仍为空 → 电压/电流读数不可用（或 daemon 还没写过一行）。
   */
  private powerStaleWhyOf(last: StatusRow | undefined): string {
    if (!last) return ''
    if (last.charge !== 'discharging') {
      const known = ['charging', 'full', 'not_charging']
      const name = known.includes(last.charge) ? t(`logs.charge.${last.charge}`) : last.charge
      return t('overview.power.reason.charge', { state: name })
    }
    if (this.powerAvgUsesAverage && !last.screenOn) return t('overview.power.reason.screen')
    return t('overview.power.reason.noreading')
  }

  // [batteryRead]
  /** 电池读数选项（meta.yaml 的 5 个字段，电池读数二级页直写）：读快照，写入走 setBatteryFields */
  get oplusChg(): boolean {
    return this.metaSnapshot?.values?.oplus_chg === true
  }
  get oplusDualCell(): boolean {
    return this.metaSnapshot?.values?.oplus_dual_cell === true
  }
  get voltageDouble(): boolean {
    return this.metaSnapshot?.values?.voltage_double === true
  }
  get currentDouble(): boolean {
    return this.metaSnapshot?.values?.current_double === true
  }
  /** 电压校准除数：缺省/非法一律按 1000000（标准 Android ABI µV 口径，与 daemon
   * DEFAULT_UNIT_DIVISOR 同值）；OPlus 私有节点报 mV，安装脚本会写成 1000。
   * 旧键 unit_divisor 作为兜底 */
  get voltageDivisor(): number {
    const values = this.metaSnapshot?.values
    for (const v of [values?.voltage_divisor, values?.unit_divisor]) {
      if (typeof v === 'number' && Number.isFinite(v) && v > 0) return v
    }
    return 1_000_000
  }
  /** 电流校准除数：缺省/非法一律按 1000000（标准节点 µA → A；batt_power_w 按安培消费） */
  get currentDivisor(): number {
    const v = this.metaSnapshot?.values?.current_divisor
    return typeof v === 'number' && Number.isFinite(v) && v > 0 ? v : 1_000_000
  }
  /** 电池读数页写入中（直写 meta.yaml） */
  battPending = $state(false)
  /**
   * meta.yaml 直写互斥：写路径是「读-改-写」，两笔并发会各自读旧内容、后一笔把前一笔的
   * 字段覆盖掉（丢字段）。页面上的直写入口（setBatteryFields / setPowerAvg）共用这一把锁。
   */
  metaWritePending = $state(false)
  battError = $state('')

  async loadDown(): Promise<void> {
    if (this.downPending) return
    const result = await readDown()
    if (result.kind === 'ok') {
      this.downActive = result.value.active
      this.downPath = result.value.path
      this.downError = ''
    } else {
      // 文件不存在按未停摆处理（契约层已归一），走到这里只剩环境不可用
      this.downError = result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
    }
  }

  /** 开启/解除 DOWN 停摆。写后回读，状态以 down.chr 实际内容为准 */
  async setDown(active: boolean): Promise<void> {
    if (this.downPending) return
    this.downPending = true
    this.downError = ''
    try {
      const result = await writeDown(active)
      if (result.kind !== 'ok') {
        this.downError = result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
        toast(this.downError)
        return
      }
      this.downActive = result.value.active
      this.downPath = result.value.path
      // down.chr 落盘成功：强制补拉静态项（写路径统一口径）
      void this.loadStatic(true)
      toast(active ? t('config.down.on') : t('config.down.off'))
      // 停摆会改 current_mode.chr（写 down / 删掉让自愈写回），总览页那份快照已过期
      await this.loadOverview()
    } finally {
      this.downPending = false
    }
  }

  /**
   * 高级设置：功耗口径开关——直写 meta.yaml 的 `power_avg`（不走草稿，立即热重载）。
   * 关闭（false，默认）= 参考值（旧值×10 与新值按 10:1 加权递推，偏历史，含息屏）；
   * 开启 = 累计平均值（等权全史，仅亮屏放电样本）。
   * 写后回读，界面以实际落盘内容为准。
   */
  async setPowerAvg(useAverage: boolean): Promise<void> {
    if (this.powerAvgPending || this.metaWritePending) return
    this.powerAvgPending = true
    this.metaWritePending = true
    this.powerAvgError = ''
    try {
      const result = await writeMetaFields({ power_avg: useAverage })
      if (result.kind !== 'ok') {
        this.powerAvgError =
          result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
        toast(this.powerAvgError)
        return
      }
      this.metaSnapshot = result.value
      this.metaValid = result.value.valid
      this.metaProblems = result.value.problems
      this.metaPath = result.value.path
      // meta 直写成功：强制补拉静态项（meta 快照刚被改；fire-and-forget 不拖慢返回）
      void this.loadStatic(true)
      toast(useAverage ? t('overview.power.avg') : t('overview.power.ref'))
    } finally {
      this.powerAvgPending = false
      this.metaWritePending = false
    }
  }

  /**
   * 高级设置：PowerBase 开关——直写 meta.yaml 的 `powerbase_enabled`（不走草稿，立即热重载）。
   * 开启后原本由 CLG 接管的档位改由 PowerBase 以放电功耗为指标调频；模式名、规则与
   * 界面等外部接口保持不变（只换实现）。写后回读，界面以实际落盘内容为准。
   */
  async setPowerbase(enabled: boolean): Promise<void> {
    if (this.powerbasePending || this.metaWritePending) return
    this.powerbasePending = true
    this.metaWritePending = true
    this.powerbaseError = ''
    try {
      const result = await writeMetaFields({ powerbase_enabled: enabled })
      if (result.kind !== 'ok') {
        this.powerbaseError =
          result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
        toast(this.powerbaseError)
        return
      }
      this.metaSnapshot = result.value
      this.metaValid = result.value.valid
      this.metaProblems = result.value.problems
      this.metaPath = result.value.path
      // meta 直写成功：强制补拉静态项（meta 快照刚被改；fire-and-forget 不拖慢返回）
      void this.loadStatic(true)
      toast(enabled ? t('config.powerbase') : t('config.advanced'))
    } finally {
      this.powerbasePending = false
      this.metaWritePending = false
    }
  }

  /**
   * 高级设置：息屏判定值——直写 meta.yaml 的 `screen_off_value`（不走草稿，立即热重载）。
   * debug.tracing.screen_state 属性等于该值视为息屏；默认 1，安装脚本会按安装期
   * 实测值自动校正。写后回读，界面以实际落盘内容为准。
   */
  async setScreenOffValue(value: 0 | 1): Promise<void> {
    if (this.screenOffPending || this.metaWritePending) return
    this.screenOffPending = true
    this.metaWritePending = true
    this.screenOffError = ''
    try {
      const result = await writeMetaFields({ screen_off_value: value })
      if (result.kind !== 'ok') {
        this.screenOffError =
          result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
        toast(this.screenOffError)
        return
      }
      this.metaSnapshot = result.value
      this.metaValid = result.value.valid
      this.metaProblems = result.value.problems
      this.metaPath = result.value.path
      // meta 直写成功：强制补拉静态项（meta 快照刚被改；fire-and-forget 不拖慢返回）
      void this.loadStatic(true)
      toast(t(value === 1 ? 'config.screenoff' : 'config.screenoff.zero'))
    } finally {
      this.screenOffPending = false
      this.metaWritePending = false
    }
  }

  /**
   * 电池读数页：直写 meta.yaml（不走草稿，立即热重载）。
   * 开启 OPlus 私有节点时把倍电压/倍电流一并清掉——两者互斥（daemon 侧也只认私有节点）；
   * 界面同时置灰这两项，这里再兜一次，避免留一个「开着但不生效」的开关。
   * 写后回读，界面以实际落盘内容为准。
   */
  async setBatteryFields(
    patch: Partial<{
      oplus_chg: boolean
      oplus_dual_cell: boolean
      voltage_double: boolean
      current_double: boolean
      voltage_divisor: number
      current_divisor: number
      power_max_w: number
    }>
  ): Promise<void> {
    if (this.battPending || this.metaWritePending) return
    this.battPending = true
    this.metaWritePending = true
    this.battError = ''
    try {
      // 强制清空排在展开之后：调用方就算多传 voltage_double: true，也写不出
      // 「私有节点开 + 倍压开」这种互斥组合（daemon 侧还会再判一次）
      const fields =
        patch.oplus_chg === true
          ? { ...patch, voltage_double: false, current_double: false }
          : patch
      const result = await writeMetaFields(fields)
      if (result.kind !== 'ok') {
        this.battError = result.kind === 'failed' ? result.error : t('state.unsupportedEnv')
        toast(this.battError)
        return
      }
      this.metaSnapshot = result.value
      this.metaValid = result.value.valid
      this.metaProblems = result.value.problems
      this.metaPath = result.value.path
      // meta 直写成功：强制补拉静态项（meta 快照刚被改；fire-and-forget 不拖慢返回）
      void this.loadStatic(true)
    } finally {
      this.battPending = false
      this.metaWritePending = false
    }
  }

  // [export]
  /** 导出阶段：running 期间禁用按钮，done/failed 显示结果 */
  exportPhase = $state<'idle' | 'running' | 'done' | 'failed'>('idle')
  /** 成功后的产物绝对路径（设备不支持 gzip 时是未压缩 .tar） */
  exportTarget = $state('')
  exportError = $state('')
  /** 进度：已处理文件数 / 总数（来自 tar -v 行数）、已写入归档字节数 */
  exportDone = $state(0)
  exportTotal = $state(0)
  exportBytes = $state(0)

  /** 进度百分比：封顶 99，100 留给完成态。分母是文件数，文件少时粒度偏粗 */
  get exportPercent(): number {
    if (this.exportTotal <= 0) return 0
    return Math.min(99, Math.round((this.exportDone / this.exportTotal) * 100))
  }

  /** 已写入的兆字节（1 位小数） */
  get exportMb(): string {
    return (this.exportBytes / 1048576).toFixed(1)
  }

  /**
   * 导出历史归档到 /sdcard/Download。
   * 打包在设备上后台跑（gzip 压几百 MB 也要一会儿），这里只轮询产物，期间界面照常可用。
   */
  async startExport(): Promise<void> {
    if (this.exportPhase === 'running') return
    this.exportPhase = 'running'
    this.exportError = ''
    this.exportTarget = ''
    this.exportDone = 0
    this.exportTotal = 0
    this.exportBytes = 0
    const started = await startExport()
    if (started.kind !== 'ok') {
      this.exportPhase = 'failed'
      this.exportError =
        started.kind === 'failed'
          ? `${started.error} · ${t('overview.export.failed.hint')}`
          : t('state.unsupportedEnv')
      return
    }
    const job = started.value
    // 1.5s 一轮，最多 160 轮（4 分钟）：gzip -6 压几百 MB 通常几十秒到一两分钟量级
    let pollFails = 0
    for (let i = 0; i < 160; i++) {
      await new Promise(resolve => setTimeout(resolve, 1500))
      const probed = await pollExport(job)
      if (probed.kind !== 'ok') {
        // 单次探测失败多半是桥的瞬时错误（授权、回调丢失、WebView 切后台）——
        // 后台 tar 多半还在跑，连续 3 次才判死，别让一次「命令失败(1)」错杀导出
        pollFails++
        if (pollFails < 3) continue
        this.exportPhase = 'failed'
        const detail = probed.kind === 'failed' ? probed.error : t('state.unsupportedEnv')
        this.exportError = `${detail} · ${t('overview.export.failed.hint')}`
        return
      }
      pollFails = 0
      this.exportDone = probed.value.progress.done
      this.exportTotal = probed.value.progress.total
      this.exportBytes = probed.value.progress.bytes
      const phase = probed.value.phase
      if (phase === 'running') continue
      if (phase === 'done-gz' || phase === 'done-tar') {
        this.exportPhase = 'done'
        this.exportTarget = phase === 'done-gz' ? job.target : job.fallback
        toast(
          phase === 'done-gz' ? t('overview.export.done') : t('overview.export.doneTar')
        )
        return
      }
      this.exportPhase = 'failed'
      this.exportError =
        phase === 'empty'
          ? t('overview.export.empty')
          : phase === 'no-logd'
            ? t('overview.export.noDir')
            : t('overview.export.fail')
      return
    }
    this.exportPhase = 'failed'
    this.exportError = t('overview.export.timeout')
  }

  // [logs]
  /** 历史归档删除中（防重复点击） */
  archivePending = $state(false)
  /** 删除归档的真实失败（环境不可用 / rm 报错） */
  archiveError = $state('')

  /** 删除历史归档（logd/ 与 devimp/）：成功后刷新日志页的列表 */
  async deleteArchives(): Promise<void> {
    if (this.archivePending) return
    this.archivePending = true
    this.archiveError = ''
    try {
      const r = await clearArchives()
      if (r.kind !== 'ok') {
        this.archiveError = r.kind === 'failed' ? r.error : t('state.unsupportedEnv')
        return
      }
      await this.loadLogs()
    } finally {
      this.archivePending = false
    }
  }

  async loadLogs(source: 'daemon' | 'status' = 'daemon'): Promise<void> {
    if (this.logLoading) return // 自刷新轮询防堆积
    this.logLoading = true
    let failed = false
    try {
      if (source === 'daemon') {
        const [log, devimp, logd] = await Promise.all([
          readDaemonLogTail(),
          listDevimp(),
          listLogd()
        ])
        if (log.kind === 'ok') {
          this.applyDaemonLog(log.value)
          this.logState = 'ok'
          this.logError = ''
        } else if (log.kind === 'absent') {
          this.logLines = []
          this.resetLogAnchor()
          this.logState = 'missing'
          this.logError = ''
        } else {
          this.logLines = []
          this.resetLogAnchor()
          this.logState = 'failed'
          this.logError = log.error
          this.notifyRefreshFail()
        }
        this.dirError =
          devimp.kind === 'failed' ? devimp.error : logd.kind === 'failed' ? logd.error : ''
        this.devimpFiles = devimp.kind === 'ok' ? devimp.value : []
        this.logdFiles = logd.kind === 'ok' ? logd.value : []
      } else {
        const csv = await readStatusCsvTail()
        if (csv.kind === 'ok') {
          this.applyStatusCsv(csv.value)
          this.statusState = this.statusRows.length > 0 ? 'ok' : 'missing'
          this.statusError = ''
        } else if (csv.kind === 'absent') {
          this.setStatusRows([])
          this.resetStatusAnchor()
          this.statusState = 'missing'
          this.statusError = ''
        } else {
          this.setStatusRows([])
          this.resetStatusAnchor()
          this.statusState = 'failed'
          this.statusError = csv.error
          this.notifyRefreshFail()
        }
      }
      if (!failed) this.refreshFailNotified = false
    } finally {
      this.logLoading = false
    }
  }

  // [logs] 增量解析应用：ok 分支共用入口（锚定命中只解析新增行，否则整段重建）
  /**
   * daemon.log 尾部应用（endsWith 内容锚定）：
   * - 命中（新尾部以旧窗口结尾 = 文件只在尾部追加）→ 解析新增字节里的完整行
   *   并 concat（首行续行语义交给 parseDaemonLog 的 continuation 参数）；
   * - 不命中（轮转/截断/首次）或上轮留有未解析半行 → 整段重解析重建。
   *   整段路径对尾部半行沿用 parseDaemonLog 的旧行为（不匹配则并入前条），
   *   「半行不进显示」的口径只在增量路径成立；半行是瞬时竞态，下轮补全后
   *   以完整行显示，无正确性影响。
   * 展示上限沿用 LOG_MAX_LINES：concat 后从头部裁剪，防增量路径无界增长，
   * 与整段 parseDaemonLog 的 maxLines 行为一致。
   */
  private applyDaemonLog(fresh: string): void {
    const cache = this.logTailCache
    const hit =
      cache !== null &&
      fresh.endsWith(cache) &&
      // 上轮尾部半行未显示：为它做跨条目合并不值得（行缓冲写盘半行是瞬时竞态），
      // 直接整段重解析保证正确性
      this.logAnchorLen === cache.length
    if (hit && cache !== null) {
      // 命中：added = 纯新增字节（anchorLen === cache.length 保证上轮无残留半行，
      // chunk 从行边界开始）；added 里末尾的半行留给下轮
      const added = fresh.slice(0, fresh.length - cache.length)
      const lastNl = added.lastIndexOf('\n')
      if (lastNl >= 0) {
        const chunk = added.slice(0, lastNl + 1)
        const prev = this.logLines.length > 0 ? this.logLines[this.logLines.length - 1] : null
        // 上限传 Infinity：裁剪在下面 concat 后统一做（chunk 本身很小）
        const parsed = parseDaemonLog(chunk, Number.POSITIVE_INFINITY, prev)
        // 返回首元素是 prev 本体（续行已原地并入），新行才是要追加的
        const addedLines = prev !== null ? parsed.slice(1) : parsed
        const merged = this.logLines.concat(addedLines)
        this.logLines =
          merged.length > LOG_MAX_LINES ? merged.slice(merged.length - LOG_MAX_LINES) : merged
      }
      // lastNl < 0：本轮无新增完整行（可能纯半行增长），logLines 不动
    } else {
      this.logLines = parseDaemonLog(fresh)
    }
    // 锚点推进：anchor = 最后一个完整行结尾（尾部半行不进锚点、不进显示）
    this.logTailCache = fresh
    this.logAnchorLen = AppStore.anchorLenOf(fresh)
  }

  /**
   * status.csv 尾部应用：与 applyDaemonLog 同一套锚定模式。CSV 每行整行写出，
   * parseStatusCsv 按字段数过滤残行，P 为空时 chunk 可直接增量解析；
   * 追加走 appendStatusRows（超 720 行头部裁剪 + 倒序副本同步）。
   */
  private applyStatusCsv(fresh: string): void {
    const cache = this.statusTailCache
    const hit =
      cache !== null &&
      fresh.endsWith(cache) &&
      this.statusAnchorLen === cache.length
    if (hit && cache !== null) {
      // P 为空（anchorLen === cache.length）→ combined 就是纯新增字节
      const added = fresh.slice(0, fresh.length - cache.length)
      const lastNl = added.lastIndexOf('\n')
      if (lastNl >= 0) {
        this.appendStatusRows(parseStatusCsv(added.slice(0, lastNl + 1)))
      }
    } else {
      // 上限与增量路径统一为 STATUS_MAX_ROWS（默认 maxRows=300，半行竞态触发
      // 整段重解析时行数会从 720 跳回 300，一次显示抖动）
      this.setStatusRows(parseStatusCsv(fresh, AppStore.STATUS_MAX_ROWS))
    }
    this.statusTailCache = fresh
    this.statusAnchorLen = AppStore.anchorLenOf(fresh)
  }

  // [actions]
  async stopDaemon(): Promise<void> {
    this.stopping = true
    try {
      const result = await stopScheduler()
      if (result.kind === 'ok') {
        toast(t('overview.stop.done'))
        // 关闭后重新探测：状态以实际观测为准，不凭假设
        await this.loadOverview()
      } else {
        const message = result.kind === 'failed' ? result.error : t('overview.stop.unavailable')
        toast(t('overview.stop.failed'))
        this.daemonError = message
      }
    } finally {
      this.stopping = false
    }
  }
}

export const app = new AppStore()
