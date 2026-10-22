<script lang="ts">
  // LogsView.svelte: [source] [terminal] [snapshot] 两类数据源都只读尾部窗口（daemon.log 上限 50MB、status.csv 8MB）
  // 界面写明「最近一段」与归档位置
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

  // [perf] 稳定 key：增量解析会复用旧行对象，按对象身份编号后追加/裁剪只动首尾节点；
  // 用 index 当 key 时新行入队会让每一行的文本都重写（2000 行 × 每秒）
  const rowIds = new WeakMap<object, number>()
  let nextRowId = 0
  function rowKey(row: object): number {
    let id = rowIds.get(row)
    if (id === undefined) {
      id = ++nextRowId
      rowIds.set(row, id)
    }
    return id
  }

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
    if (!daemonBody) return
    // 刷新期间的重建/夹回 0 不算用户操作，保留上一次真实的横向位置
    if (!logUpdating) daemonLeft = daemonBody.scrollLeft
    if (!atBottom(daemonBody)) followDaemon = false
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

  // [poll] 每秒自刷新（loadLogs 有在飞守卫，轮询不堆积）；跳过 hidden 期 tick 省电，恢复可见时立即补一次
  // 回调闭包里 source 取当前值（$state 可变），不能在 onMount 快照
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

  // 日志每秒可能整段重建（tail 窗口锚定失效时行对象全换新，keyed each 会重建 DOM），
  // DOM 更新前记住横向位置，更新后还原；否则 scrollLeft 随旧节点销毁回到 0
  let daemonLeft = 0
  // 日志刷新期间浏览器可能先把 scrollLeft 夹回 0 并触发 scroll 事件；
  // 用 updating 标记忽略这次「程序性归零」，只记住用户主动滑到的位置
  let logUpdating = false
  $effect.pre(() => {
    void visible
    logUpdating = true
  })

  // 跟随滚底在 $effect（DOM 更新后）运行；依赖整个数组而非长度——尾部窗口行数恒定时，内容替换也要滚到底
  $effect(() => {
    void visible
    if (!daemonBody) {
      logUpdating = false
      return
    }
    if (followDaemon) daemonBody.scrollTop = daemonBody.scrollHeight
    // 只还原横向位置；纵向仍由「跟随底部」或用户手动位置决定。
    // 下一帧再写一次：DOM 刚换完时 scrollWidth 可能还没算好，立刻写会被夹回 0
    if (daemonLeft > 0) {
      const left = daemonLeft
      daemonBody.scrollLeft = left
      requestAnimationFrame(() => {
        if (daemonBody) daemonBody.scrollLeft = left
      })
    }
    queueMicrotask(() => {
      logUpdating = false
    })
  })

  $effect(() => {
    void app.statusRows
    if (followStatus && snapshotBody) snapshotBody.scrollTop = snapshotBody.scrollHeight
  })
</script>

