//! Schema v1 + tiny repository helpers. Thumbnails are encrypted BLOBs in the
//! DB (whole-vault nuke stays trivial); content-hash keyed for dedup.

use crate::model::{Item, ItemKind, Status};
use rusqlite::{params, Connection};
use serde::Serialize;
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

        INSERT OR IGNORE INTO meta (key, value) VALUES ('schema_version', '2');
        UPDATE meta SET value = '2' WHERE key = 'schema_version';

        INSERT OR IGNORE INTO tag_types (id, key, label, priority, color) VALUES
            (1, 'user_status', 'Personal', 100, '#4ade80'),
            (2, 'site_flag',   'Site',      50, NULL),
            (3, 'content',     'Content',   10, NULL);

        INSERT OR IGNORE INTO tags (id, name, type_id) VALUES
            (1, 'complete', 1),
            (2, 'dnf',      1),
            (3, 'replay',   1);

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

        COMMIT;"
        ,
    )?;
    Ok(())
}

pub fn schema_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
        [],
        |r| r.get(0),
    )
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct StatusCount {
    pub status: &'static str,
    pub count: i64,
}

pub fn counts_by_status(conn: &Connection, kind: Option<&str>) -> rusqlite::Result<Vec<StatusCount>> {
    let mut stmt = conn.prepare(
        "SELECT status, count(*) FROM items
         WHERE (?1 IS NULL
                OR (?1 = 'game' AND kind = 'game')
                OR (?1 = 'media' AND kind IN ('gallery','post')))
         GROUP BY status",
    )?;
    let rows = stmt.query_map(params![kind], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
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
    let rows = stmt.query_map(params![kind], |r| Ok(SiteCount { site: r.get(0)?, count: r.get(1)? }))?;
    rows.collect()
}

pub fn insert_item(
    conn: &Connection,
    item: &Item,
) -> rusqlite::Result<()> {
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

pub fn list_items(
    conn: &Connection,
    limit: i64,
    offset: i64,
    search: Option<&str>,
    kind: Option<&str>,
    tag: Option<&str>,
    site: Option<&str>,
) -> rusqlite::Result<Vec<ItemSummary>> {
    let like = search.map(|s| format!("%{}%", s.trim()));
    // Placeholders are always ?1..?6 — binding with params! fills positions
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
             ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3";
    let mut stmt = conn.prepare(sql)?;
    let mut out = Vec::new();
    let mapped = stmt.query_map(
        params![like, limit, offset, kind, tag, site],
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
        kind: ItemKind::from_str(&kind).map(|k| k.as_str()).unwrap_or("game"),
        title: r.get(2)?,
        status: r.get(3)?,
        version: r.get(4)?,
        has_thumb: r.get(5)?,
        updated_at: r.get(6)?,
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
        params![id, title, circle, status.as_str(), version, notes, updated_at],
    )?;
    Ok(())
}

/// Delete item + cascading sources/tags/media (FK cascade is on).
pub fn delete_item(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM items WHERE id = ?1", params![id])?;
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

// ---------- tags ----------

pub const TYPE_USER_STATUS: i64 = 1;
pub const TYPE_SITE_FLAG: i64 = 2;
pub const TYPE_CONTENT: i64 = 3;

pub fn tag_get_or_create(conn: &Connection, name: &str, type_id: i64) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT OR IGNORE INTO tags (name, type_id) VALUES (?1, ?2)",
        params![name, type_id],
    )?;
    conn.query_row("SELECT id FROM tags WHERE name = ?1", params![name], |r| r.get(0))
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
        Ok(TagOnItem { id: r.get(0)?, name: r.get(1)?, type_key: r.get(2)? })
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
pub fn top_tags(conn: &Connection, limit: i64, kind: Option<&str>) -> rusqlite::Result<Vec<TagCount>> {
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
        Ok(TagCount { id: r.get(0)?, name: r.get(1)?, type_key: r.get(2)?, count: r.get(3)? })
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
    pub created_at: String,
    pub updated_at: String,
    pub has_thumb: bool,
    pub sources: Vec<SourceRow>,
    pub tags: Vec<TagOnItem>,
}

pub fn get_item(conn: &Connection, id: &str) -> rusqlite::Result<Option<ItemDetail>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, title, original_title, circle, status, version, notes,
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
                r.get::<_, String>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, bool>(10)?,
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
        created_at: base.8,
        updated_at: base.9,
        has_thumb: base.10,
        sources,
        tags: tags_for_item(conn, id)?,
    }))
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
    let mut stmt = conn.prepare(
        "SELECT site, enabled, cookie <> '', rate_ms FROM site_settings ORDER BY site",
    )?;
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
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO site_settings (site, enabled, cookie, rate_ms)
         VALUES (?1, IFNULL(?2,1), IFNULL(?3,''), IFNULL(?4,700))
         ON CONFLICT(site) DO UPDATE SET
           enabled  = IFNULL(?2, enabled),
           cookie   = IFNULL(?3, cookie),
           rate_ms  = IFNULL(?4, rate_ms)",
        params![site, enabled.map(|e| e as i64), cookie, rate_ms],
    )?;
    Ok(())
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

/// Politeness delay for this site, in ms (PLAN §6 rate limiters).
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
        "SELECT u.item_id, i.title, u.site, u.old_version, u.new_version, u.seen_at
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
    pub b_id: String,
    pub b_title: String,
    pub b_sites: String,
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

