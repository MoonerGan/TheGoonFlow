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
        let page =
            timed(|| db::search_items(&conn, Scope::Media, &plan, PAGE, None, true).expect("page"));
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

    // Decision 9 and the two cursor forms, measured with and without an index on the
    // grid sort key. Each of these sections creates and drops what it needs, so the
    // ones after it still describe the schema the app creates.
    index_and_cursor_forms(&conn).map_err(|e| format!("index options: {e}"))?;
    descending_pair(&conn).map_err(|e| format!("descending pair: {e}"))?;
    // The last unmeasured option in plan §9 decision 9, and the one that decides
    // whether the ~50 ms target is a budget the board has to restate or one the
    // code can meet.
    tag_predicate_shape(&conn).map_err(|e| format!("tag predicate shape: {e}"))?;

    // Last of the read-only sections, because it ends by deleting rows: decision 10
    // (plan §9), measured instead of argued.
    paging(&conn).map_err(|e| format!("paging: {e}"))?;

    // After everything else, because it is the only section that adds rows: the
    // synthetic alias tags its cardinality ladder needs are dropped again before it
    // returns.
    cardinality_crossover(&conn).map_err(|e| format!("cardinality crossover: {e}"))?;

    // Decision 13: which predicate shape the planner should get, measured in the three
    // states of planner knowledge a real vault can be in, with the AND and the negation
    // measured rather than reasoned about. Last because it is the only section that
    // touches `sqlite_stat1`, and it leaves the database the way the app makes it.
    shape_choice(&conn).map_err(|e| format!("shape choice: {e}"))?;

    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// The grid order, read from `db::GRID_ORDER` rather than written out again here:
/// the page, the select-all list and this probe's counter-example have to sort the
/// same way, and a probe that invented its own order would be measuring a query
/// production never runs. TEXT on both sides, which is stage S3's order.
const ORDER: &str = db::GRID_ORDER;

/// Where `updated_at` sits in `db::PAGE_COLUMNS`. Both page readers take it from
/// that column, so the cursor the probe measures is built from the same value the
/// `ORDER BY` compares — and both shapes return the same seven columns, which is
/// what makes their timings comparable.
const UPDATED_AT_COLUMN: usize = 6;

/// The page shape this stage **replaced**, kept here as the counter-example: the
/// page statement production runs contains no offset anywhere in it.
fn offset_page(
    conn: &Connection,
    where_sql: &str,
    plan: &query::Plan,
    offset: i64,
) -> Vec<(String, String)> {
    let sql = format!("{}{where_sql}{ORDER} LIMIT ? OFFSET ?", db::PAGE_COLUMNS);
    let mut params = plan.params.clone();
    params.push(query::Value::Int(PAGE));
    params.push(query::Value::Int(offset));
    let mut stmt = conn.prepare_cached(&sql).expect("offset page prepares");
    stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| {
        Ok((r.get(0)?, r.get(UPDATED_AT_COLUMN)?))
    })
    .expect("offset page runs")
    .collect::<Result<Vec<_>, _>>()
    .expect("offset page rows")
}

/// The page shape production runs, taken from `db::page_statement` itself: the
/// timings and the `EXPLAIN` below are of the shipping SQL, not a paraphrase of it.
fn cursor_page(
    conn: &Connection,
    plan: &query::Plan,
    cursor: Option<&db::PageCursor>,
) -> Vec<(String, String)> {
    let (sql, params) = db::page_statement(Scope::Media, plan, cursor, PAGE);
    let mut stmt = conn.prepare_cached(&sql).expect("cursor page prepares");
    stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| {
        Ok((r.get(0)?, r.get(UPDATED_AT_COLUMN)?))
    })
    .expect("cursor page runs")
    .collect::<Result<Vec<_>, _>>()
    .expect("cursor page rows")
}

/// Decision 10 (plan §9): does the grid page by `OFFSET`, or by a cursor over its
/// own sort key? Two things decide it, and both are measured here rather than
/// argued: the **cost** at depth (an offset has to produce the rows before the page
/// and throw them away), and the **correctness** of continuing a page walk across a
/// bulk delete, which is what #73's *Delete selected* does to a grid the user is
/// looking at.
fn paging(conn: &Connection) -> rusqlite::Result<()> {
    for text in ["big_breasts", "big_breasts -anal", "-size>1gb type:gif"] {
        page_costs(conn, text)?;
    }
    // Last, and only once, because it deletes rows: every number above is measured
    // on the fixture exactly as the table above printed it.
    delete_shift(conn, "big_breasts")
}

/// The cost half: the plan SQLite picks for both shapes, and the page time at
/// several depths. `EXPLAIN` is printed rather than summarised because it is the
/// diagnosis - whether a deep page pays for a sort of the whole match set or an
/// index walk that stops early is visible in the text.
fn page_costs(conn: &Connection, text: &str) -> rusqlite::Result<()> {
    let plan = query::parse(text, Scope::Media)
        .unwrap_or_else(|e| panic!("{text:?} must parse for the probe: {}", e.message))
        .plan();
    let where_sql = db::planned_where(Scope::Media, &plan);
    let total = db::count_matches(conn, Scope::Media, &plan)?;
    let pages = (total + PAGE - 1) / PAGE;
    println!("\npaging {text:?}: {total} matches in {pages} pages of {PAGE}");

    // A real cursor to explain against: the last row of page 1, read with the
    // statement production runs.
    let first = cursor_page(conn, &plan, None);
    let last = first.last().expect("page 1 has rows");
    let seed = db::PageCursor {
        updated_at: last.1.clone(),
        id: last.0.clone(),
    };
    let mut off_params = plan.params.clone();
    off_params.push(query::Value::Int(PAGE));
    off_params.push(query::Value::Int((pages.max(1) - 1) * PAGE));
    explain_shape(
        conn,
        "offset page",
        &format!("{}{where_sql}{ORDER} LIMIT ? OFFSET ?", db::PAGE_COLUMNS,),
        &off_params,
    );
    let (cur_sql, cur_params) = db::page_statement(Scope::Media, &plan, Some(&seed), PAGE);
    explain_shape(conn, "cursor page", &cur_sql, &cur_params);

    println!(
        "  {:>6} {:>9} {:>14} {:>14} {:>9} {:>6}",
        "page", "offset", "offset p50/p95", "cursor p50/p95", "delta", "rows"
    );
    // Depth is the whole point: page 1 is where both shapes cost the same, and the
    // last page is where an offset has walked every row before it. Never past the
    // end: a page that does not exist measures an empty result, not a deep one.
    let mut depths: Vec<i64> = vec![1, 4, pages.min(40), pages];
    depths.retain(|&p| p >= 1);
    depths.sort_unstable();
    depths.dedup();
    let mut cursor: Option<db::PageCursor> = None;
    let mut walked = 0_i64;
    for &page in &depths {
        while walked < page - 1 {
            match cursor_page(conn, &plan, cursor.as_ref()).last() {
                Some((id, at)) => {
                    cursor = Some(db::PageCursor {
                        updated_at: at.clone(),
                        id: id.clone(),
                    })
                }
                None => break,
            }
            walked += 1;
        }
        let offset = (page - 1) * PAGE;
        let offs = timed(|| offset_page(conn, &where_sql, &plan, offset));
        let cur = cursor.clone();
        let key = timed(|| cursor_page(conn, &plan, cur.as_ref()));
        // Both shapes must land on the same page. If they ever stop agreeing, the
        // timings are comparing different work, so the probe says so instead of
        // printing a pretty difference.
        let off_rows = offset_page(conn, &where_sql, &plan, offset).len();
        let cur_rows = cursor_page(conn, &plan, cur.as_ref()).len();
        assert_eq!(
            off_rows, cur_rows,
            "{text:?} page {page}: offset and cursor returned different page sizes"
        );
        println!(
            "  {:>6} {:>9} {:>7}/{:<6} {:>7}/{:<6} {:>8} ms {:>6}",
            page,
            offset,
            offs.0,
            offs.1,
            key.0,
            key.1,
            offs.0 as i64 - key.0 as i64,
            off_rows,
        );
    }
    Ok(())
}

