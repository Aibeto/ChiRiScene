<script lang="ts">
  // AppsView.svelte: [rules] [search] 应用规则查询：只读三份调度规则（rules.yaml + 特调 + FAS 白名单）
  // 按包名分组：正标题应用名、副标题包名；activity 级覆盖不缩进，模式用独立 chip 与 activity 名区分
  // 全局规则（动态/全局模式/Activity 开关/忽略包名）已并入状态页「生效配置」，本页不再重复展示
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
    <div class="rules__query">
      <label class="g-field rules__search">
        <span class="g-label">{t("apps.rules.search")}</span>
        <input
          class="g-input"
          type="search"
          placeholder={t("apps.search")}
          bind:value={app.ruleKeyword}
        />
      </label>
      <button
        type="button"
        class="g-btn rules__refresh"
        disabled={app.rulesLoading}
        onclick={() => void app.loadRules(true)}
      >
        {app.rulesLoading ? t("state.loading") : t("apps.rules.refresh")}
      </button>
    </div>

    <p class="u-note u-mt-2 u-mono">
      {t("apps.rules.count", { n: total })}{#if shown !== total} · {t("apps.filtered", { shown, total })}{/if}
    </p>
  </Panel>

  {#if app.rulesWhitelistError}
    <p class="u-note u-danger">{app.rulesWhitelistError}</p>
  {/if}

  {#if app.rules.ok}
    {#if shown === 0}
      <StateBox kind="empty" message={t("apps.rules.empty")} />
    {:else}
      <!-- 列表是一整块亚克力（g-acrylic），行是嵌在内的实心面（g-surface）：亚克力里禁止再叠模糊层 -->
      <ul class="rules g-acrylic u-list-reset">
        {#each groups as group (group.pkg)}
          {@const label = app.ruleLabels[group.pkg] ?? ""}
          <li class="rules__item g-surface">
            <div class="rules__head">
              <div class="rules__id">
                <p class="rules__name" title={group.pkg}>
                  {label || group.pkg}
                </p>
                {#if label && label !== group.pkg}
                  <p class="rules__pkg u-mono">{group.pkg}</p>
                {/if}
              </div>
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
                <span class="rules__mode rules__mode--chip u-mono">{group.mode}</span>
              </div>
            {/if}

            {#each group.activities as activity (activity.activity)}
              <div class="rules__line rules__line--activity">
                <span class="rules__activity u-mono" title={activity.activity}>
                  {activity.activity}
                </span>
                <span class="rules__mode rules__mode--chip u-mono">{activity.mode}</span>
              </div>
            {/each}
          </li>
        {/each}
      </ul>
    {/if}
  {:else if app.rulesLoading}
    <StateBox kind="loading" message={t("state.loading")} />
  {:else if app.rulesState === "missing"}
    <StateBox
      kind="missing"
      message={t("apps.rules.missing")}
      detail={t("apps.rules.missing.detail")}
    />
  {:else if app.rules.problem}
    <StateBox
      kind="error"
      message={t("state.failed")}
      detail={app.rules.problem}
    />
  {:else}
    <StateBox kind="loading" message={t("state.loading")} />
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

  .rules__id {
    display: grid;
    gap: 0.1rem;
    min-width: 0;
  }

  /* 正标题：应用名（拿不到时回落包名）；可换行，不截断 */
  .rules__name {
    margin: 0;
    color: var(--text-primary);
    font-size: 0.9375rem;
    font-weight: 600;
    line-height: 1.35;
    overflow-wrap: anywhere;
  }

  /* 副标题：包名，弱化一档 */
  .rules__pkg {
    margin: 0;
    color: var(--text-muted);
    font-size: 0.6875rem;
    line-height: 1.4;
    overflow-wrap: anywhere;
  }

  /* 规则行：层级标签 + 模式；activity 行是「activity 名 + 模式 chip」两列，不再缩进 */
  .rules__line {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }

  .rules__line--activity {
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: start;
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
    line-height: 1.45;
    /* activity 名可能很长：换行而不是截断，保证「精确到 activity」看得到全名 */
    overflow-wrap: anywhere;
  }

  /* 模式值：等宽小 chip，和 activity 名（正文等宽）在字号/字重/底色上拉开层次 */
  .rules__mode--chip {
    margin: 0;
    padding: 0.1rem 0.4rem;
    border: var(--hairline) solid var(--surface-border);
    border-radius: var(--radius-s);
    background: var(--surface-sunken);
    color: var(--text-primary);
    font-size: 0.6875rem;
    font-weight: 700;
    letter-spacing: 0.04em;
    line-height: 1.4;
    white-space: nowrap;
  }

  /* 搜索行：输入框占满，刷新按钮贴右；窄屏按钮不换行到下一行 */
  .rules__query {
    display: flex;
    align-items: flex-end;
    gap: var(--space-2);
  }

  .rules__search {
    flex: 1 1 auto;
    min-width: 0;
  }

  .rules__refresh {
    flex: 0 0 auto;
    min-height: var(--control-h);
  }
</style>
