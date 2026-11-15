<script lang="ts">
  // The toast stack. One instance, mounted by App.svelte, floating above every
  // view — a warning about what just happened must never share space with the
  // thing it is talking about (the viewer note and the Sources cookie verdict
  // both did that: #62's and #38's reviews).
  import { dismissToast, toasts } from "../lib/toast.svelte";
</script>

<div class="toasts" role="status" aria-live="polite">
  {#each toasts.list as t (t.id)}
    <div class="toast {t.kind}">
      <span class="toast-text">{t.text}</span>
      <button
        class="toast-x"
        title="dismiss"
        aria-label="dismiss this message"
        onclick={() => dismissToast(t.id)}
      >
        ×
      </button>
    </div>
  {/each}
</div>
