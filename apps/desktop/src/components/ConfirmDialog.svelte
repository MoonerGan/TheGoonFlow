<script lang="ts">
  // In-app destructive-action confirmation — replaces the OS-styled
  // window.confirm() popup that looked out of place in the dark chrome (#6).
  let {
    title,
    message,
    confirmLabel = "Confirm",
    danger = true,
    onConfirm,
    onCancel,
  }: {
    title: string;
    message: string;
    confirmLabel?: string;
    danger?: boolean;
    onConfirm: () => void;
    onCancel: () => void;
  } = $props();
</script>

<div
  class="modal-backdrop"
  role="presentation"
  onclick={onCancel}
  onkeydown={(e) => e.key === "Escape" && onCancel()}
>
  <div class="modal card confirm-card" role="alertdialog" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.stopPropagation()}>
    <h1>{title}</h1>
    <p class="sub">{message}</p>
    <div class="row modal-actions">
      <button class={danger ? "danger" : "primary"} onclick={onConfirm}>{confirmLabel}</button>
      <button class="ghost" onclick={onCancel}>Cancel</button>
    </div>
  </div>
</div>
