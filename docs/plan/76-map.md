# Filter overhaul — code-path map (inventory + seams)

Repo: `C:\Source\TheGoonFlow`. Read-only survey, 2026-11-25. All paths repo-relative.
Companion design doc: `board/plan-filter-overhaul.md` (not duplicated here).

Terminology note: **there is no `crates/goon-core/src/tags.rs`.** Tag storage/normalization
lives in `crates/goon-core/src/db.rs` §"tags" (817–941) and in the `add_item_tag` Tauri
command (`apps/desktop/src-tauri/src/lib.rs:1200–1219`). There is also **no
`parse_eh_tags`**: the e-hentai tag reader is `eh_tags` (`ingest.rs:950–962`) and it does
**no namespace splitting** — see §3.

**Working-copy caveat**: at survey time `git status` showed uncommitted edits in
`apps/desktop/src-tauri/src/http.rs` (+103), `apps/desktop/src-tauri/src/video.rs` (+53,
new `url_host`/`redirect_hop`/`is_html_ct`/`referer_for` helpers for the #64 hotlink/CAPTCHA
fix) and `tools/viewer-real-media-probe.js` (+30). Line numbers here are from the **current
working tree**, so `video.rs` numbers differ from `HEAD` (`video_info` is `112–137` in the
working copy, `~59` in `HEAD`). Expect that file's numbers to move while that work lands.

---

## 0. Seams (read this first)

The whole list/count surface is four functions and four commands. A new query engine has
exactly one plug point per concern:

| concern | single plug point |
|---|---|
| row selection (the grid) | `db::list_items` (`crates/goon-core/src/db.rs:439–475`) built from `db::ListQuery` (`db.rs:410–437`), fed only by `#[tauri::command] list_items` (`apps/desktop/src-tauri/src/lib.rs:866–888`) |
| facet badges | `db::source_stats` (`db.rs:500–523`), `db::counts_by_site` (`db.rs:353–369`), `db::top_tags` (`db.rs:917–941`), `db::counts_by_status` (`db.rs:296–328`), `db::counts_by_kind` (`db.rs:330–342`) |
| wire shape | flat named args (Tauri matches by name) — `lib.rs:859–864` explains why the command stays flat while the core call takes a struct. A booru-like box should arrive as **one extra optional arg** (`query: Option<String>`), not by nesting |
| frontend call sites | `listItems` (`apps/desktop/src/lib/api.ts:240–257`) called from exactly two places: `MediaView.svelte:93–101`, `GamesView.svelte:119–127`; facet rails inline at `MediaView.svelte:349–408` and `GamesView.svelte:399–456` |

Things that will bite a mechanical rewrite:

- **Status filtering is not in SQL at all.** `filter` (seen/tracking/…) is a client-side
  `items.filter` in `GamesView.svelte:75–77`; `list_items` has no status argument. A query
  engine that supports `status:` must add the predicate, not move it.
- **`source:` (the current nav filter) is a misnomer collision risk** with the planned
  `source:` tag type: today `ListQuery.source` means source-*state*
  (`local_only` | `no_local` | `no_tags`, `db.rs:420`, SQL at `db.rs:456–463`).
- **Everything is capped and unpaginated**: `ListQuery::limit` default 200 (`db.rs:425–427`),
  both views pass `300`, `ORDER BY updated_at DESC` only. "Select all" selects the
  *client array*, so it selects ≤300 rows, not the query result
  (`MediaView.svelte:302`, `GamesView.svelte:360`).
- **Tags have no provenance today.** One global `tags.name UNIQUE` namespace
  (`db.rs:52–56`); provenance exists only in `sources.site`, which is *not* joined to tags.
- **Duration, original dimensions, and byte sizes of anything not in the vault are not in
  the DB at all** (§4/§8). `length(data)` covers vault bytes only.

---

## 1. Tauri commands returning lists/counts (all in `apps/desktop/src-tauri/src/lib.rs`)

All are registered in `tauri::generate_handler![…]` (`lib.rs:2274–2354`; the list-items
cluster is `lib.rs:2284–2293`, tags `2299–2301`, bulk `2318–2322`).

| command | file:line | request args (wire names) | response type |
|---|---|---|---|
| `vault_stats` | `lib.rs:842–850` | `kind: Option<String>` | `Vec<StatusCount>` (`db.rs:290–294`: `status`,`count`) |
| `kind_stats` | `lib.rs:852–857` | — | `Vec<(&'static str, i64)>` → JS `Array<[string,number]>` (`api.ts:45`) |
| `list_items` | `lib.rs:864–888` | `limit`,`offset`,`search`,`kind`,`tag`,`site`,`source` | `Vec<ItemSummary>` |
| `source_stats` | `lib.rs:890–898` | `kind` | `SourceStats` (`db.rs:492–498`: `localOnly`,`noLocal`,`noTags`) |
| `site_stats` | `lib.rs:900–908` | `kind` | `Vec<SiteCount>` (`db.rs:344–349`: `site`,`count`) |
| `top_tags` | `lib.rs:1188–1197` | `limit` (default 40), `kind` | `Vec<TagCount>` (`db.rs:906–913`: `id`,`name`,`typeKey`,`count`) |
| `get_thumbs` | `lib.rs:918–933` | `ids: Vec<String>` | `Vec<ThumbPayload>` (`lib.rs:911–916`: `id`,`mime`,`data` base64) |
| `get_item` | `lib.rs:1176–1184` | `id` | `Option<ItemDetail>` (`db.rs:956–975`, incl. `sources: Vec<SourceRow>` `db.rs:945–954`, `tags: Vec<TagOnItem>` `db.rs:861–867`) |
| `media_for_item` | `lib.rs:960–976` | `itemId` | `Vec<MediaFrame>` (`lib.rs:947–958`: `id`,`kind`,`mime`,`size`) |
| `media_frame_bytes` | `lib.rs:996–1021` | `id` | `MediaFrameBytes` (`lib.rs:985–994`); `INLINE_MAX` 96 MiB at `lib.rs:939` |
| `source_images` | `lib.rs:1041–1056` | `itemId` | `Vec<CandidateImage>` (`lib.rs:1032–1039`; deliberately **no mime**) |
| `add_item_tag` / `remove_item_tag` | `lib.rs:1199–1219` / `1221–1230` | `id`,`name`,`kind` / `id`,`tagId` | `()` — `kind` maps to `TYPE_SITE_FLAG`/`TYPE_USER_STATUS`/`TYPE_CONTENT` at `lib.rs:1206–1210`; **normalization at `lib.rs:1211`**: `name.trim().to_lowercase().replace(' ', "-")` |
| `pairing_suggestions` | `lib.rs:1433–1441` | `kind` | `Vec<PairSuggestion>` (`db.rs:1358–1371`) |
| `list_updates` | `lib.rs:1421–1429` | `limit` | `Vec<UpdateRow>` (`db.rs:1272–1284`) |
| `list_match_suggestions` | `lib.rs:1554–1562` | `limit` | `Vec<SuggestionRow>` (`db.rs:1055–1065`) |
| `video_info` | `lib.rs:1588–1594` | `itemId` | `video::VideoInfo` (`video.rs:24–34`) |
| `known_sites` | `lib.rs:1247–1259` | — | `Vec<SiteInfo>` (site vocabulary for `source:`) |
| bulk: `re_enrich_items` | `lib.rs:2063–2120` | `itemIds` | `RefetchSummary` (`lib.rs:2053–2061`) |
| bulk: `delete_items` | `lib.rs:2171–2182` | `ids` | `usize` |
| `re_enrich_item` (single) | `lib.rs:2027–2048` | `itemId` | `ingest::EnrichReport` (`ingest.rs:2383–2393`) |

