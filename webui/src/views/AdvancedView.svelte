<script lang="ts">
  // AdvancedView.svelte: [bar] [fields] [notify-test] 高级设置二级页：DOWN 停摆、息屏判定值，与电池读数页同一套二级页逻辑
  // 直写状态文件 / meta.yaml（不走草稿，写后回读），与 daemon 热重载对齐；末组是通知测试（经 module 根目录 notify.sh 投递，tag 固定 ChiRi-Test）
  import NumberField from "@/components/NumberField.svelte";
  import Panel from "@/components/Panel.svelte";
  import StateBox from "@/components/StateBox.svelte";
  import TextField from "@/components/TextField.svelte";
  import ToggleField from "@/components/ToggleField.svelte";
  import { onMount } from "svelte";
  import { t } from "@/i18n/index.svelte";
  import { go } from "@/router.svelte";
  import { app } from "@/state.svelte";
  import { TEST_TAG } from "@/contract/notify";

  // 非 ChiRi 机型：daemon 不启动调度、恒为停摆（只写 current_mode.chr 投影，down.chr 在本机不生效），开关置灰并固定显示为已停摆
  // 只认「读到生效配置且不含 /」的确定态 unsupported：unknown（尚未加载 / 读失败）不能算非 ChiRi，否则 ChiRi 机型会被误锁
  const downFixed = $derived(app.deviceKind === "unsupported");

  onMount(() => {
    // DOWN 停摆开关显示的是 down.chr 的实际内容，进页时读一次
    void app.loadDown();
  });
</script>

<div class="u-stack">
  <header class="bar u-row">
    <button
      type="button"
      class="g-btn g-btn--ghost bar__back"
      onclick={() => go("config")}
    >
      <span class="bar__arrow" aria-hidden="true">←</span>
      {t("config.advanced.back")}
    </button>
    <p class="bar__title">{t("config.advanced")}</p>
  </header>

  {#if !app.metaValid}
    <!-- meta.yaml 有非法项时写入会被拒：说明原因，否则「点了没反应」无从排查 -->
    <StateBox
      kind="error"
      message={t("config.advanced.metaInvalid")}
      detail={app.metaProblems.join(" · ")}
    />
  {/if}

  <!-- 直写调度进程状态文件与 meta.yaml，不走草稿；g-form-stack 一行一个控件（g-switch 是 inline 布局，不套栅格会挤成一行） -->
  <Panel
    signal="action"
    title={t("config.advanced")}
    desc={t("config.advanced.hint")}
  >
    <div class="g-form-stack">
      <ToggleField
        label={t("config.down")}
        hint={downFixed ?
          t("config.down.hint.unsupported")
        : t("config.down.hint")}
        checked={downFixed || app.downActive}
        disabled={downFixed || app.downPending}
        onchange={(next) => app.setDown(next)}
      />
      {#if app.downError}
        <p class="u-note u-danger u-mt-2">{app.downError}</p>
      {/if}
      <!-- 息屏判定值：debug.tracing.screen_state 等于该值视为息屏（默认 1，安装时校正）；失焦/回车提交，直写 screen_off_value -->
      <NumberField
        label={t("config.screenoff")}
        hint={t("config.screenoff.hint")}
        value={app.screenOffValue}
        min={0}
        max={1}
        disabled={app.screenOffPending}
        onchange={(next) => app.setScreenOffValue(next as 0 | 1)}
      />
      {#if app.screenOffError}
        <p class="u-note u-danger u-mt-2">{app.screenOffError}</p>
      {/if}
    </div>
  </Panel>

  <!-- 通知测试：daemon 的启动/FAS 通知走同一条 notify.sh，这里用固定 tag ChiRi-Test 复现
       tag 固定不可改（同 tag 互相覆盖，乱填会顶掉启动通知）；结果区显示脚本原始输出，只报「已发送」会把真失败藏起来 -->
  <Panel signal="action" title={t("config.notifyTest")} desc={t("config.notifyTest.hint")}>
    <div class="g-form-stack">
      <TextField
        label={t("config.notifyTest.title")}
        value={app.notifyTitle}
        maxlength={64}
        disabled={app.notifyPending}
        oninput={(next) => (app.notifyTitle = next)}
      />
      <TextField
        label={t("config.notifyTest.body")}
        value={app.notifyBody}
        maxlength={200}
        disabled={app.notifyPending}
        oninput={(next) => (app.notifyBody = next)}
      />
      <!-- tag 是只读展示：AOSP 里它才是那个「同值互相覆盖」的键，用户需要知道这次用的是哪个 -->
      <p class="u-note u-mono notify__tag">
        {t("config.notifyTest.tag")} · <span class="u-note">{t("config.notifyTest.tagHint")}</span>
        <span class="notify__tagval">{TEST_TAG}</span>
      </p>
      <button
        type="button"
        class="g-btn g-btn--primary g-btn--block"
        disabled={app.notifyPending}
        onclick={() => app.sendNotifyTest()}
      >
        {app.notifyPending ? t("config.notifyTest.sending") : t("config.notifyTest.send")}
      </button>
      {#if app.notifyState !== ''}
        <div class="notify__result" data-state={app.notifyState}>
          <p class="u-note" class:u-danger={app.notifyState === "fail"}>
            {app.notifyState === "fail"
              ? `${t("config.notifyTest.failed")}：${app.notifyError}`
              : `${t("config.notifyTest.sent")}（${t("config.notifyTest.acceptedWarn")}）`}
          </p>
          {#if app.notifyOutput}
            <p class="u-note notify__label">{t("config.notifyTest.output")}</p>
            <pre class="u-mono notify__out">{app.notifyOutput}</pre>
          {/if}
        </div>
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
    color: var(--text-primary);
    font-size: 0.9375rem;
    font-weight: 700;
  }

  .notify__tag {
    margin: 0;
  }

  /* 实际 tag 值加重一点：它是这条通知的覆盖键，与上面的说明文字不是一回事 */
  .notify__tagval {
    color: var(--text-primary);
    font-weight: 700;
  }

  .notify__result {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  .notify__label {
    margin: 0;
    color: var(--text-secondary);
  }

  /* 原始输出可能很长（每条策略一行）：限高可滚，不把卡片撑破；下沉实心面 + 细边，Fluent 的「代码块」观感 */
  .notify__out {
    max-height: 12rem;
    margin: 0;
    padding: 0.5rem 0.6rem;
    overflow: auto;
    border: var(--hairline) solid var(--surface-border);
    border-radius: var(--radius-s);
    background: var(--surface-sunken);
    color: var(--text-secondary);
    font-size: 0.75rem;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-all;
  }
</style>
