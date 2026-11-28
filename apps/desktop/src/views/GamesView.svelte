<script lang="ts">
  // Games view: one grammar box for the pipeline (`status:`), tags, provenance and the
  // three #40 queues; version, updates, dupe pairing, online matches. No media concepts
  // here beyond the file facts the grammar shares (D14 is enforced in SQL, not here).
  //
  // #76 stage S6: the four sidebar rails (Five pipeline stages, Sources queues, Tagged
  // cloud, plus the client-side `filtered` hack they drove) are gone. The box is the
  // filter, the chips are a view of the box, and the counts the rails wore live inside
  // autocomplete rows scoped to the query minus the token being typed — the same shape
  // the media view got in S5. The grid pages by cursor and its actions reach the whole
  // match set, not the loaded page (rulings C).
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    addSource,
    checkUpdates,
    clearItemSuggestions,
    deleteItems,
    dismissSuggestion,
    filterSuggest,
    findMatches,
    getKindStats,
    getThumbs,
    importGameExe,
    importGameFolder,
    importGameZip,
    listMatchSuggestions,
    listUpdates,
    mergeItems,
    dismissPair,
    pairingSuggestions,
    reEnrichItem,
    reEnrichItems,
    searchItemIds,
    searchItems,
    type AppStatus,
    type GameImportReport,
    type ItemSummary,
    type MatchSuggestion,
    type PageCursor,
    type PairSuggestion,
    type Suggestion,
    type UpdateRow,
  } from "../lib/api";
  import { bad, info } from "../lib/toast.svelte";
  import { evictMedia } from "../lib/mediacache.svelte";
  import { jobState, onActivityChange } from "../lib/activity.svelte";
  import ConfirmDialog from "../components/ConfirmDialog.svelte";
  import ItemEditor from "../components/ItemEditor.svelte";
  import ReportBar from "../components/ReportBar.svelte";
  import TileGrid from "../components/TileGrid.svelte";

  let { status }: { status: AppStatus } = $props();

  let items = $state<ItemSummary[]>([]);
  let thumbs = $state<Record<string, string>>({});
  let panel = $state<"library" | "updates" | "pairs" | "matches">("library");
  let report = $state<string | null>(null);
  let gameFailures = $state<string[]>([]);
  let loadError = $state<string | null>(null);
  let updates = $state<UpdateRow[]>([]);
  let pairs = $state<PairSuggestion[]>([]);
  let checkingUpdates = $state(false);
  let matches = $state<MatchSuggestion[]>([]);
  let attaching = $state<string | null>(null);
  let selected = $state<ItemSummary | null>(null);

  // batch selection (#12)
  let selectMode = $state(false);
  let selectedIds = $state<Set<string>>(new Set());
  let confirmBatchDelete = $state(false);

  const importJob = $derived(jobState("games"));
  const updatesJob = $derived(jobState("updates"));
  const matchesJob = $derived(jobState("matches"));
  const updateCount = $derived(updates.length);

  // ---------- the query box (#76 stage S6) ----------
  // One text box is the source of truth; the chips are a *view* of it, so a chip click
  // edits the text rather than setting a second filter state (plan §5). The old view had
  // four of those parallel states — `filter`, `tagFilter`, `sourceFilter`, and the
  // `filtered` derived that applied one of them in the browser after fetching 300 rows.
  const PAGE = 300;
  let text = $state("");
  let committed = $state("");
  let queryError = $state<string | null>(null);
  let total = $state<number | null>(null);
  let needsFacts = $state(false);
  let shapes = $state<string[]>([]);
  let hasMore = $state(false);
  let libraryCount = $state(0);
  let suggest = $state<Suggestion[]>([]);
  // A dense tag page can outlive the keystroke that started it, so the response that is
  // applied has to be the one the current query asked for.
  let seq = 0;

  // The debounce is not decoration, and neither is the split: at 100k items the count is
  // a second statement over the whole result (46-151 ms measured), so a tick re-runs the
  // page and the total belongs to the query the user commits to with Enter or blur.
  let timer: ReturnType<typeof setTimeout> | undefined;
  function retype() {
    clearTimeout(timer);
    timer = setTimeout(() => {
      const next = text.trim();
      if (next === committed) return;
      committed = next;
      // A keystroke page has no total, and it must not show the previous query's number
      // either: the total is dropped the moment the query changes (invariant 27).
      total = null;
      needsFacts = false;
      shapes = [];
      void runQuery(next === "");
    }, 300);
  }

  /** A query is one or more tokens separated by spaces; a quoted phrase ("summer witch",
   *  circle:"Red Garden") is a single token and stays quoted, or the chip row would write
   *  back something that no longer parses. */
  const TOKENS = /"[^"]*"|\S+/g;
  const tokens = (): string[] => text.match(TOKENS) ?? [];

  function rewrite(next: string) {
    text = next.replace(/\s+/g, " ").trim();
    retype();
  }

  function removeTerm(term: string) {
    const kept = tokens();
    const at = kept.indexOf(term);
    if (at < 0) return;
    kept.splice(at, 1);
    rewrite(kept.join(" "));
  }

  function insertTerm(term: string) {
    const all = tokens();
    if (!all.includes(term)) all.push(term);
    rewrite(all.join(" "));
  }

  /** A preset is one click and visible in the grammar: it writes (or clears) the
   *  predicate, it does not keep a parallel filter state of its own. These are the three
   *  #40 queues the old Sources rail wore as badges. */
  const PRESETS: { label: string; term: string; why: string }[] = [
    {
      label: "no tags",
      term: "-has:tags",
      why: "games with no tags at all — the queue for tagging by hand",
    },
    {
      label: "local only",
      term: "stored:local-only",
      why: "a folder on disk and no online source — the queue for adding links and tags",
    },
    {
      label: "no local file",
      term: "-stored:disk",
      why: "no folder in the library and nothing stored: the link is all that is left",
    },
  ];

  /** Two `status:` terms cannot both be true: `items.status` is one column, so
   *  `status:seen status:played` is an empty grid by construction, not a search that
   *  found nothing. Same reasoning as the media view's `ext:` (an item carries exactly
   *  one extension tag). `stored:` is left alone because an item can be `stored:vault`
   *  *and* `stored:disk`. */
  const SINGLE_VALUED = ["ext", "status"];
  const conflict = $derived.by(() => {
    const byKey = new Map<string, string[]>();
    for (const raw of tokens()) {
      const negated = raw.startsWith("-");
      const tok = negated ? raw.slice(1) : raw;
      const m = /^([a-z]+):/.exec(tok);
      if (!m || negated) continue;
      if (!SINGLE_VALUED.includes(m[1])) continue;
      const seen = byKey.get(m[1]) ?? [];
      if (!seen.includes(tok)) seen.push(tok);
      byKey.set(m[1], seen);
    }
    for (const [key, terms] of byKey) {
      if (terms.length > 1) {
        return `${terms.join(" ")} cannot both be true: an item has exactly one \`${key}:\` value, so this matches nothing`;
      }
    }
    return null;
  });

  const cursor = $derived<PageCursor | undefined>(
    items.length
      ? { updatedAt: items[items.length - 1].updatedAt, id: items[items.length - 1].id }
      : undefined,
  );
  const querying = $derived(text.trim() !== "");

  async function runQuery(withCount: boolean, append = false) {
    queryError = null;
    const mine = ++seq;
    try {
      const page = await searchItems(
        "games",
        committed || undefined,
        PAGE,
        append ? cursor : undefined,
        withCount,
      );
      if (mine !== seq) return;
      items = append ? [...items, ...page.items] : page.items;
      // A page never changes the query, so `withCount: false` answering `total: null`
      // must not wipe a total this query already paid for (invariant 27).
      if (page.total !== null) total = page.total;
      needsFacts = page.needsFacts;
      shapes = page.shapes;
      // When the total is known the count is the authority: offering *Load more* over a
      // result the count says is exhausted is a button that can only ever do nothing.
      hasMore = page.items.length === PAGE && (page.total === null || items.length < page.total);
      await loadThumbs();
    } catch (e) {
      if (mine !== seq) return;
      // A parse error is form state: inline under the box, un-dismissed, readable a
      // minute later while the box still holds the mistake (invariant 24). The grid goes
      // empty rather than keeping the last good page under a sentence that says the
      // query is broken.
      if (append) hasMore = false;
      else {
        items = [];
        total = null;
        needsFacts = false;
        shapes = [];
        hasMore = false;
      }
      queryError = String(e);
    }
  }

  /** Enter (or leaving the box): the query is settled, so this is where the total is
   *  paid for. */
  function commitQuery() {
    clearTimeout(timer);
    committed = text.trim();
    void runQuery(true);
  }

  function loadMore() {
    if (hasMore) void runQuery(false, true);
  }

  function backToStart() {
    // The same query, only page 1 again, so the total is already paid for.
    void runQuery(total === null);
  }

  /** The autocomplete reads the box as the parser does: the query minus the token under
   *  the caret, and that token. This is where the old rail counts live now — the five
   *  pipeline stages included, each counted over the query minus the token being typed. */
  let suggestSeq = 0;
  async function suggestFor(value: string) {
    const mine = ++suggestSeq;
    try {
      const at = value.lastIndexOf(" ");
      const tail = value.slice(at + 1);
      if (!tail || tail.includes('"')) {
        suggest = [];
        return;
      }
      const rows = await filterSuggest("games", value);
      if (mine !== suggestSeq) return; // the box moved while the call was in flight
      suggest = rows;
    } catch {
      if (mine === suggestSeq) suggest = [];
    }
  }

  function acceptSuggestion(s: Suggestion) {
    const at = text.lastIndexOf(" ");
    text = `${text.slice(0, at + 1)}${s.token} `;
    suggest = [];
    retype();
  }

  async function loadThumbs() {
    const want = items.filter((i) => i.hasThumb && !thumbs[i.id]).map((i) => i.id);
    if (want.length === 0) return;
    try {
      const t = await getThumbs(want);
      thumbs = {
        ...thumbs,
        ...Object.fromEntries(t.map((x) => [x.id, `data:${x.mime};base64,${x.data}`])),
      };
    } catch {
      // A tile is decoration; the grid and its counts stand without it.
    }
  }

  /** The `M` in "N matches · M total": how many games the vault holds. It is the number
   *  the old "Everything" rail wore, and it is one statement. */
  async function loadLibraryCount() {
    try {
      const kinds = Object.fromEntries(await getKindStats());
      libraryCount = kinds["game"] ?? 0;
    } catch (e) {
      loadError = `games could not be counted: ${e}`;
    }
  }

  /** The full read: page + total, the library count, and the update list. Called on mount
   *  and whenever a job moves, which is how imported cards (and their covers) arrive one
   *  by one. No facet count is loaded here at all: it is paid for inside an autocomplete
   *  row, scoped to the query already typed (plan §5). */
  async function load() {
    loadError = null;
    committed = text.trim();
    await runQuery(true);
    await loadLibraryCount();
    try {
      updates = await listUpdates(100);
    } catch (e) {
      loadError = `updates could not be read: ${e}`;
    }
  }

  // re-read whenever job state moves, so cards (and their covers) arrive one by one
  onMount(() => {
    void load();
    return onActivityChange(() => void load());
  });

  function toggleSelect(item: ItemSummary) {
    const next = new Set(selectedIds);
    if (next.has(item.id)) next.delete(item.id);
    else next.add(item.id);
    selectedIds = next;
  }

  /** Select-all means the whole match set, not the 300 rows on screen: the grid is a
   *  query now, so the bulk actions have to reach everything the predicate matches. */
  async function selectAllMatches() {
    try {
      const ids = await searchItemIds("games", committed || undefined);
      selectedIds = new Set(ids);
    } catch (e) {
      report = `select all failed: ${e}`;
    }
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
    info("checking each tracked game once, respecting site rate limits…");
    try {
      const r = await checkUpdates();
      info(
        `checked ${r.checked} source links` +
        (r.updated ? ` → ${r.updated} updated: ${r.updatedTitles.join(", ")}` : " → nothing new") +
        (r.failures ? ` (${r.failures} could not be reached)` : "") +
        (r.stopped ? " (stopped)" : ""));
      await load();
    } catch (e) {
      bad(`update check failed: ${e}`);
    } finally {
      checkingUpdates = false;
    }
  }

  function openPairs() {
    panel = "pairs";
    pairingSuggestions("game")
      .then((p) => (pairs = p))
      .catch((e) => bad(String(e)));
  }

  async function doMerge(s: PairSuggestion, keep: "a" | "b") {
    try {
      await mergeItems(keep === "a" ? s.aId : s.bId, keep === "a" ? s.bId : s.aId);
      pairs = await pairingSuggestions();
      info(`merged into "${keep === "a" ? s.aTitle : s.bTitle}"`);
      await load();
    } catch (e) {
      bad(`merge failed: ${e}`);
    }
  }

  async function doKeepBoth(p: PairSuggestion) {
    try {
      await dismissPair(p.aId, p.bId);
      pairs = await pairingSuggestions("game");
      info("kept both — this pair will not be suggested again");
    } catch (e) {
      bad(`could not dismiss: ${e}`);
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

  async function doBatchDelete() {
    const ids = [...selectedIds];
    confirmBatchDelete = false;
    if (ids.length === 0) return;
    try {
      await deleteItems(ids);
      evictMedia(ids);
      selectedIds = new Set();
      selectMode = false;
      await load();
    } catch (e) {
      report = `batch delete failed: ${e}`;
    }
  }

  // ---- online match suggestions (#14) ----

  function openMatches() {
    panel = "matches";
    listMatchSuggestions()
      .then((m) => (matches = m))
      .catch((e) => bad(String(e)));
  }

  async function doFindMatches() {
    if (matchesJob === "running") return;
    info("searching DLsite (and f95 with a login cookie) for your local-only games…");
    try {
      const r = await findMatches();
      info(
        `scanned ${r.scanned} local-only game${r.scanned === 1 ? "" : "s"} → ` +
        (r.suggestions
          ? `${r.suggestions} suggestions for ${r.withSuggestions} game${r.withSuggestions === 1 ? "" : "s"}`
          : "no confident matches") +
        (r.failures.length ? ` (${r.failures.length} site problems — see below)` : "") +
        (r.stopped ? " (stopped)" : ""));
      matches = await listMatchSuggestions();
      await load();
    } catch (e) {
      bad(`match search failed: ${e}`);
    }
  }

  async function doAttach(s: MatchSuggestion) {
    if (attaching) return;
    attaching = `${s.site}|${s.url}`;
    try {
      await addSource(s.itemId, s.url);
      await clearItemSuggestions(s.itemId);
      info(`linked ${s.site} to "${s.itemTitle}" — fetching its metadata…`);
      try {
        await reEnrichItem(s.itemId); // job: cover + title/version arrive in the strip
      } catch {
        // enrich job busy — the user can re-fetch from the item editor
        info(`linked ${s.site} to "${s.itemTitle}" — press Re-fetch metadata in its editor when the queue is free`);
      }
      matches = await listMatchSuggestions();
      await load();
    } catch (e) {
      bad(`attach failed: ${e}`);
    } finally {
      attaching = null;
    }
  }

  async function doDismiss(s: MatchSuggestion) {
    try {
      await dismissSuggestion(s.itemId, s.site, s.url);
      matches = matches.filter((m) => !(m.itemId === s.itemId && m.site === s.site && m.url === s.url));
    } catch (e) {
      bad(`dismiss failed: ${e}`);
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
    class="search-box query-box"
    type="text"
    placeholder='status:wishlist "summer witch" renpy -stored:disk'
    aria-label="Query the games library"
    aria-autocomplete="list"
    bind:value={text}
    oninput={(e) => {
      retype();
      void suggestFor(e.currentTarget.value);
    }}
    onkeydown={(e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        commitQuery();
      } else if (e.key === "Escape") {
        suggest = [];
      }
    }}
    onblur={() => {
      suggest = [];
      if (text.trim() !== committed) commitQuery();
    }}
  />
  <span class="spacer"></span>
  {#if selectMode}
    <span class="hint">{selectedIds.size} selected</span>
    <button onclick={() => void selectAllMatches()}>Select all</button>
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

{#if panel === "library"}
  {#if queryError}
    <p class="query-error">{queryError}</p>
  {/if}
  {#if conflict}
    <p class="query-conflict">{conflict}</p>
  {/if}

  <div class="query-presets">
    <span class="hint">presets</span>
    {#each PRESETS as p (p.term)}
      <button
        class="preset"
        class:on={tokens().includes(p.term)}
        title={p.why}
        onclick={() => (tokens().includes(p.term) ? removeTerm(p.term) : insertTerm(p.term))}
      >
        {p.label}
      </button>
    {/each}
  </div>

  {#if suggest.length > 0}
    <div class="query-suggest">
      {#each suggest as s (s.token)}
        <button
          class="suggest"
          aria-label="use {s.token}"
          title={s.count === null
            ? "grammar, not a count from this vault"
            : s.token.startsWith("-")
              ? `${s.count} items carry this, and those are the rows that leave the grid`
              : `${s.count} games match this on top of the query already typed`}
          onclick={() => acceptSuggestion(s)}
        >
          <code>{s.token}</code>
          <span class="suggest-count">{s.count === null ? "" : s.count}</span>
        </button>
      {/each}
    </div>
  {/if}

  <div class="query-chips">
    {#if querying}
      <span class="hint">filters</span>
      {#each tokens() as term (term)}
        <button class="chip" title="click to remove this term from the box" onclick={() => removeTerm(term)}>
          {term}<span class="chip-x" aria-hidden="true">×</span>
        </button>
      {/each}
      <button class="link" onclick={() => rewrite("")}>clear</button>
    {:else}
      <span class="hint">no filters — the whole games library</span>
    {/if}
    <span class="spacer"></span>
    <span class="query-count">
      {#if total === null}
        … matches
      {:else if needsFacts && total === 0}
        not measured yet
      {:else}
        {total} match{total === 1 ? "" : "es"}
      {/if}
      {#if libraryCount > 0} · {libraryCount} in the library{/if}
    </span>
    {#if shapes.length > 0}
      <details class="details query-diagnostic">
        <summary>how this query ran ({shapes.length} tag term{shapes.length === 1 ? "" : "s"})</summary>
        <ul>
          {#each shapes as note (note)}
            <li><code>{note}</code></li>
          {/each}
        </ul>
      </details>
    {/if}
  </div>
{/if}

<div class="main">
  <nav class="sidebar">
    <div class="section">Library</div>
    <button class="nav-item" class:active={panel === "library"} onclick={() => (panel = "library")}>
      Everything <span class="count">{libraryCount}</span>
    </button>
    <div class="hint">The five pipeline stages are grammar now: type <code>status:</code> in
      the box, or pick one from the rows it offers — each row wears the count for the query
      already typed. A stage is a term like any other, so it composes
      (<code>status:wishlist renpy</code>), and a stage outside the five the grammar knows is
      not a term at all.</div>

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
    <div class="hint">The query box is per session and is never saved to disk, and it is
      not case-folded for tags: a tag matches exactly as it is spelled.</div>
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

    {:else if needsFacts && items.length === 0 && conflict === null}
      <div class="empty facts-row">
        <strong>Not measured yet.</strong><br />
        This query asks about facts the vault has never read off the files — sizes, durations,
        picture dimensions. A NULL fact never matches, so this is not "nothing has it", it is
        "nobody has looked".
        <br /><br />
        <button disabled title="the facts pass is stage S7 of #76: one cancellable job that reads the vault's own files">
          Measure my files
        </button>
      </div>
    {:else if items.length === 0}
      <div class="empty">
        <strong>{queryError
          ? "Nothing is shown: the box does not parse."
          : conflict
            ? "Nothing is shown: the box says this cannot match."
            : querying
              ? `Nothing matches "${committed}".`
              : "No games in the vault."}</strong><br />
        {#if conflict}
          {conflict} A fact that was never measured would read *Not measured yet* instead: a
          NULL column never matches, so an unmeasured vault is not a vault without it.
          <br /><br />
        {/if}
        {#if querying}
          A stage is one word — <code>status:wishlist</code>, and it composes with the rest.
          A bare word is one tag, <code>tag:"…"</code> is an exact name including its namespace,
          a quoted phrase searches titles and circles, and <code>source:dlsite</code> pins
          provenance. Tags match exactly, case and all: <code>RenPy</code> finds nothing where
          <code>renpy</code> does. An item has exactly one <code>status:</code> value, so
          <code>status:seen status:played</code> is empty by construction.
          <br /><br />
        {/if}
        Import games with <em>+ Game folder</em> (nested layouts OK), <em>+ Game zip</em>
        (zipped repacks), or <em>+ Game .exe</em> (point straight at the launcher).
        <br /><br />
        <span class="hint">Everything imported is editable — click any card to correct it.</span>
      </div>
    {:else}
      <TileGrid
        items={items}
        {thumbs}
        {selectMode}
        selected={selectedIds}
        onOpen={(item) => (selected = item)}
        onToggle={toggleSelect}
      />
      <div class="query-paging">
        <button disabled={!hasMore} onclick={loadMore} title="the next page starts after the last row on screen — a cursor, not a row count, so a delete behind you cannot skip rows">
          {hasMore ? "Load more" : "That is all of it"}
        </button>
        {#if items.length > PAGE}
          <button class="ghost" onclick={backToStart} title="back to the first page">Back to start</button>
        {/if}
        <span class="hint">
          showing {items.length}{total === null ? "" : ` of ${total}`}
          {#if total === null}(press Enter for the total){/if}
        </span>
      </div>
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
