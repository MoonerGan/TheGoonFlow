//! Probe: does the #76 query grammar hold up at the design target of **100k
//! items**? Plan §6 asks for measurement instead of assumption, and plan §8.3
//! names this file: it builds a throwaway vault (100k items, ~1M `item_tags`)
//! and prints p50/p95 for every worked example of plan §4, through the stage S3
//! entry points themselves — `db::search_items` (the grid page plus its paired
//! total, the one call the desktop makes) and `db::count_matches` (that total on
//! its own, so the difference between the columns is what the page alone costs).
//!
//! Run: `cargo run -p goon-core --release --example filter_scale_probe`
//!
//! Two honesty notes about what this measures:
//!
//! * the database is **plaintext** SQLite, not SQLCipher, because the point is
//!   the plan shape (index seeks vs scans), not the codec's per-page cost. Read
//!   these numbers as a floor, not as the number a locked vault shows;
//! * the fixture is synthetic on purpose (ids, titles and tag names that cannot
//!   be mistaken for a real library), and `media.bytes` is a made-up number: the
//!   vault rows hold an 8-byte stub, not 6 MB of picture.
//!
//! It writes its database under `%TEMP%` and deletes it again; nothing here
//! touches a real vault.

use goon_core::db;
use goon_core::query::{self, Scope};
use rusqlite::Connection;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// How many items the fixture holds, and how many tags each one carries. The
/// plan's target is 100k items; 10 tags each is the ~1M `item_tags` row count
/// plan §8.3 names, because that table - not `items` - is what a tag query walks.
const ITEMS: i64 = 100_000;
const TAGS_PER_ITEM: usize = 10;
const TAG_POOL: usize = 20_000;
const REPS: usize = 21;

/// The grid's ceiling, the same one the desktop command has always passed.
const PAGE: i64 = 300;

/// The queries being timed: plan §4's worked examples plus the shapes the grid
/// actually runs (a date window, a bare tag, a fact range, a negation, and the
/// free-text phrase that stays an un-indexed `LIKE`).
const QUERIES: &[&str] = &[
    "big_breasts",
    "female:big_breasts",
    "source:ehentai tag:\"female:big_breasts\"",
    "big_breasts -anal",
    "\"summer witch\" -source:local",
    "type:video length<1:30",
    "size>100mb type:image",
    "stored:vault type:video",
    "-has:tags source:ehentai",
    "width<400 type:image -has:thumb",
    "ratio:>2 height>800",
    "circle:\"Red Sun\" rating:>3",
    "added:>=2024-06-01 updated:>2023-01-15",
    "-size>1gb type:gif",
    "stored:local-only",
    "type:video length>2m -stored:vault",
    "ext:gif ext:mp4",
    "ext:gif",
];

fn main() -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!(
        "goon-filter-scale-{}-{}",
        date_stamp(),
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{dir:?}: {e}"))?;
    let path: PathBuf = dir.join("scale.db");
    let _ = std::fs::remove_file(&path);

    println!(
        "fixture: {ITEMS} items, ~{} item_tags rows, {} distinct tags",
        ITEMS as usize * TAGS_PER_ITEM,
        TAG_POOL
    );
    let built = Instant::now();
    let conn = Connection::open(&path).map_err(|e| e.to_string())?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "synchronous", "OFF")
        .map_err(|e| e.to_string())?;
    db::init_schema(&conn).map_err(|e| e.to_string())?;
    seed(&conn).map_err(|e| format!("seed: {e}"))?;
    conn.execute("ANALYZE", []).map_err(|e| e.to_string())?;
    let build_ms = built.elapsed().as_millis();
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!("build: {build_ms} ms, db {} MiB\n", bytes / 1_048_576);

    // Every number below comes out of stage S3's own functions, not out of a
    // copy of their SQL: a probe that rebuilt the statements by hand would be
    // timing something that never runs in production. `search_items` is the one
    // call the grid makes (page **and** paired total); `count_matches` is the
    // total by itself, so the difference between the two columns is what the page
    // alone costs, which is the number stage S5 needs when it decides what to
    // re-run on every keystroke.
    println!(
        "{:<44} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "query", "grid+tot", "p95", "count p50", "count p95", "matches"
    );
    let mut worst = (0_u128, String::new());
    for text in QUERIES {
        let plan = query::parse(text, Scope::Media)
            .unwrap_or_else(|e| panic!("{text:?} must parse for the probe: {}", e.message))
            .plan();
        let matches = db::count_matches(&conn, Scope::Media, &plan).expect("counts");
        let page = timed(|| db::search_items(&conn, Scope::Media, &plan, PAGE, 0).expect("page"));
        let total = timed(|| db::count_matches(&conn, Scope::Media, &plan).expect("counts"));
        println!(
            "{:<44} {:>8} ms {:>8} ms {:>8} ms {:>8} ms {:>9}",
            text, page.0, page.1, total.0, total.1, matches
        );
        if page.1 > worst.0 {
            worst = (page.1, (*text).to_string());
        }
    }
    println!(
        "\nslowest grid call (page + total): {:?} at {} ms",
        worst.1, worst.0
    );

    // The cost the grammar knowingly accepts, printed instead of left to be
    // discovered later: `CAST(created_at AS INTEGER)` makes an index on the TEXT
    // stamps useless, so a date window is a scan of `items`.
    println!("\nwhy a date query is a scan (EXPLAIN QUERY PLAN):");
    explain(&conn, "added:>=2024-06-01");
    explain(&conn, "big_breasts");
    explain(&conn, "\"summer witch\"");
    explain(&conn, "size>100mb");
    explain(&conn, "type:video");

    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Times one closure `REPS` times and returns (p50, p95) in milliseconds. The
