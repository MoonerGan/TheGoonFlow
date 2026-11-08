<script lang="ts">
  // Games view: pipeline filters (seen/tracking/wishlist/downloaded/played),
  // version, updates, dupe pairing. Media concepts do not appear here.
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    addSource,
    checkUpdates,
    clearItemSuggestions,
    deleteItems,
    dismissSuggestion,
    findMatches,
    getKindStats,
    getStats,
    getThumbs,
    importGameExe,
    importGameFolder,
    importGameZip,
    listItems,
    listMatchSuggestions,
    listUpdates,
    mergeItems,
    dismissPair,
    pairingSuggestions,
    reEnrichItem,
    reEnrichItems,
    sourceStats,
    topTags,
    type AppStatus,
    type GameImportReport,
    type ItemSummary,
    type MatchSuggestion,
    type PairSuggestion,
    type SourceStats,
    type StatusCount,
    type TagCount,
    type UpdateRow,
  } from "../lib/api";
  import { jobState, onActivityChange } from "../lib/activity.svelte";
  import ConfirmDialog from "../components/ConfirmDialog.svelte";
  import ItemEditor from "../components/ItemEditor.svelte";
  import ReportBar from "../components/ReportBar.svelte";
  import TileGrid from "../components/TileGrid.svelte";

  let { status }: { status: AppStatus } = $props();

  let items = $state<ItemSummary[]>([]);
  let stats = $state<StatusCount[]>([]);
  let tags = $state<TagCount[]>([]);
  let thumbs = $state<Record<string, string>>({});
  let gameCount = $state(0);
  let search = $state("");
  let filter = $state("all");
  let tagFilter = $state<string | null>(null);
  // #40: source-state filters — local-only games are the "add links/tags" queue,
  // no-local games lost (or never had) their folder on disk.
  let sourceFilter = $state<"local_only" | "no_local" | "no_tags" | null>(null);
  let srcStats = $state<SourceStats>({ localOnly: 0, noLocal: 0, noTags: 0 });
  let panel = $state<"library" | "updates" | "pairs" | "matches">("library");
  let report = $state<string | null>(null);
  let gameFailures = $state<string[]>([]);
  let loadError = $state<string | null>(null);
  let updates = $state<UpdateRow[]>([]);
  let updateNote = $state<string | null>(null);
  let pairNote = $state<string | null>(null);
  let pairs = $state<PairSuggestion[]>([]);
  let checkingUpdates = $state(false);
  let matches = $state<MatchSuggestion[]>([]);
  let matchNote = $state<string | null>(null);
  let attaching = $state<string | null>(null);
  let selected = $state<ItemSummary | null>(null);

  const importJob = $derived(jobState("games"));
  const updatesJob = $derived(jobState("updates"));
  const matchesJob = $derived(jobState("matches"));
  const filtered = $derived(
    filter === "all" ? items : items.filter((i) => i.status === filter),
  );
  const updateCount = $derived(updates.length);

  // debounced search (#8)
  let searchFor = $state("");
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  function onSearch(v: string) {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => (searchFor = v.trim()), 300);
  }

  // batch selection (#12)
  let selectMode = $state(false);
  let selectedIds = $state<Set<string>>(new Set());
  let confirmBatchDelete = $state(false);

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

  async function load() {
    loadError = null;
    try {
      stats = await getStats("game");
      items = await listItems(
        300,
        0,
        searchFor || undefined,
        "game",
        tagFilter ?? undefined,
        undefined,
        sourceFilter ?? undefined,
      );
      const t = await getThumbs(items.filter((i) => i.hasThumb).map((i) => i.id));
      thumbs = Object.fromEntries(t.map((x) => [x.id, `data:${x.mime};base64,${x.data}`]));
      const kinds = await getKindStats();
      gameCount = Object.fromEntries(kinds)["game"] ?? 0;
      tags = await topTags(24, "game");
      updates = await listUpdates(100);
      srcStats = await sourceStats("game");
    } catch (e) {
      // Never swallow silently — a blank grid with no reason is how M0 lied
      // about successful imports.
      items = [];
      stats = [];
      thumbs = {};
      loadError = `games could not be loaded: ${e}`;
    }
  }

  $effect(() => {
    filter;
    tagFilter;
    sourceFilter;
    searchFor;
    void load();
  });

  // re-read whenever job state moves, so cards arrive one by one and the final
  // batch lands the moment an import finishes
  onMount(() => onActivityChange(() => void load()));

  function countFor(key: string): number {
    return stats.find((s) => s.status === key)?.count ?? 0;
  }

  function gamesReport(r: GameImportReport): string {
    gameFailures = r.failures;
    return (
      `games: ${r.imported} imported (${r.withThumb} with cover)` +
      (r.skipped ? `, ${r.skipped} already in vault` : "") +
      (r.stopped ? ", stopped early — everything before that is stored" : "") +
      (r.failures.length ? `, ${r.failures.length} failed (see details)` : "")
    );
  }

  async function pickAndImport(
    opts: Parameters<typeof open>[0],
    run: (path: string) => Promise<GameImportReport>,
    failLabel: string,
  ) {
    const path = await open(opts);
    if (typeof path !== "string") return;
    report = null;
    try {
      report = gamesReport(await run(path));
      await load();
    } catch (e) {
      report = `${failLabel}: ${e}`;
    }
  }

  const doImportGames = () =>
    pickAndImport(
      { directory: true, title: "Folder containing your games" },
      importGameFolder,
      "game import failed",
    );

  const doImportZip = () =>
    pickAndImport(
      { title: "Zipped game (repack .zip)", filters: [{ name: "Zip archive", extensions: ["zip"] }] },
      importGameZip,
      "zip import failed",
    );

  const doImportExe = () =>
    pickAndImport(
      { title: "The game's .exe", filters: [{ name: "Windows program", extensions: ["exe"] }] },
      importGameExe,
      "game import failed",
    );

  async function doCheckUpdates() {
    if (checkingUpdates) return;
    checkingUpdates = true;
    updateNote = "checking each tracked game once, respecting site rate limits…";
    try {
      const r = await checkUpdates();
      updateNote =
        `checked ${r.checked} source links` +
        (r.updated ? ` → ${r.updated} updated: ${r.updatedTitles.join(", ")}` : " → nothing new") +
        (r.failures ? ` (${r.failures} could not be reached)` : "") +
        (r.stopped ? " (stopped)" : "");
      await load();
    } catch (e) {
      updateNote = `update check failed: ${e}`;
    } finally {
      checkingUpdates = false;
    }
  }

  function openPairs() {
    panel = "pairs";
    pairNote = null;
    pairingSuggestions("game")
      .then((p) => (pairs = p))
      .catch((e) => (pairNote = String(e)));
  }

  async function doMerge(s: PairSuggestion, keep: "a" | "b") {
    try {
      await mergeItems(keep === "a" ? s.aId : s.bId, keep === "a" ? s.bId : s.aId);
      pairs = await pairingSuggestions();
      pairNote = `merged into "${keep === "a" ? s.aTitle : s.bTitle}"`;
      await load();
    } catch (e) {
      pairNote = `merge failed: ${e}`;
    }
  }

  async function doKeepBoth(p: PairSuggestion) {
    try {
      await dismissPair(p.aId, p.bId);
      pairs = await pairingSuggestions("game");
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

  // ---- online match suggestions (#14) ----

  function openMatches() {
    panel = "matches";
    matchNote = null;
    listMatchSuggestions()
      .then((m) => (matches = m))
      .catch((e) => (matchNote = String(e)));
  }

  async function doFindMatches() {
    if (matchesJob === "running") return;
    matchNote = "searching DLsite (and f95 with a login cookie) for your local-only games…";
    try {
      const r = await findMatches();
      matchNote =
        `scanned ${r.scanned} local-only game${r.scanned === 1 ? "" : "s"} → ` +
        (r.suggestions
          ? `${r.suggestions} suggestions for ${r.withSuggestions} game${r.withSuggestions === 1 ? "" : "s"}`
          : "no confident matches") +
        (r.failures.length ? ` (${r.failures.length} site problems — see below)` : "") +
        (r.stopped ? " (stopped)" : "");
      matches = await listMatchSuggestions();
      await load();
    } catch (e) {
      matchNote = `match search failed: ${e}`;
    }
  }

  async function doAttach(s: MatchSuggestion) {
    if (attaching) return;
    attaching = `${s.site}|${s.url}`;
    try {
      await addSource(s.itemId, s.url);
      await clearItemSuggestions(s.itemId);
      matchNote = `linked ${s.site} to "${s.itemTitle}" — fetching its metadata…`;
      try {
        await reEnrichItem(s.itemId); // job: cover + title/version arrive in the strip
      } catch {
        // enrich job busy — the user can re-fetch from the item editor
        matchNote = `linked ${s.site} to "${s.itemTitle}" — press Re-fetch metadata in its editor when the queue is free`;
      }
      matches = await listMatchSuggestions();
      await load();
    } catch (e) {
      matchNote = `attach failed: ${e}`;
    } finally {
      attaching = null;
    }
  }

  async function doDismiss(s: MatchSuggestion) {
    try {
      await dismissSuggestion(s.itemId, s.site, s.url);
      matches = matches.filter((m) => !(m.itemId === s.itemId && m.site === s.site && m.url === s.url));
    } catch (e) {
      matchNote = `dismiss failed: ${e}`;
    }
  }

  function when(epoch: string): string {
    const n = Number(epoch);
    return Number.isFinite(n) && n > 0 ? new Date(n * 1000).toLocaleString() : "—";
  }
</script>

<div class="view-toolbar">
  <button disabled={importJob === "running"} onclick={doImportGames} title="Scans the folder (nested layouts OK) and stores a pointer to each game">
    + Game folder
  </button>
  <button disabled={importJob === "running"} onclick={doImportZip} title="Fingerprints the engine inside the zip; the zip stays where it is">
    + Game zip
  </button>
  <button disabled={importJob === "running"} onclick={doImportExe} title="Point straight at the launcher; its folder becomes the game">
    + Game .exe
  </button>
  <input
    class="search-box"
    type="text"
    placeholder="search title / circle…"
    aria-label="Search games"
    value={search}
    oninput={(e) => onSearch(e.currentTarget.value)}
  />
  <span class="spacer"></span>
  {#if selectMode}
    <span class="hint">{selectedIds.size} selected</span>
    <button onclick={() => (selectedIds = new Set(filtered.map((i) => i.id)))}>Select all</button>
    <button disabled={refetching || selectedIds.size === 0} onclick={doBatchRefetch} title="re-fetch online info for the selection — runs as a job you can stop">
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
  {#if importJob === "running"}
    <span class="hint">import running — other buttons stay usable, watch the strip above</span>
  {/if}
</div>

{#if report}
  <ReportBar text={report} onDismiss={() => (report = null)} />
  {#if gameFailures.length > 0}
    <div class="report">
      <details class="details">
        <summary>details ({gameFailures.length} problems)</summary>
        <ul>
          {#each gameFailures as f}
            <li>{f}</li>
          {/each}
        </ul>
      </details>
    </div>
  {/if}
{/if}
{#if loadError}
  <ReportBar text={loadError} tone="error" onDismiss={() => (loadError = null)} />
{/if}

<div class="main">
  <nav class="sidebar">
    <div class="section">Flow</div>
    <button class="nav-item" class:active={panel === "library" && filter === "all" && !sourceFilter} onclick={() => { panel = "library"; filter = "all"; sourceFilter = null; }}>
      Everything <span class="count">{gameCount}</span>
    </button>
    {#each ["seen", "tracking", "wishlist", "downloaded", "played"] as key}
      <button class="nav-item" class:active={panel === "library" && filter === key} onclick={() => { panel = "library"; filter = key; }}>
        {key} <span class="count">{countFor(key)}</span>
      </button>
    {/each}

    <div class="section">
      Sources
      {#if sourceFilter}
        <button class="link" onclick={() => (sourceFilter = null)}>clear</button>
      {/if}
    </div>
    <button class="nav-item" class:active={sourceFilter === "local_only"} onclick={() => (sourceFilter = sourceFilter === "local_only" ? null : "local_only")} title="Games with only a local folder and no online source — the queue for adding links and tags by hand.">
      Local only <span class="count">{srcStats.localOnly || ""}</span>
    </button>
    <button class="nav-item" class:active={sourceFilter === "no_local"} onclick={() => (sourceFilter = sourceFilter === "no_local" ? null : "no_local")} title="Games with no folder on disk — the link is all that's left.">
      No local file <span class="count">{srcStats.noLocal || ""}</span>
    </button>
    <button class="nav-item" class:active={sourceFilter === "no_tags"} onclick={() => (sourceFilter = sourceFilter === "no_tags" ? null : "no_tags")} title="Games with no tags at all.">
      No tags <span class="count">{srcStats.noTags || ""}</span>
    </button>

    <div class="section">
      Tagged
      {#if tagFilter}
        <button class="link" onclick={() => (tagFilter = null)}>clear</button>
      {/if}
    </div>
    {#if tags.length === 0}
      <div class="vault-info">No game tags yet. Clips and enriched bookmarks bring tags in; you can add your own on any item.</div>
    {:else}
      {#each tags as t (t.id)}
        <button class="nav-item" class:active={tagFilter === t.name} onclick={() => (tagFilter = tagFilter === t.name ? null : t.name)} title={t.typeKey}>
          {t.name} <span class="count">{t.count}</span>
        </button>
      {/each}
    {/if}

    <div class="section">Keep current</div>
    <button class="nav-item" class:active={panel === "updates"} onclick={() => (panel = "updates")}>
      Updates <span class="count">{updateCount}</span>
    </button>
    <button class="nav-item" class:active={panel === "pairs"} onclick={openPairs}>
      Possible dupes <span class="count">{pairs.length || ""}</span>
    </button>
    <button class="nav-item" class:active={panel === "matches"} onclick={openMatches}>
      Online matches <span class="count">{matches.length || ""}</span>
    </button>

    <div class="section">Vault</div>
    <div class="vault-info">{status.vaultRoot}</div>
  </nav>

  <main class="content">
    {#if panel === "updates"}
      <h2 class="panel-title">Updates</h2>
      <div class="row">
        <button class="primary" disabled={updatesJob === "running" || checkingUpdates} onclick={doCheckUpdates}>
          {updatesJob === "running" ? "Checking… (see strip)" : "Check updates now"}
        </button>
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
              if (item) selected = item;
            }}>{u.title}</button>
            <span class="hint">
              {u.site}{u.oldVersion ? ` v${u.oldVersion}` : ""} → {u.newVersion ? `v${u.newVersion}` : "?"} · {when(u.seenAt)}
            </span>
            {#if u.hasMods}
              <span
                class="chip"
                title="This game has Mods & tools registered. A new build lands on top of that folder — anything deployed inside it (DLL mods, MTool files) gets clobbered. Clean up before updating, re-apply after."
              >⚠ mods on this game</span>
            {/if}
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

    {:else if panel === "matches"}
      <h2 class="panel-title">Online matches for local games</h2>
      <p class="hint">Games tracked only by a local folder are matched against DLsite's and f95zone's own search.
        f95 search needs its login cookie (Sources panel). Nothing links itself — you accept a match or dismiss it.</p>
      <div class="row">
        <button class="primary" disabled={matchesJob === "running"} onclick={doFindMatches}>
          {matchesJob === "running" ? "Searching… (see strip)" : "Find online matches"}
        </button>
        <span class="hint">Only games with no online source yet; polite per-site rate limits apply.</span>
      </div>
      {#if matchNote}
        <p class="report-inline">{matchNote}</p>
      {/if}
      {#if matches.length === 0}
        <div class="empty"><strong>No open suggestions.</strong><br />Press <em>Find online matches</em> —
          imported games are matched by title once <code>[v0.12]</code> / <code>[renpy]</code> noise is stripped,
          and only confident hits (≥60%) are proposed.</div>
      {:else}
        {#each matches as s (`${s.itemId}-${s.site}-${s.url}`)}
          <div class="pair-row">
            <div>
              <strong>{s.itemTitle}</strong> <span class="hint">local</span>
              <br />
              → <strong>{s.title}</strong>
              <span class="hint">
                {s.site} · {Math.round(s.confidence * 100)}% match · {when(s.foundAt)}
              </span>
            </div>
            <div class="row">
              <button class="primary" disabled={attaching !== null} onclick={() => doAttach(s)}>Attach + fetch</button>
              <button class="ghost" onclick={() => doDismiss(s)}>Dismiss</button>
            </div>
          </div>
        {/each}
      {/if}

    {:else if filtered.length === 0}
      <div class="empty">
        <strong>{loadError ? "Couldn't load games." : searchFor ? `Nothing matches "${searchFor}".` : tagFilter ? `Nothing tagged "${tagFilter}".` : "No games in this view."}</strong><br />
        Import games with <em>+ Game folder</em> (nested layouts OK), <em>+ Game zip</em>
        (zipped repacks), or <em>+ Game .exe</em> (point straight at the launcher).
        <br /><br />
        <span class="hint">Everything imported is editable — click any card to correct it.</span>
      </div>
    {:else}
      <TileGrid
        items={filtered}
        {thumbs}
        {selectMode}
        selected={selectedIds}
        onOpen={(item) => (selected = item)}
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

{#if confirmBatchDelete}
  <ConfirmDialog
    title="Remove selected from vault"
    message={`Remove ${selectedIds.size} entries? Game folders on disk are never touched.`}
    confirmLabel="Remove {selectedIds.size}"
    onConfirm={doBatchDelete}
    onCancel={() => (confirmBatchDelete = false)}
  />
{/if}
