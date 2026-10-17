<script lang="ts">
  // TextField.svelte: [state] [input]文本输入：与 NumberField 同款结构（.g-field + .g-switch__text），
  // 但值恒为字符串、不做提交时机约束——由调用方决定何时读取（通知测试里是点按钮时才用）
  let {
    label,
    hint = '',
    value,
    placeholder = '',
    maxlength = 200,
    disabled = false,
    oninput
  } = $props<{
    label: string
    hint?: string
    value: string
    placeholder?: string
    /** 上限由调用方给：通知标题/正文都不该长到撑爆通知栏 */
    maxlength?: number
    disabled?: boolean
    /** 每次输入即回调——测试按钮读的是当前值，不需要「失焦才提交」的语义 */
    oninput: (next: string) => void
  }>()
</script>

<label class="g-field" data-disabled={disabled || undefined}>
  <span class="g-switch__text">
    <span class="g-switch__name">{label}</span>
    {#if hint}
      <span class="g-hint">{hint}</span>
    {/if}
  </span>
  <input
    class="g-input tf__input"
    type="text"
    {placeholder}
    {maxlength}
    {disabled}
    value={value}
    oninput={event => oninput((event.currentTarget as HTMLInputElement).value)}
  />
</label>

<style>
  .g-field[data-disabled] {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .tf__input {
    width: 100%;
    min-width: 0;
  }
</style>
