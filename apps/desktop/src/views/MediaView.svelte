<script lang="ts">
  // Media view: booru posts, galleries, stored images, local file links.
  // No pipeline filters here — seen/tracking/wishlist are game concepts. What
  // matters for media is which site it came from and how it is tagged.
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    getKindStats,
    getThumbs,
    importBookmarks,
    importImageFiles,
    listItems,
    siteStats,
    topTags,
    type AppStatus,
    type FailureDetail,
    type ItemSummary,
    type SiteCount,
    type TagCount,
  } from "../lib/api";
  import { jobState, onActivityChange } from "../lib/activity.svelte";
  import ItemEditor from "../components/ItemEditor.svelte";
  import TileGrid from "../components/TileGrid.svelte";

  let {
    status,
    onClip,
  }: {
    status: AppStatus;
    onClip: () => void;
  } = $props();

  let items = $state<ItemSummary[]>([]);
  let tags = $state<TagCount[]>([]);
  let sites = $state<SiteCount[]>([]);
  let thumbs = $state<Record<string, string>>({});
  let mediaCount = $state(0);
  let tagFilter = $state<string | null>(null);
  let siteFilter = $state<string | null>(null);
  let report = $state<string | null>(null);
  let bmFailures = $state<FailureDetail[]>([]);
  let bmUnrecognized = $state<string[]>([]);
  let loadError = $state<string | null>(null);
  let enrichOnline = $state(true);
  let selected = $state<ItemSummary | null>(null);

  const imagesJob = $derived(jobState("images"));
  const bookmarksJob = $derived(jobState("bookmarks"));

  async function load() {
    loadError = null;
    try {
      items = await listItems(
        300,
        0,
        undefined,
        "media",
        tagFilter ?? undefined,
        siteFilter ?? undefined,
      );
      const t = await getThumbs(items.filter((i) => i.hasThumb).map((i) => i.id));
      thumbs = Object.fromEntries(t.map((x) => [x.id, `data:${x.mime};base64,${x.data}`]));
      const kinds = await getKindStats();
      const k = Object.fromEntries(kinds);
      mediaCount = (k["gallery"] ?? 0) + (k["post"] ?? 0);
      tags = await topTags(24, "media");
      sites = await siteStats("media");
    } catch (e) {
      items = [];
      tags = [];
      thumbs = {};
      loadError = `media could not be loaded: ${e}`;
    }
  }

  $effect(() => {
    tagFilter;
    siteFilter;
    void load();
  });

  onMount(() => onActivityChange(() => void load()));

  async function doImportImages() {
    const files = await open({
      multiple: true,
      title: "Pick images to store in the vault",
      filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "webp", "gif", "bmp"] }],
    });
    if (!files) return;
    const paths = Array.isArray(files) ? files : [files];
    if (paths.length === 0) return;
    report = null;
    try {
      const r = await importImageFiles(paths);
      report =
        `images: ${r.imagesStored} stored as "${r.title}" (encrypted in the vault)` +
        (r.failed ? `, ${r.failed} skipped` : "") +
        (r.stopped ? ", stopped early — everything before that is stored" : "");
      await load();
    } catch (e) {
      report = `image import failed: ${e}`;
    }
  }

  async function doImportBookmarks() {
    const file = await open({
      title: "Bookmarks export (HTML)",
      filters: [{ name: "Bookmarks HTML", extensions: ["html", "htm"] }],
    });
    if (typeof file !== "string") return;
    report = null;
    bmFailures = [];
    bmUnrecognized = [];
    try {
      const r = await importBookmarks(file, enrichOnline);
      bmFailures = r.failures;
      bmUnrecognized = r.unrecognizedSample;
      const bySite = Object.entries(r.bySite)
        .map(([site, n]) => `${site} ${n}`)
        .join(", ");
      report =
        `bookmarks: ${r.parsed} parsed → ${r.imported} imported` +
        (bySite ? ` (${bySite})` : "") +
        (r.thumbnailed ? `, ${r.thumbnailed} thumbnails` : "") +
        (r.tagsAdded ? `, ${r.tagsAdded} tags` : "") +
        (r.duplicates ? `, ${r.duplicates} dupes` : "") +
        (r.disabled ? `, ${r.disabled} skipped (site off)` : "") +
        (r.unrecognized ? `, ${r.unrecognized} unrecognized` : "") +
        (r.fetchFailures ? `, ${r.fetchFailures} fetch failures (see details)` : "") +
        (r.stopped ? " — stopped early; everything already imported is kept" : "");
      await load();
    } catch (e) {
      report = `bookmark import failed: ${e}`;
    }
  }
