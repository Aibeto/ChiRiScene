<script lang="ts">
  // AppsView.svelte: [search] [list] [rules]应用性能模式由 rules.yaml 决定（WebUI 无写入口），只按契约事实打标签：特调命中、FAS 白名单、
  // 现存 app_modes
  import { onMount } from "svelte";
  import Panel from "@/components/Panel.svelte";
  import StateBox from "@/components/StateBox.svelte";
  import { t } from "@/i18n/index.svelte";
  import { app } from "@/state.svelte";

  const shown = $derived(app.filteredApps.length);
  const total = $derived(app.apps.length);

  onMount(() => {
    if (app.apps.length === 0) void app.loadApps();
  });
</script>

<div class="u-stack">
  <Panel title={t("apps.title")} desc={t("apps.count", { n: total })}>
    {#snippet actions()}
      <button
        type="button"
        class="g-btn"
        disabled={app.scanning}
        onclick={() => app.loadApps(true)}
      >
        {app.scanning ? t("action.scanning") : t("action.rescan")}
      </button>
    {/snippet}

    <label class="g-field">
      <span class="g-label">{t("apps.search")}</span>
      <input
        class="g-input"
        type="search"
        placeholder={t("apps.search")}
        bind:value={app.appKeyword}
      />
    </label>

    {#if shown !== total}
      <p class="u-note u-mt-2 u-mono">{t("apps.filtered", { shown, total })}</p>
    {/if}
  </Panel>

  {#if app.appsState === "missing" && total === 0}
    <StateBox
      kind="empty"
      message={t("apps.empty")}
      detail={t("apps.empty.detail")}
    />
  {:else if total === 0}
    <StateBox kind="loading" message={t("state.loading")} />
  {:else if shown === 0}
    <StateBox kind="empty" message={t("apps.empty")} />
  {:else}
    <!-- 列表是一整块玻璃（g-glass），行是嵌在内的干玻璃行（g-glass--flat）：玻璃里禁止再叠模糊玻璃 -->
    <ul class="apps g-glass u-list-reset">
      {#each app.filteredApps as entry (entry.pkg)}
        <li class="apps__item g-glass--flat">
          <div class="apps__head">
            <p class="apps__label u-truncate">{entry.label}</p>
            {#if entry.label !== entry.pkg}
              <p class="apps__pkg u-mono u-truncate">{entry.pkg}</p>
            {/if}
          </div>
          <div class="g-tag-group">
            {#if entry.special}
              <span class="g-tag g-tag--accent">
                {t("apps.tag.special")} · {entry.special.fallback}
              </span>
            {/if}
            {#if entry.fasConfig}
              <span class="g-tag">{t("apps.tag.fas")} · {entry.fasConfig}</span>
            {/if}
            {#if entry.appMode}
              <span class="g-tag g-tag--neutral">
                {t("apps.tag.mode")} · {entry.appMode}
              </span>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}

  <Panel title={t("apps.rules")} desc={t("apps.rules.hint")}>
    {#if !app.rules.ok}
      <StateBox
        kind="error"
        message={t("state.failed")}
        detail={app.rules.problem}
      />
    {:else}
      <div class="kv">
        <span class="kv__key">{t("apps.rules.dynamic")}</span>
        <span class="kv__value u-mono"
          >{app.rules.dynamicEnabled ? "true" : "false"}</span
        >
      </div>
      <div class="kv">
        <span class="kv__key">{t("apps.rules.globalMode")}</span>
        <span class="kv__value u-mono">{app.rules.globalMode}</span>
      </div>
      <div class="kv">
        <span class="kv__key">{t("apps.rules.ignored")}</span>
        <span class="kv__value u-mono">
          {app.rules.ignoredApps.length ?
            app.rules.ignoredApps.join("、")
          : "—"}
        </span>
      </div>
      <div class="kv">
        <span class="kv__key">app_modes</span>
        <span class="kv__value u-mono">
          {Object.keys(app.rules.appModes).length || "—"}
        </span>
      </div>
    {/if}
  </Panel>
</div>

<style>
  /* 列表重置来自 .u-list-reset；玻璃底/边/影来自 .g-glass，这里只补圆角与内部排布 */
  .apps {
    display: grid;
    gap: var(--space-2);
    padding: var(--space-3);
    border-radius: var(--radius-l);
  }

  /* 干玻璃行（g-glass--flat 无 backdrop-filter）：补 hairline 边与圆角成「玻璃行」 */
  .apps__item {
    display: grid;
    gap: var(--space-2);
    padding: var(--space-3) var(--space-4);
    border: var(--hairline) solid var(--glass-border);
    border-radius: var(--radius-m);
  }

  .apps__head {
    display: grid;
    gap: 0.1rem;
    min-width: 0;
  }

  .apps__label {
    margin: 0;
    color: var(--text-primary);
    font-size: 0.875rem;
    font-weight: 600;
    line-height: 1.4;
  }

  .apps__pkg {
    margin: 0;
    color: var(--text-muted);
    font-size: 0.6875rem;
    line-height: 1.4;
  }
</style>
