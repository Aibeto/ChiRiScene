// mode.ts: [catalog] [derive]
import { DOWN_WORD } from '@/data/down'
// 模式派生事实来源 src/monitor/app_detect.rs::determine_mode：fas（白名单命中且应用配置可解析）
// → 特调白名单 fallback → app_modes → global_mode。只有 reduce/default/boost/vector 在 daemon 注册为 CLG 档；
// 特调模式名由 special_tuned.yaml 定义；未注册字面值归为 unknown

// [catalog]
/**
 * 模式家族（ModeKind）：
 * - clg：reduce/default/boost（current_mode 直接是档名）；stardust：scenemode（独立息屏轴，不作 current_mode 档位，家族位照常注册占位）
 * - down：DOWN 停摆（独立家族，不是 stardust）；lab：vector/contingency/babel/frozen（rhine.chr 驱动 global_mode 覆盖）
 */
export type ModeKind = 'clg' | 'fas' | 'special' | 'lab' | 'down' | 'stardust' | 'unknown'
/** 语义信号（UI 映射到 --ak-signal-*，不用裸色值） */
export type ModeSignal = 'info' | 'success' | 'action' | 'danger' | 'accent'

export interface ModeInfo {
  /** 原始模式名（current_mode.chr 的内容） */
  id: string
  kind: ModeKind
  signal: ModeSignal
  /** i18n 键（缺失时界面回退显示原始 id） */
  labelKey: string
  descKey: string
}

const CLG_CATALOG: Record<string, {
  signal: ModeSignal; labelKey: string;
}> = {
  reduce: {
    signal: 'success', labelKey: 'mode.reduce',
  },
  default: {
    signal: 'info', labelKey: 'mode.default',
  },
  boost: {
    signal: 'action', labelKey: 'mode.boost',
  }
}

/** rhine 家族（仅实验室）：vector 运行时走 fast_lock 硬锁、不读 CLG 参数，语义归 rhine，故从 CLG_CATALOG 移到这里 */
const LAB_CATALOG: Record<string, {
  signal: ModeSignal; labelKey: string
  // descKey 停用（2026-09-18：mode.*.desc 已全部注释，UI 对空描述跳过渲染）
}> = {
  vector: {
    signal: 'danger', labelKey: 'mode.vector',
  },
  contingency: {
    signal: 'danger', labelKey: 'mode.contingency',
  },
  babel: {
    signal: 'accent', labelKey: 'mode.babel',
  },
  // frozen（待春归）：语义是「最冷/最低功耗」而非危险档，signal 用中性 info 而非 danger
  frozen: {
    signal: 'info', labelKey: 'mode.frozen',
  }
}

// [derive]
/**
 * 由 current_mode 值与特调模式集合派生展示信息specialModes` 来自 special_tuned.yaml 的 modes 并集——
 * 该文件只导出精确条目，正则条目对应的特调模式 UI 不可知，只能覆盖「已配置」部分
 * descKey 一律返回空串（mode.*.desc 已全部注释，UI 对空描述跳过渲染）
 */
export function describeMode(id: string, specialModes?: ReadonlySet<string>): ModeInfo {
  const mode = id.trim()
  if (!mode) {
    return {
      id: '',
      kind: 'unknown',
      signal: 'info',
      labelKey: 'mode.unknown',
      descKey: ''
    }
  }
  // DOWN 停摆：不是调度档位，是「调度不工作」本身；判据在 down.chr（见 data/down.ts），显示名就是 id 不分语言
  if (mode === DOWN_WORD) {
    return {
      id: mode,
      kind: 'down',
      signal: 'danger',
      labelKey: 'mode.down',
      descKey: ''
    }
  }
  // scenemode：daemon 不写这个值，但家族位照常注册占位（kind stardust）
  if (mode === 'scenemode') {
    return {
      id: mode,
      kind: 'stardust',
      signal: 'info',
      labelKey: 'mode.scenemode',
      descKey: ''
    }
  }
  if (mode === 'fas') {
    return {
      id: mode,
      kind: 'fas',
      signal: 'accent',
      labelKey: 'mode.fas',
      descKey: ''
    }
  }
  const clg = CLG_CATALOG[mode]
  if (clg) {
    return { id: mode, kind: 'clg', signal: clg.signal, labelKey: clg.labelKey, descKey: '' }
  }
  const lab = LAB_CATALOG[mode]
  if (lab) {
    return { id: mode, kind: 'lab', signal: lab.signal, labelKey: lab.labelKey, descKey: '' }
  }
  if (specialModes?.has(mode)) {
    return {
      id: mode,
      kind: 'special',
      signal: 'accent',
      labelKey: 'mode.special',
      descKey: ''
    }
  }
  return {
    id: mode,
    kind: 'unknown',
    signal: 'info',
    labelKey: 'mode.unknown',
    descKey: ''
  }
}

/** CLG 四档（配置页与规则页展示用） */
export const CLG_MODE_IDS = ['reduce', 'default', 'boost', 'vector'] as const
