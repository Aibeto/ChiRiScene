<script lang="ts">
  // StateBox.svelte: [states] 空/缺失/失败/加载四种占位：状态用文字 + 信号条表达，不依赖颜色单独区分
  import type { Snippet } from 'svelte'

  let {
    kind = 'empty',
    message = '',
    detail = '',
    action
  } = $props<{
    kind?: 'empty' | 'missing' | 'error' | 'loading'
    message?: string
    detail?: string
    action?: Snippet
  }>()
</script>

<!-- 底座来自 app.css [primitives] 的 .g-notice（实心面 + 信号点）；本组件只管结构与加载脉冲 -->
<div class="state g-notice" data-kind={kind} role={kind === 'error' ? 'alert' : undefined}>
  <span class="g-notice__signal" aria-hidden="true"></span>
  <div class="g-notice__body">
    <p class="g-notice__title">{message}</p>
    {#if detail}
      <p class="g-notice__message u-mono">{detail}</p>
    {/if}
  </div>
  {#if action}
    <div class="state__action">{@render action()}</div>
  {/if}
</div>

<style>
  /* 骨架/信号色来自 .g-notice；这里只保留加载脉冲与结构修正
     脉冲打在信号点上（.g-notice__signal），状态色由 data-kind 决定 */
  .state[data-kind='loading'] .g-notice__signal {
    animation: state-pulse 1.4s ease-in-out infinite;
  }

  .state__action {
    flex: 0 0 auto;
  }

  @keyframes state-pulse {
    0%,
    100% {
      opacity: 0.5;
    }

    50% {
      opacity: 1;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .state[data-kind='loading'] .g-notice__signal {
      animation: none;
    }
  }
</style>