/// The correctness half. Read four pages the way the grid used to read them, delete
/// 20 000 of the matched rows in grid order through the same predicate - the size of
/// #73's *Delete selected* - and then continue the walk both ways. Ranks are
/// positions in `search_item_ids`, stage S4's own endpoint, which is the id list the
/// UI would be holding while the user pages.
fn delete_shift(conn: &Connection, text: &str) -> rusqlite::Result<()> {
    let plan = query::parse(text, Scope::Media)
        .unwrap_or_else(|e| panic!("{text:?} must parse for the probe: {}", e.message))
        .plan();
    let where_sql = db::planned_where(Scope::Media, &plan);
    let total = db::count_matches(conn, Scope::Media, &plan)?;
    let seen: Vec<String> = (0..4)
        .flat_map(|p| {
            offset_page(conn, &where_sql, &plan, p * PAGE)
                .into_iter()
                .map(|(id, _)| id)
        })
        .collect();
    // The cursor of the last row on screen, taken by the shipping shape.
    let mut cursor: Option<db::PageCursor> = None;
    for _ in 0..4 {
        match cursor_page(conn, &plan, cursor.as_ref()).last() {
            Some((id, at)) => {
                cursor = Some(db::PageCursor {
                    updated_at: at.clone(),
                    id: id.clone(),
                })
            }
            None => break,
        }
    }
    let page4 = cursor.expect("four pages of a dense query exist");

    let doomed = std::cmp::min(20_000_i64, total);
    let mut doomed_params = plan.params.clone();
    doomed_params.push(query::Value::Int(doomed));
    conn.execute(
        &format!(
            "DELETE FROM items WHERE id IN (SELECT items.id FROM items{where_sql}{ORDER} LIMIT ?)"
        ),
        rusqlite::params_from_iter(doomed_params.iter()),
    )?;
    let survivors = db::search_item_ids(conn, Scope::Media, &plan)?;
    let rank: std::collections::HashMap<String, usize> = survivors
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), i))
        .collect();
    let cont_off = offset_page(conn, &where_sql, &plan, 4 * PAGE);
    let cont_key = cursor_page(conn, &plan, Some(&page4));
    let describe = |row: Option<&(String, String)>| -> String {
        match row {
            None => "nothing left".to_string(),
            Some((id, _)) => match rank.get(id) {
                Some(r) => format!("survivor #{r}, so {r} surviving rows were passed over"),
                None => "a row the survivor list does not contain".to_string(),
            },
        }
    };
    println!(
        "\ndelete shift {text:?}: {total} matches, {doomed} deleted in grid order \
         ({} of the {} rows on screen were among them)",
        seen.iter()
            .filter(|id| !rank.contains_key(id.as_str()))
            .count(),
        seen.len()
    );
    println!(
        "    offset continuation starts at {}",
        describe(cont_off.first())
    );
    println!(
        "    cursor continuation starts at {}",
        describe(cont_key.first())
    );
    Ok(())
}

/// Prints the plan SQLite chooses for one page shape, values and all, so a sort of
/// the whole match set is visible in the log instead of inferred from a number.
fn explain_shape(conn: &Connection, label: &str, sql: &str, params: &[query::Value]) {
    let mut stmt = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("explain prepares");
    for row in stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |r| {
            r.get::<_, String>(3)
        })
        .expect("explain runs")
    {
        println!("  {:<12} {}", label, row.expect("explain row"));
    }
}

/// The composite index the review asked about, by name and by DDL. An index's column
/// list takes bare column names, so the DDL writes `id` where the SQL writes `items.id`.
const COMPOSITE_INDEX: &str = "items_updated_id_idx";
const COMPOSITE_DDL: &str = "CREATE INDEX items_updated_id_idx ON items (updated_at DESC, id)";

fn index_exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
        [name],
        |r| r.get::<_, bool>(0),
    )
}

/// Puts the schema in one of the two worlds and refreshes the planner's statistics,
/// because comparing plans without re-running `ANALYZE` compares one informed planner
/// against one guessing.
fn set_index(conn: &Connection, want: bool) -> rusqlite::Result<()> {
    if index_exists(conn, COMPOSITE_INDEX)? == want {
        return Ok(());
    }
    if want {
        conn.execute_batch(COMPOSITE_DDL)?;
    } else {
        conn.execute_batch(&format!("DROP INDEX {COMPOSITE_INDEX}"))?;
    }
    conn.execute("ANALYZE", [])?;
    Ok(())
}

/// The page on its own, read through the shipping function rather than a copy of
/// its SQL: `db::page_items` is what a keystroke calls when `with_count` is false,
/// and `db::search_items` is that plus the count. Timing the two is the split the
/// review asked for - what a keystroke can afford, and what it cannot.
fn page_only(
    conn: &Connection,
    plan: &query::Plan,
    cursor: Option<&db::PageCursor>,
) -> Vec<String> {
    db::page_items(conn, Scope::Media, plan, PAGE, cursor)
        .expect("page runs")
        .into_iter()
        .map(|row| row.id)
        .collect()
}

/// The other cursor form: SQLite's row-value comparison, legal since 3.15, which
/// compares componentwise with **one operator for every component**. Whether that
/// means the same thing as `updated_at DESC, items.id ASC` is exactly the question, so
/// it is measured here and pinned by a fixture test.
fn row_value_page(
    conn: &Connection,
    where_sql: &str,
    plan: &query::Plan,
    cursor: &db::PageCursor,
) -> Result<Vec<String>, String> {
    let sql = row_value_sql(where_sql);
    let mut params = plan.params.clone();
    params.push(query::Value::Text(cursor.updated_at.clone()));
    params.push(query::Value::Text(cursor.id.clone()));
    params.push(query::Value::Int(PAGE));
    let mut stmt = match conn.prepare_cached(&sql) {
        Ok(s) => s,
        Err(e) => return Err(format!("row-value form does not prepare: {e}")),
    };
    stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| {
        r.get::<_, String>(0)
    })
    .map(|rows| rows.collect::<Result<Vec<_>, _>>().expect("row-value rows"))
    .map_err(|e| format!("row-value form does not run: {e}"))
}

fn row_value_sql(where_sql: &str) -> String {
    format!(
        "{}{where_sql} AND (items.updated_at, items.id) < (?,?){ORDER} LIMIT ?",
        db::PAGE_COLUMNS
    )
}

fn row_value_params(plan: &query::Plan, cursor: &db::PageCursor) -> Vec<query::Value> {
    let mut params = plan.params.clone();
    params.push(query::Value::Text(cursor.updated_at.clone()));
    params.push(query::Value::Text(cursor.id.clone()));
    params.push(query::Value::Int(PAGE));
    params
}

