<script lang="ts">
  // Media view: booru posts, galleries, stored images, local file links.
  // No pipeline filters here — seen/tracking/wishlist are game concepts (D14).
  // What matters for media is *the query box*: one grammar for tags, provenance,
  // storage shape and file facts (#76). The four facet rails (source-state badges,
  // sites, tag cloud) and their badge counts are gone: per-facet counts live inside
  // the autocomplete rows, scoped to the query already typed, and the three #40
  // queues survive as preset chips that write the equivalent predicate into the box.
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    deleteItems,
    filterSuggest,
    getKindStats,
    getThumbs,
    importBookmarks,
    importImageFiles,
    importVideoFiles,
    mergeItems,
    dismissPair,
    pairingSuggestions,
    reEnrichItems,
    searchItemIds,
    searchItems,
    type AppStatus,
    type FailureDetail,
    type ItemSummary,
    type PairSuggestion,
    type PageCursor,
    type Suggestion,
  } from "../lib/api";
  import { bad, info } from "../lib/toast.svelte";
  import { evictMedia } from "../lib/mediacache.svelte";
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
  let thumbs = $state<Record<string, string>>({});
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

  const imagesJob = $derived(jobState("images"));
  const videosJob = $derived(jobState("videos"));
  const bookmarksJob = $derived(jobState("bookmarks"));
  const pairCount = $derived(pairs.length);

  // ---------- the query box (#76 stage S5) ----------
  // One text box is the source of truth; the chips below are a *view* of it, so a
  // chip click edits the text rather than setting a second filter state (plan §5).
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
  // A dense tag page can outlive the keystroke that started it, so the response that
  // is applied has to be the one the *current* query asked for. Without this, a slow
  // `big_breasts` page lands on top of the empty grid the user typed after it.
  let seq = 0;

  // The debounce is not decoration: at 100k items one keystroke-replan of a dense
  // tag costs more than the page it re-runs, and the count is a second statement
  // over the whole result (46-151 ms). So a tick re-runs the **page** (withCount
  // false) and the total belongs to the query the user commits to with Enter or blur.
  let timer: ReturnType<typeof setTimeout> | undefined;
  function retype() {
    clearTimeout(timer);
    timer = setTimeout(() => {
      const next = text.trim();
      if (next === committed) return;
      committed = next;
      // A keystroke page has no total, and it must not show the previous query's
      // number either: the total is dropped the moment the query changes (inv 27).
      total = null;
      needsFacts = false;
      shapes = [];
      void runQuery(next === "");
    }, 300);
  }

  /** A query is one or more tokens separated by spaces; a quoted phrase ("summer
   *  witch", circle:"Red Garden") is a single token and stays quoted, or the chip
   *  row would write back something that no longer parses. */
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
   *  predicate, it does not keep a parallel filter state of its own. */
  const PRESETS: { label: string; term: string; why: string }[] = [
    {
      label: "no tags",
      term: "-has:tags",
      why: "items with no tags at all — bare file links and imports no post could be matched to",
    },
    {
      label: "local only",
      term: "stored:local-only",
      why: "a local file or folder and no online source — the queue for adding links and tags by hand",
    },
    {
      label: "no local file",
      term: "-stored:disk",
      why: "nothing stored in the vault and nothing on disk: links only",
    },
  ];

  /** `ext:gif ext:mp4` and friends cannot be satisfied: an item carries exactly one
   *  `extension` tag (`db::set_item_extension_tag`), so ANDing two of them is an empty
   *  grid by construction, not a search that found nothing. The grammar is right to
   *  AND — the box has to notice and say so. Only `ext:` is single-valued for certain:
   *  an item can be `stored:vault` *and* `stored:disk`, and a gallery item can hold a
   *  still and a video, so `stored:` and `type:` are left alone. */
  const SINGLE_VALUED = ["ext"];
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
        "media",
        committed || undefined,
        PAGE,
        append ? cursor : undefined,
        withCount,
      );
      if (mine !== seq) return;
      items = append ? [...items, ...page.items] : page.items;
      // A page never changes the query, so `withCount: false` answering `total: null`
      // must not wipe a total this query already paid for: the number on screen either
      // belongs to the query on screen or is not shown at all (invariant 27). Every
      // path that *does* change the query clears it first (`retype`, the error arm).
      if (page.total !== null) total = page.total;
      needsFacts = page.needsFacts;
      shapes = page.shapes;
      // A full page usually means another one exists, but when the total is known the
      // count is the authority: offering *Load more* over a result the count says is
      // exhausted is a button that can only ever do nothing.
      hasMore = page.items.length === PAGE && (page.total === null || items.length < page.total);
      await loadThumbs();
    } catch (e) {
      if (mine !== seq) return;
      // A parse error is form state: inline under the box, un-dismissed, readable a
      // minute later while the box still holds the mistake (invariant 24). It is not
      // a toast. The grid goes empty rather than keeping the last good page: rows
      // sitting under a sentence that says the query is broken would claim those rows
      // match something that does not parse.
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
   *  paid for. `withCount: true` is a second statement over the whole result — 46-151 ms
   *  at 100k items — which is exactly why it is not what a keystroke runs. */
  function commitQuery() {
    clearTimeout(timer);
    committed = text.trim();
    void runQuery(true);
  }

  function loadMore() {
    if (hasMore) void runQuery(false, true);
  }

  function backToStart() {
    // The same query, only page 1 again, so the total is already paid for and this
    // request does not buy a second one (`runQuery` keeps the number it already has).
    void runQuery(total === null);
  }

  /** The autocomplete reads the box as the parser does: the query minus the token under
   *  the caret, and that token. `filter_suggest` splits it again on its own side, so
   *  candidate counts are measured on top of the query already typed. No candidates is
   *  a normal answer (no partial token, or a box that does not parse yet). */
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
      const rows = await filterSuggest("media", value);
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

  /** The `M` in "N matches · M total": what the media library holds, from the kind
   *  counts. It is one statement, and it is the number the old "Everything" rail
   *  wore; the per-facet counts moved into the autocomplete rows. */
  async function loadLibraryCount() {
    try {
      const kinds = Object.fromEntries(await getKindStats());
      libraryCount = (kinds["gallery"] ?? 0) + (kinds["post"] ?? 0);
    } catch (e) {
      loadError = `media could not be counted: ${e}`;
    }
  }

  /** The full read: page + total, and the library count for the header line. Called on
   *  mount and whenever a job moves, which is how imported cards (and their art)
   *  arrive one by one. A facet count is no longer loaded here at all: it is paid for
   *  inside an autocomplete row, scoped to the query already typed (plan §5). */
  async function load() {
    loadError = null;
    committed = text.trim();
    await runQuery(true);
    await loadLibraryCount();
  }

  // re-read whenever job state moves, so cards (and their art) arrive one by one
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
   *  query now, so the bulk actions have to reach everything the predicate matches
   *  (plan §9 rulings C). */
  async function selectAllMatches() {
    try {
      const ids = await searchItemIds("media", committed || undefined);
      selectedIds = new Set(ids);
    } catch (e) {
      report = `select all failed: ${e}`;
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
    pairingSuggestions("media")
      .then((p) => (pairs = p))
      .catch((e) => bad(String(e)));
  }

  function backToLibrary() {
    panel = "library";
    void load();
  }

  async function doMerge(s: PairSuggestion, keep: "a" | "b") {
    try {
      await mergeItems(keep === "a" ? s.aId : s.bId, keep === "a" ? s.bId : s.aId);
      pairs = await pairingSuggestions("media");
      info(`merged into "${keep === "a" ? s.aTitle : s.bTitle}"`);
      await load();
    } catch (e) {
      bad(`merge failed: ${e}`);
    }
  }

  async function doKeepBoth(p: PairSuggestion) {
    try {
      await dismissPair(p.aId, p.bId);
      pairs = await pairingSuggestions("media");
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
    class="search-box query-box"
    type="text"
    placeholder="type:video source:ehentai big_breasts -anal"
    aria-label="Query the media library"
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
              : `${s.count} items match this on top of the query already typed`}
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
      <span class="hint">no filters — the whole media library</span>
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
    <button class="nav-item" class:active={panel === "library"} onclick={backToLibrary}>
      Everything <span class="count">{libraryCount}</span>
    </button>

    <div class="section">Keep current</div>
    <button class="nav-item" class:active={panel === "pairs"} onclick={openPairs}>
      Possible dupes <span class="count">{pairCount || ""}</span>
    </button>

    <div class="section">Vault</div>
    <div class="vault-info">{status.vaultRoot}</div>
    <div class="hint">The query box is per session and is never saved to disk, and it is
      not case-folded for tags: a tag matches exactly as it is spelled.</div>
  </nav>

  <main class="content">
    {#if panel === "pairs"}
      <h2 class="panel-title">Possible duplicates</h2>
      <p class="hint">Media whose <em>stored image bytes are identical</em> — the same picture imported twice or
        reposted across boorus. Nothing merges itself; you decide which entry to keep.</p>
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

    {:else if needsFacts && items.length === 0 && conflict === null}
      <div class="empty facts-row">
        <strong>Not measured yet.</strong><br />
        This query asks about facts the vault has never read off the files —
        sizes, durations, picture dimensions, whether a file is animated, what extension it
        calls itself. A NULL fact never matches, so this is not "nothing has it", it is
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
              : "No media in the vault."}</strong><br />
        {#if conflict}
          {conflict} A fact that was never measured would read *Not measured yet* instead: a
          NULL column never matches, so an unmeasured vault is not a vault without it.
          <br /><br />
        {/if}
        {#if querying}
          Tags match exactly, case and all: <code>Big_Breasts</code> finds nothing where
          <code>big_breasts</code> does. A bare word is one tag, <code>tag:"…"</code> is an exact
          name including its namespace, a quoted phrase searches titles, and
          <code>source:ehentai</code> pins provenance. An item has exactly one
          <code>ext:</code> value, so <code>ext:gif ext:mp4</code> is empty by construction.
          <br /><br />
        {/if}
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

{#if viewerIndex !== null && items[viewerIndex]}
  <Viewer
    items={items}
    index={viewerIndex}
    {thumbs}
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

<style>

  .facts-row button {
    margin-top: 0.5rem;
  }
</style>
