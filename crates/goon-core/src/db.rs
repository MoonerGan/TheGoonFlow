//! Schema v1 + tiny repository helpers. Thumbnails are encrypted BLOBs in the
//! DB (whole-vault nuke stays trivial); content-hash keyed for dedup.

use crate::model::{Item, ItemKind, Status};
use crate::query::{closed_values, keywords, Plan, Scope, SuggestCtx, Value};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "BEGIN;
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS items (
            id             TEXT PRIMARY KEY,
            kind           TEXT NOT NULL CHECK (kind IN ('game','gallery','post')),
            title          TEXT NOT NULL,
            original_title TEXT,
            circle         TEXT,
            status         TEXT NOT NULL DEFAULT 'seen',
            dnf            INTEGER NOT NULL DEFAULT 0,
            rating         INTEGER,
            notes          TEXT NOT NULL DEFAULT '',
            version        TEXT,
            created_at     TEXT NOT NULL,
            updated_at     TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sources (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            item_id    TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
            site       TEXT NOT NULL,
            url        TEXT NOT NULL,
            source_id  TEXT,
            fetched_at TEXT,
            raw        TEXT,
            dead       INTEGER NOT NULL DEFAULT 0,
            UNIQUE (site, url)
        );

        CREATE TABLE IF NOT EXISTS tag_types (
            id       INTEGER PRIMARY KEY,
            key      TEXT NOT NULL UNIQUE,
            label    TEXT NOT NULL,
            priority INTEGER NOT NULL DEFAULT 0,
            color    TEXT
        );

        CREATE TABLE IF NOT EXISTS tags (
            id      INTEGER PRIMARY KEY AUTOINCREMENT,
            name    TEXT NOT NULL UNIQUE,
            type_id INTEGER NOT NULL REFERENCES tag_types(id)
        );

        CREATE TABLE IF NOT EXISTS item_tags (
            item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
            tag_id  INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
            PRIMARY KEY (item_id, tag_id)
        );

        CREATE TABLE IF NOT EXISTS media (
            id      INTEGER PRIMARY KEY AUTOINCREMENT,
            item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
            kind    TEXT NOT NULL DEFAULT 'thumb',
            hash    TEXT NOT NULL,
            mime    TEXT NOT NULL,
            width   INTEGER,
            height  INTEGER,
            data    BLOB NOT NULL
        );

        CREATE UNIQUE INDEX IF NOT EXISTS media_hash_idx ON media (item_id, kind, hash);
        CREATE INDEX IF NOT EXISTS items_status_idx ON items (status);
        CREATE INDEX IF NOT EXISTS items_updated_idx ON items (updated_at DESC);
        CREATE INDEX IF NOT EXISTS sources_item_idx ON sources (item_id);

        INSERT OR IGNORE INTO meta (key, value) VALUES ('schema_version', '1');

        CREATE TABLE IF NOT EXISTS site_settings (
            site    TEXT PRIMARY KEY,
            enabled INTEGER NOT NULL DEFAULT 1,
            cookie  TEXT NOT NULL DEFAULT '',
            rate_ms INTEGER NOT NULL DEFAULT 700
        );

        CREATE TABLE IF NOT EXISTS updates (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            item_id     TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
            site        TEXT NOT NULL,
            old_version TEXT,
            new_version TEXT,
            seen_at     TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS updates_item_idx ON updates (item_id);

        CREATE TABLE IF NOT EXISTS match_suggestions (
            item_id    TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
            site       TEXT NOT NULL,
            url        TEXT NOT NULL,
            title      TEXT NOT NULL,
            confidence REAL NOT NULL,
            found_at   TEXT NOT NULL,
            UNIQUE (item_id, site, url)
        );

        CREATE TABLE IF NOT EXISTS mods (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            item_id    TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
            name       TEXT NOT NULL,
            kind       TEXT NOT NULL DEFAULT 'other',
            path       TEXT NOT NULL,
            args       TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_mods_item ON mods(item_id);

        CREATE TABLE IF NOT EXISTS pair_dismissals (
            a_id         TEXT NOT NULL,
            b_id         TEXT NOT NULL,
            dismissed_at TEXT NOT NULL,
            PRIMARY KEY (a_id, b_id)
        );

        CREATE TABLE IF NOT EXISTS source_images (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            item_id   TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
            url       TEXT NOT NULL,
            mime      TEXT,
            found_at  INTEGER NOT NULL,
            stored_at INTEGER
        );
        CREATE UNIQUE INDEX IF NOT EXISTS source_image_url_idx ON source_images (item_id, url);

        COMMIT;",
    )?;

    // Versioned migrations, run in order. Every step is idempotent so opening
    // an old vault upgrades it in place (DESIGN §4.1: no vault has been published
    // yet, but field-test vaults exist and must keep working).
    let v = schema_version(conn).unwrap_or(1);
    if v < 2 {
        conn.execute_batch(
            "BEGIN;
            INSERT OR IGNORE INTO tag_types (id, key, label, priority, color) VALUES
                (1, 'user_status', 'Personal', 100, '#4ade80'),
                (2, 'site_flag',   'Site',      50, NULL),
                (3, 'content',     'Content',   10, NULL);

            INSERT OR IGNORE INTO tags (id, name, type_id) VALUES
                (1, 'complete', 1),
                (2, 'dnf',      1),
                (3, 'replay',   1);

            UPDATE meta SET value = '2' WHERE key = 'schema_version';
            COMMIT;",
        )?;
    }
    if v < 3 {
        // Field report: direct-file sites named after the first host label
        // ("i" for i.4cdn.org, "wimg" for wimg.rule34.xxx, "booru" for
        // booru.allthefallen.moe). Fold them into their real sites.
        conn.execute_batch(
            "BEGIN;
            UPDATE sources SET site = '4chan' WHERE site = 'i' AND url LIKE '%4cdn.org%';
            UPDATE sources SET site = 'rule34' WHERE site = 'wimg' AND url LIKE '%rule34%';
            UPDATE sources SET site = 'allthefallen' WHERE site = 'booru' AND url LIKE '%allthefallen%';
            UPDATE meta SET value = '3' WHERE key = 'schema_version';
            COMMIT;",
        )?;
    }
    if v < 4 {
        // #18: optional per-game launcher override. Kept out of the base CREATE
        // so ALTER runs exactly once on both fresh and existing vaults. Guarded
        // so re-running is a no-op (idempotent migrations).
        let has_col = conn
            .prepare("SELECT 1 FROM pragma_table_info('items') WHERE name = 'launch_path'")
            .and_then(|mut s| s.exists([]))
            .unwrap_or(false);
        conn.execute_batch(&format!(
            "BEGIN;
            {alter}
            UPDATE meta SET value = '4' WHERE key = 'schema_version';
            COMMIT;",
            alter = if has_col {
                ""
            } else {
                "ALTER TABLE items ADD COLUMN launch_path TEXT;"
            }
        ))?;
    }
    if v < 5 {
        // #14: online-match suggestions. The table itself is in the base batch
        // (CREATE TABLE IF NOT EXISTS), so existing vaults pick it up there —
        // this step only records the version.
        conn.execute_batch(
            "BEGIN;
            UPDATE meta SET value = '5' WHERE key = 'schema_version';
            COMMIT;",
        )?;
    }
    if v < 6 {
        // #25: mod/tool launch recipes (D10). Like suggestions, the table lives
        // in the base batch so existing vaults gain it in place.
        conn.execute_batch(
            "BEGIN;
            UPDATE meta SET value = '6' WHERE key = 'schema_version';
            COMMIT;",
        )?;
    }
    if v < 7 {
        // #25 increment 2: DLL mod deploy state. A recipe is either just
        // linked (a launch recipe) or applied (files deployed into the game
        // folder with *.flowbak backups). Guarded ALTER for existing vaults.
        let has_col = conn
            .prepare("SELECT 1 FROM pragma_table_info('mods') WHERE name = 'status'")
            .and_then(|mut s| s.exists([]))
            .unwrap_or(false);
        conn.execute_batch(&format!(
            "BEGIN;
            {alter}
            UPDATE meta SET value = '7' WHERE key = 'schema_version';
            COMMIT;",
            alter = if has_col {
                ""
            } else {
                "ALTER TABLE mods ADD COLUMN status TEXT NOT NULL DEFAULT 'linked';
                 ALTER TABLE mods ADD COLUMN manifest TEXT;"
            }
        ))?;
    }
    if v < 8 {
        // #31: "keep both" for duplicate suggestions. The table lives in the
        // base batch so existing vaults gain it in place; dismissed pairs never
        // resurface. (ids stored canonically: a_id < b_id.)
        conn.execute_batch(
            "BEGIN;
            UPDATE meta SET value = '8' WHERE key = 'schema_version';
            COMMIT;",
        )?;
    }
    if v < 9 {
        // #53: the candidate images a source page offered, kept whether or not
        // we ever downloaded them, so "what was on that page" survives a
        // re-fetch and a half-finished gallery can be finished later. Like
        // #14/#25/#31, the table is in the base batch (CREATE TABLE IF NOT
        // EXISTS), so existing vaults pick it up there - this step only records
        // the version.
        conn.execute_batch(
            "BEGIN;
            UPDATE meta SET value = '9' WHERE key = 'schema_version';
            COMMIT;",
        )?;
    }
    if v < 10 {
        migrate_v10(conn)?;
    }
    Ok(())
}

/// #76: the data model the query box reads. Three changes in one step:
///
/// * `tags` rebuilt so one row is *(origin site, the name that site spells)* —
///   `site` / `ns` / `name` / `fold` — which is what lets e-hentai's
///   `female:big_breasts` and gelbooru's `big_breasts` stay two rows and still
///   answer one bare token (plan §3.1).
/// * The **facts** the meta predicates read. In `media` for bytes the vault
///   holds (`bytes`, `duration_ms`, `animated`, plus `probed_at` marking which
///   rows a container has already answered for) and in `sources` for bytes it
///   does not (a disk path or a bare link: `bytes`, `duration_ms`, `width`,
///   `height`, `probed_at`). Every one of them is NULL until something measured
///   it — an unknown fact never matches a predicate (76-rulings B).
/// * The `extension` tag type (76-rulings A): "what the file calls itself",
///   kept separate from `type:`, which is what GoonFlow computed from bytes.
///
/// Idempotent: each ALTER is guarded on `pragma_table_info` and the tags
/// rebuild only runs while `tags.fold` is missing.
fn migrate_v10(conn: &Connection) -> rusqlite::Result<()> {
    // ---- facts columns, guarded so a half-finished upgrade is a no-op --------
    let mut adds: Vec<String> = Vec::new();
    for (table, col, decl) in [
        ("media", "bytes", "INTEGER"),
        ("media", "duration_ms", "INTEGER"),
        ("media", "animated", "INTEGER"),
        ("media", "probed_at", "TEXT"),
        ("sources", "bytes", "INTEGER"),
        ("sources", "duration_ms", "INTEGER"),
        ("sources", "width", "INTEGER"),
        ("sources", "height", "INTEGER"),
        ("sources", "probed_at", "TEXT"),
    ] {
        if !has_column(conn, table, col) {
            adds.push(format!("ALTER TABLE {table} ADD COLUMN {col} {decl};"));
        }
    }
    if !adds.is_empty() {
        conn.execute_batch(&format!("BEGIN;\n{}\nCOMMIT;", adds.join("\n")))?;
    }

    // ---- the tags rebuild ---------------------------------------------------
    if !has_column(conn, "tags", "fold") {
        rebuild_tags_v10(conn)?;
    }

    conn.execute_batch(
        "BEGIN;
        -- 76-rulings A: 'what the file calls itself' is its own tag type, so an
        -- mp4/gif/webp tag can never be mistaken for site vocabulary. Low
        -- priority: it must not crowd the Complete chip (D9).
        INSERT OR IGNORE INTO tag_types (id, key, label, priority, color) VALUES
            (4, 'extension', 'File', 5, NULL);

        -- f95's [Completed] prefix is a site fact, not a content tag (D9); it
        -- arrived as content and is corrected here (plan §9 decision 11).
        UPDATE tags SET type_id = 2 WHERE name = 'f95-completed' AND type_id = 3;

        -- The byte size of a stored row is a fact the engine can state exactly,
        -- and without moving the BLOB: length() on an encrypted row returns the
        -- plaintext length (measured through SQLCipher, TESTING item 39).
        UPDATE media SET bytes = length(data) WHERE bytes IS NULL;

        -- A pre-v10 writer wrote 0 for a header it could not read. 0 px is not a
        -- measurement, and a `width<400` query would have matched it: those
        -- become NULL, which is what 'never measured' means (invariant 27).
        UPDATE media SET width = NULL, height = NULL
         WHERE kind = 'full' AND (width = 0 OR height = 0);

        -- What v10 deliberately does NOT do: read the stored files. Pixel size
        -- and animation are facts about bytes, and getting them means reading
        -- every original once — on a large vault that is minutes of blocking
        -- right after the passphrase, which D15 forbids. So those columns stay
        -- NULL (and so never match a predicate) until refine_stored_facts runs
        -- as the facts job (stage S7); anything stored from v10 forward carries
        -- them as it is written.
        --
        -- The planner's index set (plan §6). media(item_id,kind) is deliberately
        -- missing: media_hash_idx already covers that prefix.
        CREATE INDEX IF NOT EXISTS tags_fold_idx ON tags (fold);
        CREATE INDEX IF NOT EXISTS tags_name_idx ON tags (name);
        CREATE INDEX IF NOT EXISTS tags_site_name_idx ON tags (site, name);
        CREATE INDEX IF NOT EXISTS item_tags_tag_idx ON item_tags (tag_id, item_id);
        CREATE INDEX IF NOT EXISTS sources_site_item_idx ON sources (site, item_id);

        UPDATE meta SET value = '10' WHERE key = 'schema_version';
        COMMIT;",
    )?;
    Ok(())
}

/// Timestamps in this schema are epoch **seconds as text** — the shape every
/// command writes through `now_epoch()` in the desktop app, and the shape the
/// `added:` / `updated:` query predicates compare against.
pub fn now_epoch_text() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

/// #76: measure the two facts that can only come from the bytes themselves —
/// the picture's own pixel size (header read, no decode) and whether the
/// container says it moves (gif frame count, PNG `acTL`, WebP `ANIM`/`ANMF`).
///
/// Not part of `migrate_v10`: this reads every stored original, so it runs as
/// the facts job (stage S7) rather than inside unlock (D15: jobs, not modal
/// blocking). It is **bounded and resumable**: `limit` rows per call, a row
/// already looked at is marked in `media.probed_at` and never re-read, and an
/// existing measurement is never replaced — `IFNULL` keeps it — so the job can
/// be cancelled and resumed without un-probing anything (invariant 27).
///
/// `full` rows only: a re-encoded 360 px tile says nothing about the original's
/// size or motion, so thumb rows keep `animated` NULL. `duration_ms` is not this
/// function's to know — no image container says how long a video is, and that
/// fact comes from the same job's media probe.
pub fn refine_stored_facts(conn: &Connection, limit: usize) -> rusqlite::Result<FactScan> {
    let mut out = FactScan::default();
    let mut rows: Vec<(i64, Vec<u8>)> = Vec::new();
    {
        // probed_at is the progress marker: a row whose container would not
        // answer is still a row that was read, and must not be read again.
        let mut stmt = conn.prepare(
            "SELECT id, data FROM media
             WHERE kind = 'full' AND probed_at IS NULL
             ORDER BY id LIMIT ?1",
        )?;
        let mapped = stmt.query_map(params![limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        for row in mapped {
            rows.push(row?);
        }
    }
    // IFNULL on the measurements: a container that will not answer here must
    // not erase a size some earlier path did record (invariant 27 only lets a
    // fact be unknown, never unknown again).
    let mut upd = conn.prepare(
        "UPDATE media
         SET width = IFNULL(?2, width),
             height = IFNULL(?3, height),
             animated = IFNULL(?4, animated),
             probed_at = ?5
         WHERE id = ?1",
    )?;
    for (id, data) in rows {
        out.scanned += 1;
        let dims = crate::media::image_dimensions(&data);
        if dims.is_some() {
            out.dims += 1;
        }
        let animated = crate::media::is_animated(&data).map(|a| a as i64);
        match animated {
            Some(1) => out.animated += 1,
            Some(_) => out.still += 1,
            None => out.unknown_animation += 1,
        }
        let (w, h) = match dims {
            Some((w, h)) if w > 0 && h > 0 => (Some(w as i64), Some(h as i64)),
            _ => (None, None),
        };
        upd.execute(params![id, w, h, animated, now_epoch_text()])?;
    }
    Ok(out)
}

/// What one fact-scan pass looked at and what it learned. Returned rather than
/// logged so the job can report progress and a test can assert the numbers.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FactScan {
    pub scanned: usize,
    pub dims: usize,
    pub animated: usize,
    pub still: usize,
    pub unknown_animation: usize,
}

fn has_column(conn: &Connection, table: &str, col: &str) -> bool {
    conn.prepare("SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2")
        .and_then(|mut s| s.exists(params![table, col]))
        .unwrap_or(false)
}

/// Rebuild `tags` with provenance, reusing every old id so `item_tags` needs no
/// remapping. Dropping the parent table would cascade-delete every tag link
/// (`ON DELETE CASCADE`), and `PRAGMA foreign_keys` cannot change inside a
/// transaction — so it goes off before `BEGIN` and is restored straight after
/// `COMMIT`, which is SQLite's documented sequence for this kind of rebuild.
fn rebuild_tags_v10(conn: &Connection) -> rusqlite::Result<()> {
    let mut rows: Vec<(i64, String, i64)> = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT id, name, type_id FROM tags")?;
        for row in stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))? {
            rows.push(row?);
        }
    }
    conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    let rebuilt = (|| -> rusqlite::Result<()> {
        conn.execute_batch(
            "BEGIN;
            CREATE TABLE tags_new (
                id      INTEGER PRIMARY KEY,
                site    TEXT NOT NULL DEFAULT '',
                ns      TEXT,
                name    TEXT NOT NULL,
                fold    TEXT NOT NULL,
                type_id INTEGER NOT NULL REFERENCES tag_types(id),
                UNIQUE (site, name)
            );",
        )?;
        let mut ins = conn.prepare(
            "INSERT INTO tags_new (id, site, ns, name, fold, type_id)
             VALUES (?1, '', ?2, ?3, ?4, ?5)",
        )?;
        for (id, name, type_id) in &rows {
            let (ns, fold) = tag_ns_fold(name);
            ins.execute(params![id, ns, name, fold, type_id])?;
        }
        conn.execute_batch(
            "DROP TABLE tags;
            ALTER TABLE tags_new RENAME TO tags;
            COMMIT;",
        )
    })();
    if rebuilt.is_err() {
        // leave no half-rebuild behind; the next open runs v10 again
        let _ = conn.execute_batch("ROLLBACK;");
    }
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    rebuilt?;
    // Cheap proof the swap did not orphan a link: with FKs back on, a violated
    // or dangling key shows up here, and an upgrade that quietly lost every tag
    // would be a far worse bug than a failed one.
    let mut chk = conn.prepare("PRAGMA foreign_key_check")?;
    let broken = chk
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<Result<Vec<String>, _>>()?;
    if !broken.is_empty() {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!(
                "v10 tag rebuild left bad keys: {}",
                broken.join(", ")
            )),
        ));
    }
    Ok(())
}

