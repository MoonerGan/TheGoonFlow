# #76 progress log — filter overhaul

One dated entry per stage, with the numbers the stage actually measured.
Plan: `76-plan.md` as amended by `76-rulings.md`; seams in `76-map.md`.

---

## S1 — schema v10: tag provenance, media/source facts, indexes, backfills (2026-11-26)

**What landed.**

* `SCHEMA_VERSION` 9 → 10 (`crates/goon-core/src/lib.rs`), one new ordered
  idempotent `if v < 10` step (`migrate_v10`, `crates/goon-core/src/db.rs`) —
  the original `CREATE` batch was not edited.
* `tags` rebuilt to `id, site, ns, name, fold, type_id` with
  `UNIQUE (site, name)`, reusing every old tag id so `item_tags` needs no
  remapping. `DROP TABLE tags` would cascade every tag link away, so the
  `DROP`/`RENAME` swap runs with `PRAGMA foreign_keys = OFF` (it cannot change
  inside a transaction), restored straight after `COMMIT`, then
  `PRAGMA foreign_key_check` proves nothing was orphaned — a rebuild that lost
  links fails the migration instead of shipping a quiet empty tag cloud.
  Rollback on any error inside the swap.
* `fold` is computed in Rust against a closed, code-owned namespace list
  (`TAG_NAMESPACES`, `tag_ns_fold`): `female:big_breasts` → `big_breasts`,
  `f95-completed` → itself, `foo:bar` → itself. Backfill sets `site = ''` for
  every existing row — provenance is **not** guessed from the item's sources
  (invariant 21).
* Facts columns, all NULL-means-unprobed: `media.bytes`, `media.duration_ms`,
  `media.animated`; `sources.bytes`, `sources.duration_ms`, `sources.width`,
  `sources.height`, `sources.probed_at` (the last five are what the S7 facts
  engine will fill for disk paths and bare links — nothing writes them yet).
  Backfill in-step: `bytes = length(data)` for stored rows, and
  `animated = 1` where the stored mime is `image/gif` (rulings A: gif by mime);
  the S7 container scan refines a single-frame gif to 0.
* Extension tag type seeded: `tag_types` id 4 `extension` / label `File` /
  priority 5, plus `TYPE_EXTENSION`, `tag_get_or_create_site`,
  `set_item_extension_tag` and `extension_of` (url/path → `jpg`, refuses page
  urls and nonsense). `f95-completed` corrected from `content` to `site_flag`
  (D9, plan §9 decision 11).
* Indexes: `tags_fold_idx`, `tags_name_idx`, `tags_site_name_idx`,
  `item_tags_tag_idx`, `sources_site_item_idx`. `media(item_id,kind)` from the
  plan's list was **skipped**: `media_hash_idx (item_id,kind,hash)` already
  covers that prefix, and a second index would be pure write cost.
* Writers fill the facts they hold: `put_full_image` writes `bytes`, the
  original's `width`/`height` (header read, no decode) and `animated` read from
  the container's own chunks (gif frame count, PNG `acTL`, WebP `ANIM`/`ANMF` —
  new `is_animated` / `image_dimensions` in `media.rs`); `put_video` writes
  `bytes` and leaves duration/dimensions NULL for the probe; `set_thumbnail`
  writes `bytes` and deliberately no `animated` (a re-encoded tile says nothing
  about the original). `merge_items` carries the fact columns so a merge never
  un-probes what was measured.

**Measured.**

* `cargo test --workspace` (dev-env): **146 passed, 0 failed, 8 ignored** —
  goon-core **40** (6 new here), goonflow-desktop **100 passed + 8 ignored**
  (1 new), goon-vault **4**, goon-locator **2**. Log: `tools\cargo-test-s1.log`.
* `length(data)` under SQLCipher is exact, not an estimate: the new
  `encrypted_media_length_equals_the_plaintext_length` opens a real encrypted
  vault, stores a **1 048 613**-byte body (1 MiB + 37, many pages deep) and
  reads back `bytes = 1 048 613` from both the writer and the backfill
  statement, with the BLOB round-tripping byte for byte.
* v9 → v10 upgrade test (`v10_rebuild_keeps_links_folds_names_and_backfills_facts`):
  hand-built v9 vault (6 tags / 6 links / 2 media rows) → after two runs of
  `init_schema`: schema **10**, links **6/6** intact, `0` rows with a site,
  `female:big_breasts` fold `big_breasts`, `foo:bar` fold `foo:bar`,
  `f95-completed` type **2**, media bytes **6** and **7**, gif animated **1**,
  jpeg animated **NULL**, and the 5 indexes present.