/// Plan §9 decision 9, measured rather than argued. Two hypotheses and one split:
///
/// * does an index on the grid's **own** sort key (`updated_at DESC, id`) remove the
///   `USE TEMP B-TREE FOR ORDER BY` the tag queries pay, and what does it do to a
///   query matching most of the library versus one matching 646 rows;
/// * does a row-value cursor become a range **seek** where the `OR` form filters - and
///   is it even the same comparison as a mixed-direction order;
/// * how much of a grid call is the page and how much is the paired `COUNT(*)`.
///
/// Every timing is printed beside its row count, because a deep page that returns
/// nothing is fast for a reason that has nothing to do with the question. The index is
/// left exactly as `init_schema` made it, so `paging` and `delete_shift` still
/// describe what ships.
fn index_and_cursor_forms(conn: &Connection) -> rusqlite::Result<()> {
    let version: String = conn.query_row("SELECT sqlite_version()", [], |r| r.get(0))?;
    let shipped = index_exists(conn, COMPOSITE_INDEX)?;
    println!("\nsqlite {version}; the app schema creates {COMPOSITE_INDEX}: {shipped}");

    for want_index in [false, true] {
        set_index(conn, want_index)?;
        println!(
            "\n=== cursor page, {} the composite index ===",
            if want_index { "WITH" } else { "WITHOUT" }
        );
        for text in [
            "big_breasts",
            "female:big_breasts",
            "size>100mb type:image",
            "width<400 type:image -has:thumb",
        ] {
            let plan = query::parse(text, Scope::Media)
                .unwrap_or_else(|e| panic!("{text:?} must parse for the probe: {}", e.message))
                .plan();
            let where_sql = db::planned_where(Scope::Media, &plan);
            let total = db::count_matches(conn, Scope::Media, &plan)?;
            let pages = (total + PAGE - 1) / PAGE;
            println!("\n{text:?}: {total} matches in {pages} pages of {PAGE}");

            let (p1_sql, p1_params) = db::page_statement(Scope::Media, &plan, None, PAGE);
            explain_shape(conn, "page 1", &p1_sql, &p1_params);

            // A cursor for the **last** page, built from the last row of the page
            // before it. The cursor is a value, and walking 213 pages to get one would
            // time the walk instead of the seek; taking it from the last page itself
            // would ask for the page after the end and time an empty result.
            let mut deep: Option<db::PageCursor> = None;
            if pages >= 2 {
                if let Some((id, at)) =
                    offset_page(conn, &where_sql, &plan, (pages - 2) * PAGE).last()
                {
                    deep = Some(db::PageCursor {
                        updated_at: at.clone(),
                        id: id.clone(),
                    });
                }
            }
            if let Some(cursor) = &deep {
                let (sql, params) = db::page_statement(Scope::Media, &plan, Some(cursor), PAGE);
                explain_shape(conn, "deep cursor", &sql, &params);
                explain_shape(
                    conn,
                    "row-value",
                    &row_value_sql(&where_sql),
                    &row_value_params(&plan, cursor),
                );
            }

            let first = timed(|| page_only(conn, &plan, None));
            let first_rows = page_only(conn, &plan, None).len();
            let deep_page = deep.as_ref().map(|c| {
                (
                    timed(|| page_only(conn, &plan, Some(c))),
                    page_only(conn, &plan, Some(c)).len(),
                )
            });
            let with_total =
                timed(|| db::search_items(conn, Scope::Media, &plan, PAGE, None, true));
            let count_only = timed(|| db::count_matches(conn, Scope::Media, &plan));
            println!(
                "  page alone, page 1: {} rows, {} ms p50 / {} ms p95",
                first_rows, first.0, first.1
            );
            if let Some(((p50, p95), rows)) = deep_page {
                println!(
                    "  page alone, page {pages} (deep cursor): {rows} rows, {p50} ms p50 / {p95} ms p95"
                );
            }
            println!(
                "  page + total (the call the grid makes today): {} ms p50 / {} ms p95; count alone {} ms / {} ms",
                with_total.0, with_total.1, count_only.0, count_only.1
            );

            // Are the two cursor forms the same question? They have to answer with the
            // same rows in the same order, or one of them is paging a different order,
            // and the timings below are not comparable.
            if let Some(cursor) = &deep {
                let truth: Vec<String> = offset_page(conn, &where_sql, &plan, (pages - 1) * PAGE)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect();
                let by_or = page_only(conn, &plan, Some(cursor));
                match row_value_page(conn, &where_sql, &plan, cursor) {
                    Ok(by_row) => {
                        let shared = by_or.iter().filter(|id| by_row.contains(id)).count();
                        println!(
                            "  page {pages} by three reads: offset {} rows; shipping cursor {} rows {}; row-value {} rows {}",
                            truth.len(),
                            by_or.len(),
                            if by_or == truth { "matches" } else { "DIFFERS FROM" },
                            by_row.len(),
                            if by_row == truth { "matches" } else { "DIFFERS FROM" },
                        );
                        if by_or != by_row {
                            let first_only = by_row
                                .iter()
                                .find(|id| !by_or.contains(id))
                                .cloned()
                                .unwrap_or_else(|| "none".to_string());
                            println!(
                                "    the forms disagree: {shared} rows in common, first row the row-value read shows and the cursor read does not: {first_only}"
                            );
                        }
                    }
                    Err(e) => println!("  {e}"),
                }
            }
        }
    }

    set_index(conn, shipped)?;
    println!("\nindex left as the app schema makes it: {shipped}");
    Ok(())
}

/// The cursor form that can be correct **and** seekable. A row-value comparison uses
/// one direction for every component, so it means the grid's order only when both
/// halves of that order go the same way - and the tie-break direction is a free
/// choice, because the ids are opaque. So: order `updated_at DESC, items.id DESC`,
/// compare `(updated_at, id) < (?, ?)`, and index exactly that pair.
///
/// This SQL is written **here**, not in `db.rs`, because it is the candidate, not the
/// decision. If it wins, `GRID_ORDER`, `AFTER_CURSOR` and `page_statement` take it
/// over and this section calls them instead of writing its own. The correctness check
/// is an offset read on the same order: a form that paged a different order gets
/// caught here rather than timed.
const DESC_INDEX: &str = "items_updated_id_desc_idx";
const DESC_INDEX_DDL: &str =
    "CREATE INDEX items_updated_id_desc_idx ON items (updated_at DESC, id DESC)";
const DESC_ORDER: &str = " ORDER BY updated_at DESC, items.id DESC";

fn desc_page(
    conn: &Connection,
    where_sql: &str,
    plan: &query::Plan,
    cursor: Option<&(String, String)>,
    offset: Option<i64>,
) -> Vec<(String, String)> {
    let filter = if cursor.is_some() {
        " AND (items.updated_at, items.id) < (?,?)"
    } else {
        ""
    };
    let tail = if offset.is_some() {
        " LIMIT ? OFFSET ?"
    } else {
        " LIMIT ?"
    };
    let sql = format!("{}{where_sql}{filter}{DESC_ORDER}{tail}", db::PAGE_COLUMNS);
    let mut params = plan.params.clone();
    if let Some((_, at)) = cursor {
        params.push(query::Value::Text(at.clone()));
    }
    if let Some((id, _)) = cursor {
        params.push(query::Value::Text(id.clone()));
    }
    params.push(query::Value::Int(PAGE));
    if let Some(off) = offset {
        params.push(query::Value::Int(off));
    }
    let mut stmt = conn.prepare_cached(&sql).expect("desc page prepares");
    stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| {
        Ok((r.get(0)?, r.get(UPDATED_AT_COLUMN)?))
    })
    .expect("desc page runs")
    .collect::<Result<Vec<_>, _>>()
    .expect("desc page rows")
}

fn desc_params(plan: &query::Plan, cursor: Option<&(String, String)>) -> Vec<query::Value> {
    let mut params = plan.params.clone();
    if let Some((_, at)) = cursor {
        params.push(query::Value::Text(at.clone()));
    }
    if let Some((id, _)) = cursor {
        params.push(query::Value::Text(id.clone()));
    }
    params.push(query::Value::Int(PAGE));
    params
}

fn descending_pair(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(DESC_INDEX_DDL)?;
    conn.execute("ANALYZE", [])?;
    println!("\n=== both halves DESC, row-value cursor, index on that pair ===");
    for text in [
        "big_breasts",
        "female:big_breasts",
        "size>100mb type:image",
        "width<400 type:image -has:thumb",
    ] {
        let plan = query::parse(text, Scope::Media)
            .unwrap_or_else(|e| panic!("{text:?} must parse for the probe: {}", e.message))
            .plan();
        let where_sql = db::planned_where(Scope::Media, &plan);
        let total = db::count_matches(conn, Scope::Media, &plan)?;
        let pages = (total + PAGE - 1) / PAGE;
        println!("\n{text:?}: {total} matches in {pages} pages of {PAGE}");

        let seed = if pages >= 2 {
            desc_page(conn, &where_sql, &plan, None, Some((pages - 2) * PAGE))
                .last()
                .cloned()
        } else {
            None
        };
        explain_shape(
            conn,
            "page 1",
            &format!("{}{where_sql}{DESC_ORDER} LIMIT ?", db::PAGE_COLUMNS),
            &desc_params(&plan, None),
        );
        let mut deep: Vec<(String, String)> = Vec::new();
        if let Some(cursor) = &seed {
            explain_shape(
                conn,
                "row-value",
                &format!(
                    "{}{where_sql} AND (items.updated_at, items.id) < (?,?){DESC_ORDER} LIMIT ?",
                    db::PAGE_COLUMNS
                ),
                &desc_params(&plan, Some(cursor)),
            );
            deep = desc_page(conn, &where_sql, &plan, Some(cursor), None);
        }
        let truth: Vec<String> = desc_page(conn, &where_sql, &plan, None, Some((pages - 1) * PAGE))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let deep_ids: Vec<String> = deep.iter().map(|(id, _)| id.clone()).collect();
        println!(
            "  page {pages}: offset read {} rows, row-value cursor {} rows {}",
            truth.len(),
            deep_ids.len(),
            if deep_ids == truth {
                "matches"
            } else {
                "DIFFERS FROM"
            }
        );
        let first = timed(|| desc_page(conn, &where_sql, &plan, None, None).len());
        println!(
            "  page 1, no cursor predicate: {} ms p50 / {} ms p95",
            first.0, first.1
        );
        if seed.is_some() {
            let deep_time = timed(|| desc_page(conn, &where_sql, &plan, seed.as_ref(), None).len());
            println!(
                "  page {pages} by row-value cursor: {} ms p50 / {} ms p95",
                deep_time.0, deep_time.1
            );
        }
    }
    conn.execute_batch(&format!("DROP INDEX {DESC_INDEX}"))?;
    conn.execute("ANALYZE", [])?;
    Ok(())
}

