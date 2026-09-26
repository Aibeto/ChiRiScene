// lab.ts: [keys] [takeover] [parse] [lock]实验室（rhine）状态解析：rhine.chr 内容是 YAML 标量（模式 key），空文件/只有注释 = 未启用
// 口径必须与守护进程 src/rhine.rs::parse_state 一致，否则「界面说已启用、守护进程判非法并重置」：两边都做三件事——剥空行与 # 注释、只接受单行标量、key 必须在内置定义里

// [keys]
/**
 * LAB_MODE_KEYS：rhine-init.yaml 里已定义的实验室模式key 是实际做的事，界面名字走 i18n `lab.mode.*`，两边不要混用
 * 新增模式要同步改这里、locale、rhine-init.yaml
 */
export const LAB_MODE_KEYS = ['vector', 'contingency', 'babel', 'frozen'] as const
export type LabModeKey = (typeof LAB_MODE_KEYS)[number]

/** 界面上放开启用的模式frozen（待春归）已实现，不再是 rhine-init.yaml 里的空映射；与 rhine-init.yaml 保持同步 */
export const LAB_ENABLEABLE: readonly LabModeKey[] = ['vector', 'contingency', 'babel', 'frozen']

// [takeover]
/**
 * 各实验室模式会接管的 meta 开关（字段名与 meta.yaml 一致），配置页置灰不可切换：锁定期间手改会被守护进程
 * reassert 拉回去与 rhine-init.yaml 的影响项一一对应、必须同步改（与 LAB_MODE_KEYS 一样硬编码），
 * tests/lab.test.ts 有刚性断言兜底
 */
export const LAB_TAKEOVER: Record<LabModeKey, readonly string[]> = {
  // 与 rhine-init.yaml 一致：实验室期间 FAS/息屏场景模式被关（global_mode/special_tuned 不是 meta 字段）
  vector: ['fas_enabled', 'scenemode_enabled'],
  contingency: ['fas_enabled', 'scenemode_enabled'],
  babel: ['fas_enabled', 'scenemode_enabled'],
  // frozen 多接管 thread_bind：停掉线程亲和/绑核等额外开销
  frozen: ['fas_enabled', 'scenemode_enabled', 'thread_bind']
}

// [parse]
/**
 * 保留字：强制关闭与「写空内容」的区别在锁定期间：空内容会被挡回去，off 不会（清锁定、还原快照、写回未启用）
 * 必须与守护进程 src/rhine.rs::is_force_off 同口径（忽略大小写、允许带引号）
 */
export const LAB_FORCE_OFF = 'off'

export type LabState =
  | { kind: 'off' }
  | { kind: 'on'; mode: LabModeKey }
  /** 文件里写着保留字 off：强制关闭请求（守护进程收敛后会把文件写回未启用） */
  | { kind: 'force-off' }
  /** 内容不是合法标量或写了未定义的模式名——守护进程会把它重置掉 */
  | { kind: 'invalid'; raw: string }

