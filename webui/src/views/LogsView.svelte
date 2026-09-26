<script lang="ts">
  // LogsView.svelte: [source] [terminal] [snapshot]两类数据源都只读尾部窗口（daemon.log 上限 50MB、status.csv 8MB），界面写明「最近一段」
  // 与归档位置
  import { onMount } from 'svelte'
  import Panel from '@/components/Panel.svelte'
  import Segmented from '@/components/Segmented.svelte'
  import StateBox from '@/components/StateBox.svelte'
  import { filterByLevel, type LogLevelName } from '@/data/daemon-log'
  import { t } from '@/i18n/index.svelte'
  import { app } from '@/state.svelte'

  let source = $state<'daemon' | 'status'>('daemon')
  let level = $state<LogLevelName>('TRACE')

  const sourceItems = $derived([
    { id: 'daemon', label: t('logs.source.daemon') },
    { id: 'status', label: t('logs.source.status') }
  ])

  // 档位名与 daemon.log 级别字面量一致；最宽一档即 TRACE（不设阈值 = 显示全部），故不叫「全部/ALL」
  const levelItems = $derived([
    { id: 'TRACE', label: 'TRACE' },
    { id: 'DEBUG', label: 'DEBUG' },
    { id: 'INFO', label: 'INFO' },
    { id: 'WARN', label: 'WARN' },
    { id: 'ERROR', label: 'ERROR' }
  ])

  const visible = $derived(filterByLevel(app.logLines, level))

  function fmt(value: number | null, digits: number, suffix = ''): string {
    return value === null ? '—' : `${value.toFixed(digits)}${suffix}`
  }

  async function pick(id: string) {
    source = id === 'status' ? 'status' : 'daemon'
    // 切换数据源视为重新进入视图：恢复跟随（进入即看最新）
    followDaemon = true
    followStatus = true
    await app.loadLogs(source)
  }

  /** charge 列固定短词 → 文案键（未知/缺测显示 —） */
  const CHARGE_KEYS: Record<string, string> = {
    charging: 'logs.charge.charging',
    discharging: 'logs.charge.discharging',
    full: 'logs.charge.full',
    not_charging: 'logs.charge.not_charging'
  }

  function chargeLabel(value: string): string {
    const key = CHARGE_KEYS[value]
    return key ? t(key) : '—'
  }

  // [follow] 两个子滚动窗口共用「跟随底部」：新数据到达且处于跟随态时自动滚底；用户滑离底部即退出跟随（滚回也不自动恢复），只能点「回到底部」恢复
  let daemonBody = $state<HTMLDivElement>()
  let snapshotBody = $state<HTMLDivElement>()
  let followDaemon = $state(true)
  let followStatus = $state(true)

  /** 距底 ≤ 24px 视为「在底部」：程序性滚底也会触发 scroll 事件，不能被当成用户滑动 */
  function atBottom(el: HTMLElement): boolean {
    return el.scrollHeight - el.scrollTop - el.clientHeight <= 24
  }

  function onDaemonScroll() {
    if (daemonBody && !atBottom(daemonBody)) followDaemon = false
  }

  function onStatusScroll() {
    if (snapshotBody && !atBottom(snapshotBody)) followStatus = false
  }

  /** 回到底部并恢复跟随（滑动后唯一的恢复途径） */
  function backToBottom() {
    if (source === 'daemon') {
      followDaemon = true
      if (daemonBody) daemonBody.scrollTop = daemonBody.scrollHeight
    } else {
      followStatus = true
      if (snapshotBody) snapshotBody.scrollTop = snapshotBody.scrollHeight
    }
  }

  // [poll] 每秒自刷新（loadLogs 有在飞守卫，轮询不堆积）；跳过 hidden 期 tick 省电，恢复可见时立即补一次回调闭包里 source 取当前值（$state 可变），
  // 不能在 onMount 快照
  onMount(() => {
    void app.loadLogs(source)
    const timer = setInterval(() => {
      if (document.hidden) return
      void app.loadLogs(source)
    }, 1000)
    const onVisible = () => {
      if (document.visibilityState === 'visible') void app.loadLogs(source)
    }
    document.addEventListener('visibilitychange', onVisible)
    return () => {
      clearInterval(timer)
      document.removeEventListener('visibilitychange', onVisible)
    }
  })

  // 跟随滚底在 $effect（DOM 更新后）运行；依赖整个数组而非长度——尾部窗口行数恒定时，内容替换也要滚到底
  $effect(() => {
    void visible
    if (followDaemon && daemonBody) daemonBody.scrollTop = daemonBody.scrollHeight
  })

  $effect(() => {
    void app.statusRows
    if (followStatus && snapshotBody) snapshotBody.scrollTop = snapshotBody.scrollHeight
  })