* Container-chunk animation (`animation_comes_from_the_container_chunks`):
  encoder-written 1-frame gif → `Some(false)`, 2-frame → `Some(true)`; hand-built
  3-descriptor gif → `Some(true)`; PNG with `acTL` → `Some(true)`; WebP with
  `ANMF` → `Some(true)`, WebP without → `Some(false)`; mp4 → `None`; truncated
  gif/png/webp → `None`. Two fixture bugs the tests caught and I fixed: the
  colour-table size is `3 * 2^(n+1)` bytes (my first walk used `3^n`), and a
  graphic-control extension needs its block terminator.
* Not run in this environment: the live UI probes and the 100k scale probe
  (they belong to S3/S5/S7); nothing in S1 touched the frontend, so
  `npm run build` / `svelte-check` are unchanged this stage (they were run to
  give `cargo test --workspace` its `frontendDist`, and the build is green).

**Docs.** DESIGN §2 row **D26** (the filter model) + §5 tag/media rows;
DEVELOPMENT §3 moved to schema v10 with the new table notes and the v10
migration entry, §4 invariant **27** "an unprobed fact is NULL and never
matches"; TESTING §3 item **39** with the numbers above.

**Rebase (done at this stage boundary).** `git rebase origin/main` onto
`98ae41f fix(72)` — both #76 commits replayed on top of it. One conflict,
`docs/TESTING.md`: main's item **38** (the #72 picture-ceiling item) and my new
item now sit side by side at the list indentation the neighbouring items use
(their 5/8-space indent came out of main's own nesting, so 38 was re-indented
to 3/6 to match 36/37/39). `crates/goon-core/src/media.rs` and
`docs/DEVELOPMENT.md` auto-merged: main's `MAX_MEDIA_BYTES` (96 MiB) landed
next to the new fact writers and **no new byte ceiling is added anywhere in
this card** — anything that needs one takes it as an argument, per that commit.
Numbering after the rebase: DEVELOPMENT §4's new invariant is **27** (main
took 26), TESTING's new item is **39** (main took 38). Tests re-run after the
rebase, same numbers as above.

**Next — S2.** `crates/goon-core/src/query.rs`: the pure parser (EBNF from plan
§4 as amended: reserved keys `type ext source tag has stored status circle
rating size length width height ratio id added updated`, `-` negation, quoted
phrases, `key>` / `key:>` / `key>=` spellings, `1:30` / `90s` / `2m` durations
and `kb/mb/gb` sizes) plus the predicate planner, with a unit test per worked
example. No DB in that stage.

**Question for review.** The rulings' `ext:` key is now a reserved word in the
grammar. If an existing tag is literally named `ext` with a colon in it
(`ext:something` as a booru token), it will parse as a predicate instead of a
tag. Nothing in the repo's fixtures looks like that; I will quote-escape it in
the parser tests (`"ext:something"`) and keep the behaviour — flag it if you
would rather `ext:` only be reserved when the value is a known extension.

---

## S1 follow-up — the facts are measured, and measuring is a job (2026-11-26)

Caught while re-reading `76-map.md` §10 note 1 after the S1 commit: the plan's
`width>`/`height>` predicates were listed as if they were free, and the map's
whole point is that they were not — `media.width/height` were only ever written
on `thumb` rows, so a `width>1200` query measured the 360 px tile and lied.

**What landed.**

* `media.probed_at` (v10 column, idempotent `ALTER` guard like the rest). It
  marks that a container has been *asked* about a row — including when it
  answered nothing — which is what makes a backfill pass resumable.
* `db::refine_stored_facts(conn, limit)`: the bounded pass. For each unread
  `full` row it reads the picture's own pixels from the header (no decode) and
  `animated` from the container's chunks, writes `probed_at`, and uses `IFNULL`
  for the two measurements so a container that will not answer can never erase a
  fact some earlier path recorded. Rows already read are skipped by the marker,
  so cancellation and resume are free. Thumb rows are out of scope entirely.
  `pub` now so stage S7's job can drive it.
* **It is deliberately not in `migrate_v10`.** My first version put the scan in
  the migration; that was wrong. The scan reads every original once, so on a
  large vault it means minutes of blocking between the passphrase and the grid —
  exactly what D15 ("jobs, not modal blocking") forbids. The migration now does
  only what SQLite can prove without moving a BLOB (`bytes = length(data)`) and
  turns the old `0 px` into NULL. The version stamp moved back into the same
  batch.