/** 剥 YAML 行内注释：`#` 前必须是行首或空白才算注释daemon 侧由 serde_yaml 剥，这里必须自己剥，否则 `vector # 注释` 会误判非法 */
function stripComment(line: string): string {
  const at = line.search(/\s#/)
  return at >= 0 ? line.slice(0, at) : line
}

/** is_force_off 在 YAML 解析之前做字面量比较（剥注释→trim→逐边剥引号→忽略大小写），不能套 YAML 标量口径 */
function isForceOff(line: string): boolean {
  return stripComment(line)
    .trim()
    .replace(/^["']+|["']+$/g, '')
    .toLowerCase() === LAB_FORCE_OFF
}

/**
 * 按 daemon serde_yaml 口径取字符串标量：剥注释→trim→成对同型引号剥一层（不 trim 内侧）不成对引号判非法，否则出现「界面说已启用、守护进程判非法并重置」的裂缝
 */
function yamlScalar(line: string): string | null {
  const s = stripComment(line).trim()
  const head = s.charAt(0)
  const tail = s.charAt(s.length - 1)
  if (s.length >= 2 && ((head === '"' && tail === '"') || (head === "'" && tail === "'"))) {
    return s.slice(1, -1)
  }
  if (head === '"' || head === "'" || tail === '"' || tail === "'") return null
  return s
}

export function parseLabState(text: string): LabState {
  const body = text
    .split('\n')
    .map(line => line.trim())
    .filter(line => line.length > 0 && !line.startsWith('#'))
  if (body.length === 0) return { kind: 'off' }
  const raw = body.join('\n')
  // 多行内容解析不成字符串标量，守护进程同样判非法
  if (body.length > 1) return { kind: 'invalid', raw }
  const line = body[0]
  // 保留字先判：它既不是模式 key 也不算非法内容（daemon 也是先做字面量比较，顺序不能反）
  if (isForceOff(line)) return { kind: 'force-off' }
  const key = yamlScalar(line)
  return key && (LAB_MODE_KEYS as readonly string[]).includes(key)
    ? { kind: 'on', mode: key as LabModeKey }
    : { kind: 'invalid', raw }
}

/** 可写进 rhine.chr 的三种目标：模式 key / 保留字 off / null（关闭，写空内容） */
export type LabWriteTarget = LabModeKey | typeof LAB_FORCE_OFF | null

/** 写入内容：启用写模式 key，未启用写空内容（与「正常模式下应为空」一致） */
export function labFileContent(mode: LabWriteTarget): string {
  return mode ? `${mode}\n` : ''
}

// [lock]
/**
 * 锁定标记：tmpfs 上落一个标记，存在即表示本次开机后启用过（运行时关不掉，重启消失）文件名与候选目录必须与守护进程 src/rhine.rs 的 LOCK_NAME/LOCK_DIRS 一致，
 * 否则界面说「未锁定」而设备其实锁着
 */
export const LAB_LOCK_NAME = 'chiri-labs.lock'
export const LAB_LOCK_DIRS = ['/tmp', '/dev'] as const

export interface LabLock {
  /** 标记存在 */
  locked: boolean
  /** 标记第一行记录的模式（空串 = 没写或不是已知模式） */
  mode: string
  /** 异常痕迹（`#` 开头的行）：非空即表示检测到运行配置被改动过 */
  notes: string[]
}

/** 解析标记文件内容`exists` 为 false 时（文件不存在）一律视为未锁定 */
export function parseLabLock(text: string, exists: boolean): LabLock {
  if (!exists) return { locked: false, mode: '', notes: [] }
  const lines = text.split('\n')
  const first = (lines[0] ?? '').trim()
  const mode = (LAB_MODE_KEYS as readonly string[]).includes(first) ? first : ''
  const notes = lines
    .slice(1)
    .map(l => l.trim())
    .filter(l => l.startsWith('#'))
    .map(l => l.slice(1).trim())
    .filter(l => l.length > 0)
  return { locked: true, mode, notes }
}

/** 运行配置一致性问题的稳定代号（界面层映射成 i18n 文案，便于单测）；仅已锁定时才有意义 */
export type LabWarningCode =
  /** rhine.chr 内容不是合法标量：被手改坏过，守护进程会重置它 */
  | 'state-invalid'
  /** 锁定中但 rhine.chr 不是标记里那个模式：文件被清空或改成了别的写法 */
  | 'lock-drifted'
  /** 锁定中却没有 rhine-back.chr：原值快照丢了，关闭后不一定能完整还原 */
  | 'no-backup'

export function labWarnings(input: {
  lock: LabLock
  state: LabState
  hasBackup: boolean
}): LabWarningCode[] {
  const { lock, state, hasBackup } = input
  const out: LabWarningCode[] = []
  if (state.kind === 'invalid') out.push('state-invalid')
  if (!lock.locked) return out
  // off = 用户刚请求强制关闭：锁定与快照正被守护进程收掉，「不一致」「没快照」是预期中间态
  if (state.kind === 'force-off') return out
  if (state.kind !== 'on' || (lock.mode && state.mode !== lock.mode)) out.push('lock-drifted')
  if (!hasBackup) out.push('no-backup')
  return out
}
