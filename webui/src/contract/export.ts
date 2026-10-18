// export.ts: [start] [poll]
// 导出历史归档：把 logd/（历次重启的日志归档）打成 tar 放到 /sdcard/Download，打包交给外部脚本 scripts/pack.sh（对外暴露的稳定接口，与守护进程启动归档共用）：仅 tar 不压缩，命令自己 fork 到后台，前端只轮询产物/标记文件，进度取 .part 字节 / 源总字节
import { absOf, shQuote } from './paths'
import { isLive, run } from '@/kernel/shell'
import { absent, failed, ok, shellError, type ReadResult } from './errors'

/** 导出目录（外部可见，不需要 root 也能拿到） */
const DOWNLOAD_DIR = '/sdcard/Download'
/** 失败标记放 tmpfs：/dev 在 Android 上必有；/sdcard 不可写时标记写不进去、只能等超时 */
const FAIL_FILE = '/dev/chiri_export.fail'
/** tar 的 stderr（设备排障用；前端只把它当脚本的输出落点，不解析） */
const LOG_FILE = '/dev/chiri_export.log'
/** 源总字节（脚本用 du 预算，打包段进度分母） */
const TOTAL_FILE = '/dev/chiri_export.total'

export interface ExportJob {
  /** tar 目标路径 */
  target: string
  /** 失败标记（内容为退出码）：3 = logd 目录不可进，4 = 打包失败，5 = 没有历史归档 */
  failFlag: string
}

/** 本地时间戳 MMDD-HHmmss，人眼可辨，与归档命名同风格 */
function stamp(now = new Date()): string {
  const p = (n: number) => String(n).padStart(2, '0')
  return (
    `${p(now.getMonth() + 1)}${p(now.getDate())}-` +
    `${p(now.getHours())}${p(now.getMinutes())}${p(now.getSeconds())}`
  )
}

// [start]
/** 启动后台打包并立刻返回，完成状态由 pollExport 轮询三个标记文件得出；目录不存在 / 只剩本次运行的文件等错误由后台脚本写进 failFlag，前端读退出码映射文案 */
export async function startExport(): Promise<ReadResult<ExportJob>> {
  if (!isLive()) return absent<ExportJob>('unsupported-env')
  // pack.sh export 的 dest_base 是不带扩展名的基准名（脚本自己生成 <base>.tar）：传带 .tar 的名字产物会变 logd_X.tar.tar，
  // 轮询永远找不到目标
  const base = `${DOWNLOAD_DIR}/logd_${stamp()}`
  const job: ExportJob = {
    target: `${base}.tar`,
    failFlag: FAIL_FILE
  }
  // /sdcard/Download 刻意不创建：真撞上不可写会落到失败标记
  const cmd =
    `nohup sh ${shQuote(absOf('packSh'))} export ${shQuote(absOf('logdDir'))} ` +
    `${shQuote(base)} ${shQuote(FAIL_FILE)} ${shQuote(LOG_FILE)} ${shQuote(TOTAL_FILE)} ` +
    `>/dev/null 2>&1 & echo started`
  try {
    const { errno, stderr } = await run(cmd)
    // 带操作上下文：裸的「命令失败(1)」无法定位是启动挂了还是桥的瞬时错误
    if (errno !== 0) return failed<ExportJob>(`启动导出失败：${shellError(errno, stderr)}`)
  } catch (e) {
    return failed<ExportJob>(`启动导出失败：${e instanceof Error ? e.message : String(e)}`)
  }
  return ok(job)
}

// [poll]
export type ExportPhase = 'running' | 'done-tar' | 'empty' | 'no-logd' | 'failed'

export interface ExportProgress {
  /** 进度条百分比（0~99；分母 = 源总字节） */
  percent: number
  /** 已处理字节：.part（tar 边读边写，文件大小就是已读入量） */
  read: number
  /** 计划处理字节（源总字节） */
  planned: number
  /** 产物当前字节（.part，单调增长） */
  written: number
  /** 保留字段：导出无压缩段，恒 false */
  blind: boolean
}

export interface ExportProbe {
  phase: ExportPhase
  progress: ExportProgress
}

/** 一轮探测：产物出现 = 完成、失败标记出现 = 按退出码分类、都没有 = 还在打包；顺带读回进度三数 */
export async function pollExport(job: ExportJob): Promise<ReadResult<ExportProbe>> {
  if (!isLive()) return absent<ExportProbe>('unsupported-env')
  const tarPath = job.target
  const part = `${tarPath}.part`
  // 每个数都先 [ -f ] 守卫再读——`wc -c < 缺失文件` 是输入重定向错误，由 shell 直接打到 stderr（命令上的 2>/dev/null 覆盖不到），真机 mksh 会判整条命令失败
  const cmd =
    // fail 必须最先判：失败会留下半截 .tar.part，先看产物会把失败当完成上报
    `[ -f ${shQuote(job.failFlag)} ] && echo "s:fail:$(cat ${shQuote(job.failFlag)} 2>/dev/null)"; ` +
    `[ -f ${shQuote(tarPath)} ] && echo s:tar; ` +
    `p=0; [ -f ${shQuote(part)} ] && p=$(wc -c < ${shQuote(part)}); echo "p:$p"; ` +
    `s=0; [ -f ${shQuote(tarPath)} ] && s=$(wc -c < ${shQuote(tarPath)}); echo "s:$s"; ` +
    `t=0; [ -f ${shQuote(TOTAL_FILE)} ] && t=$(cat ${shQuote(TOTAL_FILE)} 2>/dev/null); echo "t:$t"; ` +
    `exit 0`
  try {
    const { errno, stdout, stderr } = await run(cmd)
    // 只有「非零且没拿到任何可用输出」才算探测失败——个别 shell 中途返回非零但输出齐全，误判会让导出显示假失败
    if (errno !== 0 && !/^p:/m.test(stdout)) {
      return failed<ExportProbe>(`查询导出进度失败：${shellError(errno, stderr)}`)
    }
    const out = stdout
    const num = (prefix: string): number => {
      // \s* 容忍 wc 系工具在 stdin 计数上的前导空格（GNU 多输入时会有 padding）
      const m = new RegExp(`^${prefix}:\\s*(\\d+)`, 'm').exec(out)
      return m ? Number(m[1]) : 0
    }
    const partBytes = num('p')
    const tarBytes = num('s')
    const srcBytes = num('t')
    const planned = srcBytes
    // 已处理字节：.part 还在 = 打包段（tar 边读边写，文件大小就是已读入量）；.part 已 mv 成 .tar = 完成（满载）
    const read = partBytes > 0 ? partBytes : tarBytes
    const progress: ExportProgress = {
      percent: planned > 0 ? Math.min(99, Math.round((read / planned) * 100)) : 0,
      read,
      planned,
      written: partBytes + tarBytes,
      blind: false
    }
    // fail 先于产物判定：打包失败会留下半截 .tar.part，先判产物会把失败静默上报成完成
    const fail = /^s:fail:(\d+)/m.exec(out)
    if (fail) {
      const phase = fail[1] === '5' ? 'empty' : fail[1] === '3' ? 'no-logd' : 'failed'
      return ok({ phase, progress })
    }
    if (/^s:tar$/m.test(out)) return ok({ phase: 'done-tar', progress })
    return ok({ phase: 'running', progress })
  } catch (e) {
    return failed<ExportProbe>(e instanceof Error ? e.message : String(e))
  }
}
