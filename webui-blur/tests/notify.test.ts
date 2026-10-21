// notify.test.ts: [tag] [send]。通知测试契约：固定 tag、三条预填文案、以及最关键的
// ——「脚本退出码 0 不等于用户看得到」这一结论必须带着脚本原始输出一起回到界面，
// 否则又是「只报已发送、真失败被藏起来」的形态（本轮修的就是它）
import { describe, expect, it, afterEach } from 'vitest'
import { TEST_BODY_DEFAULT, TEST_TAG, TEST_TITLE_DEFAULT, sendTestNotification } from '@/contract/notify'
import { setShell, type ExecResult, type ShellRunner } from '@/kernel/shell'

/** 可编排的假 shell：记录收到的命令，回放预置结果 */
function fakeShell(result: Partial<ExecResult>): { runner: ShellRunner; commands: string[] } {
  const commands: string[] = []
  const runner: ShellRunner = {
    live: true,
    exec: (command: string) => {
      commands.push(command)
      return Promise.resolve({
        errno: result.errno ?? 0,
        stdout: result.stdout ?? '',
        stderr: result.stderr ?? '',
      })
    },
  }
  return { runner, commands }
}

describe('测试通知固定 tag', () => {
  it('tag 是 ChiRi-Test（大写 C/S/T，与 ChiRi-FAS / ChiRi-Status 并列，不会互相覆盖）', () => {
    expect(TEST_TAG).toBe('ChiRi-Test')
  })

  it('标题与正文有预填值（用户可直接点发送，也可改）', () => {
    expect(TEST_TITLE_DEFAULT.length).toBeGreaterThan(0)
    expect(TEST_BODY_DEFAULT.length).toBeGreaterThan(0)
  })
})

describe('sendTestNotification 结果口径', () => {
  afterEach(() => {
    // 还原注入，避免污染其它用例（setShell 是模块级单例）
    setShell({ live: false, exec: () => Promise.reject(new Error('inert')) })
  })

  function use(result: Partial<ExecResult>): { commands: string[] } {
    const ctx = fakeShell(result)
    setShell(ctx.runner)
    return { commands: ctx.commands }
  }

  it('脚本退出码 0：记为成功，并把脚本输出原样带回', async () => {
    const { commands } = use({
      errno: 0,
      stdout: 'S1=su -lp 2000 -c "..."\nS1:ok\nOK S1\nLIST ok ChiRi-Test\n__EXIT:0\n',
    })
    const r = await sendTestNotification('标题', '正文')
    expect(r.kind).toBe('ok')
    if (r.kind !== 'ok') return
    expect(r.value.accepted).toBe(true)
    // 输出里必须留下「哪条策略成功」与 LIST 结论——这是判断真可见的唯一线索
    expect(r.value.output).toContain('OK S1')
    expect(r.value.output).toContain('LIST ok')
    // 内部标记不进展示文本
    expect(r.value.output).not.toContain('__EXIT')
    expect(commands[0]).toContain('notify.sh')
  })

  it('脚本退出码非 0：记为失败，但把脚本输出当错误串带出（不是干巴巴的「命令失败」）', async () => {
    use({
      errno: 0,
      stdout: 'S1:fail:1 error: not displayed\nFAIL 所有策略均失败\n__EXIT:1\n',
    })
    const r = await sendTestNotification('标题', '正文')
    expect(r.kind).toBe('failed')
    if (r.kind !== 'failed') return
    expect(r.error).toContain('S1:fail:1')
    expect(r.error).toContain('FAIL')
  })

  it('list 报 miss 也算成功（退出码 0 是判据），但输出里带着 LIST miss 供人判断', async () => {
    use({ errno: 0, stdout: 'S4:ok\nOK S4\nLIST miss ChiRi-Test\n__EXIT:0\n' })
    const r = await sendTestNotification('标题', '正文')
    expect(r.kind).toBe('ok')
    if (r.kind !== 'ok') return
    expect(r.value.output).toContain('LIST miss')
  })

  it('连 __EXIT 标记都没有（脚本没执行起来）→ 失败并指出这一点', async () => {
    use({ errno: 127, stdout: '', stderr: 'sh: notify.sh: not found' })
    const r = await sendTestNotification('标题', '正文')
    expect(r.kind).toBe('failed')
    if (r.kind !== 'failed') return
    expect(r.error).toContain('未产生输出')
  })

  it('无真实执行能力的环境返回 unsupported-env，不去起进程', async () => {
    setShell({ live: false, exec: () => Promise.reject(new Error('inert')) })
    const r = await sendTestNotification('标题', '正文')
    expect(r.kind).toBe('absent')
  })

  it('标题/正文里的引号经 shQuote 转义，不会拆坏命令', async () => {
    const { commands } = use({ errno: 0, stdout: 'OK S1\n__EXIT:0\n' })
    await sendTestNotification("it's a \"test\"", 'body')
    const cmd = commands[0]
    // shQuote 把内部单引号写成 '\''，命令里不应出现未转义的裸单引号收尾
    expect(cmd).toContain(`'it'\\''s a "test"'`)
    expect(cmd).toContain(`'ChiRi-Test'`)
  })
})