/// Tag namespaces the source sites spell in front of the name: e-hentai keeps
/// them inside the tag string it hands us (`female:big_breasts`, `artist:x`),
/// e621/danbooru use the same category words. The list is closed and code-owned
/// — it decides what `fold` strips, and a prefix that is not here is part of the
/// tag (`foo:bar` folds to itself), which is what keeps unknown namespaces from
/// silently merging two different facts (plan §9 decision 4).
pub const TAG_NAMESPACES: &[&str] = &[
    "artist",
    "character",
    "copyright",
    "species",
    "class",
    "meta",
    "general",
    "lore",
    "fanapp",
    "dragon",
    "container",
    "occasion",
    "background",
    "location",
    "parody",
    "language",
    "costuming",
    "cosmetics",
    "mixed",
    "non-h",
    "male",
    "female",
    "other",
    "temp",
    "invalid",
    "error",
    "solved",
    "reply",
    "image",
    "sound",
    "video",
];

/// Split a site-spelled tag name into its namespace and the folded name the
/// cross-site union matches on: `female:big_breasts` -> (`female`,
/// `big_breasts`), `f95-completed` -> (None, `f95-completed`). Only the leading
/// namespace is stripped, and only when it is one we know; the fold keeps the
/// site's spelling apart from it, so `big:breasts` still folds to `big:breasts`.
pub fn tag_ns_fold(name: &str) -> (Option<&str>, &str) {
    if let Some((ns, rest)) = name.split_once(':') {
        let lowered = ns.trim().to_ascii_lowercase();
        if !rest.is_empty() && TAG_NAMESPACES.iter().any(|n| *n == lowered) {
            return (Some(ns), rest);
        }
    }
    (None, name)
}

pub fn schema_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
        [],
        |r| r.get(0),
    )
}

// ---------- generic vault meta KV (#24: proxy_url lives here) ----------

pub fn get_meta(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
        r.get(0)
    })
    .ok()
    .filter(|v: &String| !v.is_empty())
}

pub fn set_meta(conn: &Connection, key: &str, value: Option<&str>) -> rusqlite::Result<()> {
    match value {
        Some(v) => conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, v],
        )?,
        None => conn.execute("DELETE FROM meta WHERE key = ?1", params![key])?,
    };
    Ok(())
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct StatusCount {
    pub status: &'static str,
    pub count: i64,
}

pub fn counts_by_status(
    conn: &Connection,
    kind: Option<&str>,
) -> rusqlite::Result<Vec<StatusCount>> {
    let mut stmt = conn.prepare(
        "SELECT status, count(*) FROM items
         WHERE (?1 IS NULL
                OR (?1 = 'game' AND kind = 'game')
                OR (?1 = 'media' AND kind IN ('gallery','post')))
         GROUP BY status",
    )?;
    let rows = stmt.query_map(params![kind], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (status, count) = row?;
        map.insert(status, count);
    }
    Ok([
        Status::Seen,
        Status::Tracking,
        Status::Wishlist,
        Status::Downloaded,
        Status::Played,
    ]
    .into_iter()
    .map(|s| StatusCount {
        status: s.as_str(),
        count: map.get(s.as_str()).copied().unwrap_or(0),
    })
    .collect())
}

pub fn counts_by_kind(conn: &Connection) -> rusqlite::Result<Vec<(&'static str, i64)>> {
    let mut stmt = conn.prepare("SELECT kind, count(*) FROM items GROUP BY kind")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (kind, count) = row?;
        map.insert(kind, count);
    }
    Ok([ItemKind::Game, ItemKind::Gallery, ItemKind::Post]
        .into_iter()
        .map(|k| (k.as_str(), map.get(k.as_str()).copied().unwrap_or(0)))
        .collect())
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteCount {
    pub site: String,
    pub count: i64,
}

/// Which sites the library actually came from — drives the Media view's
/// "where is it from" filter.
pub fn counts_by_site(conn: &Connection, kind: Option<&str>) -> rusqlite::Result<Vec<SiteCount>> {
    let mut stmt = conn.prepare(
        "SELECT s.site, count(DISTINCT s.item_id) FROM sources s JOIN items i ON i.id = s.item_id
         WHERE (?1 IS NULL
                OR (?1 = 'game' AND i.kind = 'game')
                OR (?1 = 'media' AND i.kind IN ('gallery','post')))
         GROUP BY s.site
         ORDER BY count(DISTINCT s.item_id) DESC, s.site",
    )?;
    let rows = stmt.query_map(params![kind], |r| {
        Ok(SiteCount {
            site: r.get(0)?,
            count: r.get(1)?,
        })
    })?;
    rows.collect()
}

pub fn insert_item(conn: &Connection, item: &Item) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO items (id, kind, title, original_title, circle, status, dnf, rating, notes, version, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            item.id,
            item.kind.as_str(),
            item.title,
            item.original_title,
            item.circle,
            item.status.as_str(),
            item.dnf as i64,
            item.rating.map(|r| r as i64),
            item.notes,
            item.version,
            item.created_at,
            item.updated_at
        ],
    )?;
    Ok(())
}

/// Grid view row: everything the card needs without loading media.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemSummary {
    pub id: String,
    pub kind: &'static str,
    pub title: String,
    pub status: String,
    pub version: Option<String>,
    pub has_thumb: bool,
    pub updated_at: String,
}

/// Filters for one grid page. Grew out of #40 (source-state filters) and the
/// M2 site filter; as a plain argument list it was already eight wide, so every
/// new facet would have pushed it further past clippy's limit (#51). Call sites
/// are meant to read as `ListQuery { kind: Some("game"), ..Default::default() }`.
#[derive(Debug)]
pub struct ListQuery<'a> {
    pub limit: i64,
    pub offset: i64,
    pub search: Option<&'a str>,
    /// `"game"` or `"media"` (media = galleries + posts), as the views ask for it.
    pub kind: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub site: Option<&'a str>,
    /// Sidebar source state: `local_only` | `no_local` | `no_tags`.
    pub source: Option<&'a str>,
}

impl Default for ListQuery<'_> {
    fn default() -> Self {
        Self {
            // The grid has no pagination yet; this is the safety ceiling, and the
            // desktop command overrides it the same way it always did.
            limit: 200,
            offset: 0,
            search: None,
            kind: None,
            tag: None,
            site: None,
            source: None,
        }
    }
}

pub fn list_items(conn: &Connection, q: &ListQuery<'_>) -> rusqlite::Result<Vec<ItemSummary>> {
    let like = q.search.map(|s| format!("%{}%", s.trim()));
    // Placeholders are always ?1..?7 — binding with params! fills positions
    // in order, so skipping ?1 while using ?2 would bind offset to ?2.
    let sql = "SELECT id, kind, title, status, version,
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
             ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3";
    let mut stmt = conn.prepare(sql)?;
    let mut out = Vec::new();
    let mapped = stmt.query_map(
        params![like, q.limit, q.offset, q.kind, q.tag, q.site, q.source],
        map_item_summary,
    )?;
    for row in mapped {
        out.push(row?);
    }
    Ok(out)
}

fn map_item_summary(r: &rusqlite::Row<'_>) -> rusqlite::Result<ItemSummary> {
    let kind: String = r.get(1)?;
    Ok(ItemSummary {
        id: r.get(0)?,
        kind: ItemKind::parse(&kind).map(|k| k.as_str()).unwrap_or("game"),
        title: r.get(2)?,
        status: r.get(3)?,
        version: r.get(4)?,
        has_thumb: r.get(5)?,
        updated_at: r.get(6)?,
    })
}

// ---------- the query box, run (stage S3) ---------------------------------------

/// The `WHERE` every query-shaped read runs: the view's own `kind` scope AND the
/// planned body, parenthesised. Two things that shape is deliberate about:
///
/// * the **view** decides which kinds exist and the query never does (D14), which
///   is why [`Scope`] and [`Plan`] stay separate arguments instead of the grammar
///   growing a `kind:` word of its own;
/// * the page, the paired total, the select-all id list, the tag cloud and the
///   site counts all come through this one function, so they cannot drift apart.
///   The library used to keep three hand-maintained copies of the same sentence
///   (`list_items`, `source_stats`, `games_needing_match`) and they had already
///   diverged; plan §9 names that, and this is the fix for it.
///
/// Public because `filter_scale_probe` feeds this exact string to
/// `EXPLAIN QUERY PLAN`: a probe that rebuilt the SQL by hand would be measuring
/// something other than what ships.
pub fn planned_where(scope: Scope, plan: &Plan) -> String {
    let scope_sql = match scope {
        Scope::Games => "items.kind = 'game'",
        Scope::Media => "items.kind IN ('gallery','post')",
    };
    if plan.sql.is_empty() {
        format!(" WHERE {scope_sql}")
    } else {
        format!(" WHERE {scope_sql} AND ({})", plan.sql)
    }
}

/// The plan's own parameters, then the statement's trailing values, in that
/// order: the planned `?` marks are anonymous and always appear first in the SQL.
/// One function rather than five call sites, because getting this order wrong is
/// the placeholder-skipping bug `db.rs`'s old list query had.
fn planned_params(plan: &Plan, tail: &[i64]) -> Vec<Value> {
    let mut out = plan.params.clone();
    out.extend(tail.iter().map(|v| Value::Int(*v)));
    out
}

/// One grid page, plus the size of the result it came from.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub items: Vec<ItemSummary>,
    /// Everything the predicate matches, not what fits on this page: 300 rows out
    /// of 64 000 matches has to say 64 000 (rulings C, and the old grid capped
    /// both numbers at `limit` without ever saying so).
    pub total: i64,
    /// The plan read a fact column this vault may never have measured
    /// ([`Plan::needs_facts`]). `total == 0` **and** `needs_facts` is not an answer
    /// — stage S5 renders "not measured yet · measure my files" for it instead of
    /// a count that reads like one (invariant 27).
    pub needs_facts: bool,
}

/// Where the grid stopped: the sort key of the last row the user was actually
/// shown. The grid asks for "the next `limit` rows after this one", not "skip N
/// rows", because an offset is a position in a list that a bulk action keeps
/// changing. `filter_scale_probe` deletes 20 000 matched rows out of a 64 092-row
/// result and reads the next page both ways (TESTING item 44): the `OFFSET` read
/// starts **1 200 surviving rows past** where the user was, so those rows are never
/// shown, while the cursor read starts at the first survivor after the cursor and
/// passes nothing over. #73's *Delete selected* is that delete.
///
/// Both fields are one struct rather than two optional arguments because half a
/// cursor is not a question: a cursor with the stamp but not the id would drop the
/// id tie-break, and page 1 and page 2 would stop being disjoint — the same defect
/// the `ORDER BY` already carries a tie-break to prevent.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageCursor {
    /// `items.updated_at` **as stored** — TEXT, ten digits — because the grid's
    /// order compares that TEXT, so the cursor has to compare in the same order
    /// (see the order note on [`search_items`]).
    pub updated_at: String,
    pub id: String,
}

/// The grid's order, as one string, because three readers have to agree on it: the
/// page, the select-all id list, and `filter_scale_probe`'s counter-example. The id
/// tie-break is not decoration — an importer stamps a batch in the same second, and
/// without it the page-1 and page-2 sorts are independent, which repeats some rows
/// and silently drops others. It is also what makes a cursor possible at all: a row
/// whose place in the order is not unique cannot name a position to resume after.
pub const GRID_ORDER: &str = " ORDER BY updated_at DESC, items.id";

/// The cursor half, as its own fragment, because it has to read exactly like
/// [`GRID_ORDER`]: `<` on the stamp (sorted DESC) and `>` on the id inside the tie
/// (sorted ASC). No other paging shape may use a different pair, and no row is ever
/// skipped by arithmetic.
const AFTER_CURSOR: &str = " AND (items.updated_at < ? OR (items.updated_at = ? AND items.id > ?))";

/// The row shape of one grid page, as its own fragment. Public for the same reason
/// [`planned_where`] is: `filter_scale_probe` times the `OFFSET` page this stage
/// replaced against the shipping statement, and a counter-example that selected a
/// different column list would be comparing different work — the thumbnail `EXISTS`
/// alone is a correlated subquery per row.
pub const PAGE_COLUMNS: &str = "SELECT id, kind, title, status, version,
                EXISTS (SELECT 1 FROM media m WHERE m.item_id = items.id AND m.kind = 'thumb'),
                updated_at
         FROM items";

/// The page statement: the one predicate, then the cursor, then the limit. Public
/// so `filter_scale_probe` times and explains the string production runs instead of
/// a paraphrase of it — the `OFFSET` variant the probe compares that against is the
/// shape this replaced, and it exists nowhere else in the codebase.
pub fn page_statement(
    scope: Scope,
    plan: &Plan,
    after: Option<&PageCursor>,
    limit: i64,
) -> (String, Vec<Value>) {
    let mut sql = format!("{PAGE_COLUMNS}{}", planned_where(scope, plan));
    let mut params = plan.params.clone();
    if let Some(cursor) = after {
        sql.push_str(AFTER_CURSOR);
        params.push(Value::Text(cursor.updated_at.clone()));
        params.push(Value::Text(cursor.updated_at.clone()));
        params.push(Value::Text(cursor.id.clone()));
    }
    sql.push_str(GRID_ORDER);
    sql.push_str(" LIMIT ?");
    params.push(Value::Int(limit));
    (sql, params)
}

/// The grid, filtered by a parsed query: one page and the paired total.
///
/// `after` is the cursor of the last visible row ([`PageCursor`]); `None` is page
/// one. There is deliberately **no** offset: an offset counts rows that another
/// action may have deleted in the meantime, which is how a paged grid loses rows
/// without ever showing them (measured in the probe, and pinned by the test
/// `a_bulk_delete_cannot_make_a_paged_grid_skip_rows`).
///
/// The count is a separate statement on purpose, and it counts the **whole**
/// result, cursor or no cursor — "N matches · M total" is about the predicate, not
/// about the page. `filter_scale_probe` (TESTING item 41) measured the count at
/// 88 ms for a dense tag and up to 174 ms for a fact range at 100k items, while a
/// 300-row page can stop early on the `updated_at` order and costs 1-10 ms for
/// those same fact queries: the total, not the grid, is what a keystroke must not
/// pay. Callers that do not want the number use [`count_matches`] or
/// [`search_item_ids`] directly.
///
/// `ORDER BY updated_at DESC, items.id`: the id tie-break is not decoration. An
/// importer stamps a dozen items with the same second, and with no tie-break the
/// page-1 and page-2 sorts are independent, which repeats some rows and silently
/// drops others — and a cursor could not be built from a row whose position in the
/// order is not unique.
pub fn search_items(
    conn: &Connection,
    scope: Scope,
    plan: &Plan,
    limit: i64,
    after: Option<&PageCursor>,
) -> rusqlite::Result<SearchPage> {
    let (sql, params) = page_statement(scope, plan, after, limit);
    let mut items = Vec::new();
    {
        // Scoped: the statement borrows the connection, and the count below needs
        // the connection back.
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), map_item_summary)?;
        for row in rows {
            items.push(row?);
        }
    }
    Ok(SearchPage {
        items,
        total: count_matches(conn, scope, plan)?,
        needs_facts: plan.needs_facts(),
    })
}