Serialization to `api.ts`: manual mirror, per DEVELOPMENT invariant 6
(`docs/DEVELOPMENT.md:324–325`). Types: `ItemSummary` `api.ts:47–55`, `TagCount` `89–94`,
`SiteCount` `224–227`, `SourceStats` `261–264`, `MediaFrame` `273–279`, `PairSuggestion`
`229–238`. `listItems` signature `api.ts:240–257` is the single place the seven params
are named (`source` is a union `"local_only" | "no_local" | "no_tags"`).

Note `db::counts_by_kind` returns a tuple vec, not a struct — the only count command
without a serde struct.

---

## 2. `crates/goon-core/src/db.rs` — schema, queries, tag helpers

### 2.1 Schema (`init_schema`, `db.rs:9–258`; version const `goon-core/src/lib.rs:8` = 9)

- `meta` `db.rs:12–15`; `items` `db.rs:17–30` (cols: `id, kind CHECK('game','gallery','post'), title, original_title, circle, status DEFAULT 'seen', dnf, rating, notes, version, created_at, updated_at`, all timestamps TEXT epoch)
- `sources` `db.rs:32–42` (`site`, `url`, `source_id`, `fetched_at`, `raw` **never written**, `dead`, `UNIQUE(site,url)`)
- `tag_types` `db.rs:44–50` (`key UNIQUE`, `label`, `priority`, `color`)
- `tags` `db.rs:52–56` — `id`, **`name TEXT NOT NULL UNIQUE`**, `type_id NOT NULL REFERENCES tag_types(id)` → *one global name space, no site/namespace column*
- `item_tags` `db.rs:58–62` — `(item_id, tag_id)` PK, both `ON DELETE CASCADE`
- `media` `db.rs:64–73` — `id`, `item_id`, `kind DEFAULT 'thumb'`, `hash`, `mime`, **`width`,`height` (NULLable)**, `data BLOB NOT NULL`
- indexes `db.rs:75–78`: `media_hash_idx (item_id,kind,hash)` UNIQUE, `items_status_idx (status)`, `items_updated_idx (updated_at DESC)`, `sources_item_idx (item_id)`
- `site_settings` `db.rs:82–87`; `updates` `db.rs:89–97`; `match_suggestions` `db.rs:99–107`; `mods` `db.rs:109–118`; `pair_dismissals` `db.rs:120–125`; `source_images` `db.rs:127–135` (`mime` NULLable, `found_at`, `stored_at` INTEGER)
- **No `duration`, no `bytes`/`size` column, no per-item media dimensions for originals, no tag provenance column, no FTS virtual table.**

Migrations (ordered, idempotent `if v < N`; DEVELOPMENT `docs/DEVELOPMENT.md:292–298` states the rule: *a new numbered step, never edit the CREATE batch*):
v2 `db.rs:144–160` (seed tag types 1/2/3 + tags `complete`,`dnf`,`replay`),
v3 `161–173` (site renames `i`→`4chan`, `wimg`→`rule34`, `booru`→`allthefallen`),
v4 `174–193` (`items.launch_path`, guarded `pragma_table_info` check),
v5 `194–203`, v6 `204–212`, v7 `213–233` (`mods.status`,`manifest`), v8 `234–243`,
v9 `244–256` (`source_images`). **The next step is v10** and `SCHEMA_VERSION`
(`goon-core/src/lib.rs:8`) must move with it — `schema_initializes_and_is_idempotent`
asserts equality (`db.rs:1608`).

### 2.2 The list query (the one to replace)

`ListQuery` `db.rs:410–421` (`limit`,`offset`,`search`,`kind`,`tag`,`site`,`source`) +
`Default` `db.rs:423–437`. `list_items` `db.rs:439–475`, SQL verbatim `db.rs:443–464`:

```sql
SELECT id, kind, title, status, version,
       EXISTS (SELECT 1 FROM media m WHERE m.item_id = items.id AND m.kind = 'thumb'),
       updated_at
FROM items
WHERE (?1 IS NULL OR title LIKE ?1 OR IFNULL(circle,'') LIKE ?1)
  AND (?4 IS NULL
       OR (?4 = 'game' AND kind = 'game')
       OR (?4 = 'media' AND kind IN ('gallery','post')))
  AND (?5 IS NULL OR EXISTS (
        SELECT 1 FROM item_tags it JOIN tags t ON t.id = it.tag_id
        WHERE it.item_id = items.id AND t.name = ?5))
  AND (?6 IS NULL OR EXISTS (
        SELECT 1 FROM sources ss WHERE ss.item_id = items.id AND ss.site = ?6))
  AND (?7 IS NULL
       OR (?7 = 'local_only' AND
           EXISTS (SELECT 1 FROM sources sl WHERE sl.item_id = items.id AND sl.site = 'local')
           AND NOT EXISTS (SELECT 1 FROM sources so WHERE so.item_id = items.id AND so.site <> 'local'))
       OR (?7 = 'no_local' AND
           NOT EXISTS (SELECT 1 FROM sources sn WHERE sn.item_id = items.id AND sn.site = 'local'))
       OR (?7 = 'no_tags' AND
           NOT EXISTS (SELECT 1 FROM item_tags tn WHERE tn.item_id = items.id)))
ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3
```

