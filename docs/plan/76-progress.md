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
  *(Corrected in the 2026-11-26 paging-and-index entry, which measures both halves
  per query shape: "stops early" is true of the predicate that can walk
  `items_updated_idx` and false of a dense tag match, where the page (197 ms) is the
  expensive half, not the count. The bullet's conclusion survives; its reason does
  not.)*

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

## S3 — running the plan: `db::search_items` and the four readers that must agree with it (2026-11-26)

The grammar proved what a query *means*; S3 makes it *select rows*. One new
composition point and five readers sit on it, and the point of the shape is that
the grid page, its total, the select-all list, the tag cloud and the site counts
become **one predicate seen five ways** instead of five sentences that happen to
agree. All in `crates/goon-core/src/db.rs`, still no DB handle anywhere in
`query.rs` (invariant 28).

**The one place SQL is built: `planned_where(scope, plan)` (`db.rs:813`).** It
emits ` WHERE ` + the view's own `kind` scope + ` AND (` + the plan body + `)`, and
every query-shaped read calls it. Two things that shape is deliberate about:

* the **view** decides which kinds exist and the query never does (D14). That is
  why `search_items(conn, scope, plan, limit, offset)` takes the scope as its own
  argument rather than the grammar growing a `kind:` word, and why a `status:`
  plan parsed in the games grammar and run through the media scope returns
  nothing: the scope is AND-ed in first, always. D14 stops being a UI convention
  and becomes SQL a caller cannot forget.
* `items` stays **unaliased** in every host statement, because every planned
  fragment is written against that name. An `AS i` on the grid query would break
  the plan, not the query, which is worth knowing before anyone "tidies" it.

**The five readers.** `search_items` (`db.rs:862`) returns
`SearchPage { items, total, needs_facts }` (`db.rs:838`); `count_matches`
(`db.rs:898`) is the total by itself, for callers that only want a number;
`search_item_ids` (`db.rs:909`) is every matching id in grid order with no
`LIMIT`, which is what "select all" has to mean once the grid is a query
(rulings C: plan §9's "there is no server-side *everything matching this query*"
is a description of the old code, not a decision); `top_tags_matching`
(`db.rs:930`) and `counts_by_site_matching` (`db.rs:962`) are the tag cloud and
the site counts **of the current result**, so a chip can never offer a tag or a
site the active query already excluded — the drift the whole-library rails lived on.

`total` is the whole result, never the page. `needs_facts` is
`Plan::needs_facts()` riding along on the same value, so the UI has the fact in
the same object as the count: `total = 0` **and** `needs_facts` is "not measured
yet · measure my files", not an answer (invariant 27).

**Order is `updated_at DESC, items.id`, and the `id` is not decoration.** The
fixture writes `q_big` and `q_small` with the same stamp because that is exactly
what an importer does to a batch; without the tie-break page 1 and page 2 are
independent sorts, so a paged read repeats one row and silently drops another.
`select_all_ids_are_the_whole_result_in_grid_order` walks five pages of one row
each and requires every id exactly once. The `updated_at` half is compared as
**TEXT**, with no `CAST`: correct only while every stamp the writers make
(`now_epoch_text()`) has one width, and it is what lets the page walk
`items_updated_idx` instead of sorting the table. The predicate side needs the
`CAST` for `added:`; the order side must not have one — that asymmetry is now said
out loud in the fixture doc comment and pinned in the test list.

**The scale probe stopped timing a copy of the code.** Until now
`filter_scale_probe` assembled the page and count SQL itself, so it measured
something production never ran. It now calls `db::search_items` (page **and**
paired total, the one call the grid makes) and `db::count_matches` separately,
and feeds `EXPLAIN QUERY PLAN` the string from `db::planned_where`; the hand-written
statement timer is gone, replaced by a `timed` closure that drains its result.
That rewrite also moved the conclusions, because the "grid" column used to hide
the count inside it:

| query | matches | page + total p50 / p95 | count alone p50 | page alone |
| --- | ---: | --- | ---: | ---: |
| `big_breasts` | 64 092 | 288 / 290 ms | 88 ms | 200 ms |
| `big_breasts -anal` | 29 654 | 330 / 331 ms | 137 ms | 193 ms |
| `size>100mb type:image` | 17 018 | 157 / 158 ms | 153 ms | 4 ms |
| `ratio:>2 height>800` | 6 467 | 184 / 184 ms | 174 ms | 10 ms |
| `-size>1gb type:gif` | 5 000 | 172 / 173 ms | 162 ms | 10 ms |
| `stored:local-only` | 20 000 | 92 / 92 ms | 90 ms | 2 ms |
| `added:>=2024-06-01 updated:>2023-01-15` | 7 876 | 16 / 16 ms | 15 ms | 1 ms |
| `ext:gif ext:mp4` | 0 | 24 / 25 ms | 12 ms | 12 ms |

So the earlier "everything but two queries is under 80 ms" was a page-only truth.
At 100k items the **total** is the expensive half for every fact query (153 of
157 ms, 174 of 184 ms, 90 of 92 ms) and the cheap half is the page (1 — 10 ms). That
is the number stage S5 has to design around: re-run the page on the debounce
tick, never the total per keystroke. The 0-match row is still printed with its
count, which is the honesty rule the fixture residues taught.

**Measured** (`tools/cargo-test-s3.log`, `tools/cargo-clippy-s3.log`,
`tools/filter-scale-20261126.log`):

* `cargo test --workspace` = **217 passed, 0 failed, 8 ignored** (goon-core
  **110** (was 102), desktop **101** + 8 ignored, vault 4, locator 2).