/// How many items the predicate matches across the whole library. With an empty
/// plan it is the view's total, which is the `M` in "N matches · M total".
pub fn count_matches(conn: &Connection, scope: Scope, plan: &Plan) -> rusqlite::Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM items{}", planned_where(scope, plan));
    let mut stmt = conn.prepare_cached(&sql)?;
    stmt.query_row(rusqlite::params_from_iter(plan.params.iter()), |r| r.get(0))
}

/// Every id the predicate matches, in grid order, with no `LIMIT`: what "select
/// all" has to mean once the grid is a query instead of a capped page (plan §9,
/// rulings C). Same `planned_where` as the page and the count, so the header
/// number, the visible page and a bulk action on the selection are one predicate
/// seen three ways.
pub fn search_item_ids(
    conn: &Connection,
    scope: Scope,
    plan: &Plan,
) -> rusqlite::Result<Vec<String>> {
    let sql = format!(
        "SELECT items.id FROM items{}{GRID_ORDER}",
        planned_where(scope, plan)
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(plan.params.iter()), |r| {
        r.get::<_, String>(0)
    })?;
    rows.collect()
}

/// The tag cloud **of the current result**. With a query active, a tag that only
/// appears on rows the query excluded is not a suggestion, it is a dead end — the
/// rail used to count the whole library and then show a filtered grid, which is
/// how a chip could produce an empty page. `items` is deliberately left unaliased,
/// because every planned fragment is written against that name.
///
/// Rows are grouped by the **folded** name, because that is what a bare tag term
/// matches: `t.fold = ? OR t.name = ?`. So e-hentai's `female:big_breasts` and
/// another site's `big_breasts` are one row with one count — if they were two
/// rows, a chip for `big_breasts` would promise the count of one spelling and
/// deliver both. The displayed name is the family's shortest spelling (the site
/// prefixes are longest, and a bare word is what the box will hold), and
/// `ORDER BY c DESC, length(name), name` is a real total order, unlike
/// `c DESC, name` over a grouped fold.
pub fn top_tags_matching(
    conn: &Connection,
    scope: Scope,
    plan: &Plan,
    limit: i64,
) -> rusqlite::Result<Vec<TagCount>> {
    let sql = format!(
        "SELECT MIN(t.id) AS id,
                COALESCE(MIN(NULLIF(SUBSTR(t.name, INSTR(t.name, ':') + 1), '')), MIN(t.name)) AS name,
                MIN(tt.key) AS type_key,
                count(DISTINCT it.item_id) AS c
         FROM item_tags AS it
         JOIN tags AS t ON t.id = it.tag_id
         JOIN tag_types AS tt ON tt.id = t.type_id
         JOIN items ON items.id = it.item_id
         {}
         GROUP BY tt.key, t.fold
         ORDER BY c DESC, length(name), name
         LIMIT ?",
        planned_where(scope, plan)
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let bound = planned_params(plan, &[limit]);
    let rows = stmt.query_map(rusqlite::params_from_iter(bound.iter()), |r| {
        Ok(TagCount {
            id: r.get(0)?,
            name: r.get(1)?,
            type_key: r.get(2)?,
            count: r.get(3)?,
        })
    })?;
    rows.collect()
}

/// The "where is it from" counts, over the current result, for the same reason as
/// the tag cloud. A site badge counted library-wide above a filtered grid is what
/// `source_stats` was: a second copy of the predicate, free to drift from it.
pub fn counts_by_site_matching(
    conn: &Connection,
    scope: Scope,
    plan: &Plan,
) -> rusqlite::Result<Vec<SiteCount>> {
    let sql = format!(
        "SELECT s.site, count(DISTINCT s.item_id)
         FROM sources AS s
         JOIN items ON items.id = s.item_id
         {}
         GROUP BY s.site
         ORDER BY count(DISTINCT s.item_id) DESC, s.site",
        planned_where(scope, plan)
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(plan.params.iter()), |r| {
        Ok(SiteCount {
            site: r.get(0)?,
            count: r.get(1)?,
        })
    })?;
    rows.collect()
}

/// One autocomplete row. `token` is the whole term to insert, `-type:video`
/// including the minus the user already typed, so clicking it replaces the token
/// instead of appending to it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub token: String,
    /// `keyword` (a word of the grammar), `value` (a keyword's legal value),
    /// `tag` (a tag in this vault), `site` (a site this vault has seen).
    pub kind: &'static str,
    /// Items this candidate matches **on top of the query already typed** — plan
    /// §5's "counts scoped to the query minus the token being typed". `None` means
    /// the candidate is grammar, not a fact about this vault, and there is no
    /// honest number to show for it.
    pub count: Option<i64>,
}

/// The candidate list under the query box. One call, bounded reads, no loop over
/// candidates: each pool is a `LIMIT`ed `ORDER BY count DESC` read and the prefix
/// filter runs in memory over at most `limit` rows.
///
/// A candidate that would leave the result **empty** is dropped, because that is
/// the dead-end chip the old rails were known for; a candidate with no count
/// (`None`) is grammar and stays, since "this vault has nothing with that fact
/// yet" is no reason to hide the word `stored:`.
///
/// The counts come from the same SQL the query itself would run
/// ([`counts_by_site_matching`] for `source:`, [`top_tags_matching`] for `ext:` and
/// for a bare word), so the number beside a row is the number pressing Enter
/// would print. `tag:` is not completed at all: the bare-word path already lists
/// every tag, and asking for the exact spelling has nothing to guess. The one
/// known cut is `ext:` — it reads the top tags and keeps the extension ones, so a
/// vault holding 20 000 content tags and one rare extension may not surface it
/// until the prefix narrows; worth a `type_id` filter if field use complains.
pub fn suggest(
    conn: &Connection,
    scope: Scope,
    ctx: &SuggestCtx,
    limit: i64,
) -> rusqlite::Result<Vec<Suggestion>> {
    let minus = if ctx.negated { "-" } else { "" };
    // The pool is measured against the query *without* the token being typed, so
    // `source:ehentai bi` still offers `big_breasts`.
    let base = ctx.base.plan();
    let typed = ctx.value.to_lowercase();
    let completes = |name: &str| name.to_lowercase().starts_with(&typed);
    let mut rows: Vec<Suggestion> = Vec::new();

    if let Some(key) = ctx.keyword {
        // A keyword's closed values: the grammar owns these, so they are offered
        // even in a vault where they currently match nothing.
        for value in closed_values(key) {
            if completes(value) {
                rows.push(Suggestion {
                    token: format!("{minus}{key}:{value}"),
                    kind: "value",
                    count: None,
                });
            }
        }
        // A keyword whose vocabulary is the vault's.
        match key {
            "source" => {
                for site in counts_by_site_matching(conn, scope, &base)? {
                    if completes(&site.site) && site.count > 0 {
                        rows.push(Suggestion {
                            token: format!("{minus}source:{}", site.site),
                            kind: "site",
                            count: Some(site.count),
                        });
                    }
                }
            }
            "ext" => {
                for tag in top_tags_matching(conn, scope, &base, limit)? {
                    if tag.type_key != "extension" || !completes(&tag.name) || tag.count == 0 {
                        continue;
                    }
                    rows.push(Suggestion {
                        token: format!("{minus}ext:{}", tag.name),
                        kind: "tag",
                        count: Some(tag.count),
                    });
                }
            }
            // `size:`, `added:`, `circle:` … have no enumerable value set, and
            // inventing one is not on the table (invariant 27).
            _ => {}
        }
        return Ok(rows);
    }

    // No keyword yet: the grammar's own words, then the vault's tags.
    for key in keywords(scope) {
        if completes(key) {
            rows.push(Suggestion {
                token: format!("{minus}{key}:"),
                kind: "keyword",
                count: None,
            });
        }
    }
    for tag in top_tags_matching(conn, scope, &base, limit)? {
        if completes(&tag.name) && tag.count > 0 {
            rows.push(Suggestion {
                token: format!("{minus}{}", tag.name),
                kind: "tag",
                count: Some(tag.count),
            });
        }
    }
    Ok(rows)
}

/// Sidebar badges for the source-state filters: "local only" (no online link —
/// the fix-my-metadata queue), "no local file" (folder lost / never linked),
/// "no tags" (bare imports and unresolved direct-file clips).
///
/// The three queues each have an exact grammar equivalent, pinned by
/// `the_old_source_queues_are_the_same_predicate_in_grammar_text`: `local_only` ≡
/// `stored:local-only`, `no_local` ≡ `-stored:disk`, `no_tags` ≡ `-has:tags`.
/// Stage S5 removes this function with the rails that call it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStats {
    pub local_only: i64,
    pub no_local: i64,
    pub no_tags: i64,
}

pub fn source_stats(conn: &Connection, kind: Option<&str>) -> rusqlite::Result<SourceStats> {
    let kind_sql = match kind {
        Some("game") => "AND kind = 'game'",
        Some("media") => "AND kind IN ('gallery','post')",
        _ => "",
    };
    let count = |extra: &str| -> rusqlite::Result<i64> {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM items WHERE 1=1 {kind_sql} {extra}"),
            [],
            |r| r.get(0),
        )
    };
    Ok(SourceStats {
        local_only: count(
            "AND EXISTS (SELECT 1 FROM sources sl WHERE sl.item_id = items.id AND sl.site = 'local')
             AND NOT EXISTS (SELECT 1 FROM sources so WHERE so.item_id = items.id AND so.site <> 'local')",
        )?,
        no_local: count(
            "AND NOT EXISTS (SELECT 1 FROM sources sn WHERE sn.item_id = items.id AND sn.site = 'local')",
        )?,
        no_tags: count("AND NOT EXISTS (SELECT 1 FROM item_tags tn WHERE tn.item_id = items.id)")?,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn update_item(
    conn: &Connection,
    id: &str,
    title: &str,
    circle: Option<&str>,
    status: Status,
    version: Option<&str>,
    notes: Option<&str>,
    updated_at: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE items
         SET title = ?2, circle = ?3, status = ?4, version = ?5,
             notes = IFNULL(?6, notes), updated_at = ?7
         WHERE id = ?1",
        params![
            id,
            title,
            circle,
            status.as_str(),
            version,
            notes,
            updated_at
        ],
    )?;
    Ok(())
}

/// Delete item + cascading sources/tags/media (FK cascade is on).
pub fn delete_item(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM items WHERE id = ?1", params![id])?;
    // pair_dismissals has no FK (a row names *two* items, not one), so clean it
    // by hand: an entry that no longer exists must not leave a pair permanently
    // un-suggestable.
    conn.execute(
        "DELETE FROM pair_dismissals WHERE a_id = ?1 OR b_id = ?1",
        params![id],
    )?;
    Ok(())
}

// ---------- sources ----------

pub fn source_exists(conn: &Connection, site: &str, url: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sources WHERE site = ?1 AND url = ?2",
        params![site, url],
        |_| Ok(()),
    )
    .is_ok()
}

/// Which item already tracks this exact url on this site (dedup guard for
/// clipping/importing: re-adding the same link must not fork a new item).
pub fn source_item_id(conn: &Connection, site: &str, url: &str) -> Option<String> {
    conn.query_row(
        "SELECT item_id FROM sources WHERE site = ?1 AND url = ?2",
        params![site, url],
        |r| r.get(0),
    )
    .ok()
}

/// Returns true if a new source row was created (false = already tracked).
pub fn insert_source(
    conn: &Connection,
    item_id: &str,
    site: &str,
    url: &str,
    source_id: Option<&str>,
    fetched_at: Option<&str>,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO sources (item_id, site, url, source_id, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![item_id, site, url, source_id, fetched_at],
    )?;
    Ok(n > 0)
}

/// Bump an item's updated_at without touching anything else (#17: linking a
/// freshly extracted folder reorders the library).
pub fn touch_item(conn: &Connection, id: &str, updated_at: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE items SET updated_at = ?2 WHERE id = ?1",
        params![id, updated_at],
    )?;
    Ok(n > 0)
}

/// Detach one source link from an item (source row id, from ItemDetail.sources).
pub fn delete_source(conn: &Connection, item_id: &str, source_id: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM sources WHERE id = ?1 AND item_id = ?2",
        params![source_id, item_id],
    )?;
    Ok(n > 0)
}

// ---------- candidate images on a source page (#53) ----------

/// One image a source page listed, stored or not. The page's attachment list is
/// what a re-fetch would otherwise lose (threads renumber, galleries get
/// trimmed), and `stored_at` records whether we ever pulled the bytes.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceImage {
    pub url: String,
    /// only known after a fetch sniffs the bytes, so a plain candidate has none
    pub mime: Option<String>,
    /// when this url was first seen; a re-fetch never rewrites it, because the
    /// row for a known url is never touched
    pub found_at: i64,
    /// set once the bytes are in `media`; None = still only a candidate
    pub stored_at: Option<i64>,
}

const SOURCE_IMAGE_COLS: &str = "url, mime, found_at, stored_at";

/// Remember the image urls a page offered. UNIQUE (item_id, url) makes this
/// idempotent, which is the whole point: re-fetching a thread lists the same
/// attachments again and again, and that must not grow the vault. Returns how
/// many rows were new, so the caller can report "12 new images" honestly.
pub fn remember_source_images(
    conn: &Connection,
    item_id: &str,
    urls: &[String],
    found_at: i64,
) -> usize {
    let mut added = 0;
    for url in urls {
        // OR IGNORE is the point: a page that lists one image twice, and the
        // same page re-fetched next week, both stay one row.
        let n = conn.execute(
            "INSERT OR IGNORE INTO source_images (item_id, url, found_at)
             VALUES (?1, ?2, ?3)",
            params![item_id, url, found_at],
        );
        added += n.unwrap_or(0);
    }
    added
}

/// The item's candidates, oldest sighting first - the order the page showed
/// them, which is how the reader expects the gallery to read.
pub fn source_images_for(conn: &Connection, item_id: &str) -> Vec<SourceImage> {
    let mut stmt = match conn.prepare(&format!(
        "SELECT {SOURCE_IMAGE_COLS} FROM source_images WHERE item_id = ?1 ORDER BY id",
    )) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    if let Ok(rows) = stmt.query_map(params![item_id], |r| {
        Ok(SourceImage {
            url: r.get(0)?,
            mime: r.get(1)?,
            found_at: r.get(2)?,
            stored_at: r.get(3)?,
        })
    }) {
        for row in rows.flatten() {
            out.push(row);
        }
    }
    out
}

/// Mark one candidate as stored (its bytes are in `media` now). False when the
/// url is not one of this item's candidates: the caller invented a link the
/// page never listed, and that should be visible instead of silent.
pub fn mark_source_image_stored(
    conn: &Connection,
    item_id: &str,
    url: &str,
    stored_at: i64,
) -> bool {
    let n = conn.execute(
        "UPDATE source_images SET stored_at = ?3 WHERE item_id = ?1 AND url = ?2",
        params![item_id, url, stored_at],
    );
    n.unwrap_or(0) > 0
}

// ---------- mods / tools (#25, D10) ----------

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModRow {
    pub id: i64,
    pub name: String,
    /// launch recipe: "mtool" | "cheat" | "other"; "dll" = deployable mod
    pub kind: String,
    /// the tool's .exe (or mod zip/folder) on disk — plain path, not a URL
    pub path: String,
    /// extra arguments recorded with the recipe (empty = none)
    pub args: String,
    pub created_at: String,
    /// "linked" (recipe only) or "applied" (files deployed, backups exist)
    pub status: String,
    /// JSON manifest of deployed files when status = applied
    #[serde(rename = "manifest")]
    pub manifest: Option<String>,
}

const MOD_COLS: &str = "id, name, kind, path, args, created_at, status, manifest";

fn row_to_mod(r: &rusqlite::Row) -> rusqlite::Result<ModRow> {
    Ok(ModRow {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        path: r.get(3)?,
        args: r.get(4)?,
        created_at: r.get(5)?,
        status: r.get(6)?,
        manifest: r.get(7)?,
    })
}

pub fn list_mods(conn: &Connection, item_id: &str) -> rusqlite::Result<Vec<ModRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {MOD_COLS} FROM mods WHERE item_id = ?1 ORDER BY created_at, id",
    ))?;
    let rows = stmt.query_map(params![item_id], row_to_mod)?;
    rows.collect()
}