Search is `%substr%` `LIKE` on title+circle (`db.rs:440`) — no FTS, no tag/circle
weighting, no `&`/`|`/`-` operators. Placeholder warning at `db.rs:441–442` (params! binds
positionally; skipping `?1` was a past live bug, test `db.rs:1626–1673`). Row mapper
`map_item_summary` `db.rs:477–488`.

### 2.3 Count/facet queries

- `counts_by_status` `db.rs:296–328` — `GROUP BY status` with the same `kind` tri-state,
  then re-emits all five statuses with 0 filled in (honest-count behaviour, `db.rs:315–327`).
- `counts_by_kind` `db.rs:330–342` — `SELECT kind, count(*) … GROUP BY kind`, then
  re-emits game/gallery/post with zeros.
- `counts_by_site` `db.rs:353–369` — `SELECT s.site, count(DISTINCT s.item_id) FROM sources s
  JOIN items i … GROUP BY s.site ORDER BY count DESC, s.site` (uncapped).
- `source_stats` `db.rs:500–523` — three `COUNT(*)` subqueries over a `format!`-built
  `kind_sql` string (`db.rs:501–512`), predicates identical to `list_items`'s `?7`.
  **Anything new that filters must update both copies or the badges lie.**
- `top_tags` `db.rs:917–941` — joins `item_tags`+`tags`+`tag_types`+`items`, `GROUP BY t.id
  ORDER BY c DESC, t.name LIMIT ?1`; `limit` 24 from both views.

### 2.4 Tag read/write helpers

- type ids `db.rs:819–821`: `TYPE_USER_STATUS=1`, `TYPE_SITE_FLAG=2`, `TYPE_CONTENT=3`
  (migrated names/labels/priorities at `db.rs:147–150`).
- `tag_get_or_create(conn, name, type_id)` `db.rs:823–831` — `INSERT OR IGNORE` + re-select;
  **no normalization at all**, so `Female:Big_Breasts` would be its own row.
- `link_item_tag` `db.rs:833–839`; `set_user_status_tag` `db.rs:842–857` (deletes every
  `type_id=1` link first → at most one); `add_item_tag` `db.rs:887–896`;
  `remove_item_tag` `db.rs:898–904`.
- `tags_for_item` `db.rs:869–885` — returns `id,name,type_key`, ordered
  `tt.priority DESC, t.name` (that is the "Complete chip first" rule, D9).
- `merge_items` `db.rs:1562–1597` moves `item_tags` with `INSERT OR IGNORE` (so a name
  split across two rows never merges) and moves media by full BLOB copy.

### 2.5 Other filter-adjacent readers

`games_needing_match` `db.rs:1144–1154` (the same local-only predicate as SQL, third copy);
`update_targets` `db.rs:1335–1354` (`s.site IN ('dlsite','f95') AND i.status IN (…)`);
`pairing_suggestions` `db.rs:1403–1532` (games: `pairing_key(title)` `db.rs:1376–1388` +
`title_is_placeholder` `goon-core/src/lib.rs:16–35`; media: shared `media.hash`
`db.rs:1475–1482`; bucket cap 12 `db.rs:1454`, global `truncate(40)` `db.rs:1530`);
`source_images_for` `db.rs:671–692`; `mark_source_image_stored` `db.rs:697–708`.

---

## 3. How tags enter the vault (and what string lands in `tags.name`)

Two write loops only, both `TYPE_CONTENT`, both **capped at 40 tags per item**:
`ingest.rs:2157–2162` (in `clip`) and `ingest.rs:2342–2347` (in `apply_meta`).
Plus engine tags written as **`TYPE_SITE_FLAG`** by the game importer:
`importer.rs:297–301` (zip import) and `importer.rs:385–389` (folder import).
Plus the manual path `lib.rs:1199–1219`.

| ingest path | code | exact tag strings that reach `tags.name` | site/namespace kept? |
|---|---|---|---|
| booru dapi (gelbooru, rule34) | `ingest.rs:643–672` → `post_from_json` `674–705`; tags read `684–688` | `post["tags"]` is a **space-separated string**, `split_whitespace()` → bare danbooru names: `big_breasts`, `1girl` | **no** — site lives only on `sources.site` |
| e621 | `ingest.rs:707–750`, tags `725–734` | `tags.general[]` array → bare names, **underscores kept**, category prefixes (`female:`, `species:`) are **dropped** (only `general` is read) | no |
| file-link md5 reverse match | `direct_media` `ingest.rs:1776–1886`, tags `1856–1871`, gate `1848–1850` | same bare names, only when `md5` **and** `filesize` both match (invariant 21) | no |
| e-hentai gallery | `eh_tags` `ingest.rs:950–962`, used by `parse_eh_gallery` `986–994`, copied at `ingest.rs:1124` (`eh_gallery` `1088`) | **the full namespaced token, verbatim**: `artist:horosuke`, `female:big_breasts` (asserted in test `ingest.rs:3399–3402`); `tag.contains(':')` at `956` silently drops un-namespaced tokens | the namespace is *inside the name*, and the site is not recorded |
| f95 thread | `parse_f95_thread` `ingest.rs:1424–1463`, tokens `1437–1461` | engine tokens lowercased with `-`: `renpy`, `rpg-maker`, `unity`, `gamemaker`, `twine`, `srpg-maker`; `[Completed]` → `f95-completed`. **Nothing reads the thread's tag list** (`docs/TESTING.md:75–77`) | no |
| DLsite product | `dl_product` `ingest.rs:1264`, `parse_dl_product` `1223` | **no tags at all** (title/circle/version only) | — |
| local file / direct media | `local_file` `ingest.rs:1967–1997`, `direct_media` `1776` | none unless the md5 match fires | — |
| game import engine fingerprint | `importer.rs:297–301`, `385–389`; values `importer.rs:639–731` (zip) and `787–864` (dir) | `rpg-maker`, `renpy`, `unity`, `gamemaker`, `html5`, `pc-game` written as **site_flag** | n/a |
| bookmark enrichment | `bookmarks.rs:308–338` calls `fetch_meta` + `apply_meta` | same as the site's row above; tally `tags_added` `bookmarks.rs:45`, `337` | no |
| manual user tag | `lib.rs:1200–1219` | `trim().to_lowercase()` + spaces→`-` — **the only normalization in the codebase** | n/a |

