<script lang="ts">
  // LiveStatus.svelte: [status]守护进程存活芯片（.g-status 玻璃胶囊 + 脉冲信号点）；三态用文字 + 数据属性双重表达（不只靠颜色）
  let {
    state,
    label,
    detail = ''
  } = $props<{
    state: 'running' | 'stopped' | 'unknown'
    label: string
    detail?: string
  }>()
</script>

<!-- 视觉来自 app.css [primitives] 的 .g-status；三态用修饰类 + 文字双重表达 -->
<div
  class="g-status"
  class:g-status--running={state === 'running'}
  data-signal={state === 'running' ? 'success' : state === 'stopped' ? 'danger' : 'action'}
  role="status"
>
  <span class="g-status__signal"></span>
  <span class="g-status__label">{label}</span>
  {#if detail}
    <span class="g-status__detail">{detail}</span>
  {/if}
</div>
