<script lang="ts">
  // ConfirmSheet.svelte: [dialog] 破坏性操作确认：原生 <dialog> 提供焦点陷阱、Esc 关闭与无障碍语义，视觉是亚克力强档弹层（.g-dialog）
  let {
    open = false,
    title,
    message = '',
    notes = [],
    confirmText,
    cancelText,
    danger = false,
    busy = false,
    onconfirm,
    ondismiss
  } = $props<{
    open: boolean
    title: string
    message?: string
    notes?: string[]
    confirmText: string
    cancelText: string
    danger?: boolean
    busy?: boolean
    onconfirm: () => void
    ondismiss: () => void
  }>()

  let dialogEl = $state<HTMLDialogElement | null>(null)

  $effect(() => {
    const node = dialogEl
    if (!node) return
    if (open && !node.open) node.showModal()
    else if (!open && node.open) node.close()
  })

  function onBackdrop(event: MouseEvent) {
    if (event.target === dialogEl && !busy) ondismiss()
  }
</script>

<!-- 骨架/皮肤来自 app.css .g-dialog；danger 时把 --dialog-signal 换成危险信号（眉头字与注意事项提示块同步变色） -->
<dialog
  bind:this={dialogEl}
  class="g-dialog"
  style={danger ? '--dialog-signal: var(--sig-danger)' : undefined}
  onclick={onBackdrop}
  oncancel={event => {
    // busy 期间 Esc 不关闭：操作进行中，关闭会让界面与实际状态脱节
    if (busy) event.preventDefault()
  }}
  onclose={() => {
    if (open) ondismiss()
  }}
>
  <header class="g-dialog__header">
    <p class="g-dialog__eyebrow">{danger ? 'DANGER' : 'CONFIRM'}</p>
    <h2 class="g-dialog__title">{title}</h2>
  </header>
  <div class="g-dialog__body">
    {#if message}
      <p>{message}</p>
    {/if}
    {#each notes as note}
      <p class="note">{note}</p>
    {/each}
  </div>
  <footer class="g-dialog__footer">
    <button type="button" class="g-btn g-btn--ghost" disabled={busy} onclick={ondismiss}>
      {cancelText}
    </button>
    <button
      type="button"
      class="g-btn {danger ? 'g-btn--danger' : 'g-btn--primary'}"
      disabled={busy}
      onclick={onconfirm}
    >
      {confirmText}
    </button>
  </footer>
</dialog>

<style>
  /* 弹层骨架/皮肤来自 .g-dialog，这里只保留项目特有的「注意事项」块：信号色淡底 + 同色细边（Fluent 的 InfoBar 语言）
     信号色跟随弹层（CONFIRM 品牌蓝 / DANGER 危险红），不引入第三种颜色 */
  .note {
    margin-top: var(--space-3);
    padding: var(--space-2) var(--space-3);
    border: var(--hairline) solid color-mix(in srgb, var(--dialog-signal, var(--accent)) 32%, transparent);
    border-radius: var(--radius-m);
    background: color-mix(in srgb, var(--dialog-signal, var(--accent)) 8%, transparent);
    color: var(--text-muted);
  }
</style>