* `put_full_image` writes `probed_at` as it writes the other facts (it did read
  the container), `put_video` does not (nothing probed the duration), so the job
  picks video rows up later. `merge_items` carries `probed_at` with the row.
* `db::now_epoch_text()` — the schema's epoch-seconds-as-text timestamps in one
  place, which the `added:`/`updated:` predicates will compare against in S2.

**Measured.**

* The pass costs about what reading the library once costs: **71 ms for 5 000
  tiny originals** (0.014 ms/row, header parse dominates) and **185 ms for 1 000
  rows at 200 KB** (0.19 ms/row, the BLOB read dominates ≈ 1 GB/s). A re-run over
  the same rows: **0.9 ms, scanned 0**. Extrapolated honestly, a 100k-item
  library of 2 MB originals is ~3 minutes of reading — a job with progress, not
  an unlock step.
* `cargo test -p goon-core` = **40 passed, 0 failed**. The v10 test's media
  fixtures are now **real files** bound as parameters (`media::fixtures` —
  `gif_with_frames`, `sample_png`, `gif_with_descriptors`, `png_with_chunk`,
  `webp` — now a shared `#[cfg(test)] pub mod fixtures` so `db.rs` and
  `media.rs` prove the same thing about the same bytes): after the migration
  every file-shaped fact is NULL, then `refine_stored_facts(&conn, 2)` returns
  `scanned 2, dims 2, animated 1, still 1`, writes `4x4/animated=1` and
  `8x8/animated=0`, the third call scans 0, and the tile keeps `animated NULL`.
  `stored_facts_are_written_with_the_row` also pins that a `full` row comes back
  probed and a `video` row does not.

**Process note, so the record is complete.** While timing that pass I used
`Get-Content -Raw` / `Set-Content` on `db.rs` — the exact thing `TESTING.md` §4
forbids. It wrote a BOM, mangled 43 em dashes / `§` / `…` into Shift_JIS
mojibake and swallowed three newlines. Repaired with
`node tools/restore-nonascii.js` (34 lines restored from HEAD, one merged pair)
plus a hand fix of the 9 lines that script could not resolve because they were
new this session; verified with `node tools/char-audit.js` (**CLEAN**, 0
unexpected characters) and a byte inventory against `HEAD` (`§x6 —x26→34 …x2
↔x2 ·x3` — nothing lost, 8 em dashes added by the new comments). Temporary
timing code was removed from the source; the numbers live here and in DEVELOPMENT
§3 / TESTING item 39.

---

## S2 — the grammar: `goon-core::query` (2026-11-26)

Written against the review's three requirements for this stage.

**What landed.** `crates/goon-core/src/query.rs`, new: tokenizer → AST → SQL
plan, with no database handle anywhere in the module.

* `parse(text, Scope::Media | Scope::Games) -> Result<Query, ParseError>`. A
  `Term` keeps the exact text it was typed as, because that is what stage S5's
  chip row renders and deletes; `ParseError { message, token }` is a value the
  form shows inline (invariant 24), never a toast and never a guess.
