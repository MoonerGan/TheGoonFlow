<script lang="ts">
  // Shell only: tabs, live activity strip, lock, and the clip dialog.
  // Games and Media are entirely separate views that share components only
  // where it actually helps (editor, tiles) — pipeline state never leaks into
  // media, site/tag filters never clutter games.
  import { onMount } from "svelte";
  import { getKindStats, getChrome, getPanicStatus, type AppStatus, type ClipReport, type ChromeInfo, type PanicStatus } from "../lib/api";
  import { activityTick, startWatching, stopWatching } from "../lib/activity.svelte";
  import ActivityBar from "../components/ActivityBar.svelte";
  import ClipDialog from "../components/ClipDialog.svelte";
  import ReportBar from "../components/ReportBar.svelte";
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
  // #27: when a decoy window title is stored beside the exe, the in-app
  // brand follows it — the forgot-to-lock case should say "Flow Notes" too.
  let chrome = $state<ChromeInfo | null>(null);
  // #41: if the global panic key could not be registered (another instance or
  // program owns the combination), the app must say so instead of letting the
  // user believe the panic path works.
  let panic = $state<PanicStatus | null>(null);

  // report strips must be dismissable and expire on their own (#4)
  let reportTimer: ReturnType<typeof setTimeout> | undefined;
  function showReport(text: string) {
    clipReport = text;
    clearTimeout(reportTimer);
    reportTimer = setTimeout(() => (clipReport = null), 30_000);
  }

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
    getPanicStatus()
      .then((p) => (panic = p))
      .catch(() => {
        // advisory only: if the app can't answer, the strip stays hidden
      });
    getChrome()
      .then((c) => (chrome = c))
      .catch(() => {
        // advisory: no chrome.cfg = stock brand display
      });
    return stopWatching;
  });

  function clipLabel(r: ClipReport): string {
    if (!r.created) return `already in the flow as one item (${r.site})`;
    // The note is the *why*, and it was being thrown away here: an f95 clip that
    // hit the login wall ("f95 answered, but this page shows its art to members
    // only - save your f95 cookie under Sources…") or a clip where the source
    // page offered 15 images and we stored one cover reported as a plain success
    // (#38/#46). The backend always said it; only this label dropped it.
    return (
      `clipped from ${r.site}: "${r.title}"` +
      (r.storedImage ? ", image stored" : "") +
      (r.thumb ? ", thumbnail made" : "") +
      (r.tags ? `, ${r.tags} tags` : "") +
      (r.note ? ` — ${r.note}` : "")
    );
  }
</script>

<div class="shell">
  <header class="topbar">
    <span class="brand">{chrome?.decoy ?? "The Goon Flow"}</span>
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
    {#if panic && !panic.armed}
      <span class="pill danger" title={panic.fault ?? "registration failed"}>
        panic key NOT armed
      </span>
    {/if}
    {#if status.companion}
      <span class="pill" title="Companion API port">:{status.companion.port}</span>
    {/if}
    <button class="danger" onclick={onLock} title="Closes the vault and stops every import, fetch and update check">
      Lock
    </button>
  </header>

  <ActivityBar />

  {#if panic && !panic.armed}
    <div class="report error-strip">
      <span class="report-text">
        the panic key ({panic.hotkey}) did not register, so pressing it does
        nothing: {panic.fault ?? "unknown reason"}. Close the other GoonFlow
        window/instance (or whatever else owns that combination), or pick a
        different key under Sources → Panic &amp; privacy.
      </span>
    </div>
  {/if}

  {#if clipReport}
    <ReportBar text={clipReport} onDismiss={() => (clipReport = null)} />
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
