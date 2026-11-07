//! Schema v1 + tiny repository helpers. Thumbnails are encrypted BLOBs in the
//! DB (whole-vault nuke stays trivial); content-hash keyed for dedup.

use crate::model::{Item, ItemKind, Status};
use rusqlite::{params, Connection};
use serde::Serialize;

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

        INSERT OR IGNORE INTO tag_types (id, key, label, priority, color) VALUES
            (1, 'user_status', 'Personal', 100, '#4ade80'),
            (2, 'site_flag',   'Site',      50, NULL),
            (3, 'content',     'Content',   10, NULL);

        INSERT OR IGNORE INTO tags (id, name, type_id) VALUES
            (1, 'complete', 1),
            (2, 'dnf',      1),
            (3, 'replay',   1);

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

pub fn counts_by_status(conn: &Connection) -> rusqlite::Result<Vec<StatusCount>> {
    let mut stmt =
        conn.prepare("SELECT status, count(*) FROM items GROUP BY status")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
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
) -> rusqlite::Result<Vec<ItemSummary>> {
    let like = search.map(|s| format!("%{}%", s.trim()));
    let mut out = Vec::new();
    let sql = if like.is_some() {
        "SELECT id, kind, title, status, version,
                EXISTS (SELECT 1 FROM media m WHERE m.item_id = items.id AND m.kind = 'thumb'),
                updated_at
         FROM items
         WHERE title LIKE ?1 OR IFNULL(circle,'') LIKE ?1
         ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3"
    } else {
        "SELECT id, kind, title, status, version,
                EXISTS (SELECT 1 FROM media m WHERE m.item_id = items.id AND m.kind = 'thumb'),
                updated_at
         FROM items
         ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3"
    };
    let mut stmt = conn.prepare(sql)?;
    let mapped = if let Some(ref pat) = like {
        stmt.query_map(params![pat, limit, offset], map_item_summary)?
    } else {
        stmt.query_map(params![limit, offset], map_item_summary)?
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_initializes_and_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        init_schema(&conn).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), crate::SCHEMA_VERSION);
        let counts = counts_by_status(&conn).unwrap();
        assert_eq!(counts.iter().map(|c| c.count).sum::<i64>(), 0);
        let tags: i64 = conn
            .query_row("SELECT count(*) FROM tags WHERE type_id = 1", [], |r| r.get(0))
            .unwrap();
        assert!(tags >= 3);
    }
}
