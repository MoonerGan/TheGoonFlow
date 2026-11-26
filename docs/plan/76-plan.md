# Filter system overhaul — design-level plan

> Request (user, 2026-11-25): *"Change the current filter system into more of a
> booru-like filter system. 'From site' turns into tag type of 'source'. Support
> wide variety of meta tags (video, image, gif, no thumbnail, local only, no tags,
> length greater than x, length less than x, etc). Meta tags are identified by
> GoonFlow logic, not imported. Normal tags should identify their source as well…
> Replace everything in the media and game view with this new system but keep the
> 'possible dupes' and 'Vault' items obviously. Priority 2, but do it before any
> game or media download issues."*
>
> Companion ruling: "Sources" is becoming a generalised SETTINGS tab (board card
> #73, P2); it will own *where big media lives* (vault BLOB vs a user-defined
> disk folder). The filter must therefore distinguish **vault-stored / on-disk /
> link-only**, and large media blobs in the vault are acceptable — a warning at
> most; slowdown preferred over no video support.

---

## 1. Goal + non-goals

**Goal.** One query bar per view speaks a booru-like grammar: free tags ANDed by
spaces, `-` negation, quoted phrases, and reserved prefixes (`type:`, `source:`,
`tag:`, `has:`, `stored:`, ranges like `length>1:30`, `size>100MB`). "From site"
and the source-state rails become predicates in that grammar. Content tags carry
provenance (origin site + namespace) so e-hentai's `female:big_breasts` and
gelbooru's `big_breasts` stay distinct rows yet match together on the folded
name. Meta tags are **computed predicates evaluated by GoonFlow** — they are
never imported and never stored as free tags.

**Non-goals.**
- No saved-query manager, no tag-alias translation table (DESIGN §5's
  `tag_aliases` stays planned; the folded-name key makes it additive later).
- No FTS5 / full-text index this pass (see §6 for why LIKE is enough at 100k).
- No blocklist rework: DESIGN §11 tag-safety keeps its plain tag filter; the
  grammar's `-tag` form is a hook, not the blocklist.
- No changes to ingest/clip fetching, pairing, viewer, or nuke paths.
- No new downloads work: this card **lands before** #17 Package 2 (D24).

**DESIGN decisions this respects / touches:**
- **D14** view separation is kept: one grammar, but each view exposes a
  different predicate vocabulary. **Only `status:` is Games-only**: it is the
  pipeline column and a media item has no such column to read. `circle:` and
  `rating:` are valid in *both* views, which is what §4's vocabulary table says
  and what the schema supports — `items.circle` / `items.rating` are columns of
  the shared `items` table, and an e-hentai gallery really does have a circle, so
  rejecting the key would hide a fact the row carries. (This sentence replaces
  the earlier "Games get `status:`/`circle:`; Media never do", which contradicted
  §4; pinned by `circle_and_rating_are_valid_in_both_views_because_items_carry_them`
  and `status_is_games_only…` in `goon-core::query`.)
- **D9/D16**: completion stays a *tag* — `complete` is matched as a tag, `status:`
  covers only the pipeline column. Media queries can never produce a status chip.
- **D17/D18** video semantics (link by default, vault BLOB opt-in, 512 MiB cap)
  are exactly what the `stored:` predicate codifies; no amendment needed.
- **#73 (settings tab)** overlap: this plan *defines the storage-state
  vocabulary* (`vault | disk | local-only | link`) that #73's "user-defined
  media folder" will plug into. If #73 lands later it adds facts, not new
  predicates.
- No decision in DESIGN §2 is contradicted.

---

## 2. Current state (what exists today, where)

Note: `crates/goon-core/src/tags.rs` **does not exist** — the tag read/write
paths live in `crates/goon-core/src/db.rs` (`tag_get_or_create` db.rs:823,
`add_item_tag` db.rs:887, `top_tags` db.rs:917) and in the ingest layer.