pub fn get_mod(conn: &Connection, item_id: &str, mod_id: i64) -> rusqlite::Result<Option<ModRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {MOD_COLS} FROM mods WHERE id = ?1 AND item_id = ?2",),
            params![mod_id, item_id],
            row_to_mod,
        )
        .ok())
}

pub fn add_mod(
    conn: &Connection,
    item_id: &str,
    name: &str,
    kind: &str,
    path: &str,
    args: &str,
    created_at: &str,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO mods (item_id, name, kind, path, args, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![item_id, name, kind, path, args, created_at],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn delete_mod(conn: &Connection, item_id: &str, mod_id: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM mods WHERE id = ?1 AND item_id = ?2",
        params![mod_id, item_id],
    )?;
    Ok(n > 0)
}

/// Record a deploy/revert transition (#25 increment 2). `manifest` is the
/// JSON file list when applied, None when back to linked.
pub fn set_mod_status(
    conn: &Connection,
    item_id: &str,
    mod_id: i64,
    status: &str,
    manifest: Option<&str>,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE mods SET status = ?3, manifest = ?4 WHERE id = ?1 AND item_id = ?2",
        params![mod_id, item_id, status, manifest],
    )?;
    Ok(n > 0)
}

/// Does this game carry mods right now? (#25 update-clobber guard: a version
/// bump on a modded game deserves the "update wipes mods" warning.)
pub fn has_mods(conn: &Connection, item_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT 1 FROM mods WHERE item_id = ?1 LIMIT 1",
        params![item_id],
        |_| Ok(()),
    )
    .optional()
    .map(|o| o.is_some())
}

// ---------- tags ----------

pub const TYPE_USER_STATUS: i64 = 1;
pub const TYPE_SITE_FLAG: i64 = 2;
pub const TYPE_CONTENT: i64 = 3;
/// v10 (76-rulings A): what the file calls itself — `mp4`, `gif`, `png`… Taken
/// from the source that names a file, never from the bytes: `type:` is what
/// GoonFlow computed from the bytes, `ext:` is the file's own claim, and the two
/// are allowed to disagree honestly (a `.jpg` link that serves AVIF is both).
pub const TYPE_EXTENSION: i64 = 4;

/// Find or make the row for *(site, name)*. `site` is the site that spells the
/// name — `''` for a tag the user typed or one of unknown provenance, because
/// invariant 21 forbids inventing provenance the app never recorded.
pub fn tag_get_or_create_site(
    conn: &Connection,
    site: &str,
    name: &str,
    type_id: i64,
) -> rusqlite::Result<i64> {
    let (ns, fold) = tag_ns_fold(name);
    conn.execute(
        "INSERT OR IGNORE INTO tags (site, ns, name, fold, type_id) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![site, ns, name, fold, type_id],
    )?;
    conn.query_row(
        "SELECT id FROM tags WHERE site = ?1 AND name = ?2",
        params![site, name],
        |r| r.get(0),
    )
}

pub fn tag_get_or_create(conn: &Connection, name: &str, type_id: i64) -> rusqlite::Result<i64> {
    tag_get_or_create_site(conn, "", name, type_id)
}

/// Record what the file calls itself (76-rulings A). `url_or_path` is any string
/// that ends in the file's own name — a `sources.url`, a `file:///` path — and
/// the extension is stored as an `extension` tag, lowercased, on its own. A name
/// with no usable extension returns Ok(None) rather than inventing one.
pub fn set_item_extension_tag(
    conn: &Connection,
    item_id: &str,
    url_or_path: &str,
) -> rusqlite::Result<Option<i64>> {
    let Some(ext) = extension_of(url_or_path) else {
        return Ok(None);
    };
    let id = tag_get_or_create_site(conn, "", &ext, TYPE_EXTENSION)?;
    link_item_tag(conn, item_id, id)?;
    Ok(Some(id))
}

/// The extension a path or url calls its file: lowercased, alnum only, 1..8
/// characters, query string and fragment stripped. `…/1743761874698456.jpg?x=1`
/// becomes `jpg`; a site page url (`/g/1234/`) or a bare host yields None.
pub fn extension_of(url_or_path: &str) -> Option<String> {
    let path = url_or_path
        .split(['?', '#'])
        .next()
        .unwrap_or(url_or_path)
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("");
    let (_, ext) = path.rsplit_once('.')?;
    let ext = ext.trim().to_ascii_lowercase();
    if ext.is_empty()
        || ext.len() > 8
        || !ext
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return None;
    }
    Some(ext)
}

pub fn link_item_tag(conn: &Connection, item_id: &str, tag_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO item_tags (item_id, tag_id) VALUES (?1, ?2)",
        params![item_id, tag_id],
    )?;
    Ok(())
}

/// Set the item's user_status tag (complete/dnf/replay/...) — at most one.
pub fn set_user_status_tag(
    conn: &Connection,
    item_id: &str,
    tag_name: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM item_tags
         WHERE item_id = ?1 AND tag_id IN (SELECT id FROM tags WHERE type_id = ?2)",
        params![item_id, TYPE_USER_STATUS],
    )?;
    if let Some(name) = tag_name {
        let id = tag_get_or_create(conn, name, TYPE_USER_STATUS)?;
        link_item_tag(conn, item_id, id)?;
    }
    Ok(())
}

// ---------- tags: read + write side (M1 tag grid) ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagOnItem {
    pub id: i64,
    pub name: String,
    pub type_key: String,
}

pub fn tags_for_item(conn: &Connection, item_id: &str) -> rusqlite::Result<Vec<TagOnItem>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name, tt.key FROM item_tags it
         JOIN tags t ON t.id = it.tag_id
         JOIN tag_types tt ON tt.id = t.type_id
         WHERE it.item_id = ?1
         ORDER BY tt.priority DESC, t.name",
    )?;
    let rows = stmt.query_map(params![item_id], |r| {
        Ok(TagOnItem {
            id: r.get(0)?,
            name: r.get(1)?,
            type_key: r.get(2)?,
        })
    })?;
    rows.collect()
}

pub fn add_item_tag(
    conn: &Connection,
    item_id: &str,
    name: &str,
    type_id: i64,
) -> rusqlite::Result<i64> {
    let id = tag_get_or_create(conn, name, type_id)?;
    link_item_tag(conn, item_id, id)?;
    Ok(id)
}

pub fn remove_item_tag(conn: &Connection, item_id: &str, tag_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM item_tags WHERE item_id = ?1 AND tag_id = ?2",
        params![item_id, tag_id],
    )?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCount {
    pub id: i64,
    pub name: String,
    pub type_key: String,
    pub count: i64,
}

/// Tag cloud for a view. `kind` is 'game' | 'media' | None so the Media view
/// shows media tags and the Games view shows game tags.
pub fn top_tags(
    conn: &Connection,
    limit: i64,
    kind: Option<&str>,
) -> rusqlite::Result<Vec<TagCount>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name, tt.key, count(*) AS c FROM item_tags it
         JOIN tags t ON t.id = it.tag_id
         JOIN tag_types tt ON tt.id = t.type_id
         JOIN items i ON i.id = it.item_id
         WHERE (?2 IS NULL
                OR (?2 = 'game' AND i.kind = 'game')
                OR (?2 = 'media' AND i.kind IN ('gallery','post')))
         GROUP BY t.id ORDER BY c DESC, t.name LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit, kind], |r| {
        Ok(TagCount {
            id: r.get(0)?,
            name: r.get(1)?,
            type_key: r.get(2)?,
            count: r.get(3)?,
        })
    })?;
    rows.collect()
}

// ---------- item detail (editor / viewer) ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRow {
    pub id: i64,
    pub site: String,
    pub url: String,
    pub source_id: Option<String>,
    pub fetched_at: Option<String>,
    pub dead: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemDetail {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub original_title: Option<String>,
    pub circle: Option<String>,
    pub status: String,
    pub version: Option<String>,
    pub notes: String,
    /// #18: optional per-game launcher override (absolute path to the .exe
    /// to start). Empty/None = auto-detect from the engine layout.
    pub launch_path: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub has_thumb: bool,
    pub sources: Vec<SourceRow>,
    pub tags: Vec<TagOnItem>,
}

pub fn get_item(conn: &Connection, id: &str) -> rusqlite::Result<Option<ItemDetail>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, title, original_title, circle, status, version, notes,
                launch_path,
                created_at, updated_at,
                EXISTS (SELECT 1 FROM media m WHERE m.item_id = items.id AND m.kind = 'thumb')
         FROM items WHERE id = ?1",
    )?;
    let Some(base) = stmt
        .query_row(params![id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, bool>(11)?,
            ))
        })
        .ok()
    else {
        return Ok(None);
    };
    let mut sstmt = conn.prepare(
        "SELECT id, site, url, source_id, fetched_at, dead FROM sources WHERE item_id = ?1 ORDER BY site",
    )?;
    let sources = sstmt
        .query_map(params![id], |r| {
            Ok(SourceRow {
                id: r.get(0)?,
                site: r.get(1)?,
                url: r.get(2)?,
                source_id: r.get(3)?,
                fetched_at: r.get(4)?,
                dead: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(ItemDetail {
        id: base.0,
        kind: base.1,
        title: base.2,
        original_title: base.3,
        circle: base.4,
        status: base.5,
        version: base.6,
        notes: base.7,
        launch_path: base.8,
        created_at: base.9,
        updated_at: base.10,
        has_thumb: base.11,
        sources,
        tags: tags_for_item(conn, id)?,
    }))
}

/// #18: set/clear the per-game launcher override.
pub fn set_launch_path(
    conn: &Connection,
    id: &str,
    path: Option<&str>,
    updated_at: &str,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE items SET launch_path = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, path, updated_at],
    )?;
    Ok(n > 0)
}

// ---------- match suggestions (#14) ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionRow {
    pub item_id: String,
    pub item_title: String,
    pub site: String,
    pub url: String,
    pub title: String,
    pub confidence: f64,
    pub found_at: String,
}

/// Store/replace one proposed online match for an item.
pub fn put_suggestion(
    conn: &Connection,
    item_id: &str,
    site: &str,
    url: &str,
    title: &str,
    confidence: f64,
    found_at: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO match_suggestions (item_id, site, url, title, confidence, found_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![item_id, site, url, title, confidence, found_at],
    )?;
    Ok(())
}

/// Drop all suggestions for an item (re-scan or after attaching).
pub fn clear_suggestions(conn: &Connection, item_id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM match_suggestions WHERE item_id = ?1",
        params![item_id],
    )?;
    Ok(())
}

pub fn delete_suggestion(
    conn: &Connection,
    item_id: &str,
    site: &str,
    url: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM match_suggestions WHERE item_id = ?1 AND site = ?2 AND url = ?3",
        params![item_id, site, url],
    )?;
    Ok(())
}

fn suggestion_from_row(r: &rusqlite::Row) -> rusqlite::Result<SuggestionRow> {
    Ok(SuggestionRow {
        item_id: r.get(0)?,
        item_title: r.get(1)?,
        site: r.get(2)?,
        url: r.get(3)?,
        title: r.get(4)?,
        confidence: r.get(5)?,
        found_at: r.get(6)?,
    })
}

const SUGGEST_SELECT: &str =
    "SELECT s.item_id, i.title, s.site, s.url, s.title, s.confidence, s.found_at \
     FROM match_suggestions s JOIN items i ON i.id = s.item_id ";

pub fn list_suggestions(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<SuggestionRow>> {
    let mut stmt = conn.prepare(&format!(
        "{SUGGEST_SELECT} ORDER BY s.confidence DESC LIMIT ?1"
    ))?;
    let rows = stmt.query_map(params![limit], suggestion_from_row)?;
    rows.collect()
}

pub fn suggestions_for_item(
    conn: &Connection,
    item_id: &str,
) -> rusqlite::Result<Vec<SuggestionRow>> {
    let mut stmt = conn.prepare(&format!(
        "{SUGGEST_SELECT} WHERE s.item_id = ?1 ORDER BY s.confidence DESC"
    ))?;
    let rows = stmt.query_map(params![item_id], suggestion_from_row)?;
    rows.collect()
}

/// Games tracked only by a local file source — the candidates for online
/// matching (#14). Returns (id, title).
pub fn games_needing_match(conn: &Connection) -> rusqlite::Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT i.id, i.title FROM items i
         WHERE i.kind = 'game'
           AND EXISTS (SELECT 1 FROM sources s WHERE s.item_id = i.id AND s.site = 'local')
           AND NOT EXISTS (SELECT 1 FROM sources s WHERE s.item_id = i.id AND s.site <> 'local')
         ORDER BY i.updated_at DESC",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    rows.collect()
}

pub fn set_item_version(
    conn: &Connection,
    id: &str,
    version: &str,
    updated_at: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE items SET version = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, version, updated_at],
    )?;
    Ok(())
}

// ---------- site settings (M2: enable/disable, cookies, rate) ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteSetting {
    pub site: String,
    pub enabled: bool,
    pub cookie_set: bool,
    pub rate_ms: i64,
}

pub fn get_site_settings(conn: &Connection) -> rusqlite::Result<Vec<SiteSetting>> {
    let mut stmt = conn
        .prepare("SELECT site, enabled, cookie <> '', rate_ms FROM site_settings ORDER BY site")?;
    let rows = stmt.query_map([], |r| {
        Ok(SiteSetting {
            site: r.get(0)?,
            enabled: r.get(1)?,
            cookie_set: r.get(2)?,
            rate_ms: r.get(3)?,
        })
    })?;
    rows.collect()
}

pub fn set_site_setting(
    conn: &Connection,
    site: &str,
    enabled: Option<bool>,
    cookie: Option<&str>,
    rate_ms: Option<i64>,
    default_rate_ms: i64,
) -> rusqlite::Result<()> {
    // #28: the *insert* branch (first touch of a site, e.g. just enabling it)
    // must seed the site's real default — hardcoding 700 clobbered e-hentai's
    // 10500, danbooru's 1200, etc. The update branch still keeps whatever is
    // stored when a field is absent from this call.
    conn.execute(
        "INSERT INTO site_settings (site, enabled, cookie, rate_ms)
         VALUES (?1, IFNULL(?2,1), IFNULL(?3,''), IFNULL(?4,?5))
         ON CONFLICT(site) DO UPDATE SET
           enabled  = IFNULL(?2, enabled),
           cookie   = IFNULL(?3, cookie),
           rate_ms  = IFNULL(?4, rate_ms)",
        params![
            site,
            enabled.map(|e| e as i64),
            cookie,
            rate_ms,
            default_rate_ms
        ],
    )?;
    Ok(())
}

/// #38: forget the cookie a site has, keeping its rate box and on/off state.
///
/// This could not be done through [`set_site_setting`] before: that argument is
/// `Option<&str>` and the statement uses `IFNULL(?3, cookie)`, where `None` means
/// *"leave the stored value alone"*. So "clear the cookie" from the UI wrote
/// nothing and read back as success — the stale paste stayed in the vault and
/// kept being sent. The true answer is an explicit empty write.
/// Returns whether a settings row for this site existed at all.
pub fn clear_site_cookie(conn: &Connection, site: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE site_settings SET cookie = '' WHERE site = ?1",
        params![site],
    )? > 0)
}

/// `default` matters per site: repack mirrors ship off (D8), everything else on.
pub fn site_enabled(conn: &Connection, site: &str, default: bool) -> bool {
    conn.query_row(
        "SELECT enabled FROM site_settings WHERE site = ?1",
        params![site],
        |r| r.get::<_, i64>(0),
    )
    .map(|v| v != 0)
    .unwrap_or(default)
}

/// Politeness delay for this site, in ms (DESIGN §6 rate limiters).
pub fn site_rate_ms(conn: &Connection, site: &str, fallback: i64) -> i64 {
    conn.query_row(
        "SELECT rate_ms FROM site_settings WHERE site = ?1",
        params![site],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(fallback)
}

pub fn site_cookie(conn: &Connection, site: &str) -> Option<String> {
    conn.query_row(
        "SELECT cookie FROM site_settings WHERE site = ?1",
        params![site],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .filter(|c| !c.trim().is_empty())
}

// ---------- update feed (M2) ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRow {
    pub item_id: String,
    pub title: String,
    pub site: String,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
    pub seen_at: String,
    /// #25 clobber guard: this game has mod/tool recipes registered, and an
    /// update deployed over its folder will wipe them.
    pub has_mods: bool,
}

pub fn record_update(
    conn: &Connection,
    item_id: &str,
    site: &str,
    old_version: Option<&str>,
    new_version: Option<&str>,
    seen_at: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO updates (item_id, site, old_version, new_version, seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![item_id, site, old_version, new_version, seen_at],
    )?;
    Ok(())
}

pub fn list_updates(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<UpdateRow>> {
    let mut stmt = conn.prepare(
        "SELECT u.item_id, i.title, u.site, u.old_version, u.new_version, u.seen_at,
                EXISTS (SELECT 1 FROM mods m WHERE m.item_id = u.item_id)
         FROM updates u JOIN items i ON i.id = u.item_id
         ORDER BY u.id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |r| {
        Ok(UpdateRow {
            item_id: r.get(0)?,
            title: r.get(1)?,
            site: r.get(2)?,
            old_version: r.get(3)?,
            new_version: r.get(4)?,
            seen_at: r.get(5)?,
            has_mods: r.get(6)?,
        })
    })?;
    rows.collect()
}

/// What "Check updates" should re-fetch: games the user actually cares about
/// that have a version-bearing source (DLsite product page or f95 thread).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTarget {
    pub item_id: String,
    pub title: String,
    pub old_version: Option<String>,
    pub site: String,
    pub url: String,
}

pub fn update_targets(conn: &Connection) -> rusqlite::Result<Vec<UpdateTarget>> {
    let mut stmt = conn.prepare(
        "SELECT i.id, i.title, i.version, s.site, s.url
         FROM items i JOIN sources s ON s.item_id = i.id
         WHERE i.kind = 'game'
           AND s.site IN ('dlsite', 'f95')
           AND i.status IN ('seen', 'tracking', 'wishlist', 'downloaded', 'played')
         ORDER BY i.updated_at DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(UpdateTarget {
            item_id: r.get(0)?,
            title: r.get(1)?,
            old_version: r.get(2)?,
            site: r.get(3)?,
            url: r.get(4)?,
        })
    })?;
    rows.collect()
}