/// closure returns a value on purpose: a statement whose rows are never drained
/// measures a half-finished query, which is the mistake this file exists to avoid.
fn timed<T>(f: impl Fn() -> T) -> (u128, u128) {
    let mut times: Vec<u128> = Vec::with_capacity(REPS);
    for _ in 0..REPS {
        let started = Instant::now();
        std::hint::black_box(f());
        times.push(started.elapsed().as_millis());
    }
    times.sort_unstable();
    let pct = |q: f64| times[((times.len() as f64 * q) as usize).min(times.len() - 1)];
    (pct(0.5), pct(0.95))
}

/// Prints SQLite's chosen plan, so a scan is visible in the log rather than
/// inferred from a number.
fn explain(conn: &Connection, text: &str) {
    let plan = query::parse(text, Scope::Media).expect("parses").plan();
    let sql = format!(
        "EXPLAIN QUERY PLAN SELECT items.id FROM items{}",
        db::planned_where(Scope::Media, &plan)
    );
    let params: Vec<&dyn rusqlite::ToSql> = plan.bind();
    let mut stmt = conn.prepare(&sql).expect("explain prepares");
    let rows = stmt
        .query_map(params.as_slice(), |r| r.get::<_, String>(3))
        .expect("explain runs");
    for row in rows.flatten() {
        println!("  {text:<24} {row}");
    }
}

