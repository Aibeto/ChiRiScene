<script lang="ts">
  // AppsView.svelte: [rules] [search] 应用规则查询：只读三份调度规则（rules.yaml + 特调 + FAS 白名单）
  // 按包名分组展示，activity 级覆盖缩进在包名级下面；不再执行 pm list packages 拉取完整应用列表（规则查询不需要全量清单，扫描也很慢）
  import { onMount } from "svelte";
  import Panel from "@/components/Panel.svelte";
  import StateBox from "@/components/StateBox.svelte";
  import { t } from "@/i18n/index.svelte";
  import { app } from "@/state.svelte";

  const groups = $derived(app.filteredRules);
  const total = $derived(app.ruleGroups.length);
  const shown = $derived(groups.length);

  onMount(() => {
    // 每次进入重读三份规则（三次小文件读取，不碰应用列表）；规则是 daemon 启动时写的展示副本
    void app.loadRules();
  });
</script>

<div class="u-stack">
  <Panel title={t("apps.title")} desc={t("apps.rules.priority")}>
    {#snippet actions()}
      <button
        type="button"
        class="g-btn"
        disabled={app.rulesLoading}
        onclick={() => void app.loadRules(true)}
      >
        {app.rulesLoading ? t("state.loading") : t("apps.rules.refresh")}
      </button>
    {/snippet}

    <label class="g-field">
      <span class="g-label">{t("apps.rules.search")}</span>
      <input
        class="g-input"
        type="search"
        placeholder={t("apps.rules.search")}
        bind:value={app.ruleKeyword}
      />
    </label>

    <p class="u-note u-mt-2 u-mono">
      {t("apps.rules.count", { n: total })}{#if shown !== total} · {t("apps.filtered", { shown, total })}{/if}
    </p>
  </Panel>

  <Panel title={t("apps.rules")} desc={t("apps.rules.hint")}>
    {#if app.rules.ok}
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
        <span class="kv__key">{t("apps.rules.activityAware")}</span>
        <span class="kv__value u-mono"
          >{app.rules.activityAware ? "true" : "false"}</span
        >
      </div>
      {#if !app.rules.activityAware}
        <p class="u-note u-mt-2">{t("apps.rules.activityAware.off")}</p>
      {/if}
      <div class="kv">
        <span class="kv__key">{t("apps.rules.ignored")}</span>
        <span class="kv__value">
          {#if app.rules.ignoredApps.length > 0}
            <span class="rules__ignored">
              {#each app.rules.ignoredApps as name, index (index)}
                <span class="g-tag g-tag--neutral u-mono">{name}</span>
              {/each}
            </span>
          {:else}
            <span class="u-mono">—</span>
          {/if}
        </span>
      </div>
    {:else if app.rulesLoading}
      <StateBox kind="loading" message={t("state.loading")} />
    {:else if app.rulesState === "missing"}
      <StateBox
        kind="missing"
        message={t("apps.rules.missing")}
        detail={t("apps.rules.missing.detail")}
      />
    {:else}
      <StateBox
        kind="error"
        message={t("state.failed")}
        detail={app.rules.problem}
      />
    {/if}

    {#if app.rulesWhitelistError}
      <p class="u-note u-danger u-mt-2">{app.rulesWhitelistError}</p>
    {/if}
  </Panel>

  {#if app.rules.ok}
    {#if shown === 0}
      <StateBox kind="empty" message={t("apps.rules.empty")} />
    {:else}
      <!-- 列表是一整块亚克力（g-acrylic），行是嵌在内的实心面（g-surface）：亚克力里禁止再叠模糊层 -->
      <ul class="rules g-acrylic u-list-reset">
        {#each groups as group (group.pkg)}
          <li class="rules__item g-surface">
            <div class="rules__head">
              <p class="rules__pkg u-mono u-truncate" title={group.pkg}>
                {group.pkg}
              </p>
              {#if group.ignored || group.special || group.fasConfig}
                <div class="g-tag-group">
                  {#if group.ignored}
                    <span
                      class="g-tag g-tag--neutral"
                      title={t("apps.rules.ignored.hint")}
                    >
                      {t("apps.rules.ignoredTag")}
                    </span>
                  {/if}
                  {#if group.special}
                    <span
                      class="g-tag g-tag--accent"
                      title={t("apps.rules.special.hint", {
                        modes: group.special.modes.join(" / "),
                        fallback: group.special.fallback,
                      })}
                    >
                      {t("apps.rules.special")} · {group.special.fallback}
                    </span>
                  {/if}
                  {#if group.fasConfig}
                    <span class="g-tag">
                      {t("apps.rules.fas")} · {group.fasConfig}
                    </span>
                  {/if}
                </div>
              {/if}
            </div>

            {#if group.mode}
              <div class="rules__line">
                <span class="rules__level">{t("apps.rules.level.package")}</span>
                <span class="rules__mode u-mono">{group.mode}</span>
              </div>
            {/if}

            {#each group.activities as activity (activity.activity)}
              <div class="rules__line rules__line--activity">
                <span class="rules__level"
                  >{t("apps.rules.level.activity")}</span
                >
                <span class="rules__activity u-mono" title={activity.activity}>
                  {activity.activity}
                </span>
                <span class="rules__mode u-mono">{activity.mode}</span>
              </div>
            {/each}
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</div>

<style>
  /* 列表重置来自 .u-list-reset；亚克力底/边/影来自 .g-acrylic，这里只补圆角与内部排布 */
  .rules {
    display: grid;
    gap: var(--space-2);
    padding: var(--space-3);
    border-radius: var(--radius-l);
  }

  /* 行是实心面（g-surface，无 backdrop-filter）：补细边与圆角成「列表行」 */
  .rules__item {
    display: grid;
    gap: var(--space-2);
    padding: var(--space-3) var(--space-4);
    border: var(--hairline) solid var(--surface-border);
    border-radius: var(--radius-m);
  }

  .rules__head {
    display: grid;
    gap: var(--space-2);
    min-width: 0;
  }

  .rules__pkg {
    min-width: 0;
    margin: 0;
    color: var(--text-primary);
    font-size: 0.875rem;
    font-weight: 600;
    line-height: 1.4;
  }

  /* 规则行：层级标签 + 值；activity 行多一列 activity 名（缩进表达层级，不用单侧粗边框信号条） */
  .rules__line {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }

  .rules__line--activity {
    grid-template-columns: auto minmax(0, 1fr) auto;
    padding-left: var(--space-4);
  }

  .rules__level {
    color: var(--text-muted);
    font-size: 0.625rem;
    font-weight: 700;
    letter-spacing: 0.06em;
    white-space: nowrap;
  }

  .rules__activity {
    min-width: 0;
    margin: 0;
    color: var(--text-secondary);
    font-size: 0.75rem;
    line-height: 1.4;
    /* activity 名可能很长：换行而不是截断，保证「精确到 activity」看得到全名 */
    overflow-wrap: anywhere;
  }

  .rules__mode {
    margin: 0;
    color: var(--text-primary);
    font-size: 0.75rem;
    font-weight: 600;
    white-space: nowrap;
  }

  /* ignored_apps 是模糊条目，可能有多条：标签流式换行 */
  .rules__ignored {
    display: inline-flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
</style>