</script>

{#snippet backToBottomButton()}
  <!-- 回到底部：仅「滑离底部」时出现，绝对定位在所属滚动子块内，不悬浮到页面其它区域 -->
  <button type="button" class="to-bottom" aria-label={t('action.toBottom')} onclick={backToBottom}>
    ↓
  </button>
{/snippet}

<div class="u-stack">
  <Panel title={t('logs.title')}>

    <Segmented items={sourceItems} value={source} label={t('logs.source')} onselect={pick} />

    {#if source === 'daemon'}
      <div class="ak-field u-mt-3">
        <span class="ak-label">{t('logs.level')}</span>
        <!-- 5 档在窄屏压不下：整条左右滑动（scroll），项按内容宽度排列 -->
        <Segmented
          items={levelItems}
          value={level}
          scroll
          onselect={id => (level = id as LogLevelName)}
        />
      </div>
    {/if}
  </Panel>

  {#if source === 'daemon'}
    {#if app.logState === 'failed'}
      <StateBox kind="error" message={t('state.failed')} detail={app.logError} />
    {:else if app.logState === 'missing'}
      <StateBox kind="missing" message={t('logs.missing')} detail={t('logs.archive')} />
    {:else}
      <div class="pane">
        <section class="terminal" data-ak-ui="terminal" aria-label={t('logs.source.daemon')}>
          <header class="terminal__bar u-between">
            <span class="terminal__path u-mono">logs/daemon.log</span>
            <span class="terminal__count u-mono">{t('logs.lines', { n: visible.length })}</span>
          </header>
          <div
            class="terminal__body u-scroll"
            bind:this={daemonBody}
            onscroll={onDaemonScroll}
          >
            {#each visible as line, index (index)}
              <p class="log" data-level={line.level}>
                <span class="log__time u-mono">{line.time}</span>
                <span class="log__level u-mono">{line.level}</span>
                <span class="log__module u-mono">{line.module}</span>
                <span class="log__message">{line.message}</span>
              </p>
            {/each}
          </div>
        </section>
        {#if !followDaemon}
          {@render backToBottomButton()}
        {/if}
      </div>
    {/if}

    {#if app.dirError}
      <StateBox kind="error" message={t('state.failed')} detail={app.dirError} />
    {/if}

    {#if app.logdFiles.length > 0}
      <Panel title="logd/">
        <ul class="files files--scroll u-list-reset u-scroll">
          {#each app.logdFiles as file (file)}
            <li class="files__item u-mono">{file}</li>
          {/each}
        </ul>
      </Panel>
    {/if}

    {#if app.devimpFiles.length > 0}
      <Panel title={t('logs.devimp')}>
        <ul class="files files--scroll u-list-reset u-scroll">
          {#each app.devimpFiles as file (file)}
            <li class="files__item u-mono">{file}</li>
          {/each}
        </ul>
      </Panel>
    {/if}
  {:else}
    {#if app.statusState === 'failed'}
      <StateBox kind="error" message={t('state.failed')} detail={app.statusError} />
    {:else if app.statusState === 'missing'}
      <StateBox
        kind="missing"
        message={app.isChiri ? t('logs.missing') : t('state.notApplicable')}
        detail={app.isChiri ? t('state.daemonStopped') : t('state.chiriOnly')}
      />
    {:else}
      <div class="pane">
        <div
          class="snapshot-wrap u-scroll"
          bind:this={snapshotBody}
          onscroll={onStatusScroll}
        >
          <table class="snapshot" aria-label={t('logs.source.status')}>
            <thead>
              <tr class="snapshot__row snapshot__row--head u-mono">
                <th scope="col">{t('logs.status.times')}</th>
                <th scope="col">{t('logs.status.mode')}</th>
                <th scope="col">{t('logs.status.pkg')}</th>
                <th scope="col">{t('logs.status.batt')}</th>
                <th scope="col">{t('logs.status.load')}</th>
                <th scope="col">{t('logs.status.power')}</th>
                <th scope="col">{t('logs.status.charge')}</th>
              </tr>
            </thead>
            <tbody>
              <!-- 时间升序（旧上新下），与 daemon 终端一致：新行从底部进入并跟随 -->
              {#each app.statusRows as row, index (index)}
                <tr class="snapshot__row u-mono">
                  <td>{row.timestamp}</td>
                  <td>{row.mode || '—'}</td>
                  <td>{row.pkg || '—'}</td>
                  <td>{fmt(row.battTemp, 1, t('unit.celsius'))}</td>
                  <td>{fmt(row.gpuBusy, 1, t('unit.percent'))}</td>
                  <td>{fmt(row.battPower, 1, t('unit.watt'))}</td>
                  <td>{chargeLabel(row.charge)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
        {#if !followStatus}
          {@render backToBottomButton()}
        {/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  .terminal {
    border: var(--ak-line-hairline) solid var(--ak-surface-raised);
    /* 日志终端的沉浸深底（比 canvas 更深）：组件级变量集中在此定义，不散落色值 */
    --terminal-surface: #0a0c0e;
    background: var(--terminal-surface);
  }

  .terminal__bar {
    padding: var(--ak-space-2) var(--ak-space-3);
    border-bottom: var(--ak-line-hairline) solid var(--ak-surface-raised);
    background: var(--ak-surface-panel);
  }

  .terminal__path,
  .terminal__count {
    color: var(--ak-text-secondary);
    font-size: 0.6875rem;
  }

  .terminal__body {
    max-height: 60vh;
    padding: var(--ak-space-3);
  }

  .log {
    display: grid;
    /* 全列按内容自适应 + 不换行：长消息靠容器横向滑动查看 */
    grid-template-columns: auto auto auto minmax(0, max-content);
    gap: var(--ak-space-2);
    margin: 0 0 0.15rem;
    font-size: 0.6875rem;
    line-height: 1.55;
    white-space: nowrap;
  }

  .log__time {
    color: var(--ak-text-secondary);
  }

  /* 级别列不设固定宽：min-width 会让「级别→内容」间距大于「时间→级别」（要求两者一致）；代价是 4/5 字档名混排时模块列起点差约一字宽，可接受 */
  .log__level {
    color: var(--ak-text-secondary);
    font-weight: 700;
    letter-spacing: 0.06em;
  }

  .log__module {
    color: var(--ak-text-secondary);
  }

  .log__message {
    color: var(--ak-text-primary);
  }

  .log[data-level='ERROR'] .log__level,
  .log[data-level='ERROR'] .log__message {
    color: var(--ak-signal-danger);
  }

  .log[data-level='WARN'] .log__level,
  .log[data-level='WARN'] .log__message {
    color: var(--ak-signal-action);
  }

  .log[data-level='INFO'] .log__level {
    color: var(--ak-signal-info);
  }

  .log[data-level='DEBUG'] .log__level,
  .log[data-level='DEBUG'] .log__message {
    color: var(--ak-text-secondary);
  }

  .snapshot-wrap {
    /* 列宽随数据自适应；与 daemon 终端同款子滚动窗口：限高 60vh + 双轴滑动（配合跟随底部），滚动行为来自 .u-scroll */
    max-width: 100%;
    max-height: 60vh;
    border: var(--ak-line-hairline) solid var(--ak-surface-raised);
    background: var(--ak-surface-panel);
  }

  .snapshot {
    /* width: max-content 按内容自适应（width:100% 会把多余宽度摊进各列）；窄于容器左对齐，宽于容器由外层横向滚动 */
    width: max-content;
    border-collapse: collapse;
  }

  .snapshot__row {
    white-space: nowrap;
    border-bottom: var(--ak-line-hairline) solid var(--ak-surface-raised);
    font-size: 0.6875rem;
    line-height: 1.5;
  }

  /* 内距必须落在 th/td 上：tr 的 padding 不参与表格布局（列会糊在一起）；全部左对齐 */
  .snapshot th,
  .snapshot td {
    padding: var(--ak-space-2) var(--ak-space-3);
    text-align: left;
  }

  .snapshot__row--head {
    color: var(--ak-text-secondary);
    font-weight: 700;
    letter-spacing: 0.06em;
  }

  /* 粘性表头必须落在 th 上（tr 上的 sticky + 背景在滚动时不覆盖数据行） */
  .snapshot__row--head th {
    position: sticky;
    top: 0;
    z-index: 1;
    background: var(--ak-surface-raised);
  }

  .files {
    display: grid;
    gap: 0.25rem;
  }

  .files--scroll {
    max-height: 12rem;
  }

  .files__item {
    color: var(--ak-text-secondary);
    font-size: 0.6875rem;
    line-height: 1.6;
    overflow-wrap: anywhere;
  }

  .pane {
    position: relative;
  }

  /* 右下角回到底部：绝对定位在所属滚动子块右下角，滑离底部时出现、回底即消失 */
  .to-bottom {
    position: absolute;
    right: var(--ak-space-3);
    bottom: var(--ak-space-3);
    z-index: 20;
    width: 2.75rem;
    height: 2.75rem;
    border: var(--ak-line-hairline) solid var(--ak-surface-raised);
    border-radius: 999px;
    background: var(--ak-surface-panel);
    color: var(--ak-text-primary);
    font-size: 1rem;
    box-shadow: var(--ak-shadow-panel);
  }
</style>
