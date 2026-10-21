<script lang="ts">
  // Panel.svelte: [layout] [signal] 面板容器：玻璃卡片（.g-card，三层玻璃结构 [glass]/[primitives]）
  import type { Snippet } from 'svelte'

  let {
    title = '',
    desc = '',
    signal = '',
    actions,
    children
  } = $props<{
    title?: string
    desc?: string
    /** 语义信号：info / action / danger / success / accent（映射 --signal） */
    signal?: string
    actions?: Snippet
    children?: Snippet
  }>()
</script>

<section class="panel g-glass g-card" data-signal={signal || undefined}>
  <!-- 无点击元素不铺扫光闪光（.g-sheen 已从 .g-card 宿主选择器移除） -->
  {#if title || actions}
    <header class="g-card__header">
      <div class="g-card__titles">
        <h2 class="g-card__title">{title}</h2>
        {#if desc}
          <p class="g-card__desc">{desc}</p>
        {/if}
      </div>
      {#if actions}
        <div class="g-card__actions">{@render actions()}</div>
      {/if}
    </header>
  {/if}
  <div class="panel__body">
    {@render children?.()}
  </div>
</section>

<style>
  /* 玻璃结构/信号条/悬浮全部来自 app.css 的 .g-glass 与 .g-card；这里只保留无标题时的首元素贴合 */
  .panel {
    display: block;
  }

  .panel__body {
    min-width: 0;
  }

  /* 无标题/操作的面板：内容区没有 .g-card__header 的间距可借 */
  .panel__body:first-child {
    margin-top: 0;
  }
</style>