{#snippet backToBottomButton()}
  <!-- 回到底部：仅「滑离底部」时出现，绝对定位在所属滚动子块内（.pane position:relative），不悬浮到页面其它区域 -->
  <button type="button" class="to-bottom" aria-label={t('action.toBottom')} onclick={backToBottom}>
    ↓
  </button>
{/snippet}

<div class="u-stack">
  <Panel title={t('logs.title')}>

    <Segmented items={sourceItems} value={source} label={t('logs.source')} onselect={pick} />

    {#if source === 'daemon'}
      <div class="g-field u-mt-3">
        <span class="g-label">{t('logs.level')}</span>
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
        <!-- 终端是嵌在浅色页面里的沉浸深底（Windows Terminal 观感）：深色实心面，不叠模糊层 -->
        <section class="terminal" aria-label={t('logs.source.daemon')}>
          <header class="terminal__bar u-between">
            <span class="terminal__path u-mono">logs/daemon.log</span>
            <span class="terminal__count u-mono">{t('logs.lines', { n: visible.length })}</span>
          </header>
          <div
            class="terminal__body u-scroll"
            bind:this={daemonBody}
            onscroll={onDaemonScroll}
          >
            {#each visible as line (rowKey(line))}
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
          class="snapshot-wrap g-surface u-scroll"
          bind:this={snapshotBody}
          onscroll={onStatusScroll}
        >
          <table class="snapshot" aria-label={t('logs.source.status')}>
            <thead>
              <tr class="snapshot__row snapshot__row--head u-mono">
                <th scope="col">{t('logs.status.times')}</th>
                <th scope="col">{t('logs.status.mode')}</th>
                <th scope="col">{t('logs.status.pkg')}</th>
                <th scope="col">{t('logs.status.load')}</th>
                <th scope="col">{t('logs.status.power')}</th>
                <th scope="col">{t('logs.status.charge')}</th>
              </tr>
            </thead>
            <tbody>
              <!-- 时间升序（旧上新下），与 daemon 终端一致：新行从底部进入并跟随 -->
              {#each app.statusRows as row (rowKey(row))}
                <tr class="snapshot__row u-mono">
                  <td>{row.timestamp}</td>
                  <td>{row.mode || '—'}</td>
                  <td>{row.pkg || '—'}</td>
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
  /* 终端深底是「场景里的屏幕」：等宽日志要沉浸阅读区，用系统终端的中性深灰 + 一圈细边
     深底上用一整套局部 token 覆盖（文字/信号色都换成暗色高亮档），下游 .log* 规则一行不用改 */
  .terminal {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    border: var(--hairline) solid #3b3a39;
    border-radius: var(--radius-m);
    background: #1b1a19;
    /* 局部 token：浅色主题的正文/信号色在深底上对比不足，这里按 Fluent 暗色调色板重挂 */
    --text-primary: #faf9f8;
    --text-secondary: #e1dfdd;
    --text-muted: #a19f9d;
    --surface-border: #3b3a39;
    --divider: #3b3a39;
    --sig-danger: #f1707b;
    --sig-action: #fce100;
    --sig-info: #4cc2ff;
  }

  .terminal__bar {
    padding: var(--space-2) var(--space-3);
    border-bottom: var(--hairline) solid var(--surface-border);
  }

  .terminal__path,
  .terminal__count {
    color: var(--text-muted);
    font-size: 0.6875rem;
  }

  .terminal__body {
    min-width: 0;
    max-height: 60vh;
    padding: var(--space-3);
    /* 横向滚动必须由本层承担：日志行不换行，长消息靠左右滑动查看 */
    overflow-x: auto;
    overflow-y: auto;
  }

  .log {
    display: grid;
    /* 行按自身内容撑到 max-content（至少铺满容器）：grid 容器默认填满父宽，
       长消息只会溢出而不参与父级 scrollWidth，导致外层无法横向滑动 */
    width: max-content;
    min-width: 100%;
    /* 全列按内容自适应 + 不换行：长消息靠容器横向滑动查看 */
    grid-template-columns: auto auto auto minmax(0, max-content);
    gap: var(--space-2);
    margin: 0 0 0.15rem;
    font-size: 0.6875rem;
    line-height: 1.55;
    white-space: nowrap;
    /* 不用 content-visibility：宽度 containment 会在日志行增删时把 scrollWidth 短暂压小，
       浏览器随即把横向 scrollLeft 夹回 0，表现为「每刷新一次就回到最左侧」 */
  }

  .log__time {
    color: var(--text-muted);
  }

  /* 级别列不设固定宽：min-width 会让「级别→内容」间距大于「时间→级别」；4/5 字档名混排时模块列起点差约一字宽 */
  .log__level {
    color: var(--text-muted);
    font-weight: 700;
    letter-spacing: 0.06em;
  }

  .log__module {
    color: var(--text-muted);
  }

  .log__message {
    color: var(--text-secondary);
  }

  /* 信号色只标状态（每级另有级别文字，不靠颜色单独传达）；映射与全局 [signal-map] 一致 */
  .log[data-level='ERROR'] .log__level,
  .log[data-level='ERROR'] .log__message {
    color: var(--sig-danger);
  }

  .log[data-level='WARN'] .log__level,
  .log[data-level='WARN'] .log__message {
    color: var(--sig-action);
  }

  .log[data-level='INFO'] .log__level {
    color: var(--sig-info);
  }

  .log[data-level='DEBUG'] .log__level,
  .log[data-level='DEBUG'] .log__message {
    color: var(--text-muted);
  }

  .snapshot-wrap {
    /* 列宽随数据自适应；与 daemon 终端同款子滚动窗口：限高 60vh + 双轴滑动（配合跟随底部），滚动行为来自 .u-scroll
       深色实心面不叠模糊（嵌在浅色卡片里），圆角成块 */
    max-width: 100%;
    max-height: 60vh;
    border: var(--hairline) solid var(--surface-border);
    border-radius: var(--radius-m);
  }

  .snapshot {
    /* width: max-content 按内容自适应（width:100% 会把多余宽度摊进各列）；窄于容器左对齐，宽于容器由外层横向滚动 */
    width: max-content;
    border-collapse: collapse;
  }

  .snapshot__row {
    white-space: nowrap;
    border-bottom: var(--hairline) solid var(--surface-border);
    font-size: 0.6875rem;
    line-height: 1.5;
  }

  /* 内距必须落在 th/td 上：tr 的 padding 不参与表格布局（列会糊在一起）；全部左对齐 */
  .snapshot th,
  .snapshot td {
    padding: var(--space-2) var(--space-3);
    text-align: left;
  }

  .snapshot__row--head {
    color: var(--text-muted);
    font-weight: 700;
    letter-spacing: 0.06em;
  }

  /* 粘性表头必须落在 th 上（tr 上的 sticky + 背景在滚动时不覆盖数据行）：夜景实底，滚动时数据行不透出表头下 */
  .snapshot__row--head th {
    position: sticky;
    top: 0;
    z-index: 1;
    background: var(--surface-alt);
  }

  .files {
    display: grid;
    gap: 0.25rem;
  }

  .files--scroll {
    max-height: 12rem;
  }

  .files__item {
    color: var(--text-muted);
    font-size: 0.6875rem;
    line-height: 1.6;
    overflow-wrap: anywhere;
  }

  .pane {
    position: relative;
  }

  /* 右下角回到底部：absolute 锚在所属滚动子块（.pane）右下角，滑离底部时出现、回底即消失（项目红线：禁 position: fixed） */
  .to-bottom {
    position: absolute;
    right: var(--space-3);
    bottom: var(--space-3);
    z-index: 20;
    width: 2.75rem;
    height: 2.75rem;
    border: var(--hairline) solid var(--surface-border);
    border-radius: 999px;
    background: var(--surface);
    color: var(--text-primary);
    font-size: 1rem;
    box-shadow: var(--shadow-2);
  }
</style>
