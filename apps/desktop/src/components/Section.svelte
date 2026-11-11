<script lang="ts">
  // #43: the item dialog carries ~40 controls, so it is folded into topics
  // instead of showing them all at once. Native <details>/<summary> on purpose:
  // the toggle is a real control (keyboard and screen readers work for free)
  // that answers only Enter and Space - Escape therefore stays free for the
  // dialog's own close handler.
  import type { Snippet } from "svelte";

  let {
    title,
    hint = "",
    count = null,
    open = false,
    children,
  }: {
    title: string;
    /** short explanation shown next to the title */
    hint?: string;
    /** what the section holds; hidden while it is open (.ed-section[open]) */
    count?: number | null;
    /** keep the first section open so the dialog never opens empty-looking */
    open?: boolean;
    children: Snippet;
  } = $props();

  // The fold state belongs to the user, not to the props: count/hint change
  // while an item is edited, and a reactive `open` would re-apply the default
  // and snap a section shut under the cursor (#43).
  // svelte-ignore state_referenced_locally
  let isOpen = $state(open);
</script>

<details class="ed-section" bind:open={isOpen}>
  <summary>
    <span class="ed-name">{title}</span>
    {#if count !== null && count > 0}
      <span class="ed-count">({count})</span>
    {/if}
    {#if hint}
      <span class="ed-hint">{hint}</span>
    {/if}
  </summary>
  <div class="ed-body">
    {@render children()}
  </div>
</details>
