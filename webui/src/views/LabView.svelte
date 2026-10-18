<script lang="ts">
  // LabView.svelte: [bar] [notice] [modes] [state] 实验室二级页只摆事实：rhine.chr 内容与设备当前模式分两处显示、不做因果推断
  // 套用是否真的生效要看 daemon.log
  import { onMount } from 'svelte'
  import ConfirmSheet from '@/components/ConfirmSheet.svelte'
  import Panel from '@/components/Panel.svelte'
  import StateBox from '@/components/StateBox.svelte'
  import { LAB_ENABLEABLE, LAB_MODE_KEYS, type LabModeKey } from '@/data/lab'
  import { t } from '@/i18n/index.svelte'
  import { go } from '@/router.svelte'
  import { app } from '@/state.svelte'

  /** 展示顺序即 rhine-init.yaml 里的定义顺序（新增模式不用改本文件） */
  const MODES = LAB_MODE_KEYS

  const current = $derived(app.labMode)
  const busy = $derived(app.labPending)
  /** 锁定中：直接关闭会被守护进程挡回，点关闭先进风险确认（强制关闭） */
  const locked = $derived(app.labLocked)
  /** 强制关闭的风险确认弹层 */
  let forceAsk = $state(false)
  /** 文件里写着保留字 off：请求已提交，等守护进程收敛回未启用 */
  const forcing = $derived(app.labState.kind === 'force-off')
  /** 标记里的痕迹代号 → 文案；未收录的代号原样显示（便于跟进守护进程新增的代号） */
  function noteText(code: string): string {
    const key = `lab.lock.note.${code}`
    const text = t(key)
    return text === key ? code : text
  }
  /** 运行配置被改动 / 不一致：代号映射成文案，另附守护进程留下的痕迹 */
  const warnLines = $derived([
    ...app.labWarnings.map(code => t(`lab.warn.${code}`)),
    ...(app.labLock.notes.length > 0
      ? [`${t('lab.warn.notes')}: ${app.labLock.notes.map(noteText).join(' / ')}`]
      : [])
  ])
  // 「尚无模式记录」与「模式名未知」是两件事，与总览页同口径
  const deviceMode = $derived(
    app.modeMissing ? t('mode.unknown.missing') : t(app.modeInfo.labelKey)
  )

  function enableable(key: LabModeKey): boolean {
    return (LAB_ENABLEABLE as readonly string[]).includes(key)
  }

  /** 点关闭：未锁定直接关；锁定中先把风险摊开（写空内容会被守护进程挡回去） */
  function requestDisable(): void {
    if (locked) {
      forceAsk = true
      return
    }
    void app.setLabMode(null)
  }

  function confirmForce(): void {
    forceAsk = false
    void app.forceDisableLab()
  }

  onMount(() => {
    if (!app.ready) void app.loadOverview()
    void app.loadLab()
  })
</script>