/// The third option plan §9 never wrote down. The dense-tag page is slow because the
/// predicate is **tag-first**: `items.id IN (SELECT it.item_id FROM item_tags ...)`
/// hands SQLite a list of every matching id (64 092 of them) which it then sorts to
/// apply `LIMIT`. The other shape is **row-first**: walk `items` in the grid's own
/// order and ask each row whether it carries the tag, which can stop as soon as the
/// page is full. Row-first is what the fact predicates already do, and it is the
/// shape plan §6 rejected for tags without a number beside it.
///
/// Measured dense and rare, with and without the composite index, because a shape
/// that wins on `big_breasts` and loses on a tag only 30 items carry is not a win.
/// Both shapes are written out **here**: `TAG_FIRST` is a copy of `query.rs::TAG_SEMI`
/// with the same `fold`/`name` test, `ROW_FIRST` is the candidate, and the only
/// difference between them is which side of the join the planner starts from. The
/// rare tags are read out of the fixture instead of named, so the sparse rows are
/// real rows and not a guess about the data.
const TAG_FIRST: &str = "items.id IN (SELECT it.item_id FROM item_tags AS it \
                         JOIN tags AS t ON t.id = it.tag_id \
                         WHERE (t.fold = ? OR t.name = ?))";
const ROW_FIRST: &str = "EXISTS (SELECT 1 FROM item_tags AS it \
                          JOIN tags AS t ON t.id = it.tag_id \
                          WHERE it.item_id = items.id AND (t.fold = ? OR t.name = ?))";

fn tag_sql(predicate: &str, with_cursor: bool) -> String {
    format!(
        "{} WHERE items.kind IN ('gallery','post') AND ({}){}{ORDER} LIMIT ?",
        db::PAGE_COLUMNS,
        predicate,
        if with_cursor { db::AFTER_CURSOR } else { "" },
    )
}

fn tag_params(fold: &str, cursor: Option<&db::PageCursor>) -> Vec<query::Value> {
    let mut params = vec![
        query::Value::Text(fold.to_string()),
        query::Value::Text(fold.to_string()),
    ];
    if let Some(c) = cursor {
        params.push(query::Value::Text(c.updated_at.clone()));
        params.push(query::Value::Text(c.updated_at.clone()));
        params.push(query::Value::Text(c.id.clone()));
    }
    params.push(query::Value::Int(PAGE));
    params
}

fn tag_page(
    conn: &Connection,
    predicate: &str,
    fold: &str,
    cursor: Option<&db::PageCursor>,
) -> Vec<String> {
    let sql = tag_sql(predicate, cursor.is_some());
    let params = tag_params(fold, cursor);
    let mut stmt = conn
        .prepare_cached(&sql)
        .unwrap_or_else(|e| panic!("{fold:?} tag page prepares: {e}"));
    stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| {
        r.get::<_, String>(0)
    })
    .unwrap_or_else(|e| panic!("{fold:?} tag page runs: {e}"))
    .collect::<Result<Vec<_>, _>>()
    .expect("tag page rows")
}

fn tag_count(conn: &Connection, predicate: &str, fold: &str) -> i64 {
    let sql = format!(
        "SELECT COUNT(*) FROM items WHERE items.kind IN ('gallery','post') AND ({predicate})"
    );
    let mut stmt = conn
        .prepare_cached(&sql)
        .unwrap_or_else(|e| panic!("{fold:?} tag count prepares: {e}"));
    stmt.query_row(rusqlite::params![fold, fold], |r| r.get::<_, i64>(0))
        .unwrap_or_else(|e| panic!("{fold:?} tag count runs: {e}"))
}

/// The sparsest tags in the fixture, so the "rare" half of this comparison is data
/// the library actually has rather than a tag name that happens to match nothing.
fn sparse_folds(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT t.fold, COUNT(*) AS c FROM tags AS t \
         JOIN item_tags AS it ON it.tag_id = t.id \
         WHERE t.fold IS NOT NULL GROUP BY t.fold HAVING c BETWEEN 3 AND 200 \
         ORDER BY c ASC LIMIT 3",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    rows.collect::<Result<Vec<_>, _>>()
}

fn tag_predicate_shape(conn: &Connection) -> rusqlite::Result<()> {
    let mut rare = sparse_folds(conn)?;
    rare.push("big_breasts".to_string());
    rare.push("anal".to_string());
    println!("\n=== tag predicate shape: tag-first (`IN`) against row-first (`EXISTS`) ===");
    for want_index in [false, true] {
        set_index(conn, want_index)?;
        println!(
            "\n=== {} the composite index ===",
            if want_index { "WITH" } else { "WITHOUT" }
        );
        for fold in &rare {
            let total = tag_count(conn, TAG_FIRST, fold);
            let pages = (total + PAGE - 1) / PAGE;
            println!("\n{fold:?}: {total} matched items in {pages} pages of {PAGE}");
            if total == 0 {
                println!("  no such tag in the fixture - nothing to time, skipped");
                continue;
            }
            for (label, predicate) in [("tag-first", TAG_FIRST), ("row-first", ROW_FIRST)] {
                let rows = tag_page(conn, predicate, fold, None);
                let page1 = timed(|| tag_page(conn, predicate, fold, None).len());
                let count = timed(|| tag_count(conn, predicate, fold));
                println!(
                    "  {label:<10} page 1: {} rows, {} ms p50 / {} ms p95; count alone {} ms / {} ms",
                    rows.len(),
                    page1.0,
                    page1.1,
                    count.0,
                    count.1
                );
                if pages >= 2 {
                    let seed_sql = format!(
                        "SELECT items.id, items.updated_at FROM items WHERE items.kind IN \
                         ('gallery','post') AND ({predicate}){ORDER} LIMIT ? OFFSET ?"
                    );
                    let mut stmt = conn.prepare_cached(&seed_sql).expect("seed prepares");
                    let seed = stmt
                        .query_map(
                            rusqlite::params![fold, fold, PAGE, (pages - 2) * PAGE],
                            |r| {
                                Ok(db::PageCursor {
                                    updated_at: r.get(1)?,
                                    id: r.get(0)?,
                                })
                            },
                        )
                        .expect("seed runs")
                        .collect::<Result<Vec<_>, _>>()
                        .expect("seed rows")
                        .last()
                        .cloned()
                        .expect("a page before the last one");
                    drop(stmt);
                    let deep = tag_page(conn, predicate, fold, Some(&seed));
                    let deep_time = timed(|| tag_page(conn, predicate, fold, Some(&seed)).len());
                    println!(
                        "             page {pages} by cursor: {} rows, {} ms p50 / {} ms p95",
                        deep.len(),
                        deep_time.0,
                        deep_time.1
                    );
                }
                let plan_sql = tag_sql(predicate, false);
                let mut stmt = conn.prepare(&plan_sql).expect("plan prepares");
                let details = stmt
                    .query_map(rusqlite::params![fold, fold, PAGE], |r| {
                        r.get::<_, String>(3)
                    })
                    .expect("plan runs")
                    .collect::<Result<Vec<_>, _>>()
                    .expect("plan rows");
                drop(stmt);
                for line in details {
                    if line.contains("items") || line.contains("TEMP") || line.contains("SCAN") {
                        println!("    plan: {line}");
                    }
                }
            }
        }
    }
    set_index(conn, false)?;
    Ok(())
}