* `Query::plan() -> Plan { sql, params }` — one `WHERE` body of AND-ed fragments
  with `?` marks and the parameters produced in the same pass, so a fragment can
  never drift from its binding (the failure mode of the old `?1 IS NULL OR …`
  shape, `db.rs`'s list query). S3 joins it with the view's `kind` predicate and
  binds `Plan::bind()`.
* **Requirement 1 — `ext:` is always reserved.** Keywords are a closed,
  code-owned set and reservation never consults a table, so `ext:banana` is a
  predicate exactly like `ext:gif` (`ext_is_reserved_whatever_the_value_is`).
  The escape hatch is pinned in both directions: `tag:"ext:something"`,
  `tag:"type:video"` and `tag:size` reach a literal name that contains (or is) a
  keyword. Autocomplete note for S5: when a keyword form matches nothing, the
  suggester may add "no items with `ext:x`; did you mean the tag literally named
  `ext:x`?" — that is what `Node::ExactTag` exists for.
* **Requirement 2 — dates are integer comparisons.** The parser converts the
  typed value itself (`2026-11-01`, `2026-11-01T14:30`, `…:14:30:15`, or a bare
  epoch) and the plan compares `CAST(items.created_at AS INTEGER)` /
  `CAST(items.updated_at AS INTEGER)` with only `Value::Int` bound — never a
  date string. Both spellings parse (`added:` and `added:>=`), and a bare
  `added:2026-11-01` is the **half-open day** `[1793491200, 1793577600)`, not an
  equality against a second-resolution stamp. `>`, `<`, `>=`, `<=` are mapped
  onto that interval so "after that day" means the next day, not 00:00:01 of it.
  `days_from_civil` is written inline (25 lines) rather than adding chrono to
  goon-core, and the day boundary is **UTC** — the only clock `now_epoch()`
  writes — which DEVELOPMENT §3 states instead of promising the user's timezone.
  The pinned trap: `('999999999' > '1000000000')` is **1**, and the same pair
  under `CAST` is **0** (`a_date_is_compared_as_a_number_never_as_text`).
* Fact predicates never read a `thumb` row (code map review note 1): every
  `size>` / `width>` / `height>` / `ratio:` / `stored:vault` arm says
  `m.kind IN ('full','video')`, and only `has:thumb` may look at the tile.
  `ratio` guards `width IS NOT NULL AND height > 0` before dividing, because an
  unmeasured fact must neither match nor divide (invariant 27).
* Negation is always SQL — `NOT (items.id IN (…))`, `NOT EXISTS (…)` — so the
  count, the grid and *Select all* act on the same whole-library set (rulings C).
* A quoted phrase's LIKE wildcards are escaped (`50%_off` → `%50\%\_off%` with
  `ESCAPE '\'`): titles in this library are full of `_`, and an unescaped LIKE
  would read it as a one-character wildcard and match the wrong items.
* `ingest::VIDEO_EXTS` **is** `goon_core::query::VIDEO_EXTS` now, one list, with
  `the_video_extension_list_is_the_one_the_grammar_reads` pinning it, so
  `type:video` cannot miss a file the importer called a video.

**Requirement 3 — #77 stays visible.** `db::set_item_extension_tag` still has no
production call site; that is what the two stages below owe it. The writers, by
name:

| stage | call site | what it writes |
|---|---|---|
| S4 | `ingest::apply_meta` (`ingest.rs:2359`) — the clip and re-enrich write loop | extension tag from the source url |
| S4 | `ingest::local_file` (`ingest.rs:1996`) and the direct-media path (`ingest.rs:1806`) | tag from the file / direct-media url |
| S4 | `importer::import_image_files` (`importer.rs:401`) — *+ Images* | tag per file (the `full` row's facts already come from S1's writer) |
| S7 | `importer::import_video_files` (`importer.rs:499`) — *+ Video* | tag, plus duration and dimensions from the OS probe |
| S7 | `importer::import_game_folder` (`:189`), `import_game_exe` (`:221`), `import_game_zip` (`:242`) — folder import | tag from the `file:///` source |
| S7 | bookmark enrichment (`bookmarks.rs:308`, which runs `apply_meta`) | inherits the clip row |

I will repeat that list in the S4 and S7 entries when the call sites actually
exist — that is the point at which #77 moves.

**Measured** (`tools/cargo-test-s2.log`, `tools/cargo-clippy-s2.log`):

* `cargo test --workspace` = **194 passed, 0 failed, 8 ignored** (goon-core
  **87**, desktop **101** + 8 ignored, vault 4, locator 2) — 47 new grammar tests
  plus the one drift pin in `ingest.rs`.
* `cargo clippy --workspace --all-targets` = **0 warnings**; `rustfmt` clean on
  the touched files; `node tools/char-audit.js` **CLEAN**.
* parse + plan over 20 000 rounds: **0.0032 ms** for one term, **0.0088 ms** two,
  **0.0096 ms** three, **0.0162 ms** five, **0.0199 ms** seven. Typing the longest
  worked example parses every one of its **77** prefixes in **0.62 ms** total, so
  S5's suggester can re-plan on every debounce tick with no cache.
* The **22**-query example set prepares, binds and steps against the real v10
  schema in memory (`every_example_is_sql_the_schema_can_run`).

**One bug this stage caught before a DB ever ran.** `type:video`'s extension arm
was one parenthesis short — an AST test would have passed it and SQLite would
have refused to prepare it. The paren-depth half of
`every_placeholder_has_exactly_one_parameter` is what found it, and it stays as
an invariant for every fragment S3 adds.

**Not in this stage.** Nothing runs yet: `plan()` has no production caller until
S3's `db::search_items`, and there is no command, wire type or UI — the stage
order in plan §7. The query text is session state (new DEVELOPMENT §4 invariant
28): `query.rs` has no connection and no write path, so nothing about a search
can reach `meta`, disk, or a locked vault.
