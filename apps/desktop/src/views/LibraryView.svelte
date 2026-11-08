<script lang="ts">
  // Shell only: tabs, live activity strip, lock, and the clip dialog.
  // Games and Media are entirely separate views that share components only
  // where it actually helps (editor, tiles) — pipeline state never leaks into
  // media, site/tag filters never clutter games.
  import { onMount } from "svelte";
  import { getKindStats, type AppStatus, type ClipReport } from "../lib/api";
  import { activityTick, startWatching, stopWatching } from "../lib/activity.svelte";
  import ActivityBar from "../components/ActivityBar.svelte";
  import ClipDialog from "../components/ClipDialog.svelte";
  import SourcesPanel from "../components/SourcesPanel.svelte";
  import GamesView from "./GamesView.svelte";
  import MediaView from "./MediaView.svelte";

  let {
    status,
    onLock,
  }: {
    status: AppStatus;
    onLock: () => Promise<void>;
  } = $props();

  let tab = $state<"games" | "media" | "sources">("games");
  let clipOpen = $state(false);
  let clipReport = $state<string | null>(null);
  let counts = $state<Record<string, number>>({});

  const gamesCount = $derived(counts["game"] ?? 0);
  const mediaCount = $derived((counts["gallery"] ?? 0) + (counts["post"] ?? 0));

  async function loadCounts() {
    try {
      counts = Object.fromEntries(await getKindStats());
    } catch {
      // locked or busy — counts just stay stale for a moment
    }
  }

  $effect(() => {
    activityTick.n;
    void loadCounts();
  });

  onMount(() => {
    startWatching();
    void loadCounts();
    return stopWatching;
  });

  function clipLabel(r: ClipReport): string {
    if (!r.created) return `already in the flow as one item (${r.site})`;
    return (
      `clipped from ${r.site}: "${r.title}"` +
      (r.storedImage ? ", image stored" : "") +
      (r.thumb ? ", thumbnail made" : "") +
      (r.tags ? `, ${r.tags} tags` : "")
    );
  }
</script>

<div class="shell">
  <header class="topbar">
    <span class="brand">The Goon Flow</span>
    <nav class="tabs">
      <button class="tab" class:active={tab === "games"} onclick={() => (tab = "games")}>
        Games <span class="count">{gamesCount}</span>
      </button>
      <button class="tab" class:active={tab === "media"} onclick={() => (tab = "media")}>
        Media <span class="count">{mediaCount}</span>
      </button>
      <button class="tab" class:active={tab === "sources"} onclick={() => (tab = "sources")}>
        Sources
      </button>
    </nav>
    <span class="spacer"></span>
    {#if status.companion}
      <span class="pill" title="Companion API port">:{status.companion.port}</span>
    {/if}
    <button class="danger" onclick={onLock} title="Closes the vault and stops every import, fetch and update check">
      Lock
    </button>
  </header>

  <ActivityBar />

  {#if clipReport}
    <div class="report">{clipReport}</div>
  {/if}

  {#if tab === "games"}
    <GamesView {status} />
  {:else if tab === "media"}
    <MediaView {status} onClip={() => (clipOpen = true)} />
  {:else}
    <main class="content sources-page">
      <SourcesPanel />
    </main>
  {/if}
</div>

{#if clipOpen}
  <ClipDialog
    onClose={() => (clipOpen = false)}
    onDone={(r) => (clipReport = clipLabel(r))}
  />
{/if}