Consequence for the "every tag carries provenance" requirement: today `female:big_breasts`
(e-hentai) and `big_breasts` (danbooru-family) are two unrelated rows, and the same
vocabulary can appear under two `type_id`s (`renpy` as `site_flag` from a folder import,
as `content` from an f95 title). Nothing in the frontend strips colons, so the collision
`femalebig_breasts` is not present in code today — it appears only if the new query
engine normalizes by deleting `:`.

Re-fetch / bulk paths:

- `ingest::reenrich_item` `ingest.rs:2399–2521` — re-runs `classify → fetch_meta →
  apply_meta` per source row; takes a lane slot per source `2450–2465`; tags added through
  the same 40-cap loop.
- single command `re_enrich_item` `lib.rs:2027–2048`; batch `re_enrich_items`
  `lib.rs:2063–2120` (child job handle `lib.rs:2093`, one `enrich` job at a time).
- bulk bar UI: `MediaView.svelte:300–316` and `GamesView.svelte:358–374`
  (Select all / Re-fetch metadata / Delete selected / Done); delete path
  `delete_items` `lib.rs:2171–2182`.

---

## 4. Where duration / dimensions / byte size / mime / storage shape can be read

### 4.1 `crates/goon-core/src/media.rs`

- `detect_mime` `media.rs:20–42` → `image/png|jpeg|gif|webp|bmp|avif|heif`; `detect_video_mime`
  `58–85` → `video/webm|x-matroska|x-msvideo|x-flv|mpeg|mp2t|mp4`; `sniff_media`/`Sniffed`
  `89–104`; `thumbnailable` `116–121` (HEIF **not** decodable, D22).
- Rows written: `set_thumbnail` `media.rs:227–245` — the **only** writer that fills
  `width`/`height`; `put_full_image` `media.rs:361–371` — `kind='full'`, `width/height = NULL`
  (`media.rs:367`); `put_video` `media.rs:296–304` — `kind='video'`, `NULL` dims (`300`).
- `has_video(item_id) -> Option<(String, i64)>` `media.rs:307–314` —
  `SELECT mime, length(data) … kind='video' LIMIT 1` (proof that `length(data)` works on
  encrypted BLOBs).
- `best_for` `media.rs:327–344` (`full` > `video` > `thumb`), `frames_for` `media.rs:434–458`
  (`SELECT id, kind, mime, length(data) FROM media WHERE item_id = ?1 ORDER BY id`, drops
  the `thumb` row as soon as any non-thumb row exists, `454–456`), `bytes_for_id` `463–476`,
  `media_count` `480–489`, `thumbs_for` `384–409` (`IN (…)` built by string
  interpolation of placeholders).
- Ceilings that decide what exists in the vault at all: `MAX_COVER_BYTES` 16 MiB
  (`ingest.rs:509`, enforced in `store_bytes_in` `ingest.rs:515–545` and `storable_picture`
  `ingest.rs:1621–1635`) and `video::STORE_CAP` 512 MiB (`video.rs:20`);
  `INLINE_MAX` 96 MiB (`lib.rs:939`). #72 in `docs/TESTING.md:374–382` says the 16 MiB
  ceiling is why big gifs/videos have no stored bytes.

### 4.2 Video/duration

- `video::video_info` `video.rs:112–137` → `VideoInfo{has_source, source_size, source_kind,
  stored, stored_size}` (`video.rs:24–34`). `source_size` is `std::fs::metadata(...).len()`
  **at call time for `file:///` sources only** (`video.rs:128–130`); never persisted.
- `video_source` `video.rs:43–48` decides "is a video" with `ingest::media_ext(url)`
  (`ingest.rs:141–152`, extension allowlists `IMAGE_EXTS`/`VIDEO_EXTS` `ingest.rs:138–139`)
  — a Rust-only predicate, not expressible in the schema.
- `videothumb::first_frame` `videothumb.rs:18` / `57` (Windows Shell `IThumbnailProvider`)
  returns `Frame{rgba,width,height}` (`videothumb.rs:12–16`) — **no duration**.
- Grep result: the token `duration` appears **nowhere** in `crates/` or in the Rust app
  sources. There is no ffprobe/Media Foundation duration read, and the booru APIs'
  `file.ext`/`width`/`height`/`file_size`/`duration` JSON fields are never parsed
  (`post_from_json` `ingest.rs:679–688` and `e621_post` `ingest.rs:720–734` read only
  `file_url` + `tags`; `filesize` is read once and only to verify identity,
  `ingest.rs:1843–1849`).

### 4.3 Storage shape vocabulary

- vault BLOB: `media.kind` ∈ `'thumb' | 'full' | 'video'` (no CHECK constraint, `db.rs:67`).
- on-disk: a `sources` row with `site='local'` and `url` like `file:///D:/…`
  (`importer.rs:556`, `importer.rs:295`; decoded back by `local_source_path`
  `lib.rs:1469–1486`, `extract.rs:49`, `video.rs:50–53`).
- link-only: any `sources` row whose `site <> 'local'` and no `media` row.
- candidates that were never downloaded: `source_images.stored_at IS NULL` (`db.rs:127–135`).
- A `file://` **image** clip *does* copy bytes into the vault
  (`ingest.rs:1986–1995` → `put_full_image`), while a `file://` **video** never does
  (`ingest.rs:1979–1984`, `direct_media` `ingest.rs:1782–1787`,
  `import_video_files` `importer.rs:499–580` with the note text at `importer.rs:547`).
- `+ Images` writes N `full` rows + 1 `thumb` onto **one** Gallery item
  (`importer.rs:401–484`, esp. `459–463`) — so "the item's size" is ambiguous for
  multi-frame items; `frames_for` is the per-frame reader.

