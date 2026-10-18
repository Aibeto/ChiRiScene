<script lang="ts">
  // ToggleField.svelte: [switch] 玻璃开关（.g-switch）：原生 checkbox[role=switch]，轨道必须作 input 的兄弟元素（项目红线禁 :has()）
  let {
    label,
    hint = '',
    checked = false,
    disabled = false,
    pending = false,
    onchange
  } = $props<{
    label: string
    hint?: string
    checked?: boolean
    disabled?: boolean
    /** 有未提交的改动（界面标记，不影响表单语义） */
    pending?: boolean
    onchange: (next: boolean) => void
  }>()
</script>

<!-- 轨道/旋钮/选中色/焦点环来自 app.css .g-switch；这里只保留文本结构与「待提交」标记 -->
<label class="g-switch" data-disabled={disabled || undefined}>
  <input
    class="g-switch__input"
    type="checkbox"
    role="switch"
    {checked}
    {disabled}
    onchange={event => onchange((event.currentTarget as HTMLInputElement).checked)}
  />
  <span class="g-switch__control">
    <span class="g-switch__track"></span>
    <span class="g-switch__thumb"></span>
  </span>
  <span class="g-switch__text">
    <span class="g-switch__name">
      {label}
      {#if pending}
        <span class="g-pending" aria-hidden="true"></span>
      {/if}
    </span>
    {#if hint}
      <span class="g-hint">{hint}</span>
    {/if}
  </span>
</label>
