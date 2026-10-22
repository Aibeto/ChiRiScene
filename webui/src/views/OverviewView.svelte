<script lang="ts">
  // OverviewView.svelte: [status] [mode] [info] [stop]
  import { onMount } from "svelte";
  import Panel from "@/components/Panel.svelte";
  import LiveStatus from "@/components/LiveStatus.svelte";
  import ConfirmSheet from "@/components/ConfirmSheet.svelte";
  import StateBox from "@/components/StateBox.svelte";
  import { t } from "@/i18n/index.svelte";
  import { clgFamilyKey } from "@/data/mode";
  import { app } from "@/state.svelte";

  let confirmOpen = $state(false);

  // 存活判据是心跳文件（LiveTime.chr）新鲜度；状态芯片从顶栏移到这里，取代原「调度主服务正常」文字行
  // 读取/内容异常（daemonError）不塞进芯片（错误文本可能很长），单独用说明行展示，比通用 detail 更有排查价值
  const daemonLabel = $derived(t(`daemon.${app.daemonState}`));
  const daemonError = $derived(app.daemonError || "");
  // 调度未运行时不把 current_mode.chr 的陈旧值当「当前模式」，卡片只陈述「调度未运行」；文件缺失同口径显示「尚未产生模式记录」
  const modeIdle = $derived(app.daemonState === "stopped");
  const modeName = $derived(
    modeIdle ? t("mode.unknown.stopped")
    : app.modeMissing ? t("mode.unknown.missing")
    : t(app.modeInfo.labelKey),
  );
  // 模式家族（CLG/特调/实验室/停摆/FAS），与详细模式分开显示；未运行/无记录时不显示
  // 本机启用相对能效模型（daemon 写的 pf.chr）时 CLG 家族名显示 CLG-EEM，与 daemon 日志前缀同口径
  const familyLabel = $derived(
    modeIdle || app.modeMissing ? ""
    : app.modeInfo.kind === "clg" ? t(clgFamilyKey(app.clgPfEnabled))
    : t(`mode.family.${app.modeInfo.kind}`),
  );
  const modeId = $derived(
    modeIdle || app.modeMissing ? "—" : app.modeInfo.id || "—",
  );
  // 三层信息（家族 / 模式名 / 原始 id）逐级去重，同一个词只说一次
  // 家族行只在 clg/lab/stardust 这类名字看不出家族的成员上显示；id 行只在它与模式名不是同一个词时显示
  const FAMILY_KINDS = new Set(["clg", "lab", "stardust"]);
  const showFamily = $derived(
    familyLabel !== "" && FAMILY_KINDS.has(app.modeInfo.kind),
  );
  const nameKey = $derived(modeName.trim().toLowerCase());
  const showModeId = $derived(
    !modeIdle &&
      !app.modeMissing &&
      modeId !== "—" &&
      modeId.trim().toLowerCase() !== nameKey,
  );
  const modeSignal = $derived(modeIdle ? "info" : app.modeInfo.signal);
  const deviceLabel = $derived(
    app.deviceKind === "chiri" ?
      t("overview.device.chiri")
    : app.deviceKind === "unsupported" ?
      t("overview.device.unsupported")
    : t("overview.device.unknown"),
  );

  // 仪表盘进度：功耗 / 满量程（meta.power_max_w，默认 12W），夹在 0~100%
  const powerPercent = $derived.by(() => {
    const w = app.powerAvgWatt;
    if (w === null) return 0;
    return Math.min(100, Math.max(0, (w / app.powerMaxWatt) * 100));
  });

  const nowPercent = $derived.by(() => {
    const w = app.powerNowWatt;
    if (w === null) return 0;
    return Math.min(100, Math.max(0, (w / app.powerMaxWatt) * 100));
  });

  const configValue = $derived.by(() => {
    if (app.configState === "ok") return app.configRel;
    if (app.configState === "missing") return t("logs.missing");
    return t("state.failed");
  });

  onMount(() => {
    void app.loadOverview();
    // [poll] 每秒自刷新（loadOverview 有在飞共享，轮询不堆积）；跳过 hidden 期 tick 省电，恢复可见时立即补一次，避免后台数据空窗
    const timer = setInterval(() => {
      if (document.hidden) return;
      void app.loadOverview();
    }, 1000);
    const onVisible = () => {
      if (document.visibilityState === "visible") void app.loadOverview();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  });

  async function confirmStop() {
    await app.stopDaemon();
    confirmOpen = false;
  }

  /** 删除历史归档的二次确认 */
  let deleteOpen = $state(false);
  async function confirmDelete() {
    await app.deleteArchives();
    deleteOpen = false;
  }
</script>

<div class="u-stack">
  <Panel signal={app.daemonState === "running" ? "success" : "action"}>
    <div class="daemon-card">
      <div class="daemon">
        <p class="daemon__name u-mono">
          {app.moduleProp.name || t("app.title")}
        </p>
        <p class="u-note u-mono">
          {app.moduleProp.version || "—"}{app.moduleProp.versionCode ?
            ` (${app.moduleProp.versionCode})`
          : ""}
        </p>
        {#if app.moduleProp.author}
          <p class="u-note">
            {t("overview.module.author")}：{app.moduleProp.author}
          </p>
        {/if}
        <div class="daemon__status">
          <LiveStatus state={app.daemonState} label={daemonLabel} />
        </div>
        {#if daemonError}
          <p class="u-note u-danger">{daemonError}</p>
        {/if}
      </div>
      {#if app.isChiri}
        <!-- 耗电情况：PowerAVG.chr（daemon 每 1s 采样写入），口径随 meta.power_avg；量程 = meta.power_max_w（默认 12W），
             进度条直接复用 g-progress -->
        <div class="power">
          <p class="u-note">
            {app.powerAvgUsesAverage ?
              t("overview.power.avg")
            : t("overview.power.ref")}
          </p>
          <p class="power__value u-mono">
            {app.powerAvgWatt === null ?
              "—"
            : `${app.powerAvgWatt.toFixed(2)} ${t("unit.watt")}`}
          </p>
          <!-- g-progress 结构：--progress-signal 与 --progress-width 的默认值都声明在 .g-progress 根上，
               fill 的宽度与颜色全部由变量驱动；壳层由 .power__bar 剥掉 -->
          <div
            class="g-progress power__bar"
            role="progressbar"
            aria-label={app.powerAvgUsesAverage ?
              t("overview.power.avg")
            : t("overview.power.ref")}
            aria-valuemin="0"
            aria-valuemax="100"
            aria-valuenow={Math.round(powerPercent)}
            style={`--progress-width: ${powerPercent.toFixed(1)}%`}
          >
            <div class="g-progress__track">
              <span class="g-progress__fill"></span>
            </div>
          </div>
          <!-- 数字与条并列：条看着空时能区分「读数为 0/缺失」与「条没画出来」 -->
          <span class="g-progress__value u-mono"
            >{Math.round(powerPercent)}%</span
          >
        </div>
      {/if}
    </div>
    {#if app.powerAvgMissing || app.powerStaleReason}
      <!-- 文件缺失是「调度没跑过」的正常形态，不是错误 → 次要色说明；有具体原因（未取样：非放电 / 平均模式息屏 / 读数不可用）时优先显示它 -->
      <p class="u-note u-mt-2">
        {app.powerStaleReason || t("overview.power.missing")}
      </p>
    {/if}
  </Panel>

  {#if app.loading && !app.ready}
    <StateBox kind="loading" message={t("state.loading")} />
  {:else if app.configState === "failed"}
    <StateBox
      kind="error"
      message={t("state.failed")}
      detail={app.configError}
    />
  {/if}

  <Panel
    title={t("overview.mode")}
    // desc={t("overview.mode.observed")}
    signal={modeSignal}
  >
    <div class="mode-card">
      <div class="mode" data-signal={modeSignal}>
        <div class="mode__main">
          {#if showFamily}
            <p class="mode__family">{familyLabel}</p>
          {/if}
          <p class="mode__name">{modeName}</p>
          {#if showModeId}
            <p class="mode__id u-mono">{modeId}</p>
          {/if}
        </div>
        {#if app.modeError}
          <StateBox
            kind="error"
            message={t("state.failed")}
            detail={app.modeError}
          />
        {/if}
        {#if app.metaProblems.length > 0}
          <ul class="mode__problems">
            {#each app.metaProblems as problem, index (index)}
              <li>{problem}</li>
            {/each}
          </ul>
        {/if}
      </div>
      {#if app.isChiri}
        <!-- 当前功耗：status.csv 最后一行的 batt_power_w，结构/样式与首卡的平均/参考放电功耗块完全一致（同一批 .power 类） -->
        <div class="power">
          <p class="u-note">{t("overview.power.now")}</p>
          <p class="power__value u-mono">
            {app.powerNowWatt === null ?
              "—"
            : `${app.powerNowWatt.toFixed(2)} ${t("unit.watt")}`}
          </p>
          <div
            class="g-progress power__bar"
            role="progressbar"
            aria-label={t("overview.power.now")}
            aria-valuemin="0"
            aria-valuemax="100"
            aria-valuenow={Math.round(nowPercent)}
            style={`--progress-width: ${nowPercent.toFixed(1)}%`}
          >
            <div class="g-progress__track">
              <span class="g-progress__fill"></span>
            </div>
          </div>
          <span class="g-progress__value u-mono"
            >{Math.round(nowPercent)}%</span
          >
        </div>
      {/if}
    </div>
  </Panel>

  <Panel title={t("overview.config")}>
    <div class="kv">
      <span class="kv__key">{t("overview.config")}</span>
      <span class="kv__value u-mono">{configValue}</span>
    </div>
    <div class="kv">
      <span class="kv__key">{t("overview.device")}</span>
      <span class="kv__value">{deviceLabel}</span>
    </div>
    {#if app.deviceKind === "chiri"}
      <div class="kv">
        <span class="kv__key">{t("apps.tag.special")}</span>
        <span class="kv__value u-mono">{app.specialCount}</span>
      </div>
      <div class="kv">
        <span class="kv__key">{t("apps.tag.fas")}</span>
        <span class="kv__value u-mono">{app.fasCount}</span>
      </div>
    {:else}
      <div class="kv">
        <span class="kv__key"
          >{t("apps.tag.special")} / {t("apps.tag.fas")}</span
        >
        <span class="kv__value u-muted">
          {t("overview.device.unknown")}
        </span>
      </div>
    {/if}
    {#if app.whitelistError}
      <StateBox
        kind="error"
        message={t("state.failed")}
        detail={app.whitelistError}
      />
    {/if}
    {#if app.metaPath}
      <div class="kv">
        <span class="kv__key">meta.yaml</span>
        <span class="kv__value u-mono">{app.metaPath}</span>
      </div>
    {/if}
  </Panel>

  <Panel title={t("overview.export.title")} desc={t("overview.export.desc")}>
    <ul class="export__notice u-notice">
      <li>{t("overview.export.notice.session")}</li>
      <li>{t("overview.export.notice.busy")}</li>
    </ul>
    {#if app.exportPhase === "done"}
      <p class="export__msg export__done u-note u-mt-3">
        {t("overview.export.saved", { path: app.exportTarget })}
      </p>
    {/if}
    {#if app.exportError}
      <p class="export__msg u-note u-mt-3 u-danger">{app.exportError}</p>
    {/if}
    <button
      type="button"
      class="g-btn g-btn--primary g-btn--block"
      disabled={app.exportPhase === "running"}
      onclick={() => void app.startExport()}
    >
      {app.exportPhase === "running" ?
        t("overview.export.running")
      : t("overview.export.action")}
    </button>
    {#if app.exportPhase === "running"}
      <!-- 进度区整块用 g-progress（标题行 + 刻度轨道）；填充宽度由下方 .g-progress__fill 的内联 width 决定 -->
      <div class="g-progress u-mt-3">
        <div class="g-progress__header">
          <span>
            {app.exportPlanned > 0 ?
              t("overview.export.progress", {
                mb: app.exportReadMb,
                total: app.exportPlannedMb,
              })
            : t("overview.export.preparing")}
          </span>
          <span class="g-progress__value">{app.exportPercent}%</span>
        </div>
        <div
          class="g-progress__track"
          role="progressbar"
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow={app.exportPercent}
        >
          <!-- 同功耗条：宽度直接内联，不依赖上游变量继承 -->
          <span class="g-progress__fill" style={`width: ${app.exportPercent}%`}></span>
        </div>
      </div>
    {/if}

    <!-- 历史归档清理：导出之外的另一半，只删 logd/（devimp/ 是当前诊断现场，保留）；打包期间一并置灰（导出正在 tar logd/，此时 rm 会把半删的目录打进归档） -->
    <button
      type="button"
      class="g-btn g-btn--danger g-btn--block u-mt-3"
      disabled={app.archivePending || app.exportPhase === "running"}
      onclick={() => (deleteOpen = true)}
    >
      {app.archivePending ? t("state.loading") : t("overview.delete.action")}
    </button>
    {#if app.archiveError}
      <p class="u-note u-danger u-mt-2">{app.archiveError}</p>
    {/if}
  </Panel>

  <Panel
    title={t("overview.stop")}
    // desc={t("overview.stop.desc")}
    signal="danger"
  >
    <button
      type="button"
      class="g-btn g-btn--danger g-btn--block"
      disabled={app.stopping || app.daemonState !== "running"}
      onclick={() => (confirmOpen = true)}
    >
      {app.stopping ? t("state.loading") : t("overview.stop")}
    </button>
    <p class="u-note u-mt-3">
      {app.actionAvailable ?
        t("overview.stop.confirm.recover")
      : t("overview.stop.recover.reboot")}
    </p>
  </Panel>
</div>

<ConfirmSheet
  open={confirmOpen}
  danger
  busy={app.stopping}
  title={t("overview.stop.confirm.title")}
  message={t("overview.stop.confirm.message")}
  notes={[
    t("overview.stop.confirm.effects"),
    t("overview.stop.confirm.recover"),
  ]}
  confirmText={t("overview.stop")}
  cancelText={t("action.cancel")}
  onconfirm={confirmStop}
  ondismiss={() => (confirmOpen = false)}
/>

<ConfirmSheet
  open={deleteOpen}
  danger
  busy={app.archivePending}
  title={t("overview.delete.confirm.title")}
  message={t("overview.delete.confirm.message")}
  confirmText={t("overview.delete.action")}
  cancelText={t("action.cancel")}
  onconfirm={confirmDelete}
  ondismiss={() => (deleteOpen = false)}
/>

<style>
  /* 第一张卡片：左侧设备/守护进程信息，右侧耗电读数板（平行两栏） */
  .daemon-card {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: var(--space-4);
    align-items: center;
  }

  .daemon {
    display: grid;
    gap: 0.15rem;
    min-width: 0;
  }

  /* 状态芯片按内容宽度贴左，不在单列栅格里被拉满整行 */
  .daemon__status {
    justify-self: start;
    margin-top: 0.25rem;
  }

  /* 耗电读数：口径标签 + 数值 + 量程条，右栏紧凑排列 */
  .power {
    display: grid;
    justify-items: end;
    align-content: center;
    gap: 0.3rem;
    min-width: 8.5rem;
  }

  .power__value {
    margin: 0;
    font-size: 1.125rem;
    font-weight: 700;
    line-height: 1.1;
  }

  .power__bar {
    width: 100%;
    /* .g-progress 根自带壳层（内距/边框/深底）——标签与数值在外层栅格，条只取它的轨道/填充机制，壳层剥掉保持原布局 */
    padding: 0;
    border: 0;
    background: none;
  }

  /* 模式卡：左侧模式信息，右侧当前功耗读数板（复用首卡 .power 同款类） */
  .mode-card {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: var(--space-4);
    align-items: center;
  }

  .daemon__name {
    margin: 0;
    font-size: 1.125rem;
    font-weight: 700;
    letter-spacing: 0.02em;
  }

  .mode {
    display: grid;
    gap: var(--space-2);
  }

  /* 家族名：小字上标（与 .mode__id 同档）；无规则时 <p> 的默认外边距会顶开 .mode__main 的 0.2rem 行距 */
  .mode__family {
    margin: 0;
    color: var(--text-muted);
    font-size: 0.6875rem;
    font-weight: 700;
    letter-spacing: 0.08em;
  }

  .mode__main {
    display: grid;
    gap: 0.2rem;
  }

  /* 信号色由全局 [data-signal] 映射提供（与 Panel 同一份） */
  .mode__name {
    margin: 0;
    color: var(--signal, var(--text-primary));
    font-size: 1.5rem;
    font-weight: 700;
    letter-spacing: 0.01em;
    line-height: 1.2;
  }

  .mode__id {
    margin: 0;
    color: var(--text-muted);
    font-size: 0.6875rem;
    letter-spacing: 0.08em;
  }

  .mode__problems {
    margin: 0;
    padding-left: var(--space-4);
    color: var(--sig-danger);
    font-size: 0.75rem;
    line-height: 1.6;
  }

  /* 提示清单主体来自全局 .u-notice，这里只补与下方按钮的间距 */
  .export__notice {
    margin-bottom: var(--space-3);
  }

  /* 产物路径可能很长：允许断行，避免把卡片撑宽（文字样式来自 .u-note） */
  .export__msg {
    word-break: break-all;
  }

  .export__done {
    color: var(--sig-success);
  }
</style>
