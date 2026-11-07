//! Core domain types. Mapping to SQLite rows is 1:1 where practical.

use serde::{Deserialize, Serialize};

/// What kind of thing an item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Game,
    Gallery,
    Post,
}

impl ItemKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ItemKind::Game => "game",
            ItemKind::Gallery => "gallery",
            ItemKind::Post => "post",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "game" => Some(ItemKind::Game),
            "gallery" => Some(ItemKind::Gallery),
            "post" => Some(ItemKind::Post),
            _ => None,
        }
    }
}

/// Library pipeline stage (NOT completion — completion is a `user_status` tag).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Seen,
    Tracking,
    Wishlist,
    Downloaded,
    Played,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Seen => "seen",
            Status::Tracking => "tracking",
            Status::Wishlist => "wishlist",
            Status::Downloaded => "downloaded",
            Status::Played => "played",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "seen" => Some(Status::Seen),
            "tracking" => Some(Status::Tracking),
            "wishlist" => Some(Status::Wishlist),
            "downloaded" => Some(Status::Downloaded),
            "played" => Some(Status::Played),
            _ => None,
        }
    }
}

/// A tracked thing. One item can carry many `sources` (DLsite origin,
/// f95 thread, repack mirrors...) — pairing lives in the sources table.
#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub id: String,
    pub kind: ItemKind,
    pub title: String,
    pub original_title: Option<String>,
    pub circle: Option<String>,
    pub status: Status,
    pub dnf: bool,
    pub rating: Option<u8>,
    pub notes: String,
    pub version: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Per-site identity of an item (url, scraped id, raw snapshot for re-parse).
#[derive(Clone, Debug, Serialize)]
pub struct Source {
    pub id: i64,
    pub item_id: String,
    pub site: String,
    pub url: String,
    pub source_id: Option<String>,
    pub fetched_at: Option<String>,
    pub dead: bool,
}

/// Tag kinds with display attributes (PLAN.md D9). `user_status` tags are the
/// personal state chips — e.g. Complete (green, highest priority).
#[derive(Clone, Debug, Serialize)]
pub struct TagType {
    pub id: i64,
    pub key: String,
    pub label: String,
    pub priority: i64,
    pub color: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub type_id: i64,
}