---

## 5. Frontend inventory

**No shared filter store exists.** Each view owns its own state; `App.svelte:1–62` and
`views/LibraryView.svelte:1–154` are a tab shell only.

`views/MediaView.svelte`:
- state `48–75`: `items`, `tags: TagCount[]`, `sites: SiteCount[]`, `thumbs`, `search`,
  `tagFilter`, `siteFilter`, `sourceFilter: "local_only"|"no_tags"|null` (note: **no
  `no_local` for media**), `stats: SourceStats`, `panel: "library"|"pairs"`,
  `selectMode`/`selectedIds: Set<string>`/`confirmBatchDelete`, `viewerIndex`, `pairs`.
- debounced search 300 ms: `82–88`.
- `load()` `90–116` — one `listItems(300, 0, search, "media", tag, site, source)` at
  `93–101`, then `getThumbs` `102–103`, `getKindStats` `104–106`, `topTags(24,"media")`
  `107`, `siteStats("media")` `108`, `sourceStats("media")` `109`.
- re-run trigger: `$effect` on the four filter primitives `118–124`; job-driven reload
  `127`.
- rails `349–408`: section headers are literally `Media` / `Sources` / **`From site`** /
  `Tagged` / `Keep current` / `Vault` (`351`, `356`, `369–374`, `385–390`, `401`, `406`);
  each has its own `clear` button (`359`, `372`, `388`); tag buttons render `{t.name}` with
  `title={t.typeKey}` (`395–397`); site buttons `{s.site}` (`379–381`).
- bulk bar `300–316`; empty-state strings that mention filters `452`; `TileGrid` `460–468`;
  `Viewer` mounted with `items` + `index` `481–492`.

`views/GamesView.svelte`:
- state `49–71`: `filter` (status, default `"all"`), `tagFilter`, `sourceFilter`
  (adds `no_local`), `srcStats`, `panel: "library"|"updates"|"pairs"|"matches"`, `updates`,
  `pairs`, `matches`, selection state `88–91`.
- `filtered` derived, **status filter client-side** `75–77`; used for the grid `575` and for
  "Select all" `360`.
- `load()` `115–143` — `getStats("game")` `118`, `listItems(..., "game", tag, undefined,
  source)` `119–127`, `topTags(24,"game")` `132`, `listUpdates(100)` `133`,
  `sourceStats("game")` `134`. `$effect` `145–151`.
- rails `399–456`: `Flow` (`401`, incl. the status loop `405–409`), `Sources`
  (`411–425`), `Tagged` (`427–441`), `Keep current` (`443–452`), `Vault` (`454–455`).
- bulk bar `358–374`; empty state mentions `tagFilter` `567`.

Shared pieces: `components/TileGrid.svelte:1–56` (the only genuinely shared list widget;
props `items, thumbs, showStatus, selectMode, selected, onOpen, onToggle`);
`components/Section.svelte` (editor only); `components/ItemEditor.svelte` tag chips
`967–991` (chips keyed by `t.typeKey`, `976`), add-tag input `989–990`, dropTag `339`;
`components/Viewer.svelte` navigates the **caller's array by index** (`42–73`, `342–396`,
counter `784`), so any change in the row set silently changes ←/→ semantics;
`lib/mediacache.svelte.ts` (thumb cache, `evictMedia` used by both bulk deletes);
`lib/activity.svelte.ts` (job polling, `onActivityChange` drives reloads).