/// The fixture itself: everything synthetic, everything in one transaction.
fn seed(conn: &Connection) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    let tags = (0..TAG_POOL)
        .map(|i| format!("fixture_tag_{i:05}"))
        .collect::<Vec<String>>();
    {
        // `fold` is written by the schema's own helper, so the probe measures the
        // index the real library has, not one the fixture invented.
        let mut stmt = tx.prepare(
            "INSERT INTO tags (name, type_id, site, ns, fold) VALUES (?1, 3, 'fixture', ?2, ?3)",
        )?;
        for name in &tags {
            let (ns, fold) = db::tag_ns_fold(name);
            stmt.execute(rusqlite::params![name, ns, fold])?;
        }
    }
    // Names the worked examples actually ask for, so the timed queries are not all
    // guaranteed misses. Deliberately **dense** - the three named tags land in
    // three of every item's ten slots, so `big_breasts` selects most of the
    // library. A rare tag is cheap to time; a tag that matches 70 000 items and
    // still has to sort its way to `LIMIT 300` is the case worth measuring.
    for name in ["big_breasts", "female:big_breasts", "anal", "renpy"] {
        let (ns, fold) = db::tag_ns_fold(name);
        tx.execute(
            "INSERT OR IGNORE INTO tags (name, type_id, site, ns, fold) VALUES (?1, 3, 'fixture', ?2, ?3)",
            rusqlite::params![name, ns, fold],
        )?;
    }
    // The `extension` tags #77 will start writing in stage S4/S7. Nothing writes
    // them today - that is the backfill gap the review named - so the probe seeds
    // them deliberately: otherwise `ext:` and `type:video`'s extension arm measure
    // an empty index instead of the predicate.
    for name in ["png", "jpg", "gif", "mp4", "webm"] {
        let (ns, fold) = db::tag_ns_fold(name);
        tx.execute(
            "INSERT OR IGNORE INTO tags (name, type_id, site, ns, fold) VALUES (?1, 4, '', ?2, ?3)",
            rusqlite::params![name, ns, fold],
        )?;
    }
    let extra_tag_ids: Vec<i64> = tx
        .prepare("SELECT id FROM tags WHERE name LIKE 'fixture_tag_%' ORDER BY id")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<Vec<i64>, _>>()?;
    let named: Vec<i64> = tx
        .prepare(
            "SELECT id FROM tags WHERE type_id = 3 AND name NOT LIKE 'fixture_tag_%' ORDER BY id",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<Result<Vec<i64>, _>>()?;
    let ext_ids: Vec<(String, i64)> = tx
        .prepare("SELECT name, id FROM tags WHERE type_id = 4 ORDER BY id")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<Result<Vec<(String, i64)>, _>>()?;

    // The slugs the fixture writes. `local` is the disk copy (invariant 10: the
    // site vocabulary is code-owned), the rest stand in for whatever galleries a
    // real vault happens to hold.
    const REMOTE: [&str; 4] = ["ehentai", "gelbooru", "rule34", "f95"];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    {
        let mut items = tx.prepare(
            "INSERT INTO items (id, kind, title, original_title, circle, status, dnf,
                                rating, notes, version, created_at, updated_at)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5, 0, ?6, '', NULL, ?7, ?8)",
        )?;
        // A `local` source carries the facts of a file the vault did not store, so
        // the probe seeds them the way #77's probe will: 70 % measured, 30 % NULL,
        // which is the half of invariant 27 the `size>`/`width>`/`length>`
        // predicates have to respect for a disk or link item too.
        let mut sources = tx.prepare(
            "INSERT OR IGNORE INTO sources (item_id, site, url, source_id, fetched_at,
                                            bytes, duration_ms, width, height, probed_at)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        let mut media = tx.prepare(
            "INSERT INTO media (item_id, kind, hash, mime, width, height, data,
                                bytes, duration_ms, animated, probed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, x'89504e470d0a1a0a', ?7, ?8, ?9, ?10)",
        )?;
        let mut item_tags =
            tx.prepare("INSERT OR IGNORE INTO item_tags (item_id, tag_id) VALUES (?1, ?2)")?;
        for n in 0..ITEMS {
            let id = format!("s-{n:06}");
            let kind = match n % 5 {
                0..=2 => "gallery",
                3 => "post",
                _ => "game",
            };
            let title = if n % 97 == 0 {
                format!("Summer Witch fixture {n}")
            } else {
                format!("Fixture item {n}")
            };
            let circle = if n % 11 == 0 { Some("Red Sun") } else { None };
            let status = ["seen", "tracking", "wishlist", "downloaded", "played"][(n % 5) as usize];
            let rating: Option<i64> = if n % 3 == 0 { Some(1 + n % 5) } else { None };
            // Four years of stamps, so `added:` windows select a real slice.
            let created = 1_600_000_000 + n * 1_300 % 130_000_000;
            let created_at = created.to_string();
            let updated_at = (created + 60).to_string();
            items.execute(rusqlite::params![
                id, kind, title, circle, status, rating, created_at, updated_at
            ])?;
            // Storage shapes are mixed so every `stored:` value selects a slice of
            // the fixture rather than an empty set: a fifth are folder imports
            // whose only source is the disk copy (the old local-only queue), a
            // tenth were scraped *and* kept, and the rest are links. 70 % of those
            // sources have been probed, and the other 30 % are the NULL that a
            // `size>`/`width>`/`length>` must never match (invariant 27).
            let local_only = n % 5 == 1;
            let also_local = !local_only && n % 10 == 3;
            // `shape` is what the media facts and the extension are derived from,
            // and 24 cases is wide enough that no other rule in this fixture lands
            // on the same residue. An earlier version wrote video as `n % 20` and
            // animation as `n % 40`, which quietly made every animated item a
            // video, so `type:gif` timed an empty set - a timing table of empty
            // sets is the trap this whole file is here to avoid.
            let shape = (n % 24) as usize;
            let vaulted = n % 2 == 0;
            let is_video = vaulted && matches!(shape, 4 | 8 | 12);
            let is_animated = vaulted && !is_video && matches!(shape, 2 | 14);
            let source_facts = if n % 10 < 7 {
                (
                    Some(40_000 + (n * 104_729) % 1_500_000_000),
                    Some(1_000 + n * 37 % 600_000),
                    Some(200 + n % 3_000),
                    Some(200 + n % 2_000),
                    Some(updated_at.as_str()),
                )
            } else {
                (None, None, None, None, None)
            };
            let (sbytes, sdur, sw, sh, sprobe) = source_facts;
            if local_only || also_local {
                let url = format!("file:///D:/fixture/{id}.png");
                sources.execute(rusqlite::params![
                    id, "local", url, updated_at, sbytes, sdur, sw, sh, sprobe
                ])?;
            }
            if !local_only {
                // The remote slug moves on two different residues, so the untagged
                // fifth and the vaulted half are not all on one site.
                let site = REMOTE[(n / 3 + n / 7) as usize % REMOTE.len()];
                let url = format!("https://fixture.example/{site}/{id}");
                sources.execute(rusqlite::params![
                    id, site, url, updated_at, sbytes, sdur, sw, sh, sprobe
                ])?;
            }
            // What the file calls itself, derived the way #77's writer will derive
            // it: from the stored container for a vaulted item, from the URL for a
            // link or a disk file. Five in six items know; the rest are the pre-v10
            // rows whose extension nobody wrote, which is the backfill gap #77 has
            // to close before `ext:` means what it says.
            let ext = if vaulted {
                if is_video {
                    if shape == 4 {
                        "webm"
                    } else {
                        "mp4"
                    }
                } else if is_animated {
                    "gif"
                } else if shape.is_multiple_of(2) {
                    "png"
                } else {
                    "jpg"
                }
            } else {
                match shape {
                    0..=11 => "jpg",
                    12..=17 => "mp4",
                    _ => "png",
                }
            };
            // Half the items are vaulted; 70 % of those have been probed, and the
            // rest are the honest NULL the grammar must never match.
            if vaulted {
                let probed = (n / 2) % 10 < 7;
                // A vaulted video is its own media row, and its `animated` stays
                // NULL: the picture probe cannot answer that about a container,
                // and inventing 0 would make `-type:gif` a lie (invariant 27).
                let (w, h): (Option<i64>, Option<i64>) = if probed {
                    (Some(200 + n % 3_000), Some(200 + n % 2_000))
                } else {
                    (None, None)
                };
                let animated: Option<i64> = if probed && !is_video {
                    Some(is_animated as i64)
                } else {
                    None
                };
                let duration: Option<i64> = if probed && is_video {
                    Some(1_000 + n * 37 % 300_000)
                } else {
                    None
                };
                // Up to ~1.5 GB, so `size>100mb` and `-size>1gb` each select a real
                // slice instead of all of nothing.
                let bytes = 40_000 + (n * 7_919) % 1_500_000_000;
                let mime = if is_video {
                    format!("video/{ext}")
                } else {
                    "image/png".to_string()
                };
                media.execute(rusqlite::params![
                    id,
                    if is_video { "video" } else { "full" },
                    format!("fixture-hash-{n}"),
                    mime,
                    w,
                    h,
                    bytes,
                    duration,
                    animated,
                    if probed {
                        Some(updated_at.as_str())
                    } else {
                        None
                    }
                ])?;
                if n % 3 == 0 {
                    media.execute(rusqlite::params![
                        id,
                        "thumb",
                        format!("fixture-thumb-{n}"),
                        "image/webp",
                        Some(360),
                        Some(360),
                        4_096_i64,
                        None::<i64>,
                        None::<i64>,
                        Some(updated_at.as_str())
                    ])?;
                }
            }
            // 10 tags each. A Zipf-ish draw: the named tags land on a few thousand
            // items each, the pool tags are spread thin, which is what a real
            // library looks like and what makes a tag query's cost honest. One in
            // twelve carries none at all, because that is the population the
            // `-has:tags` rail and the old `no_tags` badge exist for.
            if n % 12 != 5 {
                for k in 0..TAGS_PER_ITEM {
                    let pick = rng.next();
                    let tag_id = if k < 3 {
                        named[(pick as usize) % named.len()]
                    } else {
                        extra_tag_ids[(pick as usize) % extra_tag_ids.len()]
                    };
                    item_tags.execute(rusqlite::params![id, tag_id])?;
                }
            }
            // Five in six items know their own extension, which is what #77's
            // writer will produce; the rest are the pre-v10 rows nobody backfilled
            // yet, and `ext:` must not match those. One tag per item, as
            // `db::set_item_extension_tag` writes it - so a conjunction like
            // `ext:gif ext:mp4` selects nothing by construction, and the timing
            // line says so rather than hiding it.
            if n % 12 != 5 && !rng.next().is_multiple_of(6) {
                if let Some((_, tag_id)) = ext_ids.iter().find(|(name, _)| name == ext) {
                    item_tags.execute(rusqlite::params![id, tag_id])?;
                }
            }
        }
    }
    tx.commit()
}

/// A deterministic xorshift: the fixture has to be reproducible, and a random
/// one makes a timing difference impossible to tell apart from luck.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn date_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Days since the epoch, rendered as YYYYMMDD, so the log file name reads as a
    // date without pulling in a date crate.
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}{m:02}{d:02}")
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
