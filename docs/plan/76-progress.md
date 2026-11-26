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
  keyword, while the `<site>:<name>` form (`ehentai:big_breasts`) stays a literal
  tag because decision 2 deliberately does not reserve that shorthand — a sugar
  added later cannot break a query that exists today. Autocomplete note for S5:
  when a keyword form matches nothing, the suggester may add "no items with
  `ext:x`; did you mean the tag literally named `ext:x`?" — that is what
  `Node::ExactTag` exists for.
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

**Two things this stage caught before a DB ever ran.** `type:video`'s extension arm
was one parenthesis short — an AST test would have passed it and SQLite would
have refused to prepare it. The paren-depth half of
`every_placeholder_has_exactly_one_parameter` is what found it, and it stays as an
invariant for every fragment S3 adds. And `id:` was first validated as a bare
alphanumeric word, which would have made the predicate useless in the field: the
plan's sketch says `id:<ulid>`, but `new_id(prefix)` writes
`c-1f9a2b7c11d4e5a6b7c8`, so every id in a real vault carries a hyphen. Checked
against the code rather than the sketch, and pinned by
`an_id_predicate_is_exact_and_knows_the_real_id_shape`.

**Not in this stage.** Nothing runs yet: `plan()` has no production caller until
S3's `db::search_items`, and there is no command, wire type or UI — the stage
order in plan §7. The query text is session state (new DEVELOPMENT §4 invariant
28): `query.rs` has no connection and no write path, so nothing about a search
can reach `meta`, disk, or a locked vault.


---

## S2 review pass — the grammar's second half: what the planned SQL **selects** (2026-11-26)

The second grammar review said *yes, with fixes*: the tokenizer, the AST and the
"never interpolate" discipline were accepted, but every predicate had only been
tested as **text**. This pass adds the other half — a synthetic library and the
planned `WHERE` run against it — and fixes the four blockers with their
should-fix list in one commit.

**B1 — `stored:` had three values where the app has four queues.**
`stored:disk` as a preset for the old *local only* badge would have silently
widened it: the badge means "a local source **and** no source from any other
site", `stored:disk` means only the first half. So `stored:` now takes
`vault | disk | local-only | link` (`closed_values("stored")` is what the error
text lists, so a wrong value is told the real vocabulary), and
`StoredShape::LocalOnly` writes the rail's SQL word for word, aliases `sl`/`so`
included, from `db.rs:763`. `the_old_source_queues_are_the_same_predicate_in_grammar_text`
proves the identity twice over: the planned rows are the badge rows, **and** the
badge count from `db::source_stats` equals the query count for all three queues
(`local_only` ≡ `stored:local-only`, `no_local` ≡ `-stored:disk`,
`no_tags` ≡ `-has:tags`). Counting the same predicate two ways is the point -
two predicates that happen to agree today are not the same predicate.

**B2 — the epoch arithmetic overflowed.** A 12-digit year parsed, multiplied and
added without checks: a panic under `cargo test`, a silently wrapped timestamp in
release, which is worse because it selects the wrong rows for 30 years and never
mentions it. Years must be four digits, a bare epoch at most 12 digits and at most
`LAST_DATE_EPOCH` (9999-12-31), and every conversion is `checked_mul`/`checked_add`
with a "too far away" error. `length>999999999999999:00` is refused for the same
reason on the milliseconds side.

**B3 — a negated fact is not `NOT (…)`.** `NOT (bytes > ?)` is *true* of an item
nobody ever measured, so `-size>1gb` used to select every un-probed item: "never
measured" is not the fact "small", and the two being confused is exactly the lie
invariant 27 exists to stop. `Cmp::opposite_sql` (`>`→`<=`, `=`→`<>`) plus
`Term::self_negates` make the contrary comparison the plan, in one place, and
`a_negated_fact_never_counts_an_unmeasured_item` runs a never-probed row through
every fact predicate **and** its negation and requires it to be missed by both.
Existence and derived predicates (`has:`, `stored:`, `source:`, `ext:`, a tag, a
date) keep the plain complement, because nothing behind them is unknown.
`type:` negation is derived the same way: `-type:video` wants a still row,
`-type:image` wants a video row *or* an animated one.

