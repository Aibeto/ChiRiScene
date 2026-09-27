// export.ts: [start] [poll]。导出历史归档：把 logd/（历次重启的日志归档）打成 tar.gz 放到 /sdcard/Download。打包交给外部脚本 scripts/pack
// sh（对外暴露的稳定接口，与守护进程启动归档共用、构建流程不得修改）：先 tar 再 gzip，完成后删除中间 .tar；设备无 gzip 时保留未压缩 .tar 作为产物硬约束——后台执行：归档可达几百 MB、
// 压缩数十秒级，前台等ksu exec 会被桥的超时掐断，命令自己 fork 到后台、结束写产物/标记文件，前端只轮询——进度分两段，都取文件/内核读数而不是脚本回显：
// 打包段 = .part 字节 / 源总字节，压缩段 = /proc/<压缩器 pid>/io 的 rchar（已读入输入字节）/ .tar 字节
import { absOf, shQuote } from './paths'
import { isLive, run } from '@/kernel/shell'
import { absent, failed, ok, shellError, type ReadResult } from './errors'

/** 导出目录（外部可见，不需要 root 也能拿到） */
const DOWNLOAD_DIR = '/sdcard/Download'
/** 失败标记放 tmpfs：/dev 在 Android 上必有；/sdcard 不可写时标记写不进去、只能等超时 */
const FAIL_FILE = '/dev/chiri_export.fail'
/** tar/gzip 的 stderr（设备排障用；前端只把它当脚本的输出落点，不解析） */
const LOG_FILE = '/dev/chiri_export.log'
/** 源总字节（脚本用 du 预算，打包段进度分母） */
const TOTAL_FILE = '/dev/chiri_export.total'
/** 压缩器 PID（脚本写、压缩结束即删）：前端据此读 /proc/<pid>/io 的 rchar = 压缩已读入字节 */
const PID_FILE = `${FAIL_FILE}.pid`