/// Where the two tag shapes cross over, measured.
///
/// `tag_predicate_shape` shows the two shapes disagree about which queries are cheap:
/// `big_breasts` (64 092 matches) pages in 204 ms tag-first and 1 ms row-first, while
/// a tag 8 items carry pages in 0 ms tag-first and 145 ms row-first. Neither shape may
/// be shipped as the only shape, so the plan has to choose between them, and a
/// threshold that is chosen by the planner needs a number under it.
///
/// The fixture's own tag pool sits at ~50 postings per tag, which is one point on the
/// curve, so this section builds the rest of the curve: synthetic alias tags that point
/// at the first *n* postings of `big_breasts`, for n in a ladder. They are added here
/// and dropped here, so every other section still measures the schema the app creates.
///
/// The last row of each measurement is the price of deciding at all: the posting-count
/// query the planner would have to run before it picks a shape. A decision that costs
/// 40 ms per keystroke is not a decision, it is a second query.
/// The tag cardinalities the crossover is measured at. `20_000` and `30_000` are in the
/// list because they are a third and a half of the fixture's 60 000 media rows: the log
/// then shows which shape wins at the fractions the shipping rule (`query::PROBE_MIN_RATIO`)
/// could plausibly draw its line at, instead of leaving a gap wide enough to hide the
/// crossing inside.
const CARDINALITY_LADDER: [i64; 8] = [1, 20, 200, 2_000, 10_000, 20_000, 30_000, 40_000];

fn add_alias_tags(conn: &Connection) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut made = Vec::new();
    for n in CARDINALITY_LADDER {
        let name = format!("fixture_alias_{n}");
        let (ns, fold) = db::tag_ns_fold(&name);
        conn.execute(
            "INSERT INTO tags (name, type_id, site, ns, fold) VALUES (?1, 3, 'fixture', ?2, ?3)",
            rusqlite::params![&name, ns, fold],
        )?;
        conn.execute(
            // `DISTINCT`, because the fold `big_breasts` belongs to **two** tags in the
            // fixture (`big_breasts` and `female:big_breasts`), and an item carrying
            // both spellings would otherwise be offered twice to one alias tag. That
            // duplication is the fold semantics the query box relies on, and it is the
            // reason the ladder counts items and not postings.
            "INSERT INTO item_tags (tag_id, item_id) \
             SELECT (SELECT id FROM tags WHERE fold = ?1), it.item_id \
             FROM item_tags AS it JOIN tags AS t ON t.id = it.tag_id \
             WHERE t.fold = 'big_breasts' GROUP BY it.item_id LIMIT ?2",
            rusqlite::params![&name, n],
        )?;
        made.push((name, n));
    }
    conn.execute("ANALYZE", [])?;
    Ok(made)
}

fn drop_alias_tags(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM item_tags WHERE tag_id IN \
         (SELECT id FROM tags WHERE fold LIKE 'fixture_alias_%')",
        [],
    )?;
    conn.execute("DELETE FROM tags WHERE fold LIKE 'fixture_alias_%'", [])?;
    conn.execute("ANALYZE", [])?;
    Ok(())
}

/// What the planner would pay to know the shape: count the postings for one token, the
/// way `item_tags_tag_idx` can answer it.
fn decision_cost(conn: &Connection, fold: &str) -> (u128, u128, i64) {
    let sql = "SELECT COUNT(*) FROM item_tags AS it JOIN tags AS t ON t.id = it.tag_id \
               WHERE t.fold = ? OR t.name = ?";
    let count = timed(|| {
        let mut stmt = conn.prepare_cached(sql).expect("decision prepares");
        stmt.query_row(rusqlite::params![fold, fold], |r| r.get::<_, i64>(0))
            .expect("decision runs")
    });
    let mut stmt = conn.prepare_cached(sql).expect("decision prepares");
    let rows = stmt
        .query_row(rusqlite::params![fold, fold], |r| r.get::<_, i64>(0))
        .expect("decision runs");
    (count.0, count.1, rows)
}

fn cardinality_crossover(conn: &Connection) -> rusqlite::Result<()> {
    let ladder = add_alias_tags(conn)?;
    println!("\n=== where the two tag shapes cross over ===");
    for (fold, wanted) in &ladder {
        let total = tag_count(conn, TAG_FIRST, fold);
        let pages = (total + PAGE - 1) / PAGE;
        println!(
            "\n{fold:?}: asked for {wanted} postings, the tag matches {total} items in {pages} pages"
        );
        if total == 0 {
            println!("  the alias has no postings - nothing to time");
            continue;
        }
        for (label, predicate) in [("tag-first", TAG_FIRST), ("row-first", ROW_FIRST)] {
            let rows = tag_page(conn, predicate, fold, None);
            let page1 = timed(|| tag_page(conn, predicate, fold, None).len());
            let count = timed(|| tag_count(conn, predicate, fold));
            let mut deep = String::new();
            if pages >= 2 {
                let seed_sql = format!(
                    "SELECT items.id, items.updated_at FROM items WHERE items.kind IN \
                     ('gallery','post') AND ({predicate}){ORDER} LIMIT ? OFFSET ?"
                );
                let mut stmt = conn.prepare_cached(&seed_sql).expect("seed prepares");
                let seed = stmt
                    .query_map(
                        rusqlite::params![fold, fold, PAGE, (pages - 2) * PAGE],
                        |r| {
                            Ok(db::PageCursor {
                                updated_at: r.get(1)?,
                                id: r.get(0)?,
                            })
                        },
                    )
                    .expect("seed runs")
                    .collect::<Result<Vec<_>, _>>()
                    .expect("seed rows")
                    .last()
                    .cloned()
                    .expect("a page before the last one");
                drop(stmt);
                let n = tag_page(conn, predicate, fold, Some(&seed));
                let t = timed(|| tag_page(conn, predicate, fold, Some(&seed)).len());
                deep = format!(
                    "page {pages} by cursor {} rows, {} ms p50 / {} ms p95",
                    n.len(),
                    t.0,
                    t.1
                );
            }
            println!(
                "  {label:<10} page 1: {} rows, {} ms p50 / {} ms p95; count alone {} ms / {} ms{}",
                rows.len(),
                page1.0,
                page1.1,
                count.0,
                count.1,
                if deep.is_empty() {
                    String::new()
                } else {
                    format!("; {deep}")
                }
            );
        }
        let (d50, d95, postings) = decision_cost(conn, fold);
        println!(
            "  the shape decision itself (count this token's postings): {} ms p50 / {} ms p95, {} postings",
            d50, d95, postings
        );
    }
    drop_alias_tags(conn)?;
    println!("\nalias tags dropped; schema back to what the app creates");
    Ok(())
}

