<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    deleteItem,
    getStats,
    getKindStats,
    getThumbs,
    importBookmarks,
    importGameFolder,
    importGameZip,
    importGameExe,
    importImageFiles,
    listItems,
    setComplete,
    updateItem,
    type AppStatus,
    type FailureDetail,
    type ItemSummary,
    type StatusCount,
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
  let report = $state<string | null>(null);
  let bmFailures = $state<FailureDetail[]>([]);
  let bmUnrecognized = $state<string[]>([]);
  let gameFailures = $state<string[]>([]);
  let loadError = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let enrichOnline = $state(true);
  let view = $state<"all" | "game" | "media">("all");
  let filter = $state<string>("all"); // status filter within view
  let selected = $state<ItemSummary | null>(null);

  let editTitle = $state("");
  let editCircle = $state("");
  let editStatus = $state("seen");
  let editVersion = $state("");
  let editComplete = $state(false);
  let editorError = $state<string | null>(null);

  const viewKind = $derived(view === "all" ? undefined : view);
  const filtered = $derived(
    filter === "all" ? items : items.filter((i) => i.status === filter),
  );

  async function load() {
    loadError = null;
    try {
      stats = await getStats(viewKind);
      items = await listItems(300, 0, undefined, viewKind);
      const t = await getThumbs(items.filter((i) => i.hasThumb).map((i) => i.id));
      thumbs = Object.fromEntries(t.map((x) => [x.id, `data:${x.mime};base64,${x.data}`]));
      kindCounts = Object.fromEntries(await getKindStats());
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
    view; // reload when the Games/Media view changes
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
        (r.thumbnailed ? `, ${r.thumbnailed} thumbnails fetched` : "") +
        (r.duplicates ? `, ${r.duplicates} dupes` : "") +
        (r.unrecognized ? `, ${r.unrecognized} unrecognized` : "") +
        (r.fetchFailures ? `, ${r.fetchFailures} fetch failures (see details)` : "");
      await load();
    } catch (e) {
      report = `bookmark import failed: ${e}`;
    } finally {
      busy = null;
    }
  }

  function openEditor(item: ItemSummary) {
    selected = item;
    editTitle = item.title;
    editStatus = item.status;
    editVersion = item.version ?? "";
    editCircle = "";
    editComplete = false;
    editorError = null;
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
      selected = null;
      await load();
    } catch (e) {
      editorError = String(e);
    }
  }

  async function removeItem(item: ItemSummary) {
    if (!confirm(`Remove "${item.title}" from the vault? Files on disk are not touched.`)) return;
    try {
      await deleteItem(item.id);
      if (selected?.id === item.id) selected = null;
      await load();
    } catch (e) {
      report = `delete failed: ${e}`;
    }
  }
</script>

<div class="shell">
  <header class="topbar">
    <span class="brand">The Goon Flow</span>
    <button class="primary" disabled={busy !== null} onclick={doImportGames}>+ Game folder</button>
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
            <p class="hint">Bookmark fetch failures (the item is still imported — only the online art/tags failed):</p>
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
      <button class="nav-item" class:active={view === "all"} onclick={() => (view = "all")}>
        All <span class="count">{(kindCounts.game ?? 0) + (kindCounts.gallery ?? 0) + (kindCounts.post ?? 0)}</span>
      </button>
      <button class="nav-item" class:active={view === "game"} onclick={() => (view = "game")}>
        Games <span class="count">{kindCounts.game ?? 0}</span>
      </button>
      <button class="nav-item" class:active={view === "media"} onclick={() => (view = "media")}>
        Media <span class="count">{(kindCounts.gallery ?? 0) + (kindCounts.post ?? 0)}</span>
      </button>

      <div class="section">Flow</div>
      <button class="nav-item" class:active={filter === "all"} onclick={() => (filter = "all")}>
        Anything <span class="count">{items.length}</span>
      </button>
      {#each ["seen", "tracking", "wishlist", "downloaded", "played"] as key}
        <button class="nav-item" class:active={filter === key} onclick={() => (filter = key)}>
          {key} <span class="count">{countFor(key)}</span>
        </button>
      {/each}
      <div class="section">Tagged</div>
      <button class="nav-item disabled" title="Complete is a user_status tag chip (M1 expands tag UI)">
        ✓ Complete
      </button>
      <div class="section">Vault</div>
      <div class="vault-info">{status.vaultRoot}</div>
    </nav>

    <main class="content">
      {#if filtered.length === 0}
        <div class="empty">
          <strong>{loadError ? "Couldn't load the flow." : "Nothing in this view."}</strong><br />
          {#if view === "game"}
            Import games with <em>+ Game folder</em> (nested layouts OK), <em>+ Game zip</em>
            (zipped repacks), or <em>+ Game .exe</em> (point straight at the launcher).
          {:else if view === "media"}
            <em>+ Images</em> stores image files inside the encrypted vault;
            <em>+ Bookmarks</em> tracks posts and galleries from boorus, e-hentai and DLsite.
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
      <p class="sub">{selected.kind} · imported {new Date(Number(selected.updatedAt) * 1000).toLocaleString()}</p>

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