export interface ExportJob {
  /** tar.gz 目标路径 */
  target: string
  /** 未压缩回退产物（设备不支持 gzip 时保留的 .tar） */
  fallback: string
  /** 失败标记（内容为退出码）：3 = logd 目录不可进，4 = 打包/压缩失败，5 = 没有历史归档 */
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
/**
 * 启动后台打包并立刻返回，完成状态由 pollExport 轮询三个标记文件得出；目录不存在
 * / 只剩本次运行的文件等错误由后台脚本写进 failFlag，前端读退出码映射文案
 */
export async function startExport(): Promise<ReadResult<ExportJob>> {
  if (!isLive()) return absent<ExportJob>('unsupported-env')
  // pack.sh export 的 dest_base 是不带扩展名的基准名（脚本自己生成 <base>.tar 再压成<base>.tar.gz）：传带 .tar 的名字产物会变 logd_X.tar.tar
  // gz，轮询永远找不到目标
  const base = `${DOWNLOAD_DIR}/logd_${stamp()}`
  const job: ExportJob = {
    target: `${base}.tar.gz`,
    fallback: `${base}.tar`,
    failFlag: FAIL_FILE
  }
  // pack.sh 打包细节见文件头；/sdcard/Download 刻意不创建——真撞上不可写会落到失败标记
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
export type ExportPhase = 'running' | 'done-gz' | 'done-tar' | 'empty' | 'no-logd' | 'failed'

export interface ExportProgress {
  /** 进度条百分比（0~99；分母 = 计划处理字节 = 源总字节 + .tar 字节） */
  percent: number
  /** 已处理字节：打包段 = .part（tar 边读边写），压缩段 = .tar 满载 + 压缩器已读入字节 */
  read: number
  /** 计划处理字节（打包期间 .tar 还没出现，先用源总字节顶替） */
  planned: number
  /** 产物当前字节（.part + .tar + .tar.gz 之和，单调增长；压缩进度读不到时当兜底文案） */
  written: number
  /** 压缩进行中却读不到「已读入字节」（/proc 不可读或内核未计 io）→ 分母无从推算，前端改走不定态 */
  blind: boolean
}

export interface ExportProbe {
  phase: ExportPhase
  progress: ExportProgress
}

/** 一轮探测：产物出现且无「压缩中」标记 = 完成、失败标记出现 = 按退出码分类、都没有 = 还在压；顺带读回进度六数 */
export async function pollExport(job: ExportJob): Promise<ReadResult<ExportProbe>> {
  if (!isLive()) return absent<ExportProbe>('unsupported-env')
  const tarPath = job.fallback
  const part = `${tarPath}.part`
  const mode = `${job.failFlag}.mode`
  // 每个数都先 [ -f ] 守卫再读——`wc -c < 缺失文件` 是输入重定向错误，由 shell 直接打到 stderr（命令上的 2>/dev/null 覆盖不到），真机 mksh 会判整条命令失败
  const cmd =
    // fail 必须最先判：失败时脚本已删 .mode 而半截产物可能还在，先看产物会把失败当完成上报
    `[ -f ${shQuote(job.failFlag)} ] && echo "s:fail:$(cat ${shQuote(job.failFlag)} 2>/dev/null)"; ` +
    // .mode = gzip 进行中标记：gzip 就地流式写 <base>.tar.gz（边压边长），不看标记会在压缩刚开始就误判完成、下载到半截归档
    `[ ! -f ${shQuote(mode)} ] && [ -f ${shQuote(job.target)} ] && echo s:gz; ` +
    `[ ! -f ${shQuote(mode)} ] && [ -f ${shQuote(tarPath)} ] && echo s:tar; ` +
    `p=0; [ -f ${shQuote(part)} ] && p=$(wc -c < ${shQuote(part)}); echo "p:$p"; ` +
    `s=0; [ -f ${shQuote(tarPath)} ] && s=$(wc -c < ${shQuote(tarPath)}); echo "s:$s"; ` +
    `g=0; [ -f ${shQuote(job.target)} ] && g=$(wc -c < ${shQuote(job.target)}); echo "g:$g"; ` +
    `t=0; [ -f ${shQuote(TOTAL_FILE)} ] && t=$(cat ${shQuote(TOTAL_FILE)} 2>/dev/null); echo "t:$t"; ` +
    // r = 压缩器已读入的输入字节（/proc/<pid>/io 的 rchar），压缩段唯一的进度源；pid 标记或 io 文件读不到就保持 0（→ 前端不定态）
    `r=0; [ -f ${shQuote(PID_FILE)} ] && { z=$(cat ${shQuote(PID_FILE)} 2>/dev/null); ` +
    `[ -n "$z" ] && [ -r "/proc/$z/io" ] && while read -r k v; do [ "$k" = "rchar:" ] && r=$v; done < "/proc/$z/io"; }; ` +
    `echo "r:$r"; ` +
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
    const gzBytes = num('g')
    const srcBytes = num('t')
    const consumed = num('r')
    // 计划处理字节 = 源总字节（打包的输入）+ .tar 字节（压缩的输入）；打包期间 .tar 还没出现，先用源总字节顶替，tar 只比源文件多
    // 头/填充、两值接近，切换时分母不变形、进度不回跳
    const planned = srcBytes + (tarBytes > 0 ? tarBytes : srcBytes)
    // 已处理字节：.part 还在 = 打包段（tar 边读边写，文件大小就是已读入量）；.part 已 mv 成 .tar = 压缩段（那一步已完成，再加压缩器读掉的量）
    const read = partBytes > 0 ? partBytes : tarBytes + consumed
    const progress: ExportProgress = {
      percent: planned > 0 ? Math.min(99, Math.round((read / planned) * 100)) : 0,
      read,
      planned,
      written: partBytes + tarBytes + gzBytes,
      // .tar 已就绪却读不到 rchar = 压缩在跑但拿不到进度源（/proc 不可读或内核未计 io）→ 界面走不定态
      blind: tarBytes > 0 && consumed <= 0
    }
    // fail 先于产物判定：打包/压缩失败会留下半截 .tar 或 .tar.gz（且 .mode 已删），先判产物会把失败静默上报成完成
    const fail = /^s:fail:(\d+)/m.exec(out)
    if (fail) {
      const phase = fail[1] === '5' ? 'empty' : fail[1] === '3' ? 'no-logd' : 'failed'
      return ok({ phase, progress })
    }
    // 产物判定已由命令侧 `[ ! -f .mode ]` 守卫：.mode 在 = gzip 进行中，此刻产物存在也不算完成
    if (/^s:gz$/m.test(out)) return ok({ phase: 'done-gz', progress })
    if (/^s:tar$/m.test(out)) return ok({ phase: 'done-tar', progress })
    return ok({ phase: 'running', progress })
  } catch (e) {
    return failed<ExportProbe>(e instanceof Error ? e.message : String(e))
  }
}