* **8 new tests** in `db::tests` against a new synthetic vault
  (`query_fixture()`, ids `q_big` / `q_small` / `q_unprobed` / `q_video` /
  `q_disk` / `q_game`, built with this module's own helpers where they reach):
  `the_page_and_the_paired_total_come_from_one_predicate`,
  `a_never_measured_item_matches_neither_a_fact_nor_its_negation`,
  `the_total_counts_the_result_and_the_page_limits_the_rows`,
  `select_all_ids_are_the_whole_result_in_grid_order`,
  `the_view_scope_is_what_keeps_kinds_apart_not_the_query`,
  `the_tag_cloud_and_the_site_counts_follow_the_query`,
  `a_query_reading_unmeasured_facts_says_so_on_the_page`,
  `the_scope_is_anded_before_the_plan_never_replaced_by_it`.
  The invariant-27 one is the reason this stage exists: `q_unprobed` has a stored
  row with every fact column NULL and is matched by **neither** `size>1gb` nor
  `-size>1gb`; `q_video` keeps `animated` NULL, because a container says nothing
  about frames and writing 0 would make `-type:gif` a lie, so that row answers
  neither `type:gif` nor `-type:gif` either.
* `cargo clippy --workspace --all-targets` = **0 warnings**; `rustfmt` clean on
  `db.rs` and the example (`cookies.rs`, `http.rs`, `ingest.rs`, `video.rs` are
  still non-clean on main, untouched here); `node tools/char-audit.js` **CLEAN**.
* Probe re-run: same fixture (**100 000 items**, ~1 000 000 `item_tags`, 104 MiB,
  built in 13.8 s), 18 queries, p50/p95 over 21 rounds, every timing printed
  beside its matched row count.

**Deliberately not replaced yet.** `list_items`, `source_stats` and
`games_needing_match` still exist and are still what the two views call, because
the rails that call them come out in S5/S6 — removing the callers and the SQL in the
same commit is how a review loses the ability to see what changed. Their three
counts were measured instead of assumed: each planned facet count costs the same
**45·90 ms** at 100k as the ones S3 now runs, so a sidebar that re-counted all three
per keystroke would cost ~200 ms per keypress for numbers that shrink to nothing
once the rails go. Nothing drifts meanwhile, because the grammar tests already
prove `stored:local-only` ≡ `local_only`, `-stored:disk` ≡ `no_local` and
`-has:tags` ≡ `no_tags` against `db::source_stats` itself.

**Docs updated in this commit.** `docs/DEVELOPMENT.md` §3 gains a
"**Running the query box**" bullet: the one-composition-point rule, the
scope-is-SQL rule, `SearchPage`, the `updated_at DESC, items.id` order and the
TEXT-stamp contract, and what is *not* replaced yet. `docs/TESTING.md` item 41 is
rewritten around the numbers above (it described the page-only probe), and item
**42** records the eight S3 predicate tests.

**For S4 (the wire).** `search_items` / `count_matches` / `search_item_ids` are
ready to wrap as commands; the `api.ts` mirror has to carry `needs_facts` across
the boundary or the front end cannot tell "0 matches" from "not measured yet".
`top_tags_matching` is the suggestion source for `filter_suggest` — it is already
result-scoped, so the suggester does not need its own filtering. The
`add_item_tag` site argument and the extension-tag writer call sites listed under
#77 stay queued for their own stage.

---

## S4 — the wire: three commands, one view word, and a suggester that counts through the same SQL (2026-11-26)

**What landed.** No new SQL shape — S3's readers are what runs — and one new column
write. S4 is the boundary: three Tauri commands, the view's **name** on the wire, a
split for reading the box while it is being typed, and a `site:` argument on
`add_item_tag`. Four files: `crates/goon-core/src/query.rs`,
`crates/goon-core/src/db.rs`, `apps/desktop/src-tauri/src/lib.rs`,
`apps/desktop/src/lib/api.ts`.

**The commands, and the one place a view can be wrong.** `planned_query(view, text)`
(`lib.rs:897`) is the only code between a wire argument and a `Plan`, and the three
commands call nothing else: `search_items(view, query, limit, offset)` →
`SearchPage` (`:913`), `search_item_ids(view, query)` → every matching id in grid
order (`:938`), `filter_suggest(view, text, limit)` → `Vec<Suggestion>` (`:954`),
registered at `:2403-2406`. `limit` defaults to **300**, which is plan §6's
"the ceiling has to become visible rather than stay silent": the page carries an
`offset`, and `total` rides along in the same object. One naming hazard for a
reviewer: `db::Suggestion` (`db.rs:1007`) is an autocomplete row, while
`db::SuggestionRow` (`db.rs:1750`) is a game *match* suggestion from an older stage.
Two things, two names; do not merge them.

**The view travels as a word.** `Scope::parse_wire` (`query.rs:71`) accepts exactly
`media` and `games` and anything else is a `ParseError` whose message names both.
A boolean, or an `Option` where `None` means "no scope", is how a frontend typo ends
up quietly searching the other library; refusing by name is invariant 10's rule
applied to the wire instead of to the database. And an error crosses as **the
sentence the user reads**: `planned_query` maps `ParseError` to its `message`,
because stage S5's job is to put that sentence inline under the box as form state
(invariant 24). The offending token stays inside `ParseError` for a frontend that
wants to underline it.

**The suggester is split where the rest of the query path is split.** `query.rs`
reads the half-typed token and touches no database (invariant 28):
`parse_suggestion` (`:538`) returns `SuggestCtx` (`:500`) — the query *without* that
token, the negated flag, the keyword, the value — and `value_completion` (`:521`)
decides what may be completed at all: `type:`, `stored:`, `source:` and `ext:` yes;
`size: length: width: height: ratio: rating: added: updated: circle: id:` no, because
the only value available would be an invented one (invariant 27). `tag:` and `circle:`
get nothing either — the bare-word path already lists the vault's tags, and asking
for an exact spelling leaves nothing to guess. `db::suggest` (`db.rs:1036`) then
counts candidates through **the same readers the query would run**
(`counts_by_site_matching` `:978` for `source:`, `top_tags_matching` `:941` for a bare
word and for `ext:`), so the number beside a row is the number pressing Enter prints.
A candidate that would leave the result **empty** is dropped — that is the dead-end
chip the old rails were known for — while a grammar row keeps `count: None`, meaning
"no honest number", never zero. The minus rides inside the token (`-has:tags`), so
clicking a row replaces the term instead of silently dropping the negation.

Two bugs the new tests caught, both worth recording:

* `keyword_of` (`query.rs:585`) first stripped one operator character with an
  `unwrap_or` fallback that restarted from the original slice, so `size:>100`
  completed as `:>100` and `type:vi` produced `":vi"`. The operator run is now
  consumed as a run (`:` and/or `>`/`<`/`=`, in either order), which is also what makes
  `size>10` and `size:>10` the same keyword: the parser and the suggester cannot
  disagree about what a token is.
* `top_tags_matching` grouped by `t.id`, which in the fixture is two chips
  (`big_breasts (1)` and `female:big_breasts (1)`) that each **deliver two rows** —
  a bare tag term matches `t.fold = ? OR t.name = ?`, so the two spellings are one
  family, which is exactly what `tag:"…"` exists to override. It now groups by
  `tt.key, t.fold`, labels the row with the family's shortest spelling, and orders
  `c DESC, length(name), name`, a real total order where `c DESC, name` over a grouped
  fold is not. That is why the line numbers in the S3 entry have drifted
  (930 → 941, 962 → 978).

**`add_item_tag` learned who spells a tag.** `tag_site_slug` (`lib.rs:1283`) trims,
lower-cases, and refuses anything outside `ingest::KNOWN_SITES` — a made-up slug would
sit in `tags.site` where `source:` can never reach it (invariants 10 and 21) — and the
write goes through `db::tag_get_or_create_site` (`db.rs:1463`) plus `link_item_tag`
(`:1526`) rather than `add_item_tag`'s site-blind path, which every importer still
uses. The desktop test is the honest version of that claim, told in both directions:
`'ehentai'` on the site-spelled row and `''` on the hand-typed one; then
`source:ehentai tag:"female:big_breasts"` matching **nothing** until the `sources`
ledger row a clip writes exists. Tag provenance is not item provenance, and a
recorded spelling must not fabricate a source (invariant 21).

**`api.ts` mirrors it** — `SearchPage` (:270), `Suggestion` (:281), `searchItems`
(:287), `searchItemIds` (:303), `filterSuggest` (:308), `addItemTag(…, site)` (:409) —
camelCase fields over snake_case wire arguments (invariant 6). `listItems` and its
`tag`/`site`/`source` arguments stay, because plan §5 removes them in the **same**
commit as the rails that call them (S5); there is never a moment where two wire
shapes are both live.

**Cost, stated honestly rather than measured twice.** The scale probe was **not**
re-run: S4 changes no plan shape and no SQL the probe times, so its numbers are the
S3 ones. What S4 does spend per keystroke-set is one facet read — the same
`counts_by_site_matching` / `top_tags_matching` shape measured at **45·90 ms** at
100k items in S3 — which is why `filter_suggest` belongs on the debounce tick next to
the page, and why each candidate pool is a `LIMIT`ed `ORDER BY count DESC` read with
the prefix filter applied in memory. That last part has a known cut, recorded here
rather than waiting to be found in the field: a bare word or `ext:` can only offer
tags inside the top-`limit` window, so a rare extension or a tag ranked 40th is not
suggested until the prefix narrows. The fix would be a prefix-filtered read through
`planned_where`, which costs more per keystroke than the window buys.

**Measured** (`tools/cargo-test-s4.log`, `tools/cargo-s4-desktop.log`,
`tools/cargo-clippy-s4.log`):

* `cargo test --workspace` = **229 passed, 0 failed, 8 ignored** (goon-core **120**
  (was 110), desktop **103** + 8 ignored (was 101), vault 4, locator 2).
* **12 new tests**: 5 in `query::tests` (the two-word view vocabulary, the mid-flight
  token split, a colon inside a tag name is not a keyword, "nothing to suggest", and
  "only values it can be honest about"); 5 in `db::tests`, against the S3
  `query_fixture()` so the candidate counts and the page counts come from one vault
  (grammar first then the vault's tags, a closed keyword is never a made-up number, a
  site completion counted through the query's own SQL, the minus surviving into the
  token, `ext:` reading the tag table the predicate reads); 2 in
  `integration_tests` (the wire's view and grammar refusals; the tag-site provenance
  identity above).
* `cargo clippy --workspace --all-targets` = **0 warnings**; `rustfmt --edition 2021
  --check` exits 0 for `query.rs` and `db.rs`, and for `lib.rs` reports **0 diff blocks
  in that file** — its 40 blocks are all in the four known non-clean siblings
  (`cookies.rs`, `http.rs`, `ingest.rs`, `video.rs`), which are left exactly as
  committed; `npm run build` clean (**135** modules); `svelte-check` **0 errors,
  0 warnings**.
* `node tools/char-audit.js` over all **seven** touched files (the eighth dropped out
  when the branch was rebased onto main, whose own `char-audit.js` rewrite took the
  allow-list change): one hit,
  `query.rs:456` **U+FF1A** — and that hit is on purpose: the line is the doc comment
  naming the fullwidth colon the grammar refuses, so it is an example of the bug, not
  the bug. Two fixes to the audit itself, because the S3 entry's "**CLEAN**" was
  **vacuous**: `char-audit.js` reads only `process.argv` file arguments, so running it
  with none prints `CLEAN` without opening anything, and `±` (the zoom buttons), `’`
  (prose) and `≡` (the predicate-identity sign S3 wrote) were missing from its
  allow-list, which buried real findings under three known-good lines in
  `TESTING.md`/`DEVELOPMENT.md`. It now reports 0 lines for six of the seven files and
  that one deliberate hit — **`U+FF1A` was deliberately not added to the allow-list**,
  since a fullwidth colon is exactly what the tool exists to find.
* Self-inflicted damage, caught before the commit: `rustfmt` follows `mod`
  declarations, so formatting `lib.rs` had also rewritten `cookies.rs`, `http.rs`,
  `ingest.rs` and `video.rs` — ~470 lines of pure reflow in files this stage never
  touches. All four are reverted to HEAD, and the trap is written down in
  `docs/TESTING.md` §1 so the next stage checks `git status --short` after formatting.

**Docs updated in this commit.** `docs/DEVELOPMENT.md` §3 gains "**The query box on
the wire**": the three commands and `planned_query`, the two-word view vocabulary,
`ParseError.message` as form state, how `db::suggest` counts through the query's own
readers and what it refuses to complete, and the tag-name-vs-item-source rule.
`docs/TESTING.md` gains item **43** (the twelve tests), loses a duplicated paragraph
item 41 picked up in S3, and §1 gains the rustfmt warning plus the note that
`char-audit.js` needs its file arguments. `tools/char-audit.js` itself gains three
allow-listed codepoints (`± ’ ≡`), documented by the comment next to them.

**For S5 (the media view).** Everything the UI needs is callable: one `searchItems`
per debounce tick — page **and** paired total in one call, 16-341 ms at 100k items, so
the tick is what protects typing and the debounce is not optional (the follow-up entry
has the split) — the suggestion rows with their counts, `searchItemIds` for select-all,
and `needs_facts` beside `total` for the "not measured yet" row. "Load more" is
`after`: the cursor `{ updatedAt, id }` of the last row on screen, taken from
`ItemSummary` fields the grid already renders — there is no page number and no offset
to keep in state (invariant 30). S5 also carries the deletions that make
this one wire shape instead of two: `list_items`, `source_stats`, the four facet rails,
and `listItems`' `tag`/`site`/`source` arguments, plus `tools/filter-ui-probe.js`.
Two facts the UI has to say out loud, both already true in the data: tags are binary
and case-sensitive while the title `LIKE` is not, and `ext:gif ext:mp4` is
unsatisfiable by construction (one `extension` tag per item).

---

## S4 follow-up — the audit's last two glyphs, and open decision 10 closed by measurement (2026-11-26)

Two pieces of work sit between S4 and S5. One closes a gate main handed this branch;
the other answers the one demand on #76 that had an argument instead of a number
attached: *is the grid paginated by row count or by a cursor?*

**The audit's last two glyphs** (commit `9753579`). Main's repo-wide
`char-audit.js` (77ba492) reported five lines in two files from this branch: `❌` ×3
and `∈` ×1 in this map's own feasibility legend — `✅` and `⚠` were already
allow-listed, so `❌` joins them with the reason stated and `∈` is element-of notation
in one state-vocabulary line — and one fullwidth colon at `query.rs:456`. That last
one is not a glyph problem: it sat inside the doc comment that *explains* the
grammar's refusal of fullwidth lookalikes, so an auditor reading the finding cannot
tell it from a PowerShell round-trip, and a gate that exits non-zero on it can never
go green. The comment names the codepoint now (`U+FF1A`) and the refusal it documents
is untouched — **still not allow-listed**, because a fullwidth colon in real text is
exactly what the tool exists to find. Bare `node tools\char-audit.js`: **115 file(s)
audited / CLEAN**, exit 0.

**The measurement.** `filter_scale_probe` grew two sections, and both are shaped to
measure the SQL production runs rather than a paraphrase of it:

* `paging` times `db::page_statement` itself (the shipping statement, cursor variant),
  against `offset_page`, the shape this stage replaced, built from the same
  `db::PAGE_COLUMNS` and `db::GRID_ORDER`. That mattered: the first version compared a
  two-column offset page against a seven-column cursor page and made the cursor look
  **60 ms slower at page 1**, which was the thumbnail `EXISTS`, not the paging. Both
  shapes now return the same seven columns, the probe asserts they return the same page
  size at every depth, and `EXPLAIN QUERY PLAN` is printed for each so the diagnosis
  sits in the log rather than being inferred from a number. Fixture: 100 000 items,
  ~1 000 000 `item_tags` rows, 20 000 tags, db 104 MiB, plaintext SQLite (a floor, not
  a locked vault's number), 21 reps, p50/p95 in ms, offset → cursor:

  | query | matches | page 1 | page 4 | last page |
  | --- | --- | --- | --- | --- |
  | `big_breasts` | 64 092 / 214 pages | 198/198 → 198/201 | 202/203 → 198/200 | 200/201 → **91/91** (page 214, 192 rows) |
  | `big_breasts -anal` | 29 654 / 99 pages | 189/190 → 188/190 | 191/192 → 188/189 | 188/190 → **136/137** (page 99, 254 rows) |
  | `-size>1gb type:gif` | 5 000 / 17 pages | 11/11 → 11/11 | 45/45 → **18/19** | 187/193 → **7/7** (page 17, 200 rows) |

  The plans explain the two shapes of the result. The tag queries end in
  `USE TEMP B-TREE FOR ORDER BY`: SQLite materialises all 64 092 matching ids and sorts
  them before `LIMIT`, so the sort dominates and the cursor only saves the walk over
  the skipped rows (0 ms at page 1, 109 ms at page 214). The fact query walks
  `items_updated_idx` and prints `USE TEMP B-TREE FOR LAST TERM OF ORDER BY`, which is
  where an offset is pure throwaway work: **180 ms to reach page 17 of a 5 000-row
  result**, 7 ms for the cursor.
* `delete shift` is the correctness half, and it is the half that decides. Four pages
  read the way the grid read them, then 20 000 of the matched rows deleted in grid
  order through the same predicate — the size and shape of #73's *Delete selected* —
  then the continuation read both ways:

  ```
  delete shift "big_breasts": 64092 matches, 20000 deleted in grid order
    (1200 of the 1200 rows on screen were among them)
      offset continuation starts at survivor #1200, so 1200 surviving rows were passed over
      cursor continuation starts at survivor #0, so 0 surviving rows were passed over
  ```

**The decision (plan §9 decision 10, closed).** The grid pages by cursor:
`search_items(view, query, limit, after)`, `after = { updatedAt, id }` — the sort key
of the last row actually on screen — and there is no `OFFSET` anywhere in the query
path. The cursor is never the slower shape at the top (198/198 against 198/201 at page
1, i.e. within noise), it is up to 180 ms cheaper at depth, and the offset shape is
simply wrong for a library where bulk actions run while a page is open: 1 200 rows
that were never shown leave the walk. "Load more" means "the next 300 after this row".
Two details that make the cursor safe rather than clever: the order is one string,
`db::GRID_ORDER`, used by the page, by `search_item_ids` and by the probe, and
`db::AFTER_CURSOR` is its cursor form (`<` on the stamp, `>` on the id inside the tie)
— a cursor that compared the pair differently would skip or repeat rows at exactly the
stamps an importer writes in one second. `updatedAt` crosses as the stored TEXT stamp,
because that is what the grid orders by; casting it would compare in one order and
sort in another.

**What changed.** `db.rs`: `PageCursor`, `GRID_ORDER`, `PAGE_COLUMNS`, `AFTER_CURSOR`,
`page_statement`, and `search_items(…, after)` in place of `search_items(…, offset)`.
`list_items` keeps its offset and its **tie-break-less** `ORDER BY` — the same defect
in a worse form — and stage S5 deletes it. `lib.rs`/`api.ts`: the command argument and
the `PageCursor` type. Two new pins: `a_bulk_delete_cannot_make_a_paged_grid_skip_rows`
(goon-core, 5 rows, both shapes, the offset one kept as the counter-example) and
`the_page_cursor_travels_the_wire_on_the_grid_sort_key` (desktop, serde spelling plus
five rows sharing one stamp). `select_all_ids_are_the_whole_result_in_grid_order` walks
by cursor now and asserts the page past the end is empty while `total` still says 5.

**Gates.** `cargo test --workspace` = **231 passed, 0 failed, 8 ignored** (goon-core
**121**, desktop **104** + 8 ignored, vault 4, locator 2), `tools/cargo-test-s5-paging.log`;
release probe `tools/filter-scale-20261126d.log`; `cargo build -p goon-core --examples`
with no compiler warnings beyond the vendored-openssl PDB line; `rustfmt --check` 0 for
`db.rs` and the probe; `npm run build` and `svelte-check` **0/0** (`api.ts` is in this
contract); `node tools\char-audit.js` = **115 file(s) audited / CLEAN**.

**The honest budget outcome, which goes on #76 rather than staying in a log.** Plan §6
asked for the §4 worked set "under ~50 ms end-to-end" at 100k items. **The budget is
missed, and the miss is not the paging.** Same fixture, page+total p50/p95: 16/16
(`added:>=2024-06-01 updated:>2023-01-15`), 24/24 (`ext:gif ext:mp4`), 34/34
(`circle:"Red Sun" rating:>3`), 38/38 (`ext:gif`) — and 285/289 (`big_breasts`),
339/341 (`big_breasts -anal`), 227/228 (`width<400 type:image -has:thumb`), 180/181
(`ratio:>2 height>800`). Of 18 worked rows, **4 are under 50 ms**; the slowest grid
call is 341 ms. Splitting the cost: the paired count alone runs 12-171 ms, and the
`big_breasts` page by itself is 198 ms, because the plan sorts all 64 092 matching ids
in a temp B-tree before `LIMIT 300`. What follows for S5, and is now evidence rather
than a guess: the total must not be paid per keystroke — and as written `search_items`
**always** pays it, since the page and the count are one call — so the debounce tick is
what protects typing today, and a page-only read (the same `page_statement` without the
`count_matches` beside it) is one function away if S5's own measurement says it is
needed. Decision 9 (FTS5 vs `LIKE`) should be reopened on the
dense-tag-sort number rather than on a title-scan guess, and this card cannot be called
Done against the §6 budget as written — the board should either restate the budget as
"narrow predicates under 50 ms, dense tags measured and paginated" or buy the index
that removes the sort.

## 2026-11-26 — S3/S4 follow-up: what a page costs, measured four ways

**Why.** The review accepted the cursor and asked for the 196 ms dense-tag page to be
explained before the UI built on top of it. Three hypotheses were on the table — buy an
index on the grid’s sort key, switch the cursor to a row-value comparison, stop paying the
count per keystroke — and one sentence in the S3 entry needed correcting. All four are now
answered with numbers in `tools/filter-scale-20261126h.log` (100k items, 21 reps,
p50/p95, plaintext SQLite, so a floor), and TESTING item 45 carries the tables.

**What landed.**

* `db::page_items` and `search_items(…, with_count)`: the page and the total are one call
  or two, chosen by the caller. The count is a second statement over the whole result, and
  measured against the page it is the expensive half for every fact query (page 3 ms /
  count 151 ms for `size>100mb type:image`), so it is what a keystroke skips.
  `SearchPage.total` became `Option<i64>` — uncounted crosses as `null`, never as `0`,
  pinned on the wire as well as in the library.
* Two checks that can fail, as the review asked: the row-value cursor dropping a row of a
  same-second batch, and the stamp-width assumption (`the_stamp_is_one_width_…` shows both
  the agreement on one-width data and the disagreement on mixed widths). DEVELOPMENT gains
  invariant 31 for the second, plus the row-value warning beside the cursor bullet.
* Four probe sections, printing plan and row count beside every timing: `index options`,
  `both halves DESC`, `tag predicate shape`, and a cardinality ladder for the crossover
  (synthetic alias tags, created and dropped inside the section).

**The outcomes.** The composite index: not bought — it fixes the plan it can reach (3 ms →
2 ms, `OR` cursor → two seeks) and leaves `big_breasts` at 196 ms. The row-value cursor:
seeks, and is wrong for a mixed-direction order; the correct version wins nothing
(89/94 ms against the shipping cursor’s 90/91 ms); and the probe cannot even see the
tie — its 100k fixture stamps every row with a different second, so all three page reads
report "matches" there, which is why the fixture test rather than the probe is the pin.
64 092-row id list; the row-first probe over the same indexes pages those rows in 1 ms,
and tag-first is 100-140 ms better at the other end of the range (0 ms vs ~100 ms at 157
matches), which makes the choice a per-token one — plan open decision 13, with the
crossover measured at about a fifth of the library and the deciding query at 0-1 ms.

**Correction to the record.** The S3 entry’s claim that a `LIMIT 300` page "can stop early
on the `updated_at` order and costs 1-10 ms" is true only of predicates that walk
`items_updated_idx`. A dense tag match cannot stop early at all: SQLite collects and sorts
every matching id in a temporary B-tree before the limit, which is why its page (196 ms) is
the expensive half rather than its count (86 ms). The S3 bullet now carries the correction
inline, and `db::search_items` says the same thing where a caller would read it.

**UI consequence to carry into S5 and S6.** Keyset paging means there is no page number to
jump to: the honest controls are *load more* (cursor of the last visible row) and *back to
start*. A "page 4" button would have to re-scan to know where page 4 starts, which is the
cost this stage just measured. The games view and #73 inherit that shape rather than
reinventing offsets.

**Gates.** `cargo test --workspace` = **235 passed, 0 failed, 8 ignored** (goon-core **124**
was 123, desktop **105** + 8 ignored was 104, vault 4, locator 2) in
`tools/cargo-test-s5-count.log`; `cargo clippy --workspace --all-targets` 0 findings
(`tools/cargo-clippy-s5-count.log`); `rustfmt --check` clean on the two Rust files touched,
with the four hand-formatted siblings restored.

---

## Branch rewrite — fixture URLs that could not be proved synthetic (2026-11-27)

**What was found.** The review’s leak scan flagged a gallery URL in stage S4’s test
data: an e-hentai gallery link with a six-digit id in `src-tauri/src/lib.rs`
(`1ca918f`, `sources` fixture). It was typed as a fixture in a stage that only ever
opened in-memory SQLite, and that id appears nowhere on `main` — but "typed as a
fixture" is not proof, and a well-formed gallery link is exactly what the audit has to
refuse, because it is indistinguishable from a link out of a real library. The rule is
that a fixture must be **self-evidently** fake.

A second one, mine rather than the reviewer’s, is the same shape: a gelbooru CDN link
in `crates/goon-core/src/db.rs` (`d3f59c4`). Its filename and post id are the
repo’s long-standing synthetic tokens — the same filename is on `main` in the 4cdn
ingest fixtures — but the path shape makes it a plausible clip link, so it went too.

**What was done.** `git rebase -i d140602` with `edit` on `d3f59c4` and `1ca918f`, one
line changed in each, then `git push --force-with-lease`. Old tip `b0a6cfc`; rewritten
range `5a4402c`…`341d556`, which is the parent of this entry. `git diff b0a6cfc
341d556` is exactly two lines, and `git grep` over every tree in `origin/main..HEAD`
(12 trees) finds no trace of either string. `main` was not touched and nothing merged
was rewritten. The replacements are `…/g/9999999/0123456789abcdef/` and
`…/images/00/00000000000000000000000000000000.jpg?9999999`; no assertion referenced
either value, so the suites move unchanged (235 passed / 0 failed / 8 ignored).

**The guard.** `tools/fixture-url-audit.js` (TESTING item 46, DEVELOPMENT invariant 32)
audits the added lines of a branch range and prints its coverage. It exits 1 on the
history it was written for — 2 violations, naming both commits — and 0 on the rewritten
branch, so the check is known to be able to fail. Honest limit, recorded because it is
the interesting part: force-pushing removes the ref, not the object. The old tip may
stay reachable on the host until garbage-collected, so the string is treated as
disclosed and reported to the board, not as retracted.