// notify.ts: [send] [contract]。通知测试契约：把标题/tag/正文交给模块自带的 scripts/notify.sh 投递。
//
// 为什么不在这里自己拼 `cmd notification post`：见 scripts/notify.sh 的文件头。要点是
// ①各 ROM 旗标支持不一、要按序试错；②`-c` 不是渠道、第三位置参数是 tag；③**以 root 投递时
// `cmd` 退出码为 0 但通知栏什么都不出现**（uid 0 归属伪包 `root`，enqueue 阶段就被丢弃），
// 必须换身份到 uid 2000（`su 2000 /system/bin/cmd …`，真机实测可用）。这三点都需要 shell
// 里的条件分支与引号转义，因此整体下沉到脚本，WebUI 只做「传参 + 原样回显脚本输出」。
//
// 回显是刻意的：脚本 stdout 逐行带着「第几条策略、是否成功、cmd 的报错首行」。若只显示一句
// 「已发送」，真的没发出去时用户和我们都无从判断——这正是 daemon 那个 bug 的形态
import { absOf, shQuote } from './paths'
import { isLive, run } from '@/kernel/shell'
import { absent, failed, ok, shellError, type ReadResult } from './errors'

/** 测试通知固定使用的 tag（AOSP 语义的 tag，本仓库历史命名「渠道」）：与 FAS/启动通知互不覆盖 */
export const TEST_TAG = 'ChiRi-Test'

/** 预填标题（可改） */
export const TEST_TITLE_DEFAULT = 'ChiRi 通知测试'
/** 预填正文（可改） */
export const TEST_BODY_DEFAULT = '如果你看到这条通知，说明通知通道可用'

export interface NotifyResult {
  /** 脚本原始输出（逐行策略与结果）；界面原样展示，便于把真实失败原因带出来 */
  output: string
  /** 是否至少有一条策略被 cmd 接受（脚本退出码 0）——**不代表用户一定看得到** */
  accepted: boolean
}

/**
 * 发一条测试通知。三参交给 `scripts/notify.sh`；脚本退出码非 0 记为失败但**仍带着输出**，
 * 免得「命令失败(1)」这种没有上下文的报错把真正的诊断盖掉
 */
export async function sendTestNotification(
  title: string,
  body: string
): Promise<ReadResult<NotifyResult>> {
  if (!isLive()) return absent<NotifyResult>('unsupported-env')
  // tag 固定，title/body 走 shQuote；三者都是脚本的位置参数，不经内层 shell，天然免二次转义
  const cmd =
    `sh ${shQuote(absOf('notifySh'))} ${shQuote(title)} ${shQuote(TEST_TAG)} ${shQuote(body)}` +
    `; echo "__EXIT:$?"`
  try {
    const { errno, stdout, stderr } = await run(cmd)
    // 退出码从 stdout 尾行取：把「脚本说失败」与「脚本根本没跑起来」分成两种结果
    const m = /__EXIT:(-?\d+)\s*$/m.exec(stdout)
    const output = stdout.replace(/__EXIT:-?\d+\s*$/m, '').trim()
    if (!m) {
      // 连标记都没打出来 → shell 层就没执行成功（脚本缺失 / sh 不可用）
      return failed<NotifyResult>(
        `通知脚本未产生输出：${shellError(errno, stderr)}`
      )
    }
    const code = Number(m[1])
    if (code === 0) return ok({ output, accepted: true })
    // 失败也回传 output：脚本已把「哪条策略、cmd 报了什么」写在里面
    return failed<NotifyResult>(
      output || `通知脚本退出码 ${code}${stderr.trim() ? `：${stderr.trim()}` : ''}`
    )
  } catch (e) {
    return failed<NotifyResult>(e instanceof Error ? e.message : String(e))
  }
}