<div class="u-stack">
  <header class="bar u-row">
    <button type="button" class="g-btn g-btn--ghost bar__back" onclick={() => go('config')}>
      <span class="bar__arrow" aria-hidden="true">←</span>
      {t('lab.action.back')}
    </button>
    <p class="bar__title">{t('lab.title')}</p>
  </header>

  {#if !app.isChiri}
    <StateBox kind="missing" message={t('state.notApplicable')} detail={t('state.chiriOnly')} />
  {:else}
    <Panel signal="action" title={t('lab.subtitle')}>
      <ul class="u-notice">
        <li>{t('lab.notice.stability')}</li>
        <li>{t('lab.notice.reboot')}</li>
        <li>{t('lab.notice.effects')}</li>
        <li>{t('lab.notice.manual')}</li>
      </ul>
    </Panel>

    {#if warnLines.length > 0}
      <StateBox
        kind="error"
        message={t('lab.warn.title')}
        detail={`${t('lab.warn.hint')} ${warnLines.join(' · ')}`}
      />
    {/if}

    {#if app.labError}
      <StateBox kind="error" message={t('state.failed')} detail={app.labError} />
    {/if}

    {#if forcing}
      <StateBox
        kind="missing"
        message={t('lab.force.pending')}
        detail={t('lab.force.pendingHint')}
      />
    {/if}

    <Panel title={t('lab.title')} desc={app.labPath || undefined}>
      <div class="modes">
        {#each MODES as key (key)}
          <article
            class="mode"
            data-active={current === key ? 'yes' : undefined}
            data-reserved={enableable(key) ? undefined : 'yes'}
          >
            <div class="u-between">
              <h3 class="mode__name">{t(`lab.mode.${key}`)}</h3>
              {#if current === key}
                <span class="g-tag g-tag--danger mode__tag">{t('lab.status.on')}</span>
              {:else if !enableable(key)}
                <span class="g-tag g-tag--neutral mode__tag">{t('lab.status.reserved')}</span>
              {:else}
                <span class="g-tag g-tag--neutral mode__tag">{t('lab.status.off')}</span>
              {/if}
            </div>
            <p class="u-note">{t(`lab.mode.${key}.detail`)}</p>

            {#if current === key}
              <button
                type="button"
                class="g-btn g-btn--ghost mode__action"
                disabled={busy}
                onclick={requestDisable}
              >
                {busy ? t('lab.status.busy') : t('lab.action.disable')}
              </button>
              {#if locked}
                <p class="mode__lock u-note">{t('lab.lock.hint')}</p>
              {/if}
            {:else}
              <button
                type="button"
                class="g-btn g-btn--primary mode__action"
                disabled={busy || !enableable(key)}
                onclick={() => app.setLabMode(key)}
              >
                {busy ? t('lab.status.busy') : t('lab.action.enable')}
              </button>
            {/if}
          </article>
        {/each}
      </div>
    </Panel>

    <Panel title={t('lab.current.lab')} desc={current ? t(`lab.mode.${current}`) : t('lab.status.off')}>
      <div class="kv">
        <span class="kv__key">{t('lab.current.device')}</span>
        <span class="kv__value">
          {deviceMode}
          <span class="u-mono u-muted"> {app.modeInfo.id || '—'}</span>
        </span>
      </div>
      <p class="u-note u-mt-3">{t('lab.current.hint')}</p>
    </Panel>
  {/if}
</div>

<ConfirmSheet
  open={forceAsk}
  danger
  busy={busy}
  title={t('lab.force.title')}
  message={t('lab.force.risk')}
  notes={[t('lab.force.note1'), t('lab.force.note2')]}
  confirmText={t('lab.force.confirm')}
  cancelText={t('action.cancel')}
  onconfirm={confirmForce}
  ondismiss={() => (forceAsk = false)}
/>

<style>
  .bar {
    align-items: center;
    gap: 0.5rem;
  }

  .bar__back {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
  }

  .bar__arrow {
    font-size: 1rem;
    line-height: 1;
  }

  .bar__title {
    margin: 0;
    color: var(--text-primary);
    font-size: 0.9375rem;
    font-weight: 700;
  }

  .modes {
    display: grid;
    gap: var(--space-3);
  }

  /* 单个模式卡：嵌在玻璃面板里，干玻璃（无 backdrop-filter，避免嵌套模糊）
     「启用中」由右下角 g-tag--danger「已启用」文字标签 + 整圈危险色描边共同承担，不靠颜色单独传递状态 */
  .mode {
    display: grid;
    gap: 0.35rem;
    padding: var(--space-3);
    border: var(--hairline) solid var(--glass-border);
    border-radius: var(--radius-m);
    background: var(--glass-bg-weak);
    box-shadow: var(--glass-shadow-inset);
    transition: border-color var(--motion) var(--spring), box-shadow var(--motion) var(--spring);
  }

  /* 启用中：只换描边颜色 + 一圈内发光，不做整块染色（玻璃无色，颜色只来自场景） */
  .mode[data-active='yes'] {
    border-color: color-mix(in srgb, var(--sig-danger) 50%, var(--glass-border));
    box-shadow: var(--glass-shadow-inset), inset 0 0 0 1px color-mix(in srgb, var(--sig-danger) 22%, transparent);
  }

  /* 保留字：整体退后（背景透明化），文字保持原亮度（不叠 opacity，避免低于 --text-muted 对比线） */
  .mode[data-reserved='yes'] {
    background: transparent;
    box-shadow: none;
  }

  .mode__name {
    margin: 0;
    font-size: 0.9375rem;
    font-weight: 600;
    letter-spacing: 0.02em;
  }

  /* 只收字号、保留 .g-tag 自带高度；启用中配色走 .g-tag--danger */
  .mode__tag {
    flex: 0 0 auto;
    font-size: 0.6875rem;
  }

  /* 命中区保持 .g-btn 的 44px（--control-h），不要在这里收 */
  .mode__action {
    justify-self: start;
    margin-top: 0.35rem;
  }

  /* 文字样式来自 .u-note，这里只补色调（警示色而非次要色）与上间距 */
  .mode__lock {
    margin: 0.35rem 0 0;
    color: var(--sig-action);
  }

  @media (min-width: 40rem) {
    .modes {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
</style>