/// Games whose titles match once version/engine brackets are stripped — the
/// usual DLsite origin ↔ f95 thread ↔ local folder triple. These are only ever
/// *suggestions*; a human confirms the merge.
pub fn pairing_suggestions(conn: &Connection) -> rusqlite::Result<Vec<PairSuggestion>> {
    let mut stmt = conn.prepare(
        "SELECT id, title,
                (SELECT group_concat(DISTINCT site) FROM sources s WHERE s.item_id = items.id)
         FROM items WHERE kind = 'game' ORDER BY title",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut groups: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    for (id, title, sites) in rows {
        let key = pairing_key(&title);
        if key.is_empty() {
            continue;
        }
        groups.entry(key).or_default().push((id, title, sites));
    }

    let mut out = Vec::new();
    for mut g in groups.into_values() {
        if g.len() < 2 {
            continue;
        }
        g.sort_by(|a, b| a.0.cmp(&b.0));
        for i in 0..g.len() {
            for j in (i + 1)..g.len() {
                out.push(PairSuggestion {
                    a_id: g[i].0.clone(),
                    a_title: g[i].1.clone(),
                    a_sites: g[i].2.clone(),
                    b_id: g[j].0.clone(),
                    b_title: g[j].1.clone(),
                    b_sites: g[j].2.clone(),
                });
            }
        }
    }
    out.truncate(40);
    Ok(out)
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
    conn.execute(
        "DELETE FROM sources WHERE item_id = ?1",
        params![drop_id],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO item_tags (item_id, tag_id)
         SELECT ?1, tag_id FROM item_tags WHERE item_id = ?2",
        params![keep_id, drop_id],
    )?;
    conn.execute("DELETE FROM item_tags WHERE item_id = ?1", params![drop_id])?;
    // Media rows: keep hashes distinct (UNIQUE item/kind/hash).
    conn.execute(
        "INSERT OR IGNORE INTO media (item_id, kind, hash, mime, width, height, data)
         SELECT ?1, kind, hash, mime, width, height, data FROM media WHERE item_id = ?2",
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
            .query_row("SELECT count(*) FROM tags WHERE type_id = 1", [], |r| r.get(0))
            .unwrap();
        assert!(tags >= 3);
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
        let all = list_items(&conn, 300, 0, None, None, None, None).unwrap();
        assert_eq!(all.len(), 3);
        let games = list_items(&conn, 300, 0, None, Some("game"), None, None).unwrap();
        assert_eq!(games.len(), 2);
        let search = list_items(&conn, 300, 0, Some("gamma"), None, None, None).unwrap();
        assert_eq!(search.len(), 1);
        let counts = counts_by_status(&conn, Some("game")).unwrap();
        assert_eq!(counts.iter().map(|c| c.count).sum::<i64>(), 2);
    }

    #[test]
    fn tags_settings_updates_and_merge() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), 2);

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
        insert_source(&conn, "g1", "dlsite", "https://www.dlsite.com/maniax/work/=/product_id/RJ123456.html", Some("RJ123456"), None).unwrap();
        insert_source(&conn, "g2", "f95", "https://f95zone.to/threads/summer-witch.9999/", Some("9999"), None).unwrap();

        // Tag write path + tag filter + top tags.
        add_item_tag(&conn, "g1", "renpy", TYPE_CONTENT).unwrap();
        add_item_tag(&conn, "g2", "renpy", TYPE_CONTENT).unwrap();
        let tagged = list_items(&conn, 300, 0, None, None, Some("renpy"), None).unwrap();
        assert_eq!(tagged.len(), 2);
        let tops = top_tags(&conn, 10, None).unwrap();
        assert_eq!(tops[0].name, "renpy");
        assert_eq!(tops[0].count, 2);
        remove_item_tag(&conn, "g2", tops[0].id).unwrap();
        assert_eq!(list_items(&conn, 300, 0, None, None, Some("renpy"), None).unwrap().len(), 1);

        // Site filter + per-site counts (Media view filters).
        let from_dlsite = list_items(&conn, 300, 0, None, None, None, Some("dlsite")).unwrap();
        assert_eq!(from_dlsite.len(), 1);
        let sites = counts_by_site(&conn, Some("game")).unwrap();
        assert_eq!(sites.len(), 2);
        assert!(sites.iter().any(|s| s.site == "f95" && s.count == 1));
        assert!(counts_by_site(&conn, Some("media")).unwrap().is_empty());
        assert!(top_tags(&conn, 10, Some("media")).unwrap().is_empty());

        // Site settings upsert semantics.
        set_site_setting(&conn, "ehentai", None, None, None).unwrap();
        assert!(site_enabled(&conn, "ehentai", true));
        set_site_setting(&conn, "ehentai", Some(false), Some("ipb_member_id=1"), None).unwrap();
        assert!(!site_enabled(&conn, "ehentai", true));
        assert_eq!(site_cookie(&conn, "ehentai").as_deref(), Some("ipb_member_id=1"));
        set_site_setting(&conn, "ehentai", Some(true), None, None).unwrap();
        assert_eq!(site_cookie(&conn, "ehentai").as_deref(), Some("ipb_member_id=1"));
        assert_eq!(site_rate_ms(&conn, "ehentai", 10_500), 700);
        assert_eq!(site_rate_ms(&conn, "exhentai", 10_500), 10_500);
        assert_eq!(source_item_id(&conn, "dlsite", "https://www.dlsite.com/maniax/work/=/product_id/RJ123456.html").as_deref(), Some("g1"));
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
        let sugg = pairing_suggestions(&conn).unwrap();
        assert_eq!(sugg.len(), 1);
        merge_items(&conn, "g1", "g2").unwrap();
        assert!(get_item(&conn, "g2").unwrap().is_none());
        let d = get_item(&conn, "g1").unwrap().unwrap();
        assert_eq!(d.sources.len(), 2);
        assert_eq!(d.tags.len(), 1);
    }
}