CSS for the rails, `apps/desktop/src/app.css`: `.main` `184–188`, `.sidebar` `190–201`
(220 px, own scrollbar, #30), `.sidebar .section` `203–209`, `.nav-item` `211–224`,
`.nav-item:hover` `226`, `.nav-item.disabled` `230`, `.nav-item .count` `235–241`,
`.nav-item.active` `310–314`, `.vault-info` `315`, `.grid` `325`, `.chip` `564`,
`.chip-status` `575`, `.chip-site` `580`, `.chip-x` `585`, `.view-toolbar` `816`,
`.search-box` `925–937`. Layout guarantees are pinned by TESTING item 10
(`docs/TESTING.md:155–157`).

---

## 6. Tests / live probes / tool checks / TESTING items that pin current behaviour

Rust unit tests that must change with the filter/tag model:

| test | file:line | what it pins |
|---|---|---|
| `list_items_binds_limit_offset_without_search` | `db.rs:1626–1673` | placeholder order, `kind` tri-state, `search` on title |
| `source_state_filters_and_badges` | `db.rs:1676–1757` | the three `source=` predicates **and** `source_stats` numbers agreeing with them (`1748–1756`) |
| `tags_settings_updates_and_merge` | `db.rs:1822–1982` | tag filter (`1859–1868`), `top_tags` order/count (`1869–1871`), site filter + `counts_by_site` scoping (`1888–1902`) |
| `placeholder_titles_are_never_suggested` | `db.rs:1987–2022` | dupe pairing guard |
| `cross_site_pairs_win_and_a_bucket_is_capped` | `db.rs:2027–2068` | cap 12 |
| `media_pairing_by_shared_content_hash` | `db.rs:2107–2130` | media dupes by hash |
| `v3_migration_renames_mislabeled_cdn_sites` | `db.rs:2072–2103` | site-name normalization |
| `games_needing_match_targets_local_only_games` | `db.rs:2258–2293` | local-only predicate, third copy |
| `schema_initializes_and_is_idempotent` | `db.rs:1603–1623` | `SCHEMA_VERSION` equality + v9 table present (must be extended for v10) |
| `media::*` frame/dedup/best_for tests | `media.rs:717–839` | `frames_for`/`best_for`/`media_count` semantics that any `video:`/`image:` predicate relies on |
| `import_then_list_roundtrip_on_real_vault` | `lib.rs:2436–2508` | `list_items` over a real SQLCipher vault |
| `f95_thread_parse_title_version_tags` | `ingest.rs:3243–3251` | f95 tag token set (`renpy`) |
| `eh_gallery_page_reads_the_2026_shape` | `ingest.rs:3378–3424` | **the namespaced tag string shape** `["artist:horosuke","female:big_breasts"]` (`3399–3402`) — the assertion any provenance redesign must rewrite |

`#[ignore]` live probes (all in `ingest.rs` unless noted): `match_live_search_smoke`
`ingest.rs:3761–3808`; `f95_live_art_probe` `3809–3894` (needs `GOON_F95_COOKIE`);
`f95_live_clip_rehearsal` `3895–4146` (clips into a scratch encrypted vault and prints the
**vault rows**, incl. `tags N` at `3924–3925`); `eh_live_clip_rehearsal` `4147–4285`
(asserts `gallery.tags > 0` at `4191`, prints `tags` at `4181`, and the slideshow case
`4236–4240`); `eh_cookie_verdicts_live` `4286+`; `first_frame_live_probe`
`videothumb.rs:167–185` (`GOON_FF_PATH`); `live_site_answers` `http.rs:459` (the working copy also adds
`http::file_client_for` `http.rs:52` / `probe_client_for` `http.rs:65` — separate client
policies for file downloads vs probes, relevant to anything that fetches bytes for a
`size:`/`gif:` facet);
`live_f95_markers_separate_member_from_guest_under_our_ua` `cookies.rs:640`. How to run
them: `docs/TESTING.md:34–60`.

`tools/*.js` (there is **no existing filter check** — the closest patterns to copy):
`tools/view-zoom-check.js` (imports a pure TS module from `src/lib/`, asserts arithmetic;
DEVELOPMENT `docs/DEVELOPMENT.md:555–560`), `tools/viewer-layout-probe.js` (starts a copy
of the exe in `%TEMP%\goon-viewer-probe` with a throwaway vault, drives it over the
WebView2 DevTools port), `tools/viewer-real-media-probe.js` (same, plus
`window.__TAURI_INTERNALS__.invoke(command, args)` at `:108`, `:125`, `:174`; cases file
`tools/viewer-real-media-cases.json`, gitignored, example
`tools/viewer-real-media-cases.example.json`; `--reuse`, `--keep`; cookie env
`GOON_F95_COOKIE`/`GOON_EH_COOKIE`), `tools/char-audit.js`, `tools/lost-calls.js`
(diffs `await someApi(...)` + `onclick={handler}` per file — it will fire on the rail
rewrite), `tools/restore-nonascii.js`, `tools/char-compare.js`. Frontend gates:
`svelte-check` 0 errors **and 0 warnings**, `npm run build`
(`docs/DEVELOPMENT.md:546–564`, `docs/TESTING.md:10–20`).

`docs/TESTING.md` numbered items that pin filter/facet/tag behaviour: **14** (file links +
`Sources → No tags` + select-all + Re-fetch, `165–169`), **10** (sidebar layout/scroll,
`155–157`), **11** (dupe panel + Keep both, `158–160`), **19** (batch counters + no
placeholder dupes, `191–193`), **27** (e-hentai "16 tags arrive", `244–254`), **28**
(rule34 file link has *no* tags, `255–261`), **36** (real-material probe verdict + the
"no picture stored" gap #72/#64, `345–382`), and the `#[ignore]` list **§1** `34–60`
(incl. the `tags 0` f95 fact `66–77`).

---

## 7. Docs passages describing the current rails (all need updating)

- `docs/DESIGN.md:49` **D14** — "Media owns provenance (source-site + tag filters)"; the
  games/media filter split is stated here.
- `docs/DESIGN.md:136–138` §5 preamble ("Schema of record … anything marked *planned* is
  design intent"), `:151–156` tag types + the *planned* `tag_aliases` ("nothing translates
  between site vocabularies today, and the blocklist is a plain tag filter"),
  `:157` `media` BLOBs.
- `docs/DESIGN.md:172` (§6 "As built" `Meta{title, store_bytes, thumb_bytes, tags, version,
  note}`), `:175` booru family + "e926 filter wiring available via tag blocklist", `:178`
  f95 prefixes → `site_flag`.
- `docs/DEVELOPMENT.md:255–257` (tags/tag_types/item_tags: "`list_items(…, tag)` filters on
  them"), `:237` heading "schema v9" (→ v10), `:292–298` migration rule,
  `:339–344` **invariant 9 "Views stay separate"** ("site filters only in the Media view;
  the backend helpers take `kind` … so each view's sidebar counts are always scoped") —
  this is the invariant a unified query box most directly touches;
  `:365–373` invariant 12 (suggestions are proposals), `:455–460` invariant 21
  (no unverified identity), `:492–513` invariant 24 (toast vs state),
  `:782` (list_items wire-shape note), `:300–308` invariant 1 (SQLCipher `PRAGMA key`).
- `docs/USER-GUIDE.md:113–117` (per-view filters), `:399–409` (**Tagged**, **From site**,
  **Sources** rails, incl. "it stacks with search, tag and site filters"),
  `:389–392` (editor tags), `:413–423` (Possible dupes + Keep both), `:303` (viewer walks
  "the current grid (filters included)"), `:564` and `:568` (troubleshooting rows that tell
  users to find bare items under **Sources → No tags**).
- `PLAN.md:172` (the original "facets: source, status, tag types, artist/circle, date"),
  `:119` (`tag_aliases` "translate booru tags ↔ DLsite genres ↔ f95 prefixes into one
  facet"), `:187` (M2.5: "pipeline filters gone from media; site/tag filters gone from
  games"), `:421` and `:566` (handoff notes naming the three queues), `:306–310`
  (viewer navigates "the current filtered grid"), `:134` (`e926 filter wiring`).

---

## 8. Per-capability SQL feasibility **today**

Legend: ✅ expressible now with existing columns; ⚠ possible only with a fragile
approximation; ❌ needs schema and/or ingest change.

| requested meta tag | predicate possible today | verdict / what is missing |
|---|---|---|
| `video` | vault-stored only: `EXISTS (SELECT 1 FROM media m WHERE m.item_id = items.id AND m.kind = 'video')` — or `m.mime LIKE 'video/%'` | ⚠ only for the opt-in BLOB. A link/path video has **no** video marker in SQL: `video_source` uses `ingest::media_ext(url)` (`video.rs:43–48`); the only DB hint for `+ Video` items is the free-text `items.notes` string (`importer.rs:547`). Needs a `kind`-ish column on `sources`, a computed tag, or a `media_ext` mirror in SQL |
| `image` | `EXISTS (SELECT 1 FROM media m WHERE m.item_id = items.id AND m.kind = 'full' AND m.mime LIKE 'image/%')`; "has any picture" = `… AND m.kind IN ('thumb','full')` | ✅ for stored bytes; ⚠ for a file/link-only image (`full` row absent when >16 MiB, `ingest.rs:522–529`) |
| `gif` / `animated` | `EXISTS (SELECT 1 FROM media m WHERE m.item_id = items.id AND m.mime = 'image/gif')` | ⚠ stored gifs only. e621/dapi `file_ext`/`is_animated` are **never parsed** (`ingest.rs:679–688`, `720–734`); animated WebP/APNG are indistinguishable from stills (`media.rs:28–29` sniffs container, not frames); on-disk gifs (`file://` links) have no mime row at all — `source_images.mime` is NULL in every real code path (`db.rs:130`, `mark_source_image_stored` `db.rs:697–708` never sets it) |
| `no-thumbnail` | `NOT EXISTS (SELECT 1 FROM media m WHERE m.item_id = items.id AND m.kind = 'thumb')` | ✅ (the inverse is already the `has_thumb` column, `db.rs:444`). Caveat: an item can hold art but no tile — that is #52/#54, and HEIF art never gets a tile (`media.rs:116–121`) |
| `local-only` ("bytes on a disk path") | has a path: `EXISTS (SELECT 1 FROM sources sl WHERE sl.item_id = items.id AND sl.site = 'local')`; path **and nothing else**: `+ AND NOT EXISTS (SELECT 1 FROM sources so WHERE so.item_id = items.id AND so.site <> 'local')` | ✅ both forms exist (`db.rs:456–461`). ❌ "the file is actually present" is not SQL-able — presence is probed at runtime (`video.rs:128–130`, `extract.rs:49`, `lib.rs:1479–1485`) |
| `vault-stored` | `EXISTS (SELECT 1 FROM media m WHERE m.item_id = items.id AND m.kind IN ('full','video'))` | ✅ (a thumb-only item is *not* the original, so require `kind IN ('full','video')`; thumb-only = pre-#62 items, `media.rs:428–430`) |
| `link-only` | `NOT EXISTS (SELECT 1 FROM media m WHERE m.item_id = items.id) AND NOT EXISTS (SELECT 1 FROM sources sl WHERE sl.item_id = items.id AND sl.site = 'local')` (drop the second clause for "no vault bytes") | ✅ structurally; ⚠ semantic honesty: an item whose >16 MiB image was refused looks identical to a link-only item (`ingest.rs:522–529`) unless the note text is consulted |
| `no-tags` | `NOT EXISTS (SELECT 1 FROM item_tags tn WHERE tn.item_id = items.id)` | ✅ already `?7='no_tags'` (`db.rs:462–463`) and in `source_stats` (`db.rs:521`). Note: 40-tag ingest cap (`ingest.rs:2158`, `2343`) |
| `length>x` / `length<x` (duration) | — | ❌ **impossible today.** No `duration` column anywhere (`db.rs:64–73`), no `duration` token in `crates/` or `src-tauri/`. `video_info` (`video.rs:24–34`) has no duration field; the Shell first-frame path yields only `width/height` (`videothumb.rs:12–16`). Adding it needs: a new column (e.g. `media.duration_ms` or `items.duration_s`) + a v10 migration + a probe for stored BLOBs **and** for `file://` videos, or reading `file.duration` from booru JSON that is discarded at `ingest.rs:679–734`. On-disk videos need Media Foundation duration (`IMFSourceReader`) — no such code exists |
| `size>x` / `size<x` | vault bytes: `EXISTS (SELECT 1 FROM media m WHERE m.item_id = items.id AND length(m.data) > ?N)`; per-item total: `(SELECT IFNULL(sum(length(m.data)),0) FROM media m WHERE m.item_id = items.id) > ?N` | ✅ for vault BLOBs only (`length(data)` on encrypted rows is already used at `media.rs:309`, `436`). ⚠ mixed: an item may be "large" only in on-disk form — remote `filesize` is read once and thrown away (`ingest.rs:1843–1849`), on-disk size is only ever stat'ed live (`video.rs:129`) |
| (extra) dimensions `width>x` | `m.width`/`m.height` exist (`db.rs:70–71`) but are non-NULL **only on `thumb` rows** (`media.rs:239–240` vs `300`, `367`) | ⚠ a `width:` facet would measure the 360 px tile, not the art. Needs dims written on `full`/`video` rows |
| (extra) `source:<site>` | `EXISTS (SELECT 1 FROM sources ss WHERE ss.item_id = items.id AND ss.site = ?6)` — already `?6` (`db.rs:454–455`); the site list is `KNOWN_SITES` `ingest.rs:61–83` / `HOST_SUFFIXES` `90–117` | ✅ as-is, counts via `counts_by_site` `db.rs:353–369` |
| (extra) multi-tag AND / OR / NOT | only one exact tag name (`?5`, `db.rs:450–453`); no negation, no OR, no namespace wildcard | ⚠ needs the predicate rewritten (e.g. `GROUP BY … HAVING count(DISTINCT tag_id) = N`) — plus the provenance problem in §3 |
| (extra) `status:` | not in SQL at all; client-side `GamesView.svelte:75–77` | ⚠ add to `ListQuery`; `items_status_idx` `db.rs:76` already supports it |

---

## 9. Gotchas

1. **SQLCipher**: engine comes from `rusqlite` `bundled-sqlcipher-vendored-openssl`
   (`Cargo.toml:21`); `vault::open_db` (`crates/vault/src/lib.rs:124–136`) runs
   `PRAGMA key = "x'<hex>'"` **then** `PRAGMA cipher_memory_security = ON; PRAGMA
   foreign_keys = ON;`. Invariant 1 (`docs/DEVELOPMENT.md:302–308`): the PRAGMA must run
   before any query, and the key string must be zeroized only *after* `execute_batch`.
   Wrong key must surface as `InvalidPassphrase`, never as an empty DB — a new query path
   must not open a second connection. `cargo run -p goon-vault --example sqlcipher_probe`
   is the fast check.
2. **FTS**: no FTS table exists, and `rusqlite`'s `fts` Rust feature is not enabled — but
   the bundled amalgamation **is** compiled with `-DSQLITE_ENABLE_FTS3`,
   `-DSQLITE_ENABLE_FTS3_PARENTHESIS`, `-DSQLITE_ENABLE_FTS5`, `-DSQLITE_ENABLE_JSON1`,
   `-DSQLITE_ENABLE_RTREE` (verified at
   `libsqlite3-sys-0.35.0/build.rs:130–136`, which applies to the sqlcipher build too,
   `:146–147`). So `CREATE VIRTUAL TABLE … USING fts5(...)` inside the encrypted DB is
   available via plain SQL with no Cargo change — if added, it must be a **new numbered
   migration step** (`docs/DEVELOPMENT.md:292–298`), bump `SCHEMA_VERSION`
   (`goon-core/src/lib.rs:8`), and be covered by `schema_initializes_and_is_idempotent`
   (`db.rs:1603–1623`). `tags.name` has no COLLATE NOCASE and no normalization on the
   ingest path, so tag matching is byte-exact today.
3. **Honest counts**: the badge commands must stay derived from the same predicates as the
   grid (`db.rs:500–523` mirrors `db.rs:456–463`; `games_needing_match` `db.rs:1144` is a
   third copy). `counts_by_status`/`counts_by_kind` deliberately emit zeros
   (`db.rs:315–327`, `db.rs:337–341`). A query box that shows facet counts must compute
   them *under the current query*, or it re-introduces the "grid empty, badge says 12"
   class of lie.
4. **No unverified identity resolution (invariant 21, `docs/DEVELOPMENT.md:455–460`)**:
   a `source:` tag or a `md5`-based auto-tag must keep the `md5 && filesize` gate at
   `ingest.rs:1848–1850`. A filter may not invent provenance it never recorded; unknown
   tags must render as "unknown", like `cookie_ok: None` does (`api.ts:187–189`).
5. **Warnings are toasts, state stays in the panel** (invariant 24,
   `docs/DEVELOPMENT.md:492–513`; `lib/toast.svelte.ts`, `lib/mediacache.svelte.ts`):
   a rejected query expression or an unknown meta tag belongs in `bad(...)`/`warn(...)`,
   never in a `*Msg` `<p class="report-inline">`. `endVaultSession()` clears toasts and the
   media cache (`lib.rs:281–284`), and `app_status.generation` invalidates cached
   decrypted material — a filter cache in the webview must obey the same rule.
6. **Vault can close under you**: long loops must take the inner `Mutex` per item and
   re-check `inner.opened` (invariant 8, `docs/DEVELOPMENT.md:328–338`; the pattern is
   `reenrich_item` `ingest.rs:2489–2493`). A new bulk "select all matching query" command
   must run as a job (`jobs.rs`, one job per kind, `Jobs::start` returns `None` if one is
   running) and honour `job.cancelled()`.
7. **BLOB transport is base64** (`get_thumbs` `lib.rs:919–933`) and there is a
   **no-Blob rule** in the webview (D22 note, `docs/DESIGN.md:57`); a `size:`/`bytes:`
   facet should use `length(data)` server-side, never ship BLOBs to count them.
8. **Placeholder discipline**: `params!` fills positions in order — the current SQL skips
   `?1`-`?3` groupings deliberately (`db.rs:441–442`). Building dynamic predicates with
   `format!` (as `source_stats` does at `db.rs:506–512`) is the existing style, but keep
   every literal value bound, not interpolated.
9. **Media vs game scoping**: `kind='media'` means `('gallery','post')` in four places
   (`db.rs:302–305`, `356–358`, `448–450`, `501–504`, `927–929`) — a shared query engine
   should extract that once.
10. **Live probe tooling a new check should join**: `tools/viewer-real-media-probe.js`
    is the only harness that fills a throwaway vault from real links and then invokes
    commands inside the app (`window.__TAURI_INTERNALS__.invoke(...)` at
    `:108`/`:125`/`:174`); the Rust-side equivalent is `ingest.rs`'s
    `f95_live_clip_rehearsal` `3895–4146` / `eh_live_clip_rehearsal `4147–4285`, which
    print what the **vault** holds (pattern described in `docs/TESTING.md:62–77`). A tag/
    filter check belongs as a sibling: seed via `clip_url`/`import_bookmarks`, then call
    `list_items`/`top_tags`/`source_stats` through `invoke` and assert the row ids, not the
    report strings. Add its number to `docs/TESTING.md` §3 and its name to the `#[ignore]`
    list at `docs/TESTING.md:34–45`.

### What makes the requested meta tags impossible (or dishonest) today

- **Duration (`length>x`, `length<x`)**: no column, no reader, no decoder path that yields
  it — for **stored** and **on-disk** video alike. Requires a v10 column plus a new probe
  (Media Foundation `IMFSourceReader` duration, or parsing the booru API's `file.duration`
  that `ingest.rs:679–734` discards). This is the only capability in the list with no
  partial path at all.
- **`gif`/animated** for anything the vault does not hold: no mime row, and
  `source_images.mime` is always NULL in real paths (`db.rs:697–708`).
- **Dimensions for originals**: columns exist but are NULL on `full`/`video`
  (`media.rs:300`, `367`), so any `width:`/`height:` facet today measures tiles.
- **`size:` beyond vault bytes**: remote `filesize` and on-disk `stat` are transient
  (`ingest.rs:1843–1849`, `video.rs:76`).
- **Provenance on ordinary tags**: `tags.name UNIQUE` with no site column
  (`db.rs:52–56`), ingest writes site-relative vocabularies verbatim (booru bare names vs
  e-hentai `ns:token`), plus a `type_id` split that already duplicates `renpy` across
  `site_flag` (importer) and `content` (f95). Backfilling provenance means a migration
  that *infers* site from `sources` — which invariant 21 forbids doing confidently when an
  item has several sites.
- **Select-all / bulk semantics**: "Select all" is a client-array copy capped at the
  `limit` (300), so "act on everything matching this query" has no server-side path today
  (`MediaView.svelte:302`, `GamesView.svelte:360`, `db.rs:425–427`).
