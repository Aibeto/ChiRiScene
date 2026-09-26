<script lang="ts">
  // ConfirmSheet.svelte: [dialog]
  // 破坏性操作确认：原生 <dialog> 提供焦点陷阱、Esc 关闭与无障碍语义；视觉用语义 token（切角面板 + 状态信号）。
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

<!-- 骨架/皮肤复用官方 .ak-dialog（适配见 app.css [ak-adapt]）；danger 时把 --ak-dialog-signal 换成危险信号 -->
<dialog
  bind:this={dialogEl}
  class="ak-dialog"
  style={danger ? '--ak-dialog-signal: var(--ak-signal-danger)' : undefined}
  onclick={onBackdrop}
  oncancel={event => {
    // busy 期间 Esc 不关闭：操作进行中，关闭会让界面与实际状态脱节
    if (busy) event.preventDefault()
  }}
  onclose={() => {
    if (open) ondismiss()
  }}
>
  <header class="ak-dialog__header">
    <p class="ak-dialog__eyebrow">{danger ? 'DANGER' : 'CONFIRM'}</p>
    <h2 class="ak-dialog__title">{title}</h2>
  </header>
  <div class="ak-dialog__body">
    {#if message}
      <p>{message}</p>
    {/if}
    {#each notes as note}
      <p class="note">{note}</p>
    {/each}
  </div>
  <footer class="ak-dialog__footer">
    <button type="button" class="ak-button btn btn--ghost" disabled={busy} onclick={ondismiss}>
      {cancelText}
    </button>
    <button
      type="button"
      class="ak-button btn {danger ? 'btn--danger' : 'btn--primary'}"
      disabled={busy}
      onclick={onconfirm}
    >
      {confirmText}
    </button>
  </footer>
</dialog>

<style>
  /* 弹层骨架/皮肤来自官方 .ak-dialog（深色适配在 app.css [ak-adapt]），这里只保留项目特有的「注意事项」竖条；
     竖条跟随弹层信号色（CONFIRM 暖黄 / DANGER 危险红），固定暖色会在危险弹层多出第三种颜色 */
  .note {
    margin-top: var(--ak-space-3);
    padding-left: var(--ak-space-3);
    border-left: 2px solid var(--ak-dialog-signal);
    color: var(--ak-text-secondary);
  }
</style>
