<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    deleteItem,
    getStats,
    getThumbs,
    importBookmarks,
    importGameFolder,
    importImageFiles,
    listItems,
    setComplete,
    updateItem,
    type AppStatus,
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
  let thumbs = $state<Record<string, string>>({});
  let report = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let enrichOnline = $state(true);
  let filter = $state<string>("all"); // all | status value
  let selected = $state<ItemSummary | null>(null);

  let editTitle = $state("");
  let editCircle = $state("");
  let editStatus = $state("seen");
  let editVersion = $state("");
  let editComplete = $state(false);
  let editorError = $state<string | null>(null);

  const filtered = $derived(
    filter === "all" ? items : items.filter((i) => i.status === filter),
  );

  async function load() {
    try {
      stats = await getStats();
      items = await listItems(300, 0);
      const t = await getThumbs(items.filter((i) => i.hasThumb).map((i) => i.id));
      thumbs = Object.fromEntries(t.map((x) => [x.id, `data:${x.mime};base64,${x.data}`]));
    } catch {
      items = [];
      stats = [];
      thumbs = {};
    }
  }

  $effect(() => {
    load();
  });

  function countFor(key: string): number {
    return stats.find((s) => s.status === key)?.count ?? 0;
  }

  async function doImportGames() {
    const dir = await open({ directory: true, title: "Folder containing your games" });
    if (typeof dir !== "string") return;
    busy = "games";
    report = "scanning folder…";
    try {
      const r = await importGameFolder(dir);
      report =
        `games: ${r.imported} imported (${r.withThumb} with cover)` +
        (r.skipped ? `, ${r.skipped} already in vault` : "");
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
      const sites = Object.entries(r.bySite)
        .map(([site, n]) => `${site} ${n}`)
        .join(", ");
      report =
        `bookmarks: ${r.parsed} parsed → ${r.imported} imported` +
        (sites ? ` (${sites})` : "") +
        (r.thumbnailed ? `, ${r.thumbnailed} thumbnails fetched` : "") +
        (r.duplicates ? `, ${r.duplicates} dupes` : "") +
        (r.unrecognized ? `, ${r.unrecognized} unrecognized` : "") +
        (r.fetchFailures ? `, ${r.fetchFailures} fetch failures` : "");
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
    <button class="primary" disabled={busy !== null} onclick={doImportGames}>+ Games folder</button>
    <button disabled={busy !== null} onclick={doImportImages}>+ Images</button>
    <button disabled={busy !== null} onclick={doImportBookmarks}>+ Bookmarks</button>
    <label class="inline-check">
      <input type="checkbox" bind:checked={enrichOnline} />
      enrich online
    </label>

    <span class="spacer"></span>
    {#if status.companion}
      <span class="pill" title="Companion API port">:{status.companion.port}</span>
    {/if}
    <button onclick={onLock}>Lock</button>
  </header>

  {#if report}
    <div class="report">{report}</div>
  {/if}

  <div class="main">
    <nav class="sidebar">
      <div class="section">Flow</div>
      <button class="nav-item" class:active={filter === "all"} onclick={() => (filter = "all")}>
        All <span class="count">{items.length}</span>
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
          <strong>The flow is empty.</strong><br />
          Import what you already have: <em>+ Games folder</em> sniffs engine/version/cover
          from your library, <em>+ Images</em> stores files in the encrypted vault,
          <em>+ Bookmarks</em> pulls a browser export and (optionally) fetches thumbnails + tags
          from booru APIs and DLsite.<br /><br />
          <span class="hint">Everything imported is editable — click any card to correct it.</span>
        </div>
      {:else}
        <div class="grid">
          {#each filtered as item (item.id)}
            <button class="tile" onclick={() => openEditor(item)}>
              {#if thumbs[item.id]}
                <img src={thumbs[item.id]} alt="" loading="lazy" />
              {:else}
                <div class="noimg">{item.kind}</div>
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
