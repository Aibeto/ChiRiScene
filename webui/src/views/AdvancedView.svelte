<script lang="ts">
  // AdvancedView.svelte: [bar] [fields]
  // 高级设置二级页：DOWN 停摆、PowerBase、息屏判定值。与电池读数页同一套二级页逻辑：
  // 直写状态文件 / meta.yaml（不走草稿，写后回读），与 daemon 热重载对齐。
  import NumberField from '@/components/NumberField.svelte'
  import Panel from '@/components/Panel.svelte'
  import StateBox from '@/components/StateBox.svelte'
  import ToggleField from '@/components/ToggleField.svelte'
  import { onMount } from 'svelte'
  import { t } from '@/i18n/index.svelte'
  import { go } from '@/router.svelte'
  import { app } from '@/state.svelte'

  onMount(() => {
    // DOWN 停摆开关显示的是 down.chr 的实际内容，进页时读一次
    void app.loadDown()
  })
</script>

<div class="u-stack">
  <header class="bar u-row">
    <button type="button" class="ak-button btn btn--ghost bar__back" onclick={() => go('config')}>
      <span class="bar__arrow" aria-hidden="true">←</span>
      {t('config.advanced.back')}
    </button>
    <p class="bar__title">{t('config.advanced')}</p>
  </header>

  {#if !app.metaValid}
    <!-- meta.yaml 有非法项时写入会被拒：说明原因，否则「点了没反应」无从排查 -->
    <StateBox
      kind="error"
      message={t('config.advanced.metaInvalid')}
      detail={app.metaProblems.join(' · ')}
    />
  {/if}

  <!-- 直写调度进程状态文件与 meta.yaml，不走草稿；ak-form-stack 一行一个控件（ak-choice 是 inline-grid，不套栅格会挤成一行） -->
  <Panel signal="action" title={t('config.advanced')} desc={t('config.advanced.hint')}>
    <div class="ak-form-stack">
      <ToggleField
        label={t('config.down')}
        hint={t('config.down.hint')}
        checked={app.downActive}
        disabled={app.downPending}
        onchange={next => app.setDown(next)}
      />
      {#if app.downError}
        <p class="u-note u-danger u-mt-2">{app.downError}</p>
      {/if}
      <!-- PowerBase：以放电功耗替换 CLG 调频（模式名与外部接口不变），直写 meta.yaml 的 powerbase_enabled，热重载即时生效 -->
      <ToggleField
        label={t('config.powerbase')}
        hint={t('config.powerbase.hint')}
        checked={app.powerbaseEnabled}
        onchange={next => app.setPowerbase(next)}
      />
      <!-- 息屏判定值：debug.tracing.screen_state 等于该值视为息屏（默认 1，安装时按实测校正）；失焦/回车提交，直写 screen_off_value -->
      <NumberField
        label={t('config.screenoff')}
        hint={t('config.screenoff.hint')}
        value={app.screenOffValue}
        min={0}
        max={1}
        disabled={app.screenOffPending}
        onchange={next => app.setScreenOffValue(next as 0 | 1)}
      />
      {#if app.screenOffError}
        <p class="u-note u-danger u-mt-2">{app.screenOffError}</p>
      {/if}
    </div>
  </Panel>
</div>

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
    color: var(--ak-text-primary);
    font-size: 0.9375rem;
    font-weight: 700;
  }
</style>