</script>

<div class="view-toolbar">
  <button class="primary" onclick={onClip}>+ Clip URL</button>
  <button disabled={imagesJob === "running"} onclick={doImportImages} title="Image files are copied into the encrypted vault">
    + Images
  </button>
  <button disabled={bookmarksJob === "running"} onclick={doImportBookmarks} title="Netscape HTML export from any browser">
    + Bookmarks
  </button>
  <label class="inline-check" title="During bookmark import only: fetch thumbnails + tags for known booru/DLsite links. One request at a time, politely rate-limited. Not the browser integration — that arrives with the companion extension.">
    <input type="checkbox" bind:checked={enrichOnline} />
    enrich online <span class="hint">(fetch art for bookmarked posts)</span>
  </label>
  <span class="spacer"></span>
  {#if bookmarksJob === "running"}
    <span class="hint">import running — everything else stays usable, watch the strip above</span>
  {/if}
</div>

{#if report}
  <div class="report">
    {report}
    {#if bmFailures.length > 0 || bmUnrecognized.length > 0}
      <details class="details">
        <summary>details ({bmFailures.length} failures{bmUnrecognized.length ? `, ${bmUnrecognized.length}+ unrecognized` : ""})</summary>
        {#if bmFailures.length > 0}
          <p class="hint">Bookmark fetch failures (the link is still imported — only the online art/tags failed):</p>
          <ul>
            {#each bmFailures as f}
              <li><code>{f.url}</code> — {f.reason}</li>
            {/each}
          </ul>
        {/if}
        {#if bmUnrecognized.length > 0}
          <p class="hint">Not a known site yet (skipped; site support is added over time):</p>
          <ul>
            {#each bmUnrecognized as u}
              <li><code>{u}</code></li>
            {/each}
          </ul>
        {/if}
      </details>
    {/if}
  </div>
{/if}
{#if loadError}
  <div class="report error-strip">{loadError}</div>
{/if}

<div class="main">
  <nav class="sidebar">
    <div class="section">Media</div>
    <button class="nav-item" class:active={!tagFilter && !siteFilter} onclick={() => { tagFilter = null; siteFilter = null; }}>
      Everything <span class="count">{mediaCount}</span>
    </button>

    <div class="section">
      From site
      {#if siteFilter}
        <button class="link" onclick={() => (siteFilter = null)}>clear</button>
      {/if}
    </div>
    {#if sites.length === 0}
      <div class="vault-info">Nothing imported from a site yet.</div>
    {:else}
      {#each sites as s (s.site)}
        <button class="nav-item" class:active={siteFilter === s.site} onclick={() => (siteFilter = siteFilter === s.site ? null : s.site)}>
          {s.site} <span class="count">{s.count}</span>
        </button>
      {/each}
    {/if}

    <div class="section">
      Tagged
      {#if tagFilter}
        <button class="link" onclick={() => (tagFilter = null)}>clear</button>
      {/if}
    </div>
    {#if tags.length === 0}
      <div class="vault-info">No media tags yet. Enriched bookmarks and clips bring tags in; you can add your own on any item.</div>
    {:else}
      {#each tags as t (t.id)}
        <button class="nav-item" class:active={tagFilter === t.name} onclick={() => (tagFilter = tagFilter === t.name ? null : t.name)} title={t.typeKey}>
          {t.name} <span class="count">{t.count}</span>
        </button>
      {/each}
    {/if}

    <div class="section">Vault</div>
    <div class="vault-info">{status.vaultRoot}</div>
  </nav>

  <main class="content">
    {#if items.length === 0}
      <div class="empty">
        <strong>{loadError ? "Couldn't load media." : tagFilter || siteFilter ? "Nothing matches this filter." : "No media in the vault."}</strong><br />
        <em>+ Clip URL</em> takes a single link — a booru post, a gallery, a direct image/video URL, or a
        <code>file:///</code> link to media you already have. <em>+ Images</em> stores image files inside the
        encrypted vault; <em>+ Bookmarks</em> imports a whole browser export.
        <br /><br />
        <span class="hint">Videos stay links (they are too big for the vault); images are stored encrypted.</span>
      </div>
    {:else}
      <TileGrid items={items} {thumbs} showStatus={false} onOpen={(item) => (selected = item)} />
    {/if}
  </main>
</div>

{#if selected}
  <ItemEditor
    item={selected}
    onClose={() => (selected = null)}
    onChanged={() => void load()}
  />
{/if}
