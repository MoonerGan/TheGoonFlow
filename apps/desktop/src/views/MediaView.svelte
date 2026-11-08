<script lang="ts">
  // Media view: booru posts, galleries, stored images, local file links.
  // No pipeline filters here — seen/tracking/wishlist are game concepts (D14).
  // What matters for media is which site it came from, how it is tagged, and
  // content-duplicate detection (same bytes twice).
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    getKindStats,
    deleteItems,
    getThumbs,
    importBookmarks,
    importImageFiles,
    importVideoFiles,
    listItems,
    mergeItems,
    dismissPair,
    pairingSuggestions,
    reEnrichItems,
    siteStats,
    sourceStats,
    topTags,
    type AppStatus,
    type FailureDetail,
    type ItemSummary,
    type PairSuggestion,
    type SiteCount,
    type SourceStats,
    type TagCount,
  } from "../lib/api";
  import { jobState, onActivityChange } from "../lib/activity.svelte";
  import ConfirmDialog from "../components/ConfirmDialog.svelte";
  import ItemEditor from "../components/ItemEditor.svelte";
  import ReportBar from "../components/ReportBar.svelte";
  import TileGrid from "../components/TileGrid.svelte";
  import Viewer from "../components/Viewer.svelte";

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
  let search = $state("");
  let tagFilter = $state<string | null>(null);
  let siteFilter = $state<string | null>(null);
  // #40: source-state filters (local-only items are the "fix my metadata" queue)
  let sourceFilter = $state<"local_only" | "no_tags" | null>(null);
  let stats = $state<SourceStats>({ localOnly: 0, noLocal: 0, noTags: 0 });
  let panel = $state<"library" | "pairs">("library");
  let report = $state<string | null>(null);
  let bmFailures = $state<FailureDetail[]>([]);
  let bmUnrecognized = $state<string[]>([]);
  let loadError = $state<string | null>(null);
  let enrichOnline = $state(true);
  let selected = $state<ItemSummary | null>(null);
  // #21: media cards open the full viewer; the editor is one button away.
  let viewerIndex = $state<number | null>(null);

  // batch selection (#12)
  let selectMode = $state(false);
  let selectedIds = $state<Set<string>>(new Set());
  let confirmBatchDelete = $state(false);

  // dupes (#10)
  let pairs = $state<PairSuggestion[]>([]);
  let pairNote = $state<string | null>(null);

  const imagesJob = $derived(jobState("images"));
  const videosJob = $derived(jobState("videos"));
  const bookmarksJob = $derived(jobState("bookmarks"));
  const pairCount = $derived(pairs.length);

  // debounced search (#8)
  let searchFor = $state("");
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  function onSearch(v: string) {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => (searchFor = v.trim()), 300);
  }

  async function load() {
    loadError = null;
    try {
      items = await listItems(
        300,
        0,
        searchFor || undefined,
        "media",
        tagFilter ?? undefined,
        siteFilter ?? undefined,
        sourceFilter ?? undefined,
      );
      const t = await getThumbs(items.filter((i) => i.hasThumb).map((i) => i.id));
      thumbs = Object.fromEntries(t.map((x) => [x.id, `data:${x.mime};base64,${x.data}`]));
      const kinds = await getKindStats();
      const k = Object.fromEntries(kinds);
      mediaCount = (k["gallery"] ?? 0) + (k["post"] ?? 0);
      tags = await topTags(24, "media");
      sites = await siteStats("media");
      stats = await sourceStats("media");
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
    sourceFilter;
    searchFor;
    void load();
  });

  // re-read whenever job state moves, so cards (and their art) arrive one by one
  onMount(() => onActivityChange(() => void load()));

  function toggleSelect(item: ItemSummary) {
    const next = new Set(selectedIds);
    if (next.has(item.id)) next.delete(item.id);
    else next.add(item.id);
    selectedIds = next;
  }

  async function doBatchDelete() {
    const ids = [...selectedIds];
    confirmBatchDelete = false;
    if (ids.length === 0) return;
    try {
      await deleteItems(ids);
      selectedIds = new Set();
      selectMode = false;
      await load();
    } catch (e) {
      report = `batch delete failed: ${e}`;
    }
  }

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

  async function doImportVideos() {
    const files = await open({
      multiple: true,
      title: "Pick videos to track (files stay where they are)",
      filters: [{ name: "Videos", extensions: ["mp4", "webm", "mkv", "mov", "avi"] }],
    });
    if (!files) return;
    const paths = Array.isArray(files) ? files : [files];
    if (paths.length === 0) return;
    report = null;
    try {
      const r = await importVideoFiles(paths);
      report =
        `videos: ${r.imported} tracked by path (${r.withThumb} with first-frame previews)` +
        (r.skipped ? `, ${r.skipped} skipped (already tracked / not a video)` : "") +
        " — the files stay on disk; only links live in the vault (Store copy is per item)" +
        (r.stopped ? ", stopped early" : "");
      await load();
    } catch (e) {
      report = `video import failed: ${e}`;
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

  function openPairs() {
    panel = "pairs";
    pairNote = null;
    pairingSuggestions("media")
      .then((p) => (pairs = p))
      .catch((e) => (pairNote = String(e)));
  }

  async function doMerge(s: PairSuggestion, keep: "a" | "b") {
    try {
      await mergeItems(keep === "a" ? s.aId : s.bId, keep === "a" ? s.bId : s.aId);
      pairs = await pairingSuggestions("media");
      pairNote = `merged into "${keep === "a" ? s.aTitle : s.bTitle}"`;
      await load();
    } catch (e) {
      pairNote = `merge failed: ${e}`;
    }
  }

  async function doKeepBoth(p: PairSuggestion) {
    try {
      await dismissPair(p.aId, p.bId);
      pairs = await pairingSuggestions("media");
      pairNote = "kept both — this pair will not be suggested again";
    } catch (e) {
      pairNote = `could not dismiss: ${e}`;
    }
  }

  let refetching = $state(false);
  async function doBatchRefetch() {
    const ids = [...selectedIds];
    if (ids.length === 0 || refetching) return;
    refetching = true;
    try {
      const r = await reEnrichItems(ids);
      report =
        `re-fetched ${r.items} item(s): ${r.enriched} of ${r.sources} sources gave something` +
        (r.failed ? `, ${r.failed} item(s) failed` : "") +
        (r.stopped ? ", stopped early" : "");
      await load();
    } catch (e) {
      report = `re-fetch failed: ${e}`;
    } finally {
      refetching = false;
    }
  }
</script>

<div class="view-toolbar">
  <button class="primary" onclick={onClip}>+ Clip URL</button>
  <button disabled={imagesJob === "running"} onclick={doImportImages} title="Image files are copied into the encrypted vault">
    + Images
  </button>
  <button disabled={videosJob === "running"} onclick={doImportVideos} title="Videos are tracked by path — the file is never copied into the vault">
    + Video
  </button>
  <button disabled={bookmarksJob === "running"} onclick={doImportBookmarks} title="Netscape HTML export from any browser">
    + Bookmarks
  </button>
  <label class="inline-check" title="During bookmark import only: fetch thumbnails + tags for known booru/DLsite links. One request at a time, politely rate-limited. Not the browser integration — that arrives with the companion extension.">
    <input type="checkbox" bind:checked={enrichOnline} />
    enrich online <span class="hint">(fetch art for bookmarked posts)</span>
  </label>
  <input
    class="search-box"
    type="text"
    placeholder="search title…"
    aria-label="Search media"
    value={search}
    oninput={(e) => onSearch(e.currentTarget.value)}
  />
  <span class="spacer"></span>
  {#if selectMode}
    <span class="hint">{selectedIds.size} selected</span>
    <button onclick={() => (selectedIds = new Set(items.map((i) => i.id)))}>Select all</button>
    <button disabled={refetching || selectedIds.size === 0} onclick={doBatchRefetch} title="re-fetch art/tags/online info for the selection — runs as a job you can stop">
      {refetching ? "Re-fetching…" : "Re-fetch metadata"}
    </button>
    <button
      class="danger"
      disabled={selectedIds.size === 0}
      onclick={() => (confirmBatchDelete = true)}
    >
      Delete selected
    </button>
    <button class="ghost" onclick={() => { selectMode = false; selectedIds = new Set(); }}>Done</button>
  {:else}
    <button class="ghost" onclick={() => (selectMode = true)}>Select…</button>
  {/if}
</div>

{#if report}
  <ReportBar text={report} onDismiss={() => (report = null)} />
  {#if bmFailures.length > 0 || bmUnrecognized.length > 0}
    <div class="report">
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
    </div>
  {/if}
{/if}
{#if loadError}
  <ReportBar text={loadError} tone="error" onDismiss={() => (loadError = null)} />
{/if}

<div class="main">
  <nav class="sidebar">
    <div class="section">Media</div>
    <button class="nav-item" class:active={panel === "library" && !tagFilter && !siteFilter && !sourceFilter} onclick={() => { panel = "library"; tagFilter = null; siteFilter = null; sourceFilter = null; }}>
      Everything <span class="count">{mediaCount}</span>
    </button>

    <div class="section">
      Sources
      {#if sourceFilter}
        <button class="link" onclick={() => (sourceFilter = null)}>clear</button>
      {/if}
    </div>
    <button class="nav-item" class:active={sourceFilter === "local_only"} onclick={() => (sourceFilter = sourceFilter === "local_only" ? null : "local_only")} title="Items with only a local file/folder and no online source — the queue for adding links and tags by hand.">
      Local only <span class="count">{stats.localOnly || ""}</span>
    </button>
    <button class="nav-item" class:active={sourceFilter === "no_tags"} onclick={() => (sourceFilter = sourceFilter === "no_tags" ? null : "no_tags")} title="Items with no tags at all — bare file links and imports that no post could be matched to.">
      No tags <span class="count">{stats.noTags || ""}</span>
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

    <div class="section">Keep current</div>
    <button class="nav-item" class:active={panel === "pairs"} onclick={openPairs}>
      Possible dupes <span class="count">{pairCount || ""}</span>
    </button>

    <div class="section">Vault</div>
    <div class="vault-info">{status.vaultRoot}</div>
  </nav>

  <main class="content">
    {#if panel === "pairs"}
      <h2 class="panel-title">Possible duplicates</h2>
      <p class="hint">Media whose <em>stored image bytes are identical</em> — the same picture imported twice or
        reposted across boorus. Nothing merges itself; you decide which entry to keep.</p>
      {#if pairNote}
        <p class="report-inline">{pairNote}</p>
      {/if}
      {#if pairs.length === 0}
        <div class="empty"><strong>No duplicates found.</strong><br />Two entries holding the exact same stored
          image show up here (remote links are compared by their fetched copy, so enrich must have run).</div>
      {:else}
        {#each pairs as p (`${p.aId}-${p.bId}`)}
          <div class="pair-row">
            <div class="pair-sides">
              <div class="pair-side">
                <span class="pair-num">1</span>
                <div>
                  <strong>{p.aTitle}</strong>
                  <span class="hint">({p.aSites || "no source"})</span>
                  {#if p.aSrc}<div class="pair-src" title={p.aSrc}>{p.aSrc}</div>{/if}
                </div>
              </div>
              <div class="pair-side">
                <span class="pair-num">2</span>
                <div>
                  <strong>{p.bTitle}</strong>
                  <span class="hint">({p.bSites || "no source"})</span>
                  {#if p.bSrc}<div class="pair-src" title={p.bSrc}>{p.bSrc}</div>{/if}
                </div>
              </div>
            </div>
            <div class="row">
              <button onclick={() => doMerge(p, "a")}>Keep 1</button>
              <button onclick={() => doMerge(p, "b")}>Keep 2</button>
              <button class="ghost" title="not a real duplicate — stop suggesting this pair" onclick={() => doKeepBoth(p)}>
                Keep both
              </button>
            </div>
          </div>
        {/each}
      {/if}

    {:else if items.length === 0}
      <div class="empty">
        <strong>{loadError ? "Couldn't load media." : searchFor ? `Nothing matches "${searchFor}".` : tagFilter || siteFilter ? "Nothing matches this filter." : "No media in the vault."}</strong><br />
        <em>+ Clip URL</em> takes a single link — a booru post, a gallery, a direct image/video URL, or a
        <code>file:///</code> link to media you already have. <em>+ Images</em> stores image files inside the
        encrypted vault; <em>+ Video</em> tracks video files by path; <em>+ Bookmarks</em> imports a whole browser export.
        <br /><br />
        <span class="hint">Videos stay links (they are too big for the vault); images are stored encrypted.</span>
      </div>
    {:else}
      <TileGrid
        items={items}
        {thumbs}
        showStatus={false}
        {selectMode}
        selected={selectedIds}
        onOpen={(item) => (viewerIndex = items.indexOf(item))}
        onToggle={toggleSelect}
      />
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

{#if viewerIndex !== null && items[viewerIndex]}
  <Viewer
    items={items}
    index={viewerIndex}
    onClose={() => (viewerIndex = null)}
    onEdit={(it) => {
      viewerIndex = null;
      selected = it;
    }}
  />
{/if}

{#if confirmBatchDelete}
  <ConfirmDialog
    title="Remove selected from vault"
    message={`Remove ${selectedIds.size} entries? Vault-stored images are deleted; linked files on disk are never touched.`}
    confirmLabel="Remove {selectedIds.size}"
    onConfirm={doBatchDelete}
    onCancel={() => (confirmBatchDelete = false)}
  />
{/if}