/// Decision 13, measured under the four conditions the review set.
///
/// * **Statistics.** There is no `PRAGMA analyze` anywhere in this repo, so production's
///   planner has never seen the library it plans for. This probe has run `ANALYZE` at
///   fixture build since its first run, which means every number in the earlier logs
///   describes a planner the app never has. Three worlds are measured: no statistics,
///   statistics that predate the postings (what a vault looks like right after a bulk
///   import), and statistics taken after them.
/// * **The mixed AND.** `dense AND rare` in both typing orders, and `dense AND dense`,
///   because "anchor on the rare term" is a claim about what the planner does, not about
///   what the code intends.
/// * **Negated tag terms.** A negated dense tag has to stay a row-side probe; as an
///   anti-join it is the catastrophic shape, and that is measured, not assumed.
/// * **Print the shape.** Every timing line carries the planner's own driving step and
///   whether it sorts the whole match set, so a regression is visible in the log instead
///   of inferred from a changed number.
fn shape_choice(conn: &Connection) -> rusqlite::Result<()> {
    println!("\n=== decision 13: statistics, the AND, and negated tags ===");
    println!(
        "  semi = `db::page_items` with the term written as the tag-first semi-join, which is what \
         every earlier section of this file measured; probe = the same term written as `EXISTS`, \
         hand-built here. The section after the ladder runs the shipping path instead, where the \
         code picks the shape itself. Page of {PAGE}, p50/p95 over {REPS} reps, row count beside \
         every timing."
    );

    // ---- the statistics the app never takes ---------------------------------------
    println!("\n--- one tag, three states of the planner's knowledge (the ladder) ---");
    for n in CARDINALITY_LADDER {
        let fold = format!("fixture_alias_{n}");
        alias_down(conn, &fold)?;
        // Stale first: statistics exist but do not know these postings exist, which is
        // exactly the state of a vault after an import that never re-ran ANALYZE.
        conn.execute("ANALYZE", [])?;
        let before = stats_age(conn)?;
        alias_up(conn, &fold, n)?;
        let stale = shape_row(conn, &fold);
        let probe_stale = probe_row(conn, &fold, false);
        let analyzed = timed(|| conn.execute("ANALYZE", []).expect("analyze runs"));
        let fresh = stats_age(conn)?;
        let line_fresh = shape_row(conn, &fold);
        let probe_fresh = probe_row(conn, &fold, false);
        conn.execute("DROP TABLE IF EXISTS sqlite_stat1", [])?;
        let line_none = shape_row(conn, &fold);
        let probe_none = probe_row(conn, &fold, false);
        println!(
            "\n{fold:?}: {n} postings | sqlite_stat1 rows {before} before, {fresh} after \
             | ANALYZE costs {} ms p50 / {} ms p95",
            analyzed.0, analyzed.1
        );
        println!("  stats stale   {stale}");
        println!("  stats stale   {probe_stale}");
        println!("  stats fresh   {line_fresh}");
        println!("  stats fresh   {probe_fresh}");
        println!("  no stats      {line_none}");
        println!("  no stats      {probe_none}");
    }
    alias_reset(conn)?;

    // ---- the dense tags the fixture already has, in production's world ------------
    println!("\n--- the dense tags already in the fixture: no statistics, then fresh ---");
    conn.execute("DROP TABLE IF EXISTS sqlite_stat1", [])?;
    for fold in ["big_breasts", "female:big_breasts", "anal"] {
        let none = shape_row(conn, fold);
        let none_probe = probe_row(conn, fold, false);
        let none_neg = probe_row(conn, fold, true);
        conn.execute("ANALYZE", [])?;
        let fresh = shape_row(conn, fold);
        let fresh_probe = probe_row(conn, fold, false);
        conn.execute("DROP TABLE IF EXISTS sqlite_stat1", [])?;
        println!("\n{fold:?}");
        println!("  no stats      {none}");
        println!("  no stats      {none_probe}");
        println!("  no stats      {none_neg}");
        println!("  stats fresh   {fresh}");
        println!("  stats fresh   {fresh_probe}");
    }

    // ---- the AND: both typing orders, dense AND dense, and the negations ----------
    println!(
        "\n--- the AND under the rule decision 13 proposes: positive tag terms semi-join, \
         negated tag terms stay row-side probes (no statistics, production's world) ---"
    );
    conn.execute("DROP TABLE IF EXISTS sqlite_stat1", [])?;
    let rare = sparse_folds(conn)?
        .into_iter()
        .next()
        .expect("the fixture seeds 20 000 tags, so a tag with 3-200 postings exists");
    println!("  (the sparse term below is {rare:?})");
    and_ladder(conn, &rare)?;

    // ---- the rule as production runs it -------------------------------------------
    //
    // Everything above builds its SQL by hand, because production did not have the other
    // shape yet. This is the part that does: `query::parse` → `Query::plan_with` →
    // `db::page_items`, with the plan's own note beside the timing, and the same page
    // compared id-for-id against the all-semi-join answer. If the rule changed a row, this
    // log says so.
    println!(
        "\n--- the rule in the shipping code: `plan_with` + `db::page_items`, no hand-built SQL ---"
    );
    let shipped: Vec<(&str, String)> = vec![
        ("one dense tag", "big_breasts".to_string()),
        ("one rare tag", rare.clone()),
        ("exact form tag:\"…\"", format!("tag:\"{rare}\"")),
        ("rare AND dense", format!("{rare} big_breasts")),
        ("dense AND rare", format!("big_breasts {rare}")),
        ("dense AND dense", "big_breasts anal".to_string()),
        ("dense AND -dense", "big_breasts -anal".to_string()),
        ("rare AND -dense", format!("{rare} -big_breasts")),
        ("-dense alone", "-big_breasts".to_string()),
    ];
    for (label, text) in &shipped {
        shipped_row(conn, label, text);
    }

    // ---- what a hand-rolled estimator would cost ----------------------------------
    println!("\n--- the cost of knowing: count one token's postings before choosing ---");
    for fold in ["big_breasts", "anal", rare.as_str()] {
        let (d50, d95, postings) = decision_cost(conn, fold);
        println!("  {fold:<22} {postings:>7} postings, counted in {d50} ms p50 / {d95} ms p95");
    }
    // The rule compares a token's postings to the view's rows, and those two numbers are
    // only comparable if the postings are counted **inside the same view**: the fixture
    // hangs tags on game rows too, so a postings count that ignores `kind` reports 20 000
    // for a token only 16 018 media rows carry, and the threshold is then reading a
    // different population at the top and the bottom of the fraction. Both counts are
    // timed, because the honest one is only worth using if it is affordable.
    println!("\n  the same counts, taken inside the view the query runs on:");
    for fold in ["big_breasts", "anal", rare.as_str()] {
        let sql = "SELECT COUNT(*) FROM items \
                   WHERE items.kind IN ('gallery','post') \
                   AND EXISTS (SELECT 1 FROM item_tags AS it JOIN tags AS t ON t.id = it.tag_id \
                                WHERE it.item_id = items.id AND (t.fold = ?1 OR t.name = ?1))";
        let one = || -> i64 {
            let mut stmt = conn.prepare_cached(sql).expect("view count prepares");
            stmt.query_row([fold], |r| r.get::<_, i64>(0))
                .expect("view count runs")
        };
        let n = one();
        let t = timed(one);
        println!(
            "  {fold:<22} {n:>7} view rows carry it, counted in {} ms p50 / {} ms p95",
            t.0, t.1
        );
    }
    // The cheaper way to compare like with like: count **distinct items** carrying the
    // token over the whole library, against the library's own row count, both through an
    // index the engine can walk without touching the `items` table per candidate. If this
    // stays in the same order of magnitude as the raw postings count, it is what the rule
    // should use; if it does not, the raw count and a whole-library denominator do.
    println!("\n  distinct items carrying it, over the whole library (the affordable fix):");
    for fold in ["big_breasts", "anal", rare.as_str()] {
        let sql = "SELECT COUNT(DISTINCT it.item_id) FROM item_tags AS it \
                   JOIN tags AS t ON t.id = it.tag_id WHERE (t.fold = ?1 OR t.name = ?1)";
        let one = || -> i64 {
            let mut stmt = conn.prepare_cached(sql).expect("distinct count prepares");
            stmt.query_row([fold], |r| r.get::<_, i64>(0))
                .expect("distinct count runs")
        };
        let n = one();
        let t = timed(one);
        println!(
            "  {fold:<22} {n:>7} items carry it, counted in {} ms p50 / {} ms p95",
            t.0, t.1
        );
    }
    let all_items = || -> i64 {
        let mut stmt = conn
            .prepare_cached("SELECT COUNT(*) FROM items")
            .expect("library count prepares");
        stmt.query_row([], |r| r.get::<_, i64>(0))
            .expect("library count runs")
    };
    let items = all_items();
    let items_t = timed(all_items);
    println!(
        "\n  the library's row count, unscoped (the comparable denominator): {items} rows, \
         {} ms p50 / {} ms p95",
        items_t.0, items_t.1
    );
    // The rule needs a denominator too — "how big is this view" — and that count is not
    // free either, so it is measured before the rule is written down as cheap.
    let plain_sql = "SELECT COUNT(*) FROM items WHERE items.kind IN ('gallery','post')";
    let index_sql = "SELECT COUNT(*) FROM items WHERE items.kind IS NOT NULL \
                     AND items.kind IN ('gallery','post')";
    let est_sql = "SELECT (SELECT COUNT(*) FROM item_tags AS it JOIN tags AS t \
                   ON t.id = it.tag_id WHERE t.fold = ?1 OR t.name = ?1), \
                   (SELECT COUNT(*) FROM items WHERE items.kind IN ('gallery','post'))";
    let one_count = |sql: &str| -> i64 {
        let mut stmt = conn.prepare_cached(sql).expect("count prepares");
        stmt.query_row([], |r| r.get::<_, i64>(0))
            .expect("count runs")
    };
    let rows_plain = one_count(plain_sql);
    let t_plain = timed(|| one_count(plain_sql));
    let rows_index = one_count(index_sql);
    let t_index = timed(|| one_count(index_sql));
    // The one statement a real estimator would run: this token's postings **and** the
    // denominator, so the decision is one round trip rather than two.
    let estimate = || -> (i64, i64) {
        let mut stmt = conn.prepare_cached(est_sql).expect("estimator prepares");
        stmt.query_row(["big_breasts"], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })
        .expect("estimator runs")
    };
    let (est_postings, est_rows) = estimate();
    let t_est = timed(estimate);
    println!(
        "  view row count, as written:    {rows_plain} rows, {} ms p50 / {} ms p95",
        t_plain.0, t_plain.1
    );
    println!(
        "  view row count, kind not null: {rows_index} rows, {} ms p50 / {} ms p95",
        t_index.0, t_index.1
    );
    println!(
        "  postings + denominator, one statement: {est_postings} of {est_rows} | \
         {} ms p50 / {} ms p95",
        t_est.0, t_est.1
    );
    println!(
        "\nleft as the app leaves it: sqlite_stat1 = {}",
        stats_age(conn)?
    );
    Ok(())
}