**UI facets (single-value state vars, one rail each):**
- MediaView (`apps/desktop/src/views/MediaView.svelte`): `tagFilter`,
  `siteFilter`, `sourceFilter` (MediaView.svelte:54-57) fed by four sidebar
  sections — *Media → Everything {mediaCount}* (:351-354), *Sources → Local
  only / No tags* (:356-367, the #40 queues), *From site → {siteStats}*
  (:369-383), *Tagged → {topTags(24)}* (:385-399); plus *Keep current →
  Possible dupes* (:401-404) and the *Vault* info block (:406-407). Search is a
  plain title box, debounced 300 ms (:82-88), and `load()` re-issues
  `listItems(300, 0, search, "media", tag, site, source)` + four stats calls
  (:90-116). Facets are independent single values — you can never AND two tags,
  negate anything, or search a tag and a text term at once.
- GamesView (`GamesView.svelte`): pipeline rails (:401-409) + *Sources* queues
  (:411-425) + *Tagged* (:427-441) + *Keep current* Updates / Possible dupes /
  Online matches (:443-452). Worse, the status filter is **client-side** over a
  fetched page: `filtered = items.filter(i => i.status === filter)` (:75-77).
- Counts ("241", "ehentai 108") are global per view, never scoped to the other
  active filters: `getKindStats`, `siteStats`, `sourceStats`, `topTags`
  (MediaView.svelte:104-109; GamesView.svelte:118,131-134).

**Where search happens today:** only `db::list_items` (db.rs:439-475) —
`title LIKE %term% OR circle LIKE %term%` (db.rs:440,447). Tags never enter the
search string. `ListQuery` (db.rs:410-437) is the eight-field argument struct
(#51); commands `list_items` / `source_stats` / `site_stats` / `top_tags` sit at
`src-tauri/src/lib.rs`:864-908, 1188-1197; wire mirror in `api.ts`:240-264, 351.

**Tag provenance today:** none. `tags.name` is globally UNIQUE with no site
column (db.rs:52-56). Every imported tag — booru JSON arrays (ingest.rs:684-692,
725-738), e-hentai namespaced names kept verbatim by `eh_tags`
(`female:big_breasts`, ingest.rs:949-962), f95 engine words (ingest.rs:1437-1462)
— is written as `TYPE_CONTENT` with the bare name (ingest.rs:2157-2160,
apply_meta ingest.rs:2342-2347). So `female:big_breasts` and `big_breasts` are
two unrelated rows that can never be matched together, `other:variant_set` and a
booru `variant_set` collide the same way, and the rail shows flattened names
(`femalebig_breasts`, `othervariant_set`) with no provenance shown. Manual tags
are lowercased + space-folded at `lib.rs`:1211. f95's `f95-completed` also lands
in `content` despite D9's separate `site_flag` type.

**Meta facts today:** no meta tags exist, and the facts they need are partly
unstored. `media` rows (db.rs:64-75) carry `kind` (`thumb`/`full`/`video`),
`mime`, `width`, `height`, `data` — but **no byte-size column** (sizes are
computed ad hoc as `length(data)`, media.rs:436) and **no duration** anywhere.
Storage location today is implied: vault BLOB = a `media` row of kind
`full`/`video`; on-disk = a `sources` row with `site='local'` + `file:///` URL
(db.rs:32-42; DEVELOPMENT §3); link-only = the `no_local` EXISTS clause
(db.rs:460-461, 518-520). GIF-ness is only knowable from a stored row's `mime`
(media.rs:27). Videos: link by default (D17), vault copy ≤ 512 MiB
(`video.rs::STORE_CAP`, video.rs:20), first-frame thumbs via the OS shell (D18).

---

## 3. Data model

### 3.1 Tag provenance (site + namespace + display), schema v10

Rebuild `tags` so one row = *(origin site, name as that site spells it)*:

```sql
-- migration v10 (ordered idempotent step; never edit the CREATE batch — DEV §3)
CREATE TABLE tags_new (
    id       INTEGER PRIMARY KEY,          -- old ids reused, item_tags survives unmapped
    site     TEXT NOT NULL DEFAULT '',     -- '' = user-created / unknown provenance
    ns       TEXT,                         -- 'female' from 'female:big_breasts'
    name     TEXT NOT NULL,                -- exactly what the site spells (may contain ':')
    fold     TEXT NOT NULL,                -- cross-site join key: leading 'ns:' stripped
    type_id  INTEGER NOT NULL REFERENCES tag_types(id),
    UNIQUE (site, name)
);
-- Rust loop inside the migration: INSERT INTO tags_new(id, site, ns, name, fold, type_id)
-- SELECT id, '', NULL, name, strip_known_ns(name), type_id FROM tags;
-- (fold is computed in Rust against the known-namespace list, not SQL instr();
--  'f95-completed' has no ':' so it folds to itself)
DROP TABLE tags; ALTER TABLE tags_new RENAME TO tags;
```

Semantics:
- **Distinct:** gelbooru-clipped `big_breasts` and e-hentai `female:big_breasts`
  are different rows (`(site,name)` unique); one item can hold both.
- **Together:** the bare token `big_breasts` matches on `fold`, so it finds
  e-hentai's `female:big_breasts`, gelbooru's `big_breasts`, and a user tag
  `big_breasts` at once. The display name is always `name` (site spelled), never
  flattened; autocomplete shows provenance beside it (`big_breasts · e-hentai,
  gelbooru, rule34`).
- **Backfill:** existing rows keep `site=''` — provenance is *not guessed* from
  the item's sources (a tag named `big_breasts` on a rule34 item did not
  necessarily come from rule34). A "Re-fetch metadata" pass (§5 of DEVELOPMENT
  tooling) re-imports site-scoped rows over time; `fold` already bridges them.
  `tag_get_or_create` grows a `site` argument; ingest passes the `SiteRef` site,
  the editor passes `''`.
- `user_status` tags (complete/dnf/replay, db.rs:147-155) stay `site=''` and
  keep their type/priority (D9 untouched). Fix on the way: `f95-completed`
  imports as `TYPE_SITE_FLAG`, not content (it already reads as a site fact,
  ingest.rs:1455).

### 3.2 Source-as-tag-type

No new table. `source:` is a computed predicate over `sources.site`
(db.rs:32-42) — same site vocabulary as invariant 10 (`HOST_SUFFIXES` /
`KNOWN_SITES`). "From site → ehentai 108" becomes `source:ehentai`; autocomplete
gets its candidate list + counts from `counts_by_site` (db.rs:353-369), which
stays but becomes query-scoped (§5).

### 3.3 Meta tags are computed predicates — never imported

A meta tag is parsed and evaluated, never written to `tags`. What is stored is
the **facts** they read, added in the same v10 step:

```sql
ALTER TABLE media ADD COLUMN bytes      INTEGER;   -- backfill: UPDATE media SET bytes = length(data)
ALTER TABLE media ADD COLUMN duration_ms INTEGER;  -- videos only; NULL = not probed / unknown
ALTER TABLE media ADD COLUMN animated   INTEGER;   -- NULL unknown, 0, 1; backfill: mime='image/gif' → 1
```

Fact sources (all GoonFlow-side, never imported as tags):
- `animated`: stored-bytes sniff (`detect_mime`, media.rs:20-27); webp/apng
  animation needs a decode check — cheap for thumbs, deferred for originals
  (leave NULL = unknown rather than guessing).
- `duration_ms`: probe at `+ Video` import and at vault-store time via the
  **Windows Shell property store** (`IShellItem → IPropertyStore →
  System.Media.Duration`) — same zero-dep OS-decoder route as D18's first
  frames, no FFmpeg. An existing-vault backfill runs as a cancellable job
  (D15). Unknown duration never matches `length>`/`length<` — an admitted
  unknown beats a wrong count (same honesty as #38's three-state verdict).
- disk size: `std::fs::metadata` on the `local` source path, **cached at import
  and refreshed by the same job**; queried predicates use the cache, never stat
  100k files inline.
- storage state is derived, not a column (until #73 adds a real location):
  `vault` = EXISTS media(kind IN ('full','video')); `disk` = EXISTS
  sources(site='local') (+ #73: a future `media(path NOT NULL, data NULL)` row);
  `local-only` = `disk` AND NOT EXISTS sources(site<>'local'), which is the
  one-to-one form of the #40 "local only" badge - added at the S2 review, because
  `stored:disk` as a preset would silently widen the queue it selects (rulings C);
  `link` = neither. #73 only needs to keep those EXISTSES honest —
  the grammar never changes.

---

## 4. Query grammar

EBNF (parser is a pure Rust module, `goon-core::query`, unit-testable without a DB):

```ebnf
query    := term { WS term }                       (* space = AND *)
term     := [ "-" ] ( predicate | tag )
predicate:= key [ cmp ] ":"? value                  (* colon OR operator form *)
key      := "type" | "source" | "tag" | "has" | "stored"
          | "status" | "circle" | "rating" | "size" | "length"
          | "width" | "height" | "ratio" | "id" | "added" | "updated"
cmp      := ">" | "<" | ">=" | "<=" | "="
value    := quoted | bare
quoted   := '"' ANY* '"'                            (* spaces inside survive *)
bare     := [^ \t"-"]+                              (* ':' stays literal in tag names *)
```

- A token `a:b` whose `a` is **not** a reserved key is a literal tag name —
  that is what keeps `female:big_breasts`, `critter:feathers`, `other:variant_set`
  working unquoted. Reserved keys are a closed, code-owned set: meta tags are
  GoonFlow logic, never imported.
- Bare tag tokens match `fold` (cross-site union); `tag:"…"` matches `name`
  exactly, including namespaces. Quoting is the only place spaces work.
- Ranges: `size>100mb`, `length>1:30`, `length<90s`, `size:>100mb` (both
  spellings parse); durations accept `mm:ss`, `Ns`, `Nm`; sizes `kb/mb/gb`.

**Meta vocabulary (initial, fixed):**

| Spelling | Predicate (SQL sketch) | Unknown handling |
|---|---|---|
| `type:video` / `type:image` / `type:gif` | media kind='video' (+local video ext) / a still frame / animated=1 | unknown → no match |
| `has:thumb` / `-has:thumb` | EXISTS media kind='thumb' | exact |
| `has:tags` / `-has:tags` | EXISTS item_tags (the "no tags" queue) | exact |
| `stored:vault` / `stored:disk` / `stored:link` | §3.3 derivation | exact |
| `source:ehentai`, `-source:local` | EXISTS sources(site=…) | exact |
| `size>100mb` | max(media.bytes, cached disk bytes) | unknown → no match |
| `length>1:30` / `length<10s` | media.duration_ms | unknown → no match |
| `width>1200` `height>800` `ratio:>2` | media.width/height | NULL → no match |
| `status:wishlist` `circle:"…"` `rating:>3` | items columns (Games: status; both: circle/rating) | NULL → no match |
| `id:<ulid>` | items.id | exact |
| `added:>=2026-11-01` | items.created_at | exact |

Only `status:` is Games-only: MediaView's parser rejects it with a clear inline
error (D14 enforced in the grammar, not just the rails). `circle:` and `rating:`
are accepted in both views, exactly as the table's "both: circle/rating" says —
the columns are on the shared `items` row.

**Qualified tags — the decision asked for:** `ehentai:female:big_breasts` style
vs `source:ehentai tag:"female:big_breasts"`. **Recommend the latter.** The
colon is already overloaded — e-hentai and e621 names *contain* colons, so
`ehentai:female:big_breasts` is genuinely ambiguous (keyword? site? namespace?).
`source:` + quoted `tag:` is unambiguous, uses one mental model for every
prefix, and the chip UI makes the two-term form one click anyway. The
`<site>:<name>` shorthand is explicitly **not** reserved, so a future grammar
addition can add it as sugar without breaking any query that exists today
(it would parse as a literal tag match until then — honest, if slightly empty).

**Worked examples (what each matches):**

1. `big_breasts` — every media item tagged `big_breasts` on any site, plus
   e-hentai `female:big_breasts` (same fold).
2. `female:big_breasts` — only items whose tag name is literally that
   (e-hentai-spelled rows).
3. `source:ehentai tag:"female:big_breasts"` — provenance-pinned exact name.
4. `source:gelbooru source:rule34` — items linked from **both** sites (AND,
   like every other space; use `-source:` to exclude).
5. `big_breasts -anal` — content tag AND a negated tag (the blocklist hook).
6. `"summer witch" -source:local` — quoted title phrase, excluding on-disk items.
7. `type:video length<1:30` — videos with a known duration under 90 s.
8. `type:video length>2m -stored:vault` — long clips still living as links/disk
   files (the "should I vault this?" list).
9. `size>100mb type:image` — stored/disk originals bigger than 100 MB.
10. `stored:vault type:video` — everything the vault actually holds as video
    bytes (the #73 migration candidate list).
11. `-has:tags source:ehentai` — the "fix my metadata" queue narrowed to one site.
12. `-has:thumb stored:disk` — on-disk items with no tile (broken-thumb audit).
13. `status:wishlist renpy` (Games) — pipeline stage AND engine tag.
14. `width<400 type:image -has:thumb` — small thumbs-only tiles.

---

## 5. UI

**Both views** replace the four facet rails with:
- one search box (placeholder teaches the grammar; media view:
  `type:video source:ehentai big_breasts -anal`; the existing debounce stays);
- a **typed autocomplete** under it: reserved keys with legal values (`type:` →
  video/image/gif, `stored:` → vault/disk/link), tag completions with provenance
  and per-candidate counts scoped to the *query minus the token being typed*;
  a matched `-` prefix offers negations first. Candidates come from `top_tags`
  and `counts_by_site`, both re-scoped to the current parsed query — that is
  where the old rail counts go.
- an **active-filter chip row** rendered from the parsed query (click a chip =
  delete that term; the text box stays the source of truth). Chips are one flex
  block **above** the grid — never a flex sibling of the content they describe
  (invariant 24 / DEVELOPMENT §6 layout rule).
- counts: the old "Everything 241" becomes a header line "N matches · 241 total"
  — one `COUNT(*)` paired with the page query. Per-facet counts live only inside
  autocomplete rows (cheap: one scoped candidate query per keystroke-set, not
  per rail).
- **kept exactly as-is:** *Possible dupes* panel and button, and the *Vault*
  section (Games keep *Updates* and *Online matches* too). They stay sidebar
  items; only the facet rails die.
- the three #40 source-state queues survive as **preset chips** (one row above
  the grid: `no tags`, `local only`, `no local file`) that insert the
  equivalent predicate into the box — one click, visible in the grammar.
  (Open decision 8.)
- Games: the client-side `filtered` hack (GamesView.svelte:75-77) dies;
  `status:` is evaluated in SQL like everything else.

**Warning rules (project law):** a parse error or unsupported-in-this-view key
is **form state** — inline under the search box, un-dismissed, readable a minute
later (same rule as ClipDialog's `error`); a zero-result query is the normal
empty state, not a warning; one-shot things (a backfill job finished, sizes
refreshed) go through `lib/toast.svelte.ts` and auto-dismiss. `endVaultSession`
already clears toasts; the query string itself is per-session state and is not
persisted anywhere (nothing decrypted survives).

`api.ts` grows the mirror of the new command structs (invariant 6); the old
`listItems(tag/site/source)` arguments are removed in the same commit — one wire
shape, no legacy branch.

---

## 6. Performance

Library today: ~241 media items; the design target is 100k items without
falling over. SQL sketches (added in v10):

```sql
CREATE INDEX tags_fold_idx        ON tags(fold);
CREATE INDEX tags_site_name_idx   ON tags(site, name);
CREATE INDEX item_tags_tag_idx    ON item_tags(tag_id, item_id);  -- tag→item direction
CREATE INDEX sources_site_item_idx ON sources(site, item_id);     -- source: predicate
CREATE INDEX media_item_kind_idx  ON media(item_id, kind);        -- has:/stored: probes
```

- Tag/site predicates become tag-first semi-joins
  (`WHERE items.id IN (SELECT it.item_id FROM item_tags it JOIN tags t …WHERE t.fold = ?)`)
  instead of a per-row EXISTS probe per item; with `item_tags_tag_idx` an AND of
  five tags is five index seeks + intersect.
- Title LIKE stays un-indexed: a 100k-row `LIKE '%…%'` scan under SQLCipher is
  ~tens of ms and honest; FTS5 is not bought this pass (open decision 9).
- Range predicates read `media.bytes/width/height/duration_ms` — plain columns,
  no BLOB touched: the grid query must never include `data` (frames already
  avoid it, media.rs:436).
- The 300-row unpaginated ceiling (db.rs `ListQuery.limit`, views'
  `listItems(300…)`) is now visible: add a "load more" offset like the current
  one, plus the total from the paired COUNT. Keyset pagination is *not* required
  yet (open decision 10).
- Measurement, not assumption: a generated 100k-item fixture vault and a timing
  probe (§8) must show the worked-example set under ~50 ms end-to-end before the
  card is Done.

---

## 7. Sequencing and dependencies

Lands as its own board card **before** #17 Package 2 / any game-or-media download
fix (the user's ordering ruling; D24's download work then finds `stored:`/`size`
facts already true).

1. **S1 — schema v10**: tags rebuild + media fact columns + indexes + backfills
   (bytes, gif-animated). Idempotent `if v < 10` step; `SCHEMA_VERSION` 9 → 10
   (crates/goon-core/src/lib.rs:8). Tests: idempotency + fold backfill on a v9 fixture.
2. **S2 — `goon-core::query`**: pure parser + AST + predicate planner; grammar
   unit tests only, no DB.
3. **S3 — `db::search_items(conn, &Query)`** replacing `list_items`
   (db.rs:439-475), scoped `top_tags`/`counts_by_site` variants, total count.
4. **S4 — commands + wire**: `search_items` / `filter_suggest` commands
   (lib.rs, registered in `generate_handler!`), `api.ts` mirror, `add_item_tag`
   gains site.
5. **S5 — MediaView** swaps to query bar + chips + presets; dupes/Vault untouched.
6. **S6 — GamesView** swaps likewise (status predicates replace the client filter).
7. **S7 — facts jobs**: duration-probe + disk-size refresh as one cancellable
   job kind (`jobs.rs`, D15); `+ Video` import writes them going forward.

Overlap with **#73 (settings tab)**: freeze the `stored: vault|disk|link`
vocabulary in S1/S2 as the shared contract. When #73 lets the user point big
media at a disk folder, it writes `media(path, data NULL)` rows and the `disk`
EXISTS grows by one arm — no grammar, UI, or test changes.

---

## 8. Testing plan

The project measures, it does not assume. Existing probes stay green throughout
(`tools/run-test.ps1`, `verify-artifact.ps1`, `viewer-real-media-probe.js`,
`webview-layout-test.ps1`, the panic trio, `svelte-check 0/0`, `npm run build`).

New checks this card needs:
1. **Grammar unit tests** (Rust, no DB): every worked example parses to the
   expected AST; colon-overload cases (`female:big_breasts`, unknown `foo:bar`),
   quotes, ranges (`1:30` vs `90s` vs `2m`), negation, bad-key errors.
2. **Predicate DB tests** (goon-core, in-memory): a seeded fixture (one
   e-hentai `female:big_breasts` item, one gelbooru `big_breasts` item, a gif, a
   stored video with duration, a link video, an on-disk image, a no-tags item,
   a thumbless item) asserts each meta predicate and the fold-matches-both rule —
   this is the migration's regression pair, next to `source_state_filters_and_badges`
   (db.rs:1677).
3. **Scale probe**: `cargo run -p goon-core --example filter_scale_probe`
   generates a throwaway 100k-item/1M-item_tags vault and prints p50/p95 per
   worked-example query into a `tools\filter-scale-*.log` summary, the same
   evidence-file habit as `run-test.ps1`.
4. **Live UI probe**: `tools/filter-ui-probe.js` (same WebView2-DevTools route
   as `viewer-layout-probe.js`, throwaway vault at `%TEMP%\goon-filter-probe`):
   types real queries into the real box, asserts the chip row renders the parsed
   terms, the count line prints "N matches · M total", a parse error shows inline
   and survives (it is state), and *Possible dupes* + *Vault* still respond.
5. Manual checklist additions for the field round (chip click-to-remove, preset
   chips, Games `status:` replacing the old rails, dupes/Vault untouched).

---

## 9. Open decisions (user must okay before implementation)

1. **Tag rebuild vs bolt-on column.** Rebuild `tags` to `UNIQUE(site, name)` with
   `fold` (recommended — provenance per row, clean counts, display never lies),
   vs adding `item_tags.origin_site` (smaller diff, but tag clouds then disagree
   with provenance and the flattened-display problem survives).
2. **Qualified-tag syntax.** `source:<site> tag:"<name>"` (recommended, §4) vs
   `ehentai:female:big_breasts` shorthand. Recommendation: quoted form now,
   shorthand never reserved.
3. **Bare tokens match the folded name** (cross-site union), exact-site pinning
   via `tag:"…"`. Recommendation: yes — it is what "searchable together" means.
4. **Meta vocabulary frozen as listed** in §4's table (`has:`/`stored:`/ranges;
   no `rating:`-style additions beyond the table without a new card).
   Recommendation: freeze it; a closed set is the whole anti-import guarantee.
5. **Duration source**: OS Shell property probe at import/store + a one-off
   backfill job; unknown never matches `length:` predicates. Recommendation: yes
   (D18's no-FFmpeg route; honest unknown beats guessing).
6. **Disk sizes cached, not stat-per-query**: cached at import, refreshed by the
   jobs pass (S7). Recommendation: yes.
7. **No provenance guessing in backfill**: existing tags stay `site=''` until a
   re-fetch re-imports them. Recommendation: yes — matches the #38 honesty rule.
8. **#40 queues as preset chips** (one-click inserts the predicate) rather than
   vanishing. Recommendation: keep them as presets.
9. **FTS5 or LIKE.** LIKE un-indexed at ≤100k (recommended; measure at S3/S7
   before buying an index), FTS5 only if the scale probe misses the budget.
10. **Pagination**: keep "first 300 + load more" with the visible total
    (recommended, minimal), or keyset pagination now.
11. **`f95-completed` moves to `site_flag`** during backfill (one-row type fix
    consistent with D9). Recommendation: yes, in v10.
12. **Games grammar**: `status:` in the same query box replaces the pipeline
    rails (recommended; D14 honored by per-view vocabulary, media parser rejects
    `status:`). Alternative kept on the table: Games keeps its status rails and
    only the rest is replaced — smaller change, but two filter mental models live
    on, which is what the user asked to end.

---

## 10. Review notes after the code map (read these before deciding)

The companion map (`board/map-filter-overhaul.md`) checked this plan against the
code and found three things the plan stated too loosely. They change two of the
open decisions:

1. **`width>` / `height>` cannot read `media.width/height` as written in §4.**
   Those columns are filled **only for `kind='thumb'` rows** (`media.rs:239-240`);
   `full` and `video` rows write `NULL` (`media.rs:300`, `:367`). So a `width>1200`
   query today would measure the 360 px tile and lie. v10 must (a) record the
   original's dimensions when a `full`/`video` row is written - decodable for
   images already, and for video from the same probe that reads duration - and
   (b) backfill what can be decoded from stored bytes, leaving NULL = unknown
   (never matching) for the rest. Until then those two predicates are *not* in the
   vocabulary; they were listed in §4 as if they were free.
2. **Duration is the one capability with no partial path at all.** No column,
   and the word `duration` appears nowhere in `crates/` or `src-tauri/`; the booru
   APIs' own `file.duration` field is **discarded** on the way in
   (`ingest.rs:679-734`), so even the sites that hand us the number throw it away.
   Decision 5 therefore has a bigger cost than the plan implies: a Media
   Foundation `IMFSourceReader` (or Shell property-store) probe has to be written,
   plus ingest must stop discarding `file.duration` where a site supplies it.
   Cheap first step that is honest: import `file.duration` from booru JSON now,
   probe stored/on-disk files later, and keep `length:` invisible until a fact
   exists.
3. **`animated` beyond gif is genuinely unknowable today** - the plan already
   leaves those NULL; the map confirms `source_images.mime` is NULL in every real
   path, so `type:gif` must be read from the stored row's mime only.

Two more facts worth having in front of you for decisions 8-10:

- **there is no server-side "everything matching this query" today**: "Select all"
  selects the client array capped at `limit` 300 (`db.rs:425-427`). #73 (bulk
  *Store selected*) inherits that ceiling unless this card adds a count/ids
  endpoint. Recommendation: do it here, in S3, since the query engine is what
  makes "all matching" meaningful.
- **`status:` is not in SQL at all** - Games filters pages client-side
  (`GamesView.svelte:75-77`), and `source_stats` (`db.rs:500-523`) is a
  hand-maintained second copy of `list_items`' predicates, with
  `games_needing_match` (`db.rs:1144`) a third. The plan's S3 replaces all three
  with one planner; that is also the fix for their long-standing drift.
- FTS5 **is** available if the scale probe misses the budget: the bundled
  sqlite amalgamation is built with `SQLITE_ENABLE_FTS5` even under
  `bundled-sqlcipher` (`libsqlite3-sys-0.35.0/build.rs:130-136`), so decision 9
  can be deferred without a dependency question.

Nothing in the map contradicts the grammar (§4), the provenance model (§3.1) or
the UI replacement (§5).