// ---------- pairing / merge (M2) ----------

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairSuggestion {
    pub a_id: String,
    pub a_title: String,
    pub a_sites: String,
    /// what this entry actually is: its source links (path/URL) — the UI needs
    /// them to tell two same-titled entries apart (#31)
    pub a_src: String,
    pub b_id: String,
    pub b_title: String,
    pub b_sites: String,
    pub b_src: String,
}

/// Comparison key for pairing: drops bracketed/parenthesised tokens (versions,
/// engines, language, "Work in progress") and all punctuation, so the DLsite
/// title "Summer Witch" matches the f95 title "Summer Witch [v0.12] [renpy]".
fn pairing_key(title: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in title.chars() {
        match c {
            '[' | '(' | '{' | '<' => depth += 1,
            ']' | ')' | '}' | '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 && c.is_alphanumeric() => out.extend(c.to_lowercase()),
            _ => {}
        }
    }
    out
}

/// Suggestions grouped by view. `kind` = 'game' | 'media' | None(all).
/// - Games: titles that match once version/engine brackets are stripped — the
///   usual DLsite origin ↔ f95 thread ↔ local folder triple. Titles that name the
///   *tool* instead of the game (`mmnz-game` and friends, see
///   `goon_core::title_is_placeholder`) are never suggested — that was the #47
///   complaint of 40+ `mmnz-game` rows hiding the real pairs. A bucket is also
///   capped, and cross-site pairs come first, so one shared title cannot produce
///   N·(N-1)/2 rows.
/// - Media: **the same stored bytes** (media.hash) on two items — a repost of
///   one image across boorus, or the same file imported twice. Title matching
///   is useless for `1743761874698456.jpg`, so we match content instead.
///
/// These are only ever *suggestions*; a human confirms the merge.
pub fn pairing_suggestions(
    conn: &Connection,
    kind: Option<&str>,
) -> rusqlite::Result<Vec<PairSuggestion>> {
    let want_games = matches!(kind, None | Some("game"));
    let want_media = matches!(kind, None | Some("media"));
    let mut out = Vec::new();

    if want_games {
        let mut stmt = conn.prepare(
            "SELECT id, title,
                    (SELECT group_concat(DISTINCT site) FROM sources s WHERE s.item_id = items.id),
                    (SELECT group_concat(url, ' | ') FROM sources s WHERE s.item_id = items.id)
             FROM items WHERE kind = 'game' ORDER BY title",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut groups: BTreeMap<String, Vec<(String, String, String, String)>> = BTreeMap::new();
        for (id, title, sites, src) in rows {
            let key = pairing_key(&title);
            if key.is_empty() || crate::title_is_placeholder(&title) {
                continue; // a title that names a tool is not an identity (#47)
            }
            groups.entry(key).or_default().push((id, title, sites, src));
        }

        for mut g in groups.into_values() {
            if g.len() < 2 {
                continue;
            }
            g.sort_by(|a, b| a.0.cmp(&b.0));
            // Cross-site pairs are what pairing exists for (the DLsite origin +
            // f95 thread + local folder triple). A dozen games that merely share
            // a title must not bury those, so cross-site pairs come first and a
            // bucket is capped instead of emitting N·(N-1)/2 rows (#47).
            let mut pairs: Vec<(u8, usize, usize)> = Vec::new();
            for i in 0..g.len() {
                for j in (i + 1)..g.len() {
                    pairs.push((if g[i].2 == g[j].2 { 1 } else { 0 }, i, j));
                }
            }
            pairs.sort_by_key(|p| (p.0, p.1, p.2));
            const PAIRS_PER_BUCKET: usize = 12;
            for (_, i, j) in pairs.into_iter().take(PAIRS_PER_BUCKET) {
                if pair_dismissed(conn, &g[i].0, &g[j].0)? {
                    continue; // user said "keep both" (#31)
                }
                out.push(PairSuggestion {
                    a_id: g[i].0.clone(),
                    a_title: g[i].1.clone(),
                    a_sites: g[i].2.clone(),
                    a_src: g[i].3.clone(),
                    b_id: g[j].0.clone(),
                    b_title: g[j].1.clone(),
                    b_sites: g[j].2.clone(),
                    b_src: g[j].3.clone(),
                });
            }
        }
    }

    if want_media {
        // Two items sharing one stored image/video hash are the same media.
        let mut stmt = conn.prepare(
            "SELECT m.hash, m.item_id, i.title,
                    (SELECT group_concat(DISTINCT s.site) FROM sources s WHERE s.item_id = m.item_id),
                    (SELECT group_concat(s.url, ' | ') FROM sources s WHERE s.item_id = m.item_id)
             FROM media m JOIN items i ON i.id = m.item_id
             WHERE i.kind IN ('gallery','post')
             GROUP BY m.hash, m.item_id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut groups: BTreeMap<String, Vec<(String, String, String, String)>> = BTreeMap::new();
        for (_hash, id, title, sites, src) in rows {
            groups
                .entry(_hash)
                .or_default()
                .push((id, title, sites, src));
        }
        for mut g in groups.into_values() {
            // distinct items only (same item can store thumb + full with the
            // same hash)
            g.sort_by(|a, b| a.0.cmp(&b.0));
            g.dedup_by(|a, b| a.0 == b.0);
            if g.len() < 2 {
                continue;
            }
            for i in 0..g.len() {
                for j in (i + 1)..g.len() {
                    if pair_dismissed(conn, &g[i].0, &g[j].0)? {
                        continue; // user said "keep both" (#31)
                    }
                    out.push(PairSuggestion {
                        a_id: g[i].0.clone(),
                        a_title: g[i].1.clone(),
                        a_sites: g[i].2.clone(),
                        a_src: g[i].3.clone(),
                        b_id: g[j].0.clone(),
                        b_title: g[j].1.clone(),
                        b_sites: g[j].2.clone(),
                        b_src: g[j].3.clone(),
                    });
                }
            }
        }
    }

    out.truncate(40);
    Ok(out)
}

/// Canonical (a < b) lookup of a "keep both" dismissal.
fn pair_dismissed(conn: &Connection, a: &str, b: &str) -> rusqlite::Result<bool> {
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    conn.query_row(
        "SELECT 1 FROM pair_dismissals WHERE a_id = ?1 AND b_id = ?2",
        params![a, b],
        |_| Ok(true),
    )
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(false),
        other => Err(other),
    })
}

/// "Keep both": the pair never resurfaces as a duplicate suggestion. Stored
/// canonically so the check is one indexed lookup either way.
pub fn dismiss_pair(conn: &Connection, a: &str, b: &str) -> rusqlite::Result<()> {
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    conn.execute(
        "INSERT OR IGNORE INTO pair_dismissals (a_id, b_id, dismissed_at)
         VALUES (?1, ?2, datetime('now'))",
        params![a, b],
    )?;
    Ok(())
}