/// Creates one ladder tag with `n` postings and does **not** run `ANALYZE`, so the
/// statistics stay exactly as old as the postings are new.
fn alias_up(conn: &Connection, fold: &str, n: i64) -> rusqlite::Result<()> {
    let (ns, norm) = db::tag_ns_fold(fold);
    conn.execute(
        "INSERT INTO tags (name, type_id, site, ns, fold) VALUES (?1, 3, 'fixture', ?2, ?3)",
        rusqlite::params![fold, ns, norm],
    )?;
    conn.execute(
        "INSERT INTO item_tags (tag_id, item_id) \
         SELECT (SELECT id FROM tags WHERE fold = ?1), it.item_id \
         FROM item_tags AS it JOIN tags AS t ON t.id = it.tag_id \
         WHERE t.fold = 'big_breasts' GROUP BY it.item_id LIMIT ?2",
        rusqlite::params![fold, n],
    )?;
    Ok(())
}

fn alias_down(conn: &Connection, fold: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM item_tags WHERE tag_id IN (SELECT id FROM tags WHERE fold = ?1)",
        [fold],
    )?;
    conn.execute("DELETE FROM tags WHERE fold = ?1", [fold])?;
    Ok(())
}

fn alias_reset(conn: &Connection) -> rusqlite::Result<()> {
    for n in CARDINALITY_LADDER {
        alias_down(conn, &format!("fixture_alias_{n}"))?;
    }
    Ok(())
}

/// What the planner knows: the number of `sqlite_stat1` rows, or `absent`.
fn stats_age(conn: &Connection) -> rusqlite::Result<String> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'sqlite_stat1')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok("absent".to_string());
    }
    conn.query_row("SELECT COUNT(*) FROM sqlite_stat1", [], |r| {
        r.get::<_, i64>(0).map(|n| n.to_string())
    })
}

/// The planner's own words for how it reaches `items`, plus whether the query has to sort
/// the whole match set. Printed beside every timing, because "which shape ran" is the thing
/// a later regression has to be visible in.
///
/// Callers pass the **id-only** form of the statement, and that is not a shortcut: the
/// shipping page also carries a correlated thumbnail `EXISTS` with an ORDER BY of its own,
/// whose `USE TEMP B-TREE` shows up in the plan list with no cheap way to tell it apart
/// from the outer query's sort (a subquery's rows hang off row 0, the same parent as the
/// outer steps). Since the thumbnail is identical in both shapes, explaining it away is the
/// honest reading, not a lost measurement.
fn driving_step(conn: &Connection, sql: &str, params: &[&dyn rusqlite::ToSql]) -> String {
    let mut stmt = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("explain prepares");
    let rows = stmt
        .query_map(params, |r| r.get::<_, String>(3))
        .expect("explain runs");
    let mut reach = String::new();
    let mut sort: Option<String> = None;
    for row in rows.flatten() {
        let line = row.trim();
        if reach.is_empty() && line.contains("items") {
            reach = line.to_string();
        }
        if line.contains("TEMP B-TREE") {
            sort = Some(line.to_string());
        }
    }
    // The sort step is printed verbatim rather than as a yes/no, because the two shapes
    // differ in *which* sort they run and that is the whole cost: the semi-join's is
    // `USE TEMP B-TREE FOR ORDER BY` over every matching id, the probe's is
    // `… FOR LAST TERM OF ORDER BY`, the id tie-break inside one stamp.
    format!(
        "{}{}",
        if reach.is_empty() {
            "nothing in the plan names items".to_string()
        } else {
            reach
        },
        match sort {
            Some(line) => format!(" ; {line}"),
            None => " ; no sort step".to_string(),
        }
    )
}

/// The page as the shipping code builds it — `db::page_items`, not a copy of its SQL —
/// for one tag term.
fn shape_row(conn: &Connection, fold: &str) -> String {
    let plan = query::parse(fold, Scope::Media)
        .unwrap_or_else(|e| panic!("{fold:?} must parse: {}", e.message))
        .plan();
    let sql = format!(
        "SELECT items.id FROM items{}{ORDER} LIMIT ?",
        db::planned_where(Scope::Media, &plan)
    );
    let limit = PAGE;
    let params: Vec<&dyn rusqlite::ToSql> = plan
        .bind()
        .into_iter()
        .chain(std::iter::once(&limit as &dyn rusqlite::ToSql))
        .collect();
    let drive = driving_step(conn, &sql, &params);
    let rows = db::page_items(conn, Scope::Media, &plan, PAGE, None)
        .expect("page runs")
        .len();
    let t = timed(|| {
        db::page_items(conn, Scope::Media, &plan, PAGE, None)
            .expect("page runs")
            .len()
    });
    format!(
        "shipping page {rows} rows, {} ms p50 / {} ms p95 | {drive}",
        t.0, t.1
    )
}

/// The id-only form of a hand-built predicate statement, for `EXPLAIN QUERY PLAN`: the
/// thumbnail `EXISTS` in [`db::PAGE_COLUMNS`] is identical in every shape, and its own
/// `USE TEMP B-TREE` would be read as the page's sort. See [`driving_step`].
fn probe_explain(predicate: &str) -> String {
    format!(
        "SELECT items.id FROM items WHERE items.kind IN ('gallery','post') \
         AND ({predicate}){ORDER} LIMIT ?"
    )
}

/// The same term as a row-side probe, and the negated form of it.
fn probe_row(conn: &Connection, fold: &str, negated: bool) -> String {
    let predicate = if negated {
        format!("NOT {ROW_FIRST}")
    } else {
        ROW_FIRST.to_string()
    };
    let params = tag_params(fold, None);
    let bound: Vec<&dyn rusqlite::ToSql> =
        params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    let drive = driving_step(conn, &probe_explain(&predicate), &bound);
    let rows = tag_page(conn, &predicate, fold, None).len();
    let t = timed(|| tag_page(conn, &predicate, fold, None).len());
    format!(
        "{} probe {rows} rows, {} ms p50 / {} ms p95 | {drive}",
        if negated { "negated row" } else { "row-side" },
        t.0,
        t.1
    )
}

/// The two shapes a tag term can take in SQL, measured term by term.
///
/// * [`Shape::Semi`] is what production writes today: `items.id IN (SELECT …)`, which
///   starts at the tag and gathers every posting.
/// * [`Shape::Probe`] is the candidate: `EXISTS (… it.item_id = items.id …)`, which walks
///   `items` in the grid's own order and can stop when the page is full.
/// * A negated tag term is measured both ways ([`Shape::AntiProbe`] and
///   [`Shape::AntiSemi`]), because the rule under test says a negated token takes the
///   first, and that claim needs a number beside it rather than a warning.
#[derive(Clone, Copy)]
enum Shape {
    Semi,
    Probe,
    AntiProbe,
    AntiSemi,
}

impl Shape {
    fn label(self) -> &'static str {
        match self {
            Shape::Semi => "semi",
            Shape::Probe => "probe",
            Shape::AntiProbe => "anti-probe",
            Shape::AntiSemi => "anti-semi",
        }
    }
}

/// One AND, printed with the shapes it used, the matched total, and the planner's own
/// driving step. Typing order is exactly as given, so a difference between two rows is
/// the shape's doing and not the planner re-ordering the terms.
fn and_row(conn: &Connection, label: &str, terms: &[(&str, Shape)]) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut params: Vec<query::Value> = Vec::new();
    let mut shapes: Vec<&str> = Vec::new();
    for (fold, shape) in terms {
        let frag = match shape {
            Shape::Semi => TAG_FIRST.to_string(),
            Shape::Probe => ROW_FIRST.to_string(),
            Shape::AntiProbe => format!("NOT {ROW_FIRST}"),
            Shape::AntiSemi => format!("NOT {TAG_FIRST}"),
        };
        params.push(query::Value::Text((*fold).to_string()));
        params.push(query::Value::Text((*fold).to_string()));
        parts.push(format!("({frag})"));
        shapes.push(shape.label());
    }
    let body = parts.join(" AND ");
    let count_sql =
        format!("SELECT COUNT(*) FROM items WHERE items.kind IN ('gallery','post') AND ({body})");
    let total: i64 = {
        let mut stmt = conn.prepare_cached(&count_sql).expect("count prepares");
        stmt.query_row(rusqlite::params_from_iter(params.iter()), |r| r.get(0))
            .expect("count runs")
    };
    let drive = {
        let limit = PAGE;
        let mut bound: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        bound.push(&limit as &dyn rusqlite::ToSql);
        driving_step(conn, &probe_explain(&body), &bound)
    };
    let ids = and_page(conn, &body, &params);
    let t = timed(|| and_page(conn, &body, &params).len());
    println!(
        "    {:<40} {:<26} {:>7} matches | page {} rows, {} ms p50 / {} ms p95 | {drive}",
        label,
        shapes.join("+"),
        total,
        ids.len(),
        t.0,
        t.1
    );
    ids
}

