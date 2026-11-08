<script lang="ts">
  // The one piece both views genuinely share: card layout + lazy thumbs.
  // In select mode a click toggles selection (batch delete); otherwise it opens.
  import type { ItemSummary } from "../lib/api";

  let {
    items,
    thumbs,
    showStatus = true,
    selectMode = false,
    selected = new Set<string>(),
    onOpen,
    onToggle,
  }: {
    items: ItemSummary[];
    thumbs: Record<string, string>;
    /** pipeline status only makes sense on game cards */
    showStatus?: boolean;
    selectMode?: boolean;
    selected?: Set<string>;
    onOpen?: (item: ItemSummary) => void;
    onToggle?: (item: ItemSummary) => void;
  } = $props();

  const VIEW_LABEL: Record<string, string> = {
    game: "game",
    gallery: "gallery",
    post: "post",
  };
</script>

<div class="grid">
  {#each items as item (item.id)}
    {@const isSel = selected.has(item.id)}
    <button
      class="tile"
      class:selected={isSel}
      onclick={() => (selectMode ? onToggle?.(item) : onOpen?.(item))}
    >
      {#if selectMode}
        <span class="tile-check" aria-hidden="true">{isSel ? "✓" : ""}</span>
      {/if}
      {#if thumbs[item.id]}
        <img src={thumbs[item.id]} alt="" loading="lazy" />
      {:else}
        <div class="noimg">{VIEW_LABEL[item.kind] ?? item.kind}</div>
      {/if}
      <div class="tile-body">
        <span class="tile-title">{item.title}</span>
        <span class="tile-meta">
          {item.kind}{item.version ? ` · v${item.version}` : ""}{showStatus ? ` · ${item.status}` : ""}
        </span>
      </div>
    </button>
  {/each}
</div>
