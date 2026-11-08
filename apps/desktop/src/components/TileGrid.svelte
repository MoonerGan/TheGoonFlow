<script lang="ts">
  // The one piece both views genuinely share: card layout + lazy thumbs.
  import type { ItemSummary } from "../lib/api";

  let {
    items,
    thumbs,
    showStatus = true,
    onOpen,
  }: {
    items: ItemSummary[];
    thumbs: Record<string, string>;
    /** pipeline status only makes sense on game cards */
    showStatus?: boolean;
    onOpen: (item: ItemSummary) => void;
  } = $props();

  const VIEW_LABEL: Record<string, string> = {
    game: "game",
    gallery: "gallery",
    post: "post",
  };
</script>

<div class="grid">
  {#each items as item (item.id)}
    <button class="tile" onclick={() => onOpen(item)}>
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