/// A token and the shape to ask it in. Named because the ladder below nests three of these
/// tuples inside two more, and clippy is right that the bare type stops being readable.
type Trial<'a> = (&'a str, Shape);
/// One labelled way of running the same set of tokens.
type Set<'a> = (&'a str, Vec<Trial<'a>>);
/// One query, measured several ways.
type Group<'a> = (&'a str, Vec<Set<'a>>);

/// Condition 2 of decision 13: the AND, measured under the rule rather than under
/// production's single shape. Each group holds **one set of tokens** measured several
/// ways, and every row of a group has to return the same ids in the same order — which is
/// the equivalence the rule depends on, checked here instead of asserted.
fn and_ladder(conn: &Connection, rare: &str) -> rusqlite::Result<()> {
    let groups: Vec<Group> = vec![
        (
            "rare AND dense",
            vec![
                (
                    "both semi, rare typed first (today)",
                    vec![(rare, Shape::Semi), ("big_breasts", Shape::Semi)],
                ),
                (
                    "semi + probe, rare typed first (rule)",
                    vec![(rare, Shape::Semi), ("big_breasts", Shape::Probe)],
                ),
                (
                    "both semi, dense typed first (today)",
                    vec![("big_breasts", Shape::Semi), (rare, Shape::Semi)],
                ),
                (
                    "probe + semi, dense typed first (rule)",
                    vec![("big_breasts", Shape::Probe), (rare, Shape::Semi)],
                ),
                (
                    "both probe",
                    vec![("big_breasts", Shape::Probe), (rare, Shape::Probe)],
                ),
            ],
        ),
        (
            "dense AND dense",
            vec![
                (
                    "both semi (today)",
                    vec![("big_breasts", Shape::Semi), ("anal", Shape::Semi)],
                ),
                (
                    "semi + probe",
                    vec![("big_breasts", Shape::Semi), ("anal", Shape::Probe)],
                ),
                (
                    "both probe (rule)",
                    vec![("big_breasts", Shape::Probe), ("anal", Shape::Probe)],
                ),
            ],
        ),
        (
            "dense AND -dense",
            vec![
                (
                    "semi + anti-semi (today)",
                    vec![("big_breasts", Shape::Semi), ("anal", Shape::AntiSemi)],
                ),
                (
                    "semi + anti-probe",
                    vec![("big_breasts", Shape::Semi), ("anal", Shape::AntiProbe)],
                ),
                (
                    "probe + anti-probe (rule)",
                    vec![("big_breasts", Shape::Probe), ("anal", Shape::AntiProbe)],
                ),
            ],
        ),
        (
            "rare AND -dense",
            vec![
                (
                    "semi + anti-probe (rule)",
                    vec![(rare, Shape::Semi), ("big_breasts", Shape::AntiProbe)],
                ),
                (
                    "semi + anti-semi (the wrong choice)",
                    vec![(rare, Shape::Semi), ("big_breasts", Shape::AntiSemi)],
                ),
            ],
        ),
        (
            "-dense alone",
            vec![
                ("anti-semi (today)", vec![("big_breasts", Shape::AntiSemi)]),
                ("anti-probe (rule)", vec![("big_breasts", Shape::AntiProbe)]),
            ],
        ),
    ];
    for (group, rows) in groups {
        println!("\n  {group}");
        let mut first: Option<Vec<String>> = None;
        let mut first_label = "";
        for (label, terms) in rows {
            let ids = and_row(conn, label, &terms);
            match &first {
                None => {
                    first = Some(ids);
                    first_label = label;
                }
                Some(want) => {
                    if *want != ids {
                        println!(
                            "    !! DISAGREEMENT against {first_label:?}: {} vs {} rows, and the ids differ",
                            want.len(),
                            ids.len()
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// `tag_page` binds one fold twice; an AND carries a pair of parameters per term, so the
/// page reader takes the list the caller built.
fn and_page(conn: &Connection, body: &str, params: &[query::Value]) -> Vec<String> {
    let sql = tag_sql(body, false);
    let limit = PAGE;
    let mut stmt = conn.prepare_cached(&sql).expect("page prepares");
    stmt.query_map(
        rusqlite::params_from_iter(
            params
                .iter()
                .map(|v| v as &dyn rusqlite::ToSql)
                .chain(std::iter::once(&limit as &dyn rusqlite::ToSql)),
        ),
        |r| r.get::<_, String>(0),
    )
    .expect("page runs")
    .collect::<Result<Vec<_>, _>>()
    .expect("page rows")
}

/// The same query through the **shipping** path: parse, plan with the vault's own numbers,
/// run `db::page_items`. It prints the plan's shape note (decision 13, condition 4), the
/// timing against the semi-join's, and whether the page is id-for-id the page the
/// semi-join shape returns — the claim the whole rule rests on, checked at 100 000 items
/// instead of argued.
fn shipped_row(conn: &Connection, label: &str, text: &str) {
    let parsed = query::parse(text, Scope::Media)
        .unwrap_or_else(|e| panic!("{text:?} must parse: {}", e.message));
    let stats = db::VaultStats::new(conn);
    let shaped = parsed.plan_with(&stats);
    let plain = parsed.plan();
    let shaped_ids = db::page_items(conn, Scope::Media, &shaped, PAGE, None).expect("shaped page");
    let plain_ids = db::page_items(conn, Scope::Media, &plain, PAGE, None).expect("plain page");
    let ids = |p: &Vec<db::ItemSummary>| p.iter().map(|i| i.id.clone()).collect::<Vec<_>>();
    let same = ids(&shaped_ids) == ids(&plain_ids);
    let shaped_t = timed(|| {
        db::page_items(conn, Scope::Media, &shaped, PAGE, None)
            .expect("shaped page")
            .len()
    });
    let plain_t = timed(|| {
        db::page_items(conn, Scope::Media, &plain, PAGE, None)
            .expect("plain page")
            .len()
    });
    // The shape print explains the id-only form of the statement, for the reason
    // `driving_step` states: the thumbnail `EXISTS` is the same in both shapes.
    let explained = format!(
        "SELECT items.id FROM items{}{ORDER} LIMIT ?",
        db::planned_where(Scope::Media, &shaped)
    );
    let mut bound = shaped.bind();
    let limit = PAGE;
    bound.push(&limit as &dyn rusqlite::ToSql);
    println!(
        "\n  {label:<18} {text}\n    {} | {} rows, {} ms p50 / {} ms p95 as shaped, {} ms p50 / {} ms p95 as semi-join | {}",
        shaped
            .shapes
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        shaped_ids.len(),
        shaped_t.0,
        shaped_t.1,
        plain_t.0,
        plain_t.1,
        driving_step(conn, &explained, &bound),
    );
    println!(
        "    same rows as the semi-join page: {}",
        if same {
            "yes"
        } else {
            "NO — the rule changed the answer"
        }
    );
    println!("    plan: {}", full_plan(conn, &explained, &bound));
}

/// Every line the planner gives for one statement, in one string. The shape print above
/// names the step that reaches `items`; this is the whole plan, so a later regression can
/// be read out of the log instead of re-created from a changed number.
fn full_plan(conn: &Connection, sql: &str, params: &[&dyn rusqlite::ToSql]) -> String {
    let mut stmt = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("explain prepares");
    let rows = stmt
        .query_map(params, |r| r.get::<_, String>(3))
        .expect("explain runs");
    rows.flatten()
        .map(|d| d.trim().to_string())
        .collect::<Vec<_>>()
        .join(" / ")
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