/// Merge `drop_id` into `keep_id`: sources, tags, media, updates all move,
/// then the duplicate item disappears.
pub fn merge_items(conn: &Connection, keep_id: &str, drop_id: &str) -> rusqlite::Result<()> {
    // Sources are unique by (site, url), so moving them is an UPDATE, not a
    // copy — a plain copy would collide with the keeper's own rows. Conflicting
    // duplicate links are dropped with the item.
    conn.execute(
        "UPDATE OR IGNORE sources SET item_id = ?1 WHERE item_id = ?2",
        params![keep_id, drop_id],
    )?;
    conn.execute("DELETE FROM sources WHERE item_id = ?1", params![drop_id])?;
    conn.execute(
        "INSERT OR IGNORE INTO item_tags (item_id, tag_id)
         SELECT ?1, tag_id FROM item_tags WHERE item_id = ?2",
        params![keep_id, drop_id],
    )?;
    conn.execute("DELETE FROM item_tags WHERE item_id = ?1", params![drop_id])?;
    // Media rows: keep hashes distinct (UNIQUE item/kind/hash). The v10 fact
    // columns travel with the row — a merge must not un-probe what was measured.
    conn.execute(
        "INSERT OR IGNORE INTO media (item_id, kind, hash, mime, width, height, data, bytes, duration_ms, animated, probed_at)
         SELECT ?1, kind, hash, mime, width, height, data, bytes, duration_ms, animated, probed_at FROM media WHERE item_id = ?2",
        params![keep_id, drop_id],
    )?;
    conn.execute("DELETE FROM media WHERE item_id = ?1", params![drop_id])?;
    conn.execute(
        "INSERT OR IGNORE INTO updates (item_id, site, old_version, new_version, seen_at)
         SELECT ?1, site, old_version, new_version, seen_at FROM updates WHERE item_id = ?2",
        params![keep_id, drop_id],
    )?;
    conn.execute("DELETE FROM updates WHERE item_id = ?1", params![drop_id])?;
    conn.execute("DELETE FROM items WHERE id = ?1", params![drop_id])?;
    // the dropped half of any "keep both" pair goes with it
    conn.execute(
        "DELETE FROM pair_dismissals WHERE a_id = ?1 OR b_id = ?1",
        params![drop_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_initializes_and_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        init_schema(&conn).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), crate::SCHEMA_VERSION);
        let counts = counts_by_status(&conn, None).unwrap();
        assert_eq!(counts.iter().map(|c| c.count).sum::<i64>(), 0);
        let tags: i64 = conn
            .query_row("SELECT count(*) FROM tags WHERE type_id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(tags >= 3);
        // v9 (#53): the candidate table is part of a fresh vault too, not only
        // an upgrade artifact, or a new install would have nowhere to put them.
        let candidates: i64 = conn
            .query_row("SELECT count(*) FROM source_images", [], |r| r.get(0))
            .unwrap();
        assert_eq!(candidates, 0);
        // v10 (#76): the tag rows carry provenance, the media facts columns
        // exist, and the extension tag type is seeded.
        assert!(has_column(&conn, "tags", "site"));
        assert!(has_column(&conn, "tags", "ns"));
        assert!(has_column(&conn, "tags", "fold"));
        for col in ["bytes", "duration_ms", "animated", "probed_at"] {
            assert!(has_column(&conn, "media", col), "media.{col} missing");
        }
        for col in ["bytes", "duration_ms", "width", "height", "probed_at"] {
            assert!(has_column(&conn, "sources", col), "sources.{col} missing");
        }
        let ext_type: i64 = conn
            .query_row(
                "SELECT id FROM tag_types WHERE key = 'extension'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ext_type, TYPE_EXTENSION);
    }

    /// #76: the v10 rebuild must keep every `item_tags` link (the old id space
    /// is reused, and DROP TABLE tags would otherwise cascade them away), split
    /// namespaced names into ns + fold, prove the byte lengths the engine can
    /// state without reading a file, and leave every fact that needs a file
    /// unread until `refine_stored_facts` runs. Built on a hand-made v9-shaped
    /// vault, the same shape a field-test vault arrives as.
    #[test]
    fn v10_rebuild_keeps_links_folds_names_and_backfills_facts() {
        let gif = crate::media::fixtures::gif_with_frames(3);
        let png = crate::media::fixtures::sample_png();
        // a container we can sniff but read no size or frame count out of
        let junk = b"GIF89a".to_vec();
        let (gif_len, png_len, junk_len) = (gif.len() as i64, png.len() as i64, junk.len() as i64);
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE items (
                 id TEXT PRIMARY KEY,
                 kind TEXT NOT NULL CHECK (kind IN ('game','gallery','post')),
                 title TEXT NOT NULL, original_title TEXT, circle TEXT,
                 status TEXT NOT NULL DEFAULT 'seen', dnf INTEGER NOT NULL DEFAULT 0,
                 rating INTEGER, notes TEXT NOT NULL DEFAULT '', version TEXT,
                 created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
             CREATE TABLE sources (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
                 site TEXT NOT NULL, url TEXT NOT NULL, source_id TEXT,
                 fetched_at TEXT, raw TEXT, dead INTEGER NOT NULL DEFAULT 0,
                 UNIQUE (site, url));
             CREATE TABLE tag_types (
                 id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE, label TEXT NOT NULL,
                 priority INTEGER NOT NULL DEFAULT 0, color TEXT);
             CREATE TABLE tags (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 name TEXT NOT NULL UNIQUE,
                 type_id INTEGER NOT NULL REFERENCES tag_types(id));
             CREATE TABLE item_tags (
                 item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
                 tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                 PRIMARY KEY (item_id, tag_id));
             CREATE TABLE media (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
                 kind TEXT NOT NULL DEFAULT 'thumb', hash TEXT NOT NULL,
                 mime TEXT NOT NULL, width INTEGER, height INTEGER, data BLOB NOT NULL);
             INSERT INTO tag_types VALUES
                 (1,'user_status','Personal',100,'#4ade80'),
                 (2,'site_flag','Site',50,NULL),
                 (3,'content','Content',10,NULL);
             INSERT INTO items (id,kind,title,created_at,updated_at) VALUES
                 ('eh1','post','eh gallery','0','1'),('gb1','post','gelbooru post','0','1');
             INSERT INTO tags (id,name,type_id) VALUES
                 (10,'female:big_breasts',3),(11,'big_breasts',3),
                 (12,'f95-completed',3),(13,'complete',1),(14,'other:variant_set',3),
                 (15,'foo:bar',3);
             INSERT INTO item_tags (item_id,tag_id) VALUES
                 ('eh1',10),('eh1',12),('gb1',11),('gb1',13),('gb1',14),('gb1',15);
             INSERT INTO meta (key,value) VALUES ('schema_version','9');",
        )
        .unwrap();
        // The media rows go in as real files, bound as parameters: a fixture
        // that is merely shaped like a gif would prove nothing about a pass that
        // reads containers. This is what a pre-v10 writer left behind — `full`
        // rows with no size (the old writer wrote NULL there), one row with the
        // 0 px an interrupted write could leave, and a tile with its own dims.
        let mut ins = conn
            .prepare(
                "INSERT INTO media (item_id,kind,hash,mime,width,height,data)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )
            .unwrap();
        ins.execute(params![
            "eh1",
            "full",
            "h1",
            "image/gif",
            None::<i64>,
            None::<i64>,
            gif
        ])
        .unwrap();
        ins.execute(params![
            "gb1",
            "full",
            "h2",
            "image/png",
            None::<i64>,
            None::<i64>,
            png
        ])
        .unwrap();
        ins.execute(params!["eh1", "full", "h3", "image/gif", 0i64, 0i64, junk])
            .unwrap();
        ins.execute(params![
            "gb1",
            "thumb",
            "h4",
            "image/gif",
            360i64,
            360i64,
            crate::media::fixtures::gif_with_frames(3)
        ])
        .unwrap();
        drop(ins);

        assert_eq!(schema_version(&conn).unwrap(), 9);

        init_schema(&conn).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), crate::SCHEMA_VERSION);
        // idempotent: running the whole thing again changes nothing
        let links_before: i64 = conn
            .query_row("SELECT count(*) FROM item_tags", [], |r| r.get(0))
            .unwrap();
        init_schema(&conn).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), crate::SCHEMA_VERSION);
        assert_eq!(
            conn.query_row("SELECT count(*) FROM item_tags", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            links_before
        );

        // every old link still points at the tag it used to
        for (item, name) in [
            ("eh1", "female:big_breasts"),
            ("eh1", "f95-completed"),
            ("gb1", "big_breasts"),
            ("gb1", "complete"),
            ("gb1", "other:variant_set"),
            ("gb1", "foo:bar"),
        ] {
            let hit: i64 = conn
                .query_row(
                    "SELECT count(*) FROM item_tags it JOIN tags t ON t.id = it.tag_id
                     WHERE it.item_id = ?1 AND t.name = ?2",
                    params![item, name],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(hit, 1, "{item} lost its tag {name}");
        }

        // provenance: nothing is guessed (every backfilled row is site ''),
        // namespaced names are split, and unknown prefixes stay part of the name
        for (name, ns, fold) in [
            ("female:big_breasts", Some("female"), "big_breasts"),
            ("big_breasts", None, "big_breasts"),
            ("other:variant_set", Some("other"), "variant_set"),
            ("foo:bar", None, "foo:bar"),
            ("f95-completed", None, "f95-completed"),
        ] {
            let (got_ns, got_fold): (Option<String>, String) = conn
                .query_row(
                    "SELECT site, ns, fold FROM tags WHERE name = ?1",
                    params![name],
                    |r| Ok((r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(got_ns.as_deref(), ns, "ns of {name}");
            assert_eq!(got_fold, fold, "fold of {name}");
        }
        assert_eq!(
            conn.query_row("SELECT count(*) FROM tags WHERE site <> ''", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "backfill must not invent provenance (invariant 21)"
        );

        // f95's [Completed] prefix is a site fact, not a content tag (D9)
        let type_id: i64 = conn
            .query_row(
                "SELECT type_id FROM tags WHERE name = 'f95-completed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(type_id, TYPE_SITE_FLAG);

        // Upgrading must not read a single file: the byte length comes from
        // SQLite (no BLOB leaves the engine) and every fact that needs the
        // container is still NULL, so a `width>` or `type:gif` query matches
        // nothing yet rather than guessing (invariant 27). What a pre-v10 writer
        // left as 0 px is gone, because 0 px would match `width<400`.
        let facts = |hash: &str| -> (Option<i64>, Option<i64>, Option<i64>, Option<i64>) {
            conn.query_row(
                "SELECT bytes, width, height, animated FROM media WHERE hash = ?1",
                params![hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap()
        };
        assert_eq!(facts("h1"), (Some(gif_len), None, None, None));
        assert_eq!(facts("h2"), (Some(png_len), None, None, None));
        assert_eq!(facts("h3"), (Some(junk_len), None, None, None));
        assert_eq!(facts("h4"), (Some(gif_len), Some(360), Some(360), None));

        // Now the facts job's pass (stage S7 wires it to a job). Each container
        // answers for itself: the 3-frame gif says animated and 4x4, the still
        // PNG says 8x8 and *not* animated, and the 6-byte stub says nothing —
        // which stays NULL rather than becoming a 'still'.
        let one = refine_stored_facts(&conn, 2).unwrap();
        assert_eq!(
            one,
            FactScan {
                scanned: 2,
                dims: 2,
                animated: 1,
                still: 1,
                unknown_animation: 0,
            },
            "the first two rows by id are the gif and the png"
        );
        assert_eq!(facts("h1"), (Some(gif_len), Some(4), Some(4), Some(1)));
        assert_eq!(facts("h2"), (Some(png_len), Some(8), Some(8), Some(0)));
        let two = refine_stored_facts(&conn, 9).unwrap();
        assert_eq!(two.scanned, 1, "the stub is the only row left to read");
        assert_eq!(
            facts("h3"),
            (Some(junk_len), None, None, None),
            "an unreadable header stays unknown"
        );
        assert_eq!(
            refine_stored_facts(&conn, 9).unwrap().scanned,
            0,
            "a row already read is not read again"
        );
        // The `IFNULL` guard: a container that cannot answer must not delete a
        // fact another path recorded. `h3` keeps its size while its animation
        // stays the unknown it always was.
        conn.execute(
            "UPDATE media SET width = 999, height = 66, probed_at = NULL WHERE hash = 'h3'",
            [],
        )
        .unwrap();
        refine_stored_facts(&conn, 9).unwrap();
        assert_eq!(facts("h3"), (Some(junk_len), Some(999), Some(66), None));
        // the tile keeps its own size and never gains an animation fact: its
        // bytes are a re-encode, so they say nothing about the original
        assert_eq!(facts("h4"), (Some(gif_len), Some(360), Some(360), None));

        // the planner's indexes exist
        for idx in [
            "tags_fold_idx",
            "tags_name_idx",
            "tags_site_name_idx",
            "item_tags_tag_idx",
            "sources_site_item_idx",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name = ?1",
                    params![idx],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "index {idx} missing");
        }
    }

    #[test]
    fn tag_ns_fold_is_closed_and_byte_exact() {
        assert_eq!(
            tag_ns_fold("female:big_breasts"),
            (Some("female"), "big_breasts")
        );
        assert_eq!(tag_ns_fold("artist:horosuke"), (Some("artist"), "horosuke"));
        // a prefix we do not know is part of the tag, not a namespace
        assert_eq!(tag_ns_fold("foo:bar"), (None, "foo:bar"));
        // no namespace without something behind it
        assert_eq!(tag_ns_fold("female:"), (None, "female:"));
        // only the leading one comes off
        let (_, fold) = tag_ns_fold("female:porn:gif");
        assert_eq!(fold, "porn:gif");
        // the site's spelling survives in the name
        assert_eq!(tag_ns_fold("Female:x"), (Some("Female"), "x"));
    }

    #[test]
    fn extension_tags_come_from_what_the_file_calls_itself() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO items (id,kind,title,created_at,updated_at) VALUES ('m1','post','clip','0','1')",
            [],
        )
        .unwrap();
        // a CDN file link with the post id in the query, and a windows path
        assert_eq!(
            set_item_extension_tag(
                &conn,
                "m1",
                "https://img3.gelbooru.com/images/00/00000000000000000000000000000000.jpg?9999999"
            )
            .unwrap(),
            Some(
                conn.query_row("SELECT id FROM tags WHERE name = 'jpg'", [], |r| r.get(0))
                    .unwrap()
            )
        );
        assert!(
            set_item_extension_tag(&conn, "m1", r"file:///D:/video/clip.mp4")
                .unwrap()
                .is_some(),
            "a file path names its own file type"
        );
        // a page url names no file, so no tag is invented
        assert_eq!(
            set_item_extension_tag(&conn, "m1", "https://e-hentai.org/g/1234/abcd/").unwrap(),
            None
        );
        // and a nonsense "extension" is refused rather than stored
        assert_eq!(
            set_item_extension_tag(&conn, "m1", "https://x.to/1.averylongthing").unwrap(),
            None
        );
        let linked: i64 = conn
            .query_row(
                "SELECT count(*) FROM item_tags it JOIN tags t ON t.id = it.tag_id
                 WHERE it.item_id = 'm1' AND t.name IN ('jpg','mp4')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(linked, 2);
        let total: i64 = conn
            .query_row(
                "SELECT count(*) FROM item_tags it JOIN tags t ON t.id = it.tag_id WHERE it.item_id = 'm1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total, 2, "no invented tag should have been written");
        let kind: String = conn
            .query_row(
                "SELECT tt.key FROM tags t JOIN tag_types tt ON tt.id = t.type_id WHERE t.name = 'mp4'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kind, "extension");
    }

    #[test]
    fn list_items_binds_limit_offset_without_search() {
        // Regression: the no-search SQL used ?2/?3 while params! binds 1/2,
        // so every grid load failed and the UI silently showed "empty".
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        for (id, kind, title) in [
            ("g1", "game", "Alpha Game"),
            ("g2", "game", "Beta Game"),
            ("b1", "post", "Gamma Post"),
        ] {
            conn.execute(
                "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES (?1,?2,?3,'0','1')",
                params![id, kind, title],
            )
            .unwrap();
        }
        let all = list_items(
            &conn,
            &ListQuery {
                limit: 300,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(all.len(), 3);
        let games = list_items(
            &conn,
            &ListQuery {
                limit: 300,
                kind: Some("game"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(games.len(), 2);
        let search = list_items(
            &conn,
            &ListQuery {
                limit: 300,
                search: Some("gamma"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(search.len(), 1);
        let counts = counts_by_status(&conn, Some("game")).unwrap();
        assert_eq!(counts.iter().map(|c| c.count).sum::<i64>(), 2);
    }

    /// #40: sidebar source-state filters + their badge counts.
    #[test]
    fn source_state_filters_and_badges() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        for (id, kind, title) in [
            ("g_local", "game", "Local Only Game"),
            ("g_both", "game", "Linked Game"),
            ("g_web", "game", "Link Only Game"),
            ("m_local", "post", "Local Only Image"),
        ] {
            conn.execute(
                "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES (?1,?2,?3,'0','1')",
                params![id, kind, title],
            )
            .unwrap();
        }
        insert_source(
            &conn,
            "g_local",
            "local",
            "C:/games/local only/game.exe",
            None,
            None,
        )
        .unwrap();
        insert_source(
            &conn,
            "g_both",
            "local",
            "C:/games/linked/game.exe",
            None,
            None,
        )
        .unwrap();
        insert_source(
            &conn,
            "g_both",
            "f95",
            "https://f95zone.to/threads/x.1/",
            Some("1"),
            None,
        )
        .unwrap();
        insert_source(
            &conn,
            "g_web",
            "f95",
            "https://f95zone.to/threads/y.2/",
            Some("2"),
            None,
        )
        .unwrap();
        insert_source(&conn, "m_local", "local", "file:///D:/img.png", None, None).unwrap();
        add_item_tag(&conn, "g_both", "renpy", TYPE_CONTENT).unwrap();

        let ids = |source: &str, kind: &str| -> Vec<String> {
            let mut v: Vec<String> = list_items(
                &conn,
                &ListQuery {
                    limit: 300,
                    kind: Some(kind),
                    source: Some(source),
                    ..Default::default()
                },
            )
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();
            v.sort();
            v
        };
        assert_eq!(ids("local_only", "game"), ["g_local"]);
        assert_eq!(ids("no_local", "game"), ["g_web"]);
        assert_eq!(ids("no_tags", "game"), ["g_local", "g_web"]);
        assert_eq!(ids("local_only", "media"), ["m_local"]);

        let g = source_stats(&conn, Some("game")).unwrap();
        assert_eq!((g.local_only, g.no_local, g.no_tags), (1, 1, 2));
        let m = source_stats(&conn, Some("media")).unwrap();
        assert_eq!((m.local_only, m.no_local, m.no_tags), (1, 0, 1));
    }

    /// #31: "Keep both" has to stick. A dismissed pair leaves the library
    /// untouched but never comes back as a suggestion.
    #[test]
    fn dismissed_pairs_stop_being_suggested() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        for (id, title) in [("p1", "Same Game Title"), ("p2", "Same Game Title")] {
            conn.execute(
                "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES (?1,'game',?2,'0','1')",
                params![id, title],
            )
            .unwrap();
        }
        let sugg = pairing_suggestions(&conn, Some("game")).unwrap();
        assert_eq!(sugg.len(), 1);
        // the UI labels its rows from these — they must be the real sources
        assert_eq!((sugg[0].a_id.as_str(), sugg[0].b_id.as_str()), ("p1", "p2"));
        dismiss_pair(&conn, "p1", "p2").unwrap();
        assert!(pairing_suggestions(&conn, Some("game")).unwrap().is_empty());
        // dismissal is symmetric and keeps both rows alive
        dismiss_pair(&conn, "p2", "p1").unwrap();
        assert!(pairing_suggestions(&conn, None).unwrap().is_empty());
        assert!(get_item(&conn, "p1").unwrap().is_some());
        assert!(get_item(&conn, "p2").unwrap().is_some());
    }

    /// #31 follow-up: a dismissal row names *two* items, so there is no FK that
    /// can clean it. Deleting or merging either half must drop the row, or the
    /// vault keeps a permanently dead "never pair these" entry forever.
    #[test]
    fn dismissal_rows_follow_their_items() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        for (id, title) in [
            ("q1", "Same Game Title"),
            ("q2", "Same Game Title"),
            ("q3", "Same Game Title"),
        ] {
            conn.execute(
                "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES (?1,'game',?2,'0','1')",
                params![id, title],
            )
            .unwrap();
        }
        let rows = || -> i64 {
            conn.query_row("SELECT count(*) FROM pair_dismissals", [], |r| r.get(0))
                .unwrap()
        };
        dismiss_pair(&conn, "q1", "q2").unwrap();
        dismiss_pair(&conn, "q2", "q3").unwrap();
        assert_eq!(rows(), 2);

        delete_item(&conn, "q2").unwrap();
        assert_eq!(rows(), 0, "both rows named q2");

        // merge: the dropped half's row goes with it
        dismiss_pair(&conn, "q1", "q3").unwrap();
        assert_eq!(rows(), 1);
        merge_items(&conn, "q1", "q3").unwrap();
        assert_eq!(rows(), 0);
    }

    #[test]
    fn tags_settings_updates_and_merge() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), crate::SCHEMA_VERSION);

        for (id, kind, title) in [
            ("g1", "game", "Summer Witch"),
            ("g2", "game", "Summer Witch [renpy]"),
        ] {
            conn.execute(
                "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES (?1,?2,?3,'0','1')",
                params![id, kind, title],
            )
            .unwrap();
        }
        insert_source(
            &conn,
            "g1",
            "dlsite",
            "https://www.dlsite.com/maniax/work/=/product_id/RJ123456.html",
            Some("RJ123456"),
            None,
        )
        .unwrap();
        insert_source(
            &conn,
            "g2",
            "f95",
            "https://f95zone.to/threads/summer-witch.9999/",
            Some("9999"),
            None,
        )
        .unwrap();

        // Tag write path + tag filter + top tags.
        add_item_tag(&conn, "g1", "renpy", TYPE_CONTENT).unwrap();
        add_item_tag(&conn, "g2", "renpy", TYPE_CONTENT).unwrap();
        let tagged = list_items(
            &conn,
            &ListQuery {
                limit: 300,
                tag: Some("renpy"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(tagged.len(), 2);
        let tops = top_tags(&conn, 10, None).unwrap();
        assert_eq!(tops[0].name, "renpy");
        assert_eq!(tops[0].count, 2);
        remove_item_tag(&conn, "g2", tops[0].id).unwrap();
        assert_eq!(
            list_items(
                &conn,
                &ListQuery {
                    limit: 300,
                    tag: Some("renpy"),
                    ..Default::default()
                }
            )
            .unwrap()
            .len(),
            1
        );

        // Site filter + per-site counts (Media view filters).
        let from_dlsite = list_items(
            &conn,
            &ListQuery {
                limit: 300,
                site: Some("dlsite"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(from_dlsite.len(), 1);
        let sites = counts_by_site(&conn, Some("game")).unwrap();
        assert_eq!(sites.len(), 2);
        assert!(sites.iter().any(|s| s.site == "f95" && s.count == 1));
        assert!(counts_by_site(&conn, Some("media")).unwrap().is_empty());
        assert!(top_tags(&conn, 10, Some("media")).unwrap().is_empty());

        // Site settings upsert semantics. #28: the first touch of a site (here,
        // just enabling it) must seed that site's real rate, not a shared 700.
        set_site_setting(&conn, "ehentai", None, None, None, 10_500).unwrap();
        assert!(site_enabled(&conn, "ehentai", true));
        assert_eq!(site_rate_ms(&conn, "ehentai", 1), 10_500);
        set_site_setting(
            &conn,
            "ehentai",
            Some(false),
            Some("ipb_member_id=1"),
            None,
            700,
        )
        .unwrap();
        assert!(!site_enabled(&conn, "ehentai", true));
        assert_eq!(
            site_cookie(&conn, "ehentai").as_deref(),
            Some("ipb_member_id=1")
        );
        set_site_setting(&conn, "ehentai", Some(true), None, None, 700).unwrap();
        assert_eq!(
            site_cookie(&conn, "ehentai").as_deref(),
            Some("ipb_member_id=1")
        );
        // later calls must not clobber the rate that was seeded on insert
        assert_eq!(site_rate_ms(&conn, "ehentai", 10_500), 10_500);
        assert_eq!(site_rate_ms(&conn, "exhentai", 10_500), 10_500);
        // #38: and a cookie can actually be taken back out. Passing `None` to
        // set_site_setting means "leave it", which is why a Clear control that
        // went through it never removed anything.
        assert!(clear_site_cookie(&conn, "ehentai").unwrap());
        assert_eq!(site_cookie(&conn, "ehentai"), None);
        assert_eq!(
            get_site_settings(&conn)
                .unwrap()
                .iter()
                .find(|s| s.site == "ehentai")
                .map(|s| (s.cookie_set, s.rate_ms, s.enabled)),
            Some((false, 10_500, true)),
            "clearing the cookie leaves the rate box and the switch alone"
        );
        // a site with no settings row is reported as such, not as cleared
        assert!(!clear_site_cookie(&conn, "no-such-site").unwrap());
        assert_eq!(
            source_item_id(
                &conn,
                "dlsite",
                "https://www.dlsite.com/maniax/work/=/product_id/RJ123456.html"
            )
            .as_deref(),
            Some("g1")
        );
        assert!(source_item_id(&conn, "dlsite", "https://www.dlsite.com/nope.html").is_none());
        assert_eq!(update_targets(&conn).unwrap().len(), 2);

        // Update feed.
        set_item_version(&conn, "g1", "0.13", "2").unwrap();
        record_update(&conn, "g1", "f95", Some("0.12"), Some("0.13"), "2").unwrap();
        let ups = list_updates(&conn, 10).unwrap();
        assert_eq!(ups.len(), 1);
        assert_eq!(ups[0].title, "Summer Witch");

        // Detail read includes sources + tags.
        let d = get_item(&conn, "g1").unwrap().unwrap();
        assert_eq!(d.sources.len(), 1);
        assert_eq!(d.tags.len(), 1);

        // Pairing finds the near-identical titles; merge folds g2 into g1.
        let sugg = pairing_suggestions(&conn, Some("game")).unwrap();
        assert_eq!(sugg.len(), 1);
        assert!(pairing_suggestions(&conn, Some("media"))
            .unwrap()
            .is_empty());
        merge_items(&conn, "g1", "g2").unwrap();
        assert!(get_item(&conn, "g2").unwrap().is_none());
        let d = get_item(&conn, "g1").unwrap().unwrap();
        assert_eq!(d.sources.len(), 2);
        assert_eq!(d.tags.len(), 1);
    }

    /// Field report #47: repack placeholders (`mmnz-game` comes from
    /// `www/data/System.json`'s `gameTitle`) name the *tool*, not the game, so
    /// they must never become dupe suggestions.
    #[test]
    fn placeholder_titles_are_never_suggested() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        for n in 1..=9 {
            let id = format!("p{n}");
            conn.execute(
                "INSERT INTO items (id, kind, title, created_at, updated_at)
                 VALUES (?1,'game','mmnz-game','0','1')",
                rusqlite::params![id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sources (item_id, site, url) VALUES (?1,'local',?2)",
                rusqlite::params![id, format!("file:///D:/games/p{n}")],
            )
            .unwrap();
        }
        for (n, t) in ["game", "Demo", "new folder", "Sample Game"]
            .iter()
            .enumerate()
        {
            conn.execute(
                "INSERT INTO items (id, kind, title, created_at, updated_at)
                 VALUES (?1,'game',?2,'0','1')",
                rusqlite::params![format!("x{n}"), t],
            )
            .unwrap();
        }
        let sugg = pairing_suggestions(&conn, Some("game")).unwrap();
        assert!(
            sugg.is_empty(),
            "placeholders paired {} rows; the tool's name is not an identity",
            sugg.len()
        );
    }

    /// Ten games sharing a real title must not bury the pair pairing exists for
    /// (the DLsite/f95/local triple), so cross-site pairs come first and a bucket
    /// is capped rather than emitting N·(N-1)/2 rows (#47).
    #[test]
    fn cross_site_pairs_win_and_a_bucket_is_capped() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        for n in 1..=10 {
            let id = format!("l{n}");
            conn.execute(
                "INSERT INTO items (id, kind, title, created_at, updated_at)
                 VALUES (?1,'game','Garden Party','0','1')",
                rusqlite::params![id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sources (item_id, site, url) VALUES (?1,'local',?2)",
                rusqlite::params![id, format!("file:///D:/games/gp{n}")],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO items (id, kind, title, created_at, updated_at)
             VALUES ('f1','game','Garden Party [v1.02]','0','1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sources (item_id, site, url) VALUES ('f1','f95','https://f95zone.to/threads/gp.1/')",
            [],
        )
        .unwrap();

        let sugg = pairing_suggestions(&conn, Some("game")).unwrap();
        let crosses = sugg
            .iter()
            .filter(|p| p.a_sites == "f95" || p.b_sites == "f95")
            .count();
        assert_eq!(crosses, 10, "every local copy vs the f95 thread");
        assert!(
            sugg.len() <= 12,
            "bucket is capped, got {} of 55 possible pairs",
            sugg.len()
        );
    }

    /// Field report #1: direct-file bookmarks stored site "i"/"wimg" — opening
    /// an old vault renames them to their real site, once.
    #[test]
    fn v3_migration_renames_mislabeled_cdn_sites() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES ('m1','post','art','0','1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sources (item_id, site, url) VALUES ('m1','i','https://i.4cdn.org/h/1743761874698456.jpg')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sources (item_id, site, url) VALUES ('m1','wimg','https://wimg.rule34.xxx//images/3868/x.jpeg?11353035')",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE meta SET value = '2' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();

        init_schema(&conn).unwrap(); // simulate re-opening an old vault
        assert_eq!(schema_version(&conn).unwrap(), crate::SCHEMA_VERSION);
        let d = get_item(&conn, "m1").unwrap().unwrap();
        let sites: Vec<String> = d.sources.iter().map(|s| s.site.clone()).collect();
        assert!(sites.contains(&"4chan".to_string()), "sites = {sites:?}");
        assert!(sites.contains(&"rule34".to_string()), "sites = {sites:?}");
    }

    /// Field report #10: the same image imported twice must surface as a dupe
    /// in the *media* view, matched by content hash.
    #[test]
    fn media_pairing_by_shared_content_hash() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        for (id, title) in [("m1", "1743761874698456.jpg"), ("m2", "untitled jpeg")] {
            conn.execute(
                "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES (?1,'post',?2,'0','1')",
                params![id, title],
            )
            .unwrap();
        }
        for id in ["m1", "m2"] {
            conn.execute(
                "INSERT INTO media (item_id, kind, hash, mime, data) VALUES (?1,'full','deadbeef','image/jpeg',X'00')",
                params![id],
            )
            .unwrap();
        }

        let sugg = pairing_suggestions(&conn, Some("media")).unwrap();
        assert_eq!(sugg.len(), 1);
        // the games panel must not offer media pairs
        assert!(pairing_suggestions(&conn, Some("game")).unwrap().is_empty());
    }

    /// #24: the vault meta KV round-trips and stores "empty = absent".
    #[test]
    fn meta_kv_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        assert_eq!(get_meta(&conn, "proxy_url"), None);
        set_meta(&conn, "proxy_url", Some("socks5://127.0.0.1:9150")).unwrap();
        assert_eq!(
            get_meta(&conn, "proxy_url").as_deref(),
            Some("socks5://127.0.0.1:9150")
        );
        set_meta(&conn, "proxy_url", None).unwrap();
        assert_eq!(get_meta(&conn, "proxy_url"), None);
    }

    /// #25: mod recipes are per-item CRUD; deleting the game takes them with it.
    #[test]
    fn mod_crud_and_cascade() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES ('g1','game','t','0','0')",
            [],
        )
        .unwrap();

        let m1 = add_mod(
            &conn,
            "g1",
            "MTool",
            "mtool",
            r"D:\games\Karryn\MTool.exe",
            "",
            "t1",
        )
        .unwrap();
        let m2 = add_mod(
            &conn,
            "g1",
            "Cheat Engine",
            "cheat",
            r"C:\CE\cheatengine.exe",
            "",
            "t2",
        )
        .unwrap();
        assert_eq!(list_mods(&conn, "g1").unwrap().len(), 2);
        assert!(has_mods(&conn, "g1").unwrap());
        assert_eq!(get_mod(&conn, "g1", m1).unwrap().unwrap().kind, "mtool");
        // the row is scoped to its item: another game can't touch it
        assert!(get_mod(&conn, "other", m1).unwrap().is_none());
        assert!(!delete_mod(&conn, "other", m1).unwrap());

        assert!(delete_mod(&conn, "g1", m1).unwrap());
        assert_eq!(list_mods(&conn, "g1").unwrap().len(), 1);
        assert_eq!(list_mods(&conn, "g1").unwrap()[0].id, m2);

        // #25 increment 2: deploy state round-trips
        assert!(set_mod_status(&conn, "g1", m2, "applied", Some(r#"[{"file":"a.dll"}]"#)).unwrap());
        let m = get_mod(&conn, "g1", m2).unwrap().unwrap();
        assert_eq!(m.status, "applied");
        assert!(m.manifest.unwrap().contains("a.dll"));
        assert!(set_mod_status(&conn, "g1", m2, "linked", None).unwrap());
        assert_eq!(get_mod(&conn, "g1", m2).unwrap().unwrap().status, "linked");

        delete_item(&conn, "g1").unwrap();
        assert!(list_mods(&conn, "g1").unwrap().is_empty());
        assert!(!has_mods(&conn, "g1").unwrap());
    }

    /// #14: suggestion rows survive re-inserts, delete individually, and
    /// disappear with the item (ON DELETE CASCADE).
    #[test]
    fn match_suggestions_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES ('g1','game','Summer Witch','0','1')",
            [],
        )
        .unwrap();
        put_suggestion(
            &conn,
            "g1",
            "dlsite",
            "https://dlsite/RJ1.html",
            "Summer Witch Art",
            0.92,
            "7",
        )
        .unwrap();
        put_suggestion(
            &conn,
            "g1",
            "dlsite",
            "https://dlsite/RJ1.html",
            "Summer Witch Art",
            0.93,
            "8",
        )
        .unwrap(); // replace
        put_suggestion(
            &conn,
            "g1",
            "f95",
            "https://f95zone.to/threads/sw.1/",
            "Summer Witch [renpy]",
            0.71,
            "7",
        )
        .unwrap();
        let all = list_suggestions(&conn, 10).unwrap();
        assert_eq!(all.len(), 2); // same url replaced, not doubled
        assert_eq!(all[0].confidence, 0.93);
        assert_eq!(all[0].item_title, "Summer Witch");
        delete_suggestion(&conn, "g1", "f95", "https://f95zone.to/threads/sw.1/").unwrap();
        assert_eq!(suggestions_for_item(&conn, "g1").unwrap().len(), 1);
        clear_suggestions(&conn, "g1").unwrap();
        assert!(suggestions_for_item(&conn, "g1").unwrap().is_empty());

        conn.execute("DELETE FROM items WHERE id = 'g1'", [])
            .unwrap();
        assert!(list_suggestions(&conn, 10).unwrap().is_empty());
    }

    /// #14 targeting: only games tracked solely by a local source are scanned.
    #[test]
    fn games_needing_match_targets_local_only_games() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES
             ('g1','game','Local Only','0','3'),
             ('g2','game','Has DLsite','0','2'),
             ('p1','post','Not A Game','0','1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sources (item_id, site, url) VALUES ('g1','local','file:///D:/games/g1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sources (item_id, site, url) VALUES ('g2','local','file:///D:/games/g2')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO sources (item_id, site, url) VALUES ('g2','dlsite','https://www.dlsite.com/maniax/work/=/product_id/RJ1.html')", []).unwrap();
        conn.execute(
            "INSERT INTO sources (item_id, site, url) VALUES ('p1','local','file:///D:/img.jpg')",
            [],
        )
        .unwrap();

        let ids: Vec<String> = games_needing_match(&conn)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, vec!["g1".to_string()]);
    }

    /// #53: opening a vault written before this table gives it one, unique index
    /// and all, so a re-fetch of an old item still cannot double-list an image.
    #[test]
    fn v9_migration_adds_the_candidate_table() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES ('p1','post','art','0','1')",
            [],
        )
        .unwrap();
        // rewind it: a v8 vault has no candidate table at all
        conn.execute("DROP TABLE source_images", []).unwrap();
        conn.execute(
            "UPDATE meta SET value = '8' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();

        init_schema(&conn).unwrap(); // simulate re-opening an old vault
        assert_eq!(schema_version(&conn).unwrap(), crate::SCHEMA_VERSION);
        let urls = vec!["https://booru/x/1.jpg".to_string()];
        assert_eq!(
            remember_source_images(&conn, "p1", &urls, 7),
            1,
            "the table came back from the upgrade"
        );
        assert_eq!(
            remember_source_images(&conn, "p1", &urls, 40),
            0,
            "and its unique index came with it"
        );
    }

    /// #53: a page's attachment list is remembered once per url. Re-fetching a
    /// thread lists the same images again, and that must not grow the vault.
    #[test]
    fn source_image_candidates_are_remembered_once_per_url() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        for id in ["p1", "p2"] {
            conn.execute(
                "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES (?1,'post','art','0','1')",
                [id],
            )
            .unwrap();
        }
        let urls: Vec<String> = [
            "https://booru/x/1.jpg",
            "https://booru/x/2.jpg",
            "https://booru/x/1.jpg", // the page double-lists one of them
        ]
        .iter()
        .map(|u| u.to_string())
        .collect();
        assert_eq!(remember_source_images(&conn, "p1", &urls, 7), 2);
        assert_eq!(
            remember_source_images(&conn, "p1", &urls, 40),
            0,
            "a re-fetch of the same page adds nothing"
        );

        let rows = source_images_for(&conn, "p1");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].url, "https://booru/x/1.jpg");
        assert_eq!(rows[1].url, "https://booru/x/2.jpg");
        assert_eq!(rows[0].found_at, 7, "the first sighting is the one kept");

        // the same url on another item is that item's own candidate (#53's
        // unique key is per item, so reposts stay countable)
        assert_eq!(remember_source_images(&conn, "p2", &urls[..1], 8), 1);
        assert_eq!(source_images_for(&conn, "p2").len(), 1);
        assert_eq!(source_images_for(&conn, "p1").len(), 2);
    }

    /// #53: candidates keep their mime and stamps, turn "stored" once the bytes
    /// land, and disappear with the item (ON DELETE CASCADE).
    #[test]
    fn source_image_rows_roundtrip_and_follow_their_item() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES ('p1','post','art','0','1')",
            [],
        )
        .unwrap();
        let urls = vec!["https://booru/x/1.jpg".to_string()];
        let already = "https://booru/x/2.jpg";
        assert_eq!(remember_source_images(&conn, "p1", &urls, 1), 1);
        // only a fetch can fill mime/stored_at, so that row is written by hand:
        // the columns still have to round-trip in the order they were inserted
        conn.execute(
            "INSERT INTO source_images (item_id, url, mime, found_at, stored_at) VALUES ('p1',?1,'image/avif',2,3)",
            params![already],
        )
        .unwrap();

        let rows = source_images_for(&conn, "p1");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].url, urls[0], "insertion order, not url order");
        assert!(rows[0].mime.is_none(), "a plain candidate has no mime yet");
        assert_eq!(rows[0].found_at, 1);
        assert_eq!(rows[0].stored_at, None);
        assert_eq!(rows[1].mime.as_deref(), Some("image/avif"));
        assert_eq!(rows[1].found_at, 2);
        assert_eq!(rows[1].stored_at, Some(3));

        assert!(
            !mark_source_image_stored(&conn, "p1", "https://booru/x/never-listed.jpg", 9),
            "an url the page never listed is not this item's candidate"
        );
        assert!(mark_source_image_stored(&conn, "p1", &urls[0], 9));
        assert_eq!(source_images_for(&conn, "p1")[0].stored_at, Some(9));
        // scoped to the item: another post cannot mark this row
        conn.execute(
            "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES ('p2','post','art','0','1')",
            [],
        )
        .unwrap();
        assert!(!mark_source_image_stored(&conn, "p2", &urls[0], 9));

        delete_item(&conn, "p1").unwrap();
        assert!(
            source_images_for(&conn, "p1").is_empty(),
            "deleting the item takes its candidates with it"
        );
    }
    // ---------- #76 stage S3: the planned predicate, run -----------------------

    use crate::query::{self, Plan, Scope};

    /// A synthetic vault holding each storage shape and both states of a fact,
    /// written with this module's own helpers where they reach, so a fixture bug
    /// cannot hide a planner bug. Every id and title here is invented.
    ///
    /// The stamps are ten digits wide on purpose: `updated_at` is TEXT and the
    /// grid orders by it without a `CAST`, which is only the same thing as a
    /// numeric order while every stamp has the same width. That is the contract
    /// `now_epoch_text()` keeps, and `items_updated_idx` is the index the page
    /// walks — a `CAST` there would trade that index for a sort of the whole table,
    /// which is exactly the cost `filter_scale_probe` prints for `added:`.
    ///
    /// * `q_big` / `q_small`: measured, tagged, from ehentai, stamped the same
    ///   second — which is what an importer does to a batch, and why the grid order
    ///   needs a tie-break;
    /// * `q_unprobed`: a stored row nobody ever measured, every fact column NULL.
    ///   It must answer neither `size>1gb` nor `-size>1gb` (invariant 27);
    /// * `q_video`: a vaulted video with a duration;
    /// * `q_disk`: a folder import, local source only, no vault row;
    /// * `q_game`: the one game, so a scope mistake cannot hide inside "media".
    fn query_fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO items (id, kind, title, status, created_at, updated_at) VALUES
                 ('q_big','gallery','Fixture big','seen','1760000000','1760000500'),
                 ('q_small','gallery','Fixture small','seen','1760000000','1760000500'),
                 ('q_unprobed','gallery','Fixture unprobed','seen','1760000000','1760000400'),
                 ('q_video','post','Fixture clip','seen','1760000000','1760000300'),
                 ('q_disk','gallery','Fixture folder import','seen','1760000000','1760000200'),
                 ('q_game','game','Fixture game','wishlist','1760000000','1760000600');",
        )
        .unwrap();
        for (id, site, url) in [
            ("q_big", "ehentai", "https://eh.example/fixture/1"),
            ("q_small", "ehentai", "https://eh.example/fixture/2"),
            ("q_unprobed", "gelbooru", "https://gb.example/fixture/3"),
            ("q_video", "gelbooru", "https://gb.example/fixture/4"),
            ("q_disk", "local", "file:///D:/fixture/q_disk.png"),
            ("q_game", "f95", "https://f95.example/fixture/5"),
        ] {
            insert_source(&conn, id, site, url, None, None).unwrap();
        }
        {
            // A prepared statement borrows the connection, so it is scoped.
            let mut media = conn
                .prepare(
                    "INSERT INTO media (item_id, kind, hash, mime, width, height, data,
                                        bytes, duration_ms, animated, probed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, x'89504e470d0a1a0a', ?7, ?8, ?9, ?10)",
                )
                .unwrap();
            // measured big, moving
            media
                .execute(params![
                    "q_big",
                    "full",
                    "fixture-hash-big",
                    "image/gif",
                    400,
                    300,
                    5_000_000_000_i64,
                    None::<i64>,
                    1,
                    "1760000000"
                ])
                .unwrap();
            // measured small, still
            media
                .execute(params![
                    "q_small",
                    "full",
                    "fixture-hash-small",
                    "image/png",
                    1_200,
                    400,
                    1_024_i64,
                    None::<i64>,
                    0,
                    "1760000000"
                ])
                .unwrap();
            // never asked: the row that must stay invisible to a fact predicate
            media
                .execute(params![
                    "q_unprobed",
                    "full",
                    "fixture-hash-unprobed",
                    "image/png",
                    None::<i64>,
                    None::<i64>,
                    None::<i64>,
                    None::<i64>,
                    None::<i64>,
                    None::<i64>
                ])
                .unwrap();
            // a vaulted video: duration yes, `animated` NULL, because a container
            // says nothing about frames and inventing 0 would make `-type:gif` a lie
            media
                .execute(params![
                    "q_video",
                    "video",
                    "fixture-hash-clip",
                    "video/mp4",
                    720,
                    480,
                    900_000_i64,
                    240_000_i64,
                    None::<i64>,
                    "1760000000"
                ])
                .unwrap();
        }
        add_item_tag(&conn, "q_big", "female:big_breasts", TYPE_CONTENT).unwrap();
        add_item_tag(&conn, "q_small", "big_breasts", TYPE_CONTENT).unwrap();
        conn
    }

    /// The ids one query selects through the S3 entry point, in grid order.
    fn matched(conn: &Connection, text: &str, scope: Scope) -> Vec<String> {
        let plan = query::parse(text, scope)
            .unwrap_or_else(|e| panic!("{text:?}: {}", e.message))
            .plan();
        search_item_ids(conn, scope, &plan).unwrap()
    }

    #[test]
    fn the_page_and_the_paired_total_come_from_one_predicate() {
        let conn = query_fixture();
        let plan = query::parse("big_breasts", Scope::Media).unwrap().plan();
        let page = search_items(&conn, Scope::Media, &plan, 300, None).unwrap();
        assert_eq!(page.items.len(), 2);
        // The fold match reaches both spellings: `big_breasts` and
        // `female:big_breasts` are one tag family for a bare word.
        let ids: Vec<&str> = page.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["q_big", "q_small"]);
        assert_eq!(page.total, 2, "the count is the result, not the page");
        assert!(!page.items.iter().any(|i| i.kind == "game"), "D14");
    }

    #[test]
    fn a_never_measured_item_matches_neither_a_fact_nor_its_negation() {
        let conn = query_fixture();
        // The row with NULL facts is invisible to both halves. If `-size>1gb`
        // selected it, the filter would be claiming to know the file is small.
        assert_eq!(matched(&conn, "size>1gb", Scope::Media), ["q_big"]);
        assert_eq!(
            matched(&conn, "-size>1gb", Scope::Media),
            ["q_small", "q_video"],
            "the contrary measurement, so only the two rows known to be smaller"
        );
        assert_eq!(matched(&conn, "width>1000", Scope::Media), ["q_small"]);
        assert_eq!(
            matched(&conn, "-width>1000", Scope::Media),
            ["q_big", "q_video"],
            "q_video is 720 px wide and measured; q_unprobed was never asked"
        );
        assert_eq!(matched(&conn, "length>2m", Scope::Media), ["q_video"]);
        assert_eq!(matched(&conn, "type:gif", Scope::Media), ["q_big"]);
        assert_eq!(
            matched(&conn, "-type:gif", Scope::Media),
            ["q_small"],
            "a video row keeps `animated` NULL on purpose, so it answers neither"
        );
    }

    #[test]
    fn the_total_counts_the_result_and_the_page_limits_the_rows() {
        let conn = query_fixture();
        let page = search_items(&conn, Scope::Media, &Plan::default(), 2, None).unwrap();
        assert_eq!(page.items.len(), 2, "the page is a page");
        assert_eq!(page.total, 5, "the total is the whole media library");
        assert_eq!(
            count_matches(&conn, Scope::Games, &Plan::default()).unwrap(),
            1,
            "and the games total is the games library"
        );
    }

    #[test]
    fn select_all_ids_are_the_whole_result_in_grid_order() {
        let conn = query_fixture();
        // Grid order, with the id breaking a stamp tie - and the tie is real here,
        // because both items were stamped the same second, which is what an
        // importer writes. Without the tie-break these two rows could swap between
        // page 1 and page 2 and a paged read would repeat one and drop the other.
        assert_eq!(
            matched(&conn, "", Scope::Media),
            ["q_big", "q_small", "q_unprobed", "q_video", "q_disk"]
        );
        let page = search_items(&conn, Scope::Media, &Plan::default(), 2, None).unwrap();
        assert_eq!(
            page.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["q_big", "q_small"],
            "the page is a prefix of the select-all list, not a different set"
        );
        // Five pages of one row each, taken by cursor, and no row appears twice or
        // not at all. Page 2 is the interesting one: its cursor is `q_big`, which
        // shares its stamp with `q_small`, so only the id arm of the predicate can
        // find it.
        let mut paged: Vec<String> = Vec::new();
        let mut cursor: Option<PageCursor> = None;
        for _ in 0..5 {
            let page =
                search_items(&conn, Scope::Media, &Plan::default(), 1, cursor.as_ref()).unwrap();
            let last = page.items.last().expect("a page of the fixture has a row");
            cursor = Some(PageCursor {
                updated_at: last.updated_at.clone(),
                id: last.id.clone(),
            });
            paged.extend(page.items.into_iter().map(|i| i.id));
        }
        assert_eq!(
            paged,
            matched(&conn, "", Scope::Media),
            "paging is lossless: every id exactly once, in grid order"
        );
        let past = search_items(&conn, Scope::Media, &Plan::default(), 5, cursor.as_ref()).unwrap();
        assert!(
            past.items.is_empty(),
            "and the walk ends: the cursor past the last row matches nothing"
        );
        assert_eq!(
            past.total, 5,
            "while the total still reports the whole result"
        );
    }

    /// The reason the grid has no offset. Read one page, delete the rows on it the
    /// way #73's *Delete selected* does, then continue the walk both ways. The
    /// cursor shows the surviving rows in order; an offset counts the survivors from
    /// the top of the list and so walks past rows nobody ever saw.
    #[test]
    fn a_bulk_delete_cannot_make_a_paged_grid_skip_rows() {
        let conn = query_fixture();
        let first = search_items(&conn, Scope::Media, &Plan::default(), 2, None).unwrap();
        let shown: Vec<String> = first.items.iter().map(|i| i.id.clone()).collect();
        assert_eq!(shown, ["q_big", "q_small"]);
        let cursor = PageCursor {
            updated_at: first.items[1].updated_at.clone(),
            id: first.items[1].id.clone(),
        };
        conn.execute("DELETE FROM items WHERE id IN ('q_big','q_small')", [])
            .unwrap();

        // What production does: continue from the last row that was on screen.
        let next = search_items(&conn, Scope::Media, &Plan::default(), 2, Some(&cursor)).unwrap();
        assert_eq!(
            next.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["q_unprobed", "q_video"],
            "the next two survivors, in order, with nothing passed over"
        );
        assert_eq!(next.total, 3, "and the total is still the whole result");

        // What an offset would have done, kept here as the counter-example so the
        // hazard stays measurable. The same predicate, the same page size, the same
        // position in the walk - and two surviving rows are never shown.
        let sql = format!(
            "SELECT items.id FROM items{}{GRID_ORDER} LIMIT ? OFFSET ?",
            planned_where(Scope::Media, &Plan::default())
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        let skipped: Vec<String> = stmt
            .query_map(params![2_i64, 2_i64], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            skipped,
            ["q_disk"],
            "the offset read lands past q_unprobed and q_video: two survivors the \
             user would never have reached, which is exactly what deleting 20 000 rows \
             does at 100k (filter_scale_probe prints the same effect at scale)"
        );
    }

    #[test]
    fn the_view_scope_is_what_keeps_kinds_apart_not_the_query() {
        let conn = query_fixture();
        // `status:` parses only in the games grammar, and even a games plan run
        // through the media scope cannot reach a game row: the scope is AND-ed in
        // first, always. That is D14 enforced in SQL, not left to the UI.
        let games = query::parse("status:wishlist", Scope::Games)
            .unwrap()
            .plan();
        assert_eq!(matched(&conn, "status:wishlist", Scope::Games), ["q_game"]);
        assert!(
            search_item_ids(&conn, Scope::Media, &games)
                .unwrap()
                .is_empty(),
            "a games predicate cannot open the media view"
        );
        assert!(
            query::parse("status:wishlist", Scope::Media).is_err(),
            "and the media grammar never accepts the word in the first place"
        );
    }

    #[test]
    fn the_tag_cloud_and_the_site_counts_follow_the_query() {
        let conn = query_fixture();
        let all = top_tags_matching(&conn, Scope::Media, &Plan::default(), 10).unwrap();
        // One row per **folded** tag, because that is what a bare tag term
        // matches: `q_big`'s `female:big_breasts` and `q_small`'s `big_breasts`
        // are one family, so the rail says 2 and the grid really shows 2. The
        // site's longer spelling is not a second chip promising 1 and delivering 2.
        assert_eq!(
            all.iter()
                .map(|t| (t.name.as_str(), t.count))
                .collect::<Vec<_>>(),
            [("big_breasts", 2)]
        );
        // Narrow the result and the rail narrows with it: a tag that only exists
        // on excluded rows is not a suggestion, it is a dead end.
        let gelbooru = query::parse("source:gelbooru", Scope::Media)
            .unwrap()
            .plan();
        assert!(
            top_tags_matching(&conn, Scope::Media, &gelbooru, 10)
                .unwrap()
                .is_empty(),
            "neither tagged item came from gelbooru"
        );
        let sites = counts_by_site_matching(&conn, Scope::Media, &Plan::default()).unwrap();
        assert_eq!(
            sites
                .iter()
                .map(|s| (s.site.as_str(), s.count))
                .collect::<Vec<_>>(),
            [("ehentai", 2), ("gelbooru", 2), ("local", 1)]
        );
        let folder = query::parse("stored:local-only", Scope::Media)
            .unwrap()
            .plan();
        assert_eq!(
            counts_by_site_matching(&conn, Scope::Media, &folder)
                .unwrap()
                .iter()
                .map(|s| (s.site.as_str(), s.count))
                .collect::<Vec<_>>(),
            [("local", 1)]
        );
    }

    #[test]
    fn a_query_reading_unmeasured_facts_says_so_on_the_page() {
        let conn = query_fixture();
        let facts = query::parse("size>1gb", Scope::Media).unwrap().plan();
        assert!(
            search_items(&conn, Scope::Media, &facts, 300, None)
                .unwrap()
                .needs_facts,
            "a zero from this predicate is not yet an answer"
        );
        let plain = query::parse("big_breasts", Scope::Media).unwrap().plan();
        assert!(
            !search_items(&conn, Scope::Media, &plain, 300, None)
                .unwrap()
                .needs_facts,
            "and a tag query does not get to claim the vault is unmeasured"
        );
    }

    #[test]
    fn the_scope_is_anded_before_the_plan_never_replaced_by_it() {
        assert_eq!(
            planned_where(Scope::Games, &Plan::default()),
            " WHERE items.kind = 'game'"
        );
        let plan = query::parse("big_breasts", Scope::Media).unwrap().plan();
        let where_sql = planned_where(Scope::Media, &plan);
        assert!(
            where_sql.starts_with(" WHERE items.kind IN ('gallery','post') AND ("),
            "{where_sql}"
        );
        assert!(where_sql.ends_with(')'), "{where_sql}");
    }

    // ---------- #76 stage S4: the candidate list, counted against the query ----

    /// The candidates for a half-typed box, as (token, kind, count).
    fn suggestions(
        conn: &Connection,
        text: &str,
        scope: Scope,
    ) -> Vec<(String, &'static str, Option<i64>)> {
        let ctx = query::parse_suggestion(text, scope)
            .unwrap_or_else(|| panic!("{text:?} must have a token to complete"));
        suggest(conn, scope, &ctx, 20)
            .unwrap()
            .into_iter()
            .map(|s| (s.token, s.kind, s.count))
            .collect()
    }

    #[test]
    fn a_half_typed_word_offers_the_grammar_first_and_the_vault_second() {
        let conn = query_fixture();
        let rows = suggestions(&conn, "stor", Scope::Media);
        assert_eq!(
            rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            ["stored:"],
            "the keyword comes first, and it is offered with its colon"
        );
        assert_eq!(rows[0].1, "keyword");
        assert_eq!(
            rows[0].2, None,
            "a word of the grammar is not a fact about this vault, so it gets no number"
        );
        // The tags of this vault come through with a count. One row per folded
        // name: e-hentai's `female:big_breasts` and a gelbooru-style
        // `big_breasts` are one tag family to a bare word, which is exactly what
        // `tag:"…"` exists to override.
        let rows = suggestions(&conn, "big", Scope::Media);
        assert_eq!(
            rows.iter().map(|r| (r.0.as_str(), r.2)).collect::<Vec<_>>(),
            [("big_breasts", Some(2))]
        );
    }

    #[test]
    fn the_suggestion_for_a_closed_keyword_is_grammar_and_never_a_made_up_number() {
        let conn = query_fixture();
        assert_eq!(
            suggestions(&conn, "type:", Scope::Media),
            [
                ("type:video".to_string(), "value", None),
                ("type:image".to_string(), "value", None),
                ("type:gif".to_string(), "value", None),
            ],
            "exactly the grammar's list, in its order"
        );
        // `size:` has no enumerable values. Bare, the keyword itself is the right
        // suggestion; once the operator is typed, nothing may follow it. Offering
        // `size:1` would be inventing a fact, the same lie invariant 27 refuses.
        assert_eq!(
            suggestions(&conn, "size", Scope::Media),
            [("size:".to_string(), "keyword", None)]
        );
        assert!(
            suggestions(&conn, "size>10", Scope::Media).is_empty(),
            "a half-typed number stays a half-typed number"
        );
        let games = suggestions(&conn, "status:", Scope::Games);
        assert!(games.len() >= 5, "the Games pipeline's own states");
        assert!(games.iter().all(|r| r.0.starts_with("status:")));
    }

    #[test]
    fn a_site_completion_counts_through_the_same_sql_the_query_would_run() {
        let conn = query_fixture();
        let rows = suggestions(&conn, "source:e", Scope::Media);
        assert_eq!(rows, [("source:ehentai".to_string(), "site", Some(2))]);
        // Narrow the box and the candidate narrows with it: after the folder
        // predicate, ehentai is not an answer any more, and offering it anyway is
        // the dead-end chip the old rails were known for.
        assert!(
            suggestions(&conn, "stored:local-only source:e", Scope::Media).is_empty(),
            "no ehentai row is local-only"
        );
    }

    #[test]
    fn the_minus_being_typed_survives_into_the_completion() {
        let conn = query_fixture();
        assert_eq!(
            suggestions(&conn, "-has:t", Scope::Media)
                .into_iter()
                .map(|r| r.0)
                .collect::<Vec<_>>(),
            ["-has:thumb", "-has:tags"],
            "both values that start with `t`, and the minus stays on all of them"
        );
    }

    #[test]
    fn an_extension_completion_reads_the_tag_the_grammar_reads() {
        let conn = query_fixture();
        set_item_extension_tag(&conn, "q_big", "https://eh.example/fixture/1.gif").unwrap();
        assert_eq!(
            suggestions(&conn, "ext:g", Scope::Media),
            [("ext:gif".to_string(), "tag", Some(1))]
        );
        // The content tags are not extension values, and they must not leak in:
        // `ext:female:big_breasts` is a predicate that can never be true.
        assert!(
            suggestions(&conn, "ext:big", Scope::Media).is_empty(),
            "big_breasts is a content tag, not a file name"
        );
    }
}
