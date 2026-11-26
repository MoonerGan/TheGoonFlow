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
