<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    addItemTag,
    checkUpdates,
    clipUrl,
    deleteItem,
    getItem,
    getSiteSettings,
    getStats,
    getKindStats,
    getThumbs,
    importBookmarks,
    importGameFolder,
    importGameZip,
    importGameExe,
    importImageFiles,
    knownSites,
    listItems,
    listUpdates,
    pairingSuggestions,
    removeItemTag,
    setComplete,
    setSiteSetting,
    topTags,
    updateItem,
    mergeItems,
    type AppStatus,
    type ClipReport,
    type FailureDetail,
    type ItemDetail,
    type ItemSummary,
    type PairSuggestion,
    type SiteInfo,
    type SiteSetting,
    type StatusCount,
    type TagCount,
    type UpdateRow,
  } from "../lib/api";

  let {
    status,
    refresh,
    onLock,
  }: {
    status: AppStatus;
    refresh: () => Promise<void>;
    onLock: () => Promise<void>;
  } = $props();

  let items = $state<ItemSummary[]>([]);
  let stats = $state<StatusCount[]>([]);
  let kindCounts = $state<Record<string, number>>({});
  let thumbs = $state<Record<string, string>>({});
  let tags = $state<TagCount[]>([]);
  let report = $state<string | null>(null);
  let bmFailures = $state<FailureDetail[]>([]);
  let bmUnrecognized = $state<string[]>([]);
  let gameFailures = $state<string[]>([]);
  let loadError = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let enrichOnline = $state(true);
  let view = $state<"all" | "game" | "media">("all");
  let filter = $state<string>("all"); // status filter within view
  let tagFilter = $state<string | null>(null);
  let panel = $state<"library" | "updates" | "sources" | "pairs">("library");

  // clip
  let clipOpen = $state(false);
  let clipText = $state("");
  let clipError = $state<string | null>(null);

  // editor
  let selected = $state<ItemSummary | null>(null);
  let detail = $state<ItemDetail | null>(null);
  let editTitle = $state("");
  let editCircle = $state("");
  let editStatus = $state("seen");
  let editVersion = $state("");
  let editComplete = $state(false);
  let newTag = $state("");
  let editorError = $state<string | null>(null);

  // updates / sources / pairing panels
  let updates = $state<UpdateRow[]>([]);
  let updateNote = $state<string | null>(null);
  let siteInfos = $state<SiteInfo[]>([]);
  let siteSettings = $state<Record<string, SiteSetting>>({});
  let cookieDraft = $state<Record<string, string>>({});
  let pairNote = $state<string | null>(null);
  let pairs = $state<PairSuggestion[]>([]);

  const viewKind = $derived(view === "all" ? undefined : view);
  const filtered = $derived(
    filter === "all" ? items : items.filter((i) => i.status === filter),
  );
  const updateCount = $derived(updates.length);

  async function load() {
    loadError = null;
    try {
      stats = await getStats(viewKind);
      items = await listItems(300, 0, undefined, viewKind, tagFilter ?? undefined);
      const t = await getThumbs(items.filter((i) => i.hasThumb).map((i) => i.id));
      thumbs = Object.fromEntries(t.map((x) => [x.id, `data:${x.mime};base64,${x.data}`]));
      kindCounts = Object.fromEntries(await getKindStats());
      tags = await topTags(24);
      updates = await listUpdates(100);
    } catch (e) {
      // Never swallow silently — a blank grid with no reason is how M0 lied
      // about successful imports.
      items = [];
      stats = [];
      thumbs = {};
      loadError = `library load failed: ${e}`;
    }
  }

  $effect(() => {
    view;
    tagFilter; // reload when the view or the tag filter changes
    load();
  });

  function countFor(key: string): number {
    return stats.find((s) => s.status === key)?.count ?? 0;
  }

  const VIEW_LABEL: Record<string, string> = {
    game: "games",
    gallery: "galleries",
    post: "posts",
  };

  function gamesReport(r: {
    imported: number;
    skipped: number;
    withThumb: number;
    failures: string[];
  }): string {
    gameFailures = r.failures;
    return (
      `games: ${r.imported} imported (${r.withThumb} with cover)` +
      (r.skipped ? `, ${r.skipped} already in vault` : "") +
      (r.failures.length ? `, ${r.failures.length} failed (see details)` : "")
    );
  }

  async function doImportGames() {
    const dir = await open({ directory: true, title: "Folder containing your games" });
    if (typeof dir !== "string") return;
    busy = "games";
    report = "scanning folder (nested layouts included)…";
    try {
      report = gamesReport(await importGameFolder(dir));
      await load();
    } catch (e) {
      report = `game import failed: ${e}`;
    } finally {
      busy = null;
    }
  }

  async function doImportZip() {
    const file = await open({
      title: "Zipped game (repack .zip)",
      filters: [{ name: "Zip archive", extensions: ["zip"] }],
    });
    if (typeof file !== "string") return;
    busy = "games";
    report = "inspecting zip…";
    try {
      report = gamesReport(await importGameZip(file));
      await load();
    } catch (e) {
      report = `zip import failed: ${e}`;
    } finally {
      busy = null;
    }
  }

  async function doImportExe() {
    const file = await open({
      title: "The game's .exe",
      filters: [{ name: "Windows program", extensions: ["exe"] }],
    });
    if (typeof file !== "string") return;
    busy = "games";
    report = "importing game folder…";
    try {
      report = gamesReport(await importGameExe(file));
      await load();
    } catch (e) {
      report = `game import failed: ${e}`;
    } finally {
      busy = null;
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
    busy = "images";
    report = "storing images…";
    try {
      const r = await importImageFiles(paths);
      report = `images: ${r.imagesStored} stored as "${r.title}"` +
        (r.failed ? `, ${r.failed} skipped` : "");
      await load();
    } catch (e) {
      report = `image import failed: ${e}`;
    } finally {
      busy = null;
    }
  }

  async function doImportBookmarks() {
    const file = await open({
      title: "Bookmarks export (HTML)",
      filters: [{ name: "Bookmarks HTML", extensions: ["html", "htm"] }],
    });
    if (typeof file !== "string") return;
    busy = "bookmarks";
    report = enrichOnline
      ? "importing bookmarks (fetching online, this is polite and slow)…"
      : "importing bookmarks…";
    try {
      const r = await importBookmarks(file, enrichOnline);
      bmFailures = r.failures;
      bmUnrecognized = r.unrecognizedSample;
      const sites = Object.entries(r.bySite)
        .map(([site, n]) => `${site} ${n}`)
        .join(", ");
      report =
        `bookmarks: ${r.parsed} parsed → ${r.imported} imported` +
        (sites ? ` (${sites})` : "") +
        (r.thumbnailed ? `, ${r.thumbnailed} thumbnails` : "") +
        (r.tagsAdded ? `, ${r.tagsAdded} tags` : "") +
        (r.duplicates ? `, ${r.duplicates} dupes` : "") +
        (r.disabled ? `, ${r.disabled} skipped (site off)` : "") +
        (r.unrecognized ? `, ${r.unrecognized} unrecognized` : "") +
        (r.fetchFailures ? `, ${r.fetchFailures} fetch failures (see details)` : "");
      await load();
    } catch (e) {
      report = `bookmark import failed: ${e}`;
    } finally {
      busy = null;
    }
  }

  function clipLabel(r: ClipReport): string {
    if (!r.created) return `already in the flow as one item (${r.site})`;
    return (
      `clipped from ${r.site}: "${r.title}"` +
      (r.storedImage ? ", image stored" : "") +
      (r.thumb ? ", thumbnail made" : "") +
      (r.tags ? `, ${r.tags} tags` : "")
    );
  }

  async function doClip() {
    const url = clipText.trim();
    if (!url) return;
    busy = "clip";
    clipError = null;
    try {
      const r = await clipUrl(url);
      report = clipLabel(r);
      if (r.note) report += ` — ${r.note}`;
      clipText = "";
      clipOpen = false;
      await load();
    } catch (e) {
      clipError = String(e);
    } finally {
      busy = null;
    }
  }

  async function openEditor(item: ItemSummary) {
    selected = item;
    editTitle = item.title;
    editStatus = item.status;
    editVersion = item.version ?? "";
    editCircle = "";
    editComplete = false;
    newTag = "";
    editorError = null;
    detail = null;
    try {
      const d = await getItem(item.id);
      if (d) {
        detail = d;
        editCircle = d.circle ?? "";
        editComplete = d.tags.some((t) => t.name === "complete" && t.typeKey === "user_status");
      }
    } catch (e) {
      editorError = String(e);
    }
  }

  async function saveEditor() {
    if (!selected) return;
    editorError = null;
    try {
      await updateItem(
        selected.id,
        editTitle,
        editCircle || null,
        editStatus,
        editVersion || null,
      );
      await setComplete(selected.id, editComplete);
      closeEditor();
      await load();
    } catch (e) {
      editorError = String(e);
    }
  }

  function closeEditor() {
    selected = null;
    detail = null;
  }

  async function addTag() {
    if (!selected || !newTag.trim()) return;
    try {
      await addItemTag(selected.id, newTag.trim());
      newTag = "";
      detail = await getItem(selected.id);
      await load();
    } catch (e) {
      editorError = String(e);
    }
  }

  async function dropTag(tagId: number) {
    if (!selected) return;
    try {
      await removeItemTag(selected.id, tagId);
      detail = await getItem(selected.id);
      await load();
    } catch (e) {
      editorError = String(e);
    }
  }

  async function removeItem(item: ItemSummary) {
    if (!confirm(`Remove "${item.title}" from the vault? Files on disk are not touched.`)) return;
    try {
      await deleteItem(item.id);
      if (selected?.id === item.id) closeEditor();
      await load();
    } catch (e) {
      report = `delete failed: ${e}`;
    }
  }

  async function openPanel(which: typeof panel) {
    panel = which;
    if (which === "sources") {
      siteInfos = await knownSites();
      try {
        const rows = await getSiteSettings();
        siteSettings = Object.fromEntries(rows.map((r) => [r.site, r]));
      } catch (e) {
        report = `sources settings unavailable: ${e}`;
      }
    }
    if (which === "updates") {
      updates = await listUpdates(100);
    }
    if (which === "pairs") {
      pairNote = null;
      try {
        pairs = await pairingSuggestions();
      } catch (e) {
        pairNote = String(e);
      }
    }
  }

  async function doCheckUpdates() {
    busy = "updates";
    updateNote = "checking each tracked game once, respecting site rate limits…";
    try {
      const r = await checkUpdates();
      updateNote =
        `checked ${r.checked} source links` +
        (r.updated ? ` → ${r.updated} updated: ${r.updatedTitles.join(", ")}` : " → nothing new") +
        (r.failures ? ` (${r.failures} could not be reached)` : "");
      updates = await listUpdates(100);
      await load();
    } catch (e) {
      updateNote = `update check failed: ${e}`;
    } finally {
      busy = null;
    }
  }

  async function saveSite(site: string, patch: { enabled?: boolean; rateMs?: number; cookie?: string }) {
    try {
      await setSiteSetting(site, patch.enabled, patch.cookie, patch.rateMs);
      const rows = await getSiteSettings();
      siteSettings = Object.fromEntries(rows.map((r) => [r.site, r]));
      cookieDraft[site] = "";
    } catch (e) {
      report = `could not save ${site}: ${e}`;
    }
  }

  async function doMerge(s: PairSuggestion, keep: "a" | "b") {
    const keepId = keep === "a" ? s.aId : s.bId;
    const dropId = keep === "a" ? s.bId : s.aId;
    try {
      await mergeItems(keepId, dropId);
      pairs = await pairingSuggestions();
      pairNote = `merged into "${keep === "a" ? s.aTitle : s.bTitle}"`;
      await load();
    } catch (e) {
      pairNote = `merge failed: ${e}`;
    }
  }

  function when(epoch: string): string {
    const n = Number(epoch);
    return Number.isFinite(n) && n > 0 ? new Date(n * 1000).toLocaleString() : "—";
  }
</script>

<div class="shell">
  <header class="topbar">
    <span class="brand">The Goon Flow</span>
    <button class="primary" disabled={busy !== null} onclick={() => (clipOpen = true)}>+ Clip URL</button>
    <button disabled={busy !== null} onclick={doImportGames}>+ Game folder</button>
    <button disabled={busy !== null} onclick={doImportZip}>+ Game zip</button>
    <button disabled={busy !== null} onclick={doImportExe}>+ Game .exe</button>
    <button disabled={busy !== null} onclick={doImportImages}>+ Images</button>
    <button disabled={busy !== null} onclick={doImportBookmarks}>+ Bookmarks</button>
    <label class="inline-check" title="During bookmark import only: fetch thumbnails + tags for known booru/DLsite links. One request at a time. Not the browser integration — that arrives with the companion extension.">
      <input type="checkbox" bind:checked={enrichOnline} />
      enrich online <span class="hint">(fetch art for bookmarked posts)</span>
    </label>

    <span class="spacer"></span>
    {#if status.companion}
      <span class="pill" title="Companion API port">:{status.companion.port}</span>
    {/if}
    <button onclick={onLock}>Lock</button>
  </header>

  {#if report}
    <div class="report">
      {report}
      {#if bmFailures.length > 0 || bmUnrecognized.length > 0 || gameFailures.length > 0}
        <details class="details">
          <summary>details ({bmFailures.length + gameFailures.length} failures{bmUnrecognized.length ? `, ${bmUnrecognized.length}+ unrecognized` : ""})</summary>
          {#if gameFailures.length > 0}
            <p class="hint">Game import problems:</p>
            <ul>
              {#each gameFailures as f}
                <li>{f}</li>
              {/each}
            </ul>
          {/if}
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
      <div class="section">Views</div>
      <button class="nav-item" class:active={panel === "library" && view === "all"} onclick={() => { openPanel("library"); view = "all"; }}>
        All <span class="count">{(kindCounts.game ?? 0) + (kindCounts.gallery ?? 0) + (kindCounts.post ?? 0)}</span>
      </button>
      <button class="nav-item" class:active={panel === "library" && view === "game"} onclick={() => { openPanel("library"); view = "game"; }}>
        Games <span class="count">{kindCounts.game ?? 0}</span>
      </button>
      <button class="nav-item" class:active={panel === "library" && view === "media"} onclick={() => { openPanel("library"); view = "media"; }}>
        Media <span class="count">{(kindCounts.gallery ?? 0) + (kindCounts.post ?? 0)}</span>
      </button>

      <div class="section">Flow</div>
      <button class="nav-item" class:active={panel === "library" && filter === "all"} onclick={() => { panel = "library"; filter = "all"; }}>
        Anything <span class="count">{items.length}</span>
      </button>
      {#each ["seen", "tracking", "wishlist", "downloaded", "played"] as key}
        <button class="nav-item" class:active={panel === "library" && filter === key} onclick={() => { panel = "library"; filter = key; }}>
          {key} <span class="count">{countFor(key)}</span>
        </button>
      {/each}

      <div class="section">
        Tagged
        {#if tagFilter}
          <button class="link" onclick={() => (tagFilter = null)}>clear</button>
        {/if}
      </div>
      {#if tags.length === 0}
        <div class="vault-info">No tags yet. Clips and enriched bookmarks bring tags in; you can add your own on any item.</div>
      {:else}
        {#each tags as t (t.id)}
          <button class="nav-item" class:active={tagFilter === t.name} onclick={() => (tagFilter = tagFilter === t.name ? null : t.name)} title={t.typeKey}>
            {t.name} <span class="count">{t.count}</span>
          </button>
        {/each}
      {/if}

      <div class="section">Keep current</div>
      <button class="nav-item" class:active={panel === "updates"} onclick={() => openPanel("updates")}>
        Updates <span class="count">{updateCount}</span>
      </button>
      <button class="nav-item" class:active={panel === "pairs"} onclick={() => openPanel("pairs")}>
        Possible dupes <span class="count">{pairs.length || ""}</span>
      </button>
      <button class="nav-item" class:active={panel === "sources"} onclick={() => openPanel("sources")}>
        Sources
      </button>

      <div class="section">Vault</div>
      <div class="vault-info">{status.vaultRoot}</div>
    </nav>

    <main class="content">
      {#if panel === "sources"}
        <h2 class="panel-title">Sources</h2>
        <p class="hint">Turn a site off and its links are skipped on import/clip. Repack mirrors default to off (they are
          DLsite re-uploads, not new works). A cookie is only used for sites that need a login — paste the whole
          <code>Cookie</code> header value. Rate is the delay between requests to that site.</p>
        {#each siteInfos as s (s.site)}
          {@const cur = siteSettings[s.site]}
          <div class="site-row">
            <div class="site-name">
              <strong>{s.site}</strong>
              <span class="hint">{s.kind}{s.mirror ? " · off by default" : ""}</span>
            </div>
            <label class="inline-check">
              <input
                type="checkbox"
                checked={cur ? cur.enabled : !s.mirror}
                onchange={(e) => saveSite(s.site, { enabled: e.currentTarget.checked })}
              />
              enabled
            </label>
            <div class="site-rate">
              <input
                type="text"
                inputmode="numeric"
                value={cur ? String(cur.rateMs) : String(s.defaultRateMs)}
                onchange={(e) => {
                  const n = Number(e.currentTarget.value);
                  if (Number.isFinite(n)) saveSite(s.site, { rateMs: Math.round(n) });
                }}
              />
              <span class="hint">ms between hits</span>
            </div>
            <div class="site-cookie">
              <input
                type="text"
                placeholder={cur?.cookieSet ? "cookie saved — paste to replace" : "cookie (optional)"}
                bind:value={cookieDraft[s.site]}
                onkeydown={(e) => e.key === "Enter" && cookieDraft[s.site] && saveSite(s.site, { cookie: cookieDraft[s.site] })}
              />
              <button onclick={() => cookieDraft[s.site] && saveSite(s.site, { cookie: cookieDraft[s.site] })}>Set</button>
            </div>
          </div>
        {/each}

      {:else if panel === "updates"}
        <h2 class="panel-title">Updates</h2>
        <div class="row">
          <button class="primary" disabled={busy !== null} onclick={doCheckUpdates}>Check updates now</button>
          <span class="hint">Re-visits the DLsite pages and f95 threads of games you track, and records version changes here.</span>
        </div>
        {#if updateNote}
          <p class="report-inline">{updateNote}</p>
        {/if}
        {#if updates.length === 0}
          <div class="empty">
            <strong>No updates recorded yet.</strong><br />
            Clip or import a DLsite / f95 link for a game, then press <em>Check updates now</em>.
            A game you marked Complete still shows up here — completion is a tag, not a full stop.
          </div>
        {:else}
          {#each updates as u (`${u.itemId}-${u.seenAt}`)}
            <div class="update-row">
              <button class="link" onclick={() => {
                const item = items.find((i) => i.id === u.itemId);
                if (item) openEditor(item);
              }}>{u.title}</button>
              <span class="hint">
                {u.site}{u.oldVersion ? ` v${u.oldVersion}` : ""} → {u.newVersion ? `v${u.newVersion}` : "?"} · {when(u.seenAt)}
              </span>
            </div>
          {/each}
        {/if}

      {:else if panel === "pairs"}
        <h2 class="panel-title">Possible duplicates</h2>
        <p class="hint">Same title once version/engine brackets are dropped — the usual DLsite origin + f95 thread +
          local folder triple. Nothing merges itself; you decide which entry to keep.</p>
        {#if pairNote}
          <p class="report-inline">{pairNote}</p>
        {/if}
        {#if pairs.length === 0}
          <div class="empty"><strong>No duplicates found.</strong><br />Two games whose titles match once brackets
            (<code>[v0.12]</code>, <code>[renpy]</code>) are stripped would show up here.</div>
        {:else}
          {#each pairs as p (`${p.aId}-${p.bId}`)}
            <div class="pair-row">
              <div>
                <strong>{p.aTitle}</strong> <span class="hint">({p.aSites || "no source"})</span>
                <br />
                <strong>{p.bTitle}</strong> <span class="hint">({p.bSites || "no source"})</span>
              </div>
              <div class="row">
                <button onclick={() => doMerge(p, "a")}>Keep first</button>
                <button onclick={() => doMerge(p, "b")}>Keep second</button>
              </div>
            </div>
          {/each}
        {/if}

      {:else if filtered.length === 0}
        <div class="empty">
          <strong>{loadError ? "Couldn't load the flow." : tagFilter ? `Nothing tagged "${tagFilter}".` : "Nothing in this view."}</strong><br />
          {#if view === "game"}
            Import games with <em>+ Game folder</em> (nested layouts OK), <em>+ Game zip</em>
            (zipped repacks), or <em>+ Game .exe</em> (point straight at the launcher).
          {:else if view === "media"}
            <em>+ Clip URL</em> takes a single link — a booru post, a gallery, a direct image/video URL, or a
            <code>file:///</code> link to media you already have. <em>+ Images</em> stores image files inside the
            encrypted vault; <em>+ Bookmarks</em> imports a whole browser export.
          {:else}
            Import what you already have: games (folder / zip / exe), images (stored encrypted),
            and a browser bookmarks export (optionally fetching thumbnails + tags online).
          {/if}
          <br /><br />
          <span class="hint">Everything imported is editable — click any card to correct it.</span>
        </div>
      {:else}
        <div class="grid">
          {#each filtered as item (item.id)}
            <button class="tile" onclick={() => openEditor(item)}>
              {#if thumbs[item.id]}
                <img src={thumbs[item.id]} alt="" loading="lazy" />
              {:else}
                <div class="noimg">{VIEW_LABEL[item.kind] ?? item.kind}</div>
              {/if}
              <div class="tile-body">
                <span class="tile-title">{item.title}</span>
                <span class="tile-meta">
                  {item.kind}{item.version ? ` · v${item.version}` : ""} · {item.status}
                </span>
              </div>
            </button>
          {/each}
        </div>
      {/if}
    </main>
  </div>
</div>

{#if clipOpen}
  <div
    class="modal-backdrop"
    role="presentation"
    onclick={() => (clipOpen = false)}
    onkeydown={(e) => e.key === "Escape" && (clipOpen = false)}
  >
    <div class="modal card" role="dialog" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.stopPropagation()}>
      <h1>Clip a link</h1>
      <p class="sub">One box, everything: booru post pages and file links, e-hentai galleries, DLsite products,
        f95 threads, and <code>file:///</code> links to media on this machine.</p>
      <textarea
        class="clip-box"
        placeholder="https://… or file:///D:/Media/clip.mp4"
        bind:value={clipText}
        onkeydown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            doClip();
          }
        }}
      ></textarea>
      {#if clipError}
        <p class="error">{clipError}</p>
      {/if}
      <div class="row modal-actions">
        <button class="primary" disabled={busy !== null || !clipText.trim()} onclick={doClip}>Clip</button>
        <button class="ghost" onclick={() => (clipOpen = false)}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

{#if selected}
  <div
    class="modal-backdrop"
    role="presentation"
    onclick={() => (selected = null)}
    onkeydown={(e) => e.key === "Escape" && (selected = null)}
  >
    <div
      class="modal card"
      role="dialog"
      tabindex="-1"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
    >
      <h1>Edit item</h1>
      <p class="sub">{selected.kind} · imported {when(selected.updatedAt)}</p>

      <label for="ed-title">Title</label>
      <input id="ed-title" type="text" bind:value={editTitle} />

      <label for="ed-circle">Circle / developer</label>
      <input id="ed-circle" type="text" bind:value={editCircle} />

      <div class="row two-col">
        <div>
          <label for="ed-status">Status</label>
          <select id="ed-status" bind:value={editStatus}>
            {#each ["seen", "tracking", "wishlist", "downloaded", "played"] as s}
              <option value={s}>{s}</option>
            {/each}
          </select>
        </div>
        <div>
          <label for="ed-version">Version</label>
          <input id="ed-version" type="text" bind:value={editVersion} placeholder="0.12" />
        </div>
      </div>

      <label class="inline-check">
        <input type="checkbox" bind:checked={editComplete} />
        ✓ Complete <span class="hint">(personal marker — green chip on cards)</span>
      </label>

      <label for="ed-tag">Tags</label>
      <div class="chips">
        {#if detail}
          {#each detail.tags as t (t.id)}
            <span class="chip" class:chip-status={t.typeKey === "user_status"} class:chip-site={t.typeKey === "site_flag"}>
              {t.name}
              <button class="chip-x" title="remove" onclick={() => dropTag(t.id)}>×</button>
            </span>
          {/each}
        {/if}
      </div>
      <div class="row">
        <input id="ed-tag" type="text" placeholder="add a tag, press Enter" bind:value={newTag} onkeydown={(e) => e.key === "Enter" && addTag()} />
        <button onclick={addTag}>Add</button>
      </div>

      {#if detail && detail.sources.length > 0}
        <div class="field-label">Sources</div>
        {#each detail.sources as s (s.id)}
          <div class="source-row">
            <span class="chip">{s.site}</span>
            <code>{s.url}</code>
          </div>
        {/each}
      {/if}

      <div class="row modal-actions">
        <button class="primary" onclick={saveEditor}>Save</button>
        <button class="ghost" onclick={() => (selected = null)}>Cancel</button>
        <span class="spacer"></span>
        <button class="danger" onclick={() => removeItem(selected!)}>Remove from vault</button>
      </div>

      {#if editorError}
        <p class="error">{editorError}</p>
      {/if}
    </div>
  </div>
{/if}