**B4 — `key=value` was read as a tag.** The plan's §4 table writes `rating=3` and
`type=video`; `split_operator` only knew the comparison characters, so those two
examples became tag names and matched nothing. `=` now splits when the left side is
a keyword — and only then, which is what keeps `size-ish`, `id#3` and `added-daily`
literal tags (`an_equals_sign_makes_a_predicate_and_a_dash_does_not`).

**Also in this pass.**

* `MAX_TERMS` **50** and `MAX_INPUT_CHARS` **2 048**. Past roughly 500 `?` marks
  SQLite stops answering with a result and starts answering with a prepare error,
  so the grammar says no first and names the limit, pointing at the term that
  crossed it. A query box that fails with "too many parameters" is a bug report;
  one that says "50 terms is the most one query can be" is a sentence.
* Invisible and lookalike characters inside a term are refused: control, bidi and
  format characters, ZWSP, an en/em dash where the negating `-` was meant, a
  fullwidth colon where the keyword separator was meant. This is grammar, so
  **quoted** text is exempt — a scraped title's soft hyphen and NBSP are data and
  stay reachable. NBSP outside quotes splits terms, because that is what it does.
* Empty `""`, `circle:` and `tag:` values are refused rather than turned into a
  `LIKE '%%'` that matches the whole library — the one place a typo could silently
  *widen* a search.
* `Plan { sql, params, unmeasured }` and `Plan::needs_facts()`: the planner
  reports which probe-only columns its own fragments read (`FactKey::Pixels`,
  `Motion`, `Duration`, `CachedBytes`, `Extension`), and `FACTS_PENDING_SQL`,
  `SOURCE_FACTS_SEEN_SQL` and `EXTENSION_TAGS_SEEN_SQL` ask the vault in one cheap
  `SELECT 1 … LIMIT 1` whether it holds them. That is the difference between a UI
  that says "not measured yet · measure my files" and one that prints
  `0 matches` for a fact nobody ever measured — which reads like an answer.
* Media predicates are `EXISTS`-shaped, never `JOIN media`, and that is now said
  in the `Plan` doc comment: one item has `thumb`, `full` and `video` rows, so a
  join multiplies the grid page *and* the paired `COUNT(*)`. `ratio` reads media
  **and** `sources` (`ratio_guard`) for the same symmetry stage S6 needs.
* `circle:` and `rating:` are valid in **both** views
  (`circle_and_rating_are_valid_in_both_views_because_items_carry_them`); only
  `status:` is Games-only. Plan §1's "Games get `status:`/`circle:`; Media never
  do" contradicted §4's own vocabulary table and the schema — `items.circle` and
  `items.rating` are columns of the shared `items` row and an e-hentai gallery
  really has a circle — so the §1 and §4 sentences are corrected here, in this
  commit. `keyword_hint(scope)` is generated from `keywords()`, so a hint string
  cannot drift from the grammar it describes.
* `ingest::VIDEO_EXTS` is an alias of `goon_core::query::VIDEO_EXTS`; the
  tautological drift test that only compared the alias to itself is gone,
  replaced by `media_ext_follows_the_grammars_video_list`, which iterates the
  grammar's list and asserts the importer classifies each one.
* The digit pin (`a_value_is_bound_and_never_written_into_the_sql`) now names its
  exceptions instead of hard-coding two replacements: `CODE_OWNED_DIGITS` lists the
  only fragments of planned SQL allowed to contain a number (`SELECT 1 FROM`,
  `animated = 0`, `animated = 1`, `height > 0`), and `CODE_OWNED_LITERALS` the only
  string (`local`, the site slug of invariant 10). A new arm that wants a literal
  has to be added to that list, which is the whole point of the test.

**Measured** (`tools/cargo-test-s2fix.log`, `tools/cargo-clippy-s2fix.log`,
`tools/filter-scale-20261126.log`):

* `cargo test --workspace` = **209 passed, 0 failed, 8 ignored** (goon-core
  **102**, desktop **101** + 8 ignored, vault 4, locator 2) — the grammar module
  now holds **62** tests, **15** of them added in this pass, all against synthetic
  fixtures except the ones that read the schema.
* `cargo clippy --workspace --all-targets` = **0 warnings**; `rustfmt` clean on the
  touched files; `node tools/char-audit.js` **CLEAN**.
* The example set that has to hold for every fragment grew from 22 to **28**
  queries, and each still prepares, binds and steps against the real v10 schema.
* **The 100k scale probe plan §8.3 owed** now exists:
  `crates/goon-core/examples/filter_scale_probe.rs`, release run logged to
  `tools/filter-scale-20261126.log`. Fixture: **100 000 items**, 10 tags each ≈
  **1 000 000** `item_tags` rows over 20 000 distinct tags, half vaulted, 70 % of
  those probed, one item in twelve untagged, one in six not knowing its extension;
  built in **13.9 s**, 104 MiB, plaintext SQLite on purpose (the plan shape is what
  is being measured, not the codec's per-page cost, so these numbers are a floor).
  Timed as the two statements stage S3 will emit — the `LIMIT 300` page and the
  paired `COUNT(*)` — p50/p95 over 21 rounds, with the matched row count printed,
  because a timing table of empty sets measures nothing.
* Slowest page: the dense tag `big_breasts` (**64 092** matches) at **197 ms p50 /
  199 ms p95**; `big_breasts -anal` **190 ms**; everything else under **80 ms**,
  several under 10 (`stored:local-only` 1 ms, `size>100mb type:image` 2 ms,
  `-size>1gb type:gif` 10 ms, a date window 0 ms).
* **The `CAST(created_at AS INTEGER)` cost, asked for by name:** `EXPLAIN QUERY
  PLAN` says `SCAN items` for `added:>=…`, `size>100mb` and `type:video`, then
  `SEARCH m USING INDEX media_hash_idx (item_id=? AND kind=?)` per item — an index
  on a TEXT stamp is useless under `CAST`, so those predicates are item-at-a-time
  scans with an index probe, and their cost tracks the **row count**, not the match
  count. A tag query is the opposite shape: `MULTI-INDEX OR` over `tags_fold_idx`
  / `tags_name_idx`, then `item_tags_tag_idx` as a covering index, then
  `SEARCH items USING INDEX sqlite_autoindex_items_1`, so it gets slower the more
  items match. At 100k rows both are comfortably under a second, so the `CAST`
  stays — it is what makes `'999999999' > '1000000000'` stop being true.
* **The paired count is the one to watch:** 86 ms for `big_breasts` and up to
  **171 ms p95** for `ratio:>2 height>800`, against a page of 197 ms and 8 ms
  respectively, because a `LIMIT 300` page can stop early on the `updated_at` order
  and a total cannot. S3's job is not to make `COUNT(*)` fast but to stop running
  it on every keystroke.

**What the probe caught that the fixture was lying about.** Three residues
collided: the gif rule (`n % 40`) landed inside the video rule (`n % 20`), so every
animated item was a video and `type:gif` timed an empty set; the local-source rule
landed on the same residue as the `game` kind rule, so `stored:local-only` matched
nothing inside the media scope; and untagged items all landed on one site, so
`-has:tags source:ehentai` matched nothing. All three read as "0 matches" and were
only visible because the log prints the row count next to the timing. That is the
reason the count is printed, and the reason this file exists as a committed
example instead of a scratch script.

**Two findings that are stage S5's to handle, not the grammar's.**

* `ext:gif ext:mp4` — one of plan §4's worked examples — matches **0 rows by
  construction**: an item carries exactly one `extension` tag
  (`db::set_item_extension_tag`), so the conjunction can never be true. The
  grammar is right to AND them; the UI has to notice an unsatisfiable conjunction
  of same-key predicates and suggest the disjunction instead of showing an empty
  grid.
* Tags are **binary and case-sensitive** (`t.fold = ?`, so `Big_Breasts` finds
  nothing) while the title `LIKE` is not. Deliberate — a fuzzy tag match would
  invent equivalences nobody declared — but the box's placeholder and the
  "no results" row have to say so, or the difference looks like a bug.

**The backfill gap, recorded where #77 will read it.** Schema v10 seeds the
`extension` tag *type* and `set_item_extension_tag` exists, but nothing calls it,
so `ext:` and `type:video`'s extension arm find nothing for pre-v10 items today -
which also means `-type:video` currently under-excludes until B3's contrary arm and
the extension tags both exist. `EXTENSION_TAGS_SEEN_SQL` is the cheap runtime check
that says so. #77's S7 job therefore has to derive extension tags three ways, not
one: from stored `mime`, from the `sources` url suffix, and from disk paths — the
probe seeds type-4 tags for exactly that future state and leaves 1 item in 6
knowing nothing, so the "not yet backfilled" case stays measurable.
