//! Domain model + storage schema for The Goon Flow.

pub mod db;
pub mod media;
pub mod model;

/// Storage schema version, tracked in `meta.schema_version`.
pub const SCHEMA_VERSION: i64 = 9;

/// True when a game's self-reported title identifies the *tool* rather than the
/// game. Repack installers and RPG Maker / NW.js projects ship placeholder
/// `gameTitle` / `name` values (`mmnz-game`, `game`, `sample`, `untitled`,
/// `new folder`) and the importer would otherwise use them as the library title —
/// which then pairs every game from that tool as a duplicate (#47) and makes the
/// library unreadable. Callers fall back to the folder/archive name instead.
pub fn title_is_placeholder(title: &str) -> bool {
    const NOISE: [&str; 10] = [
        "game", "games", "demo", "sample", "test", "untitled", "data", "folder", "new", "maker",
    ];
    let words: Vec<String> = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect();
    if words.is_empty() {
        return true;
    }
    // "game", "demo test", "new folder" …
    if words.iter().all(|w| NOISE.contains(&w.as_str())) {
        return true;
    }
    // "<slug>-game" / "<slug> game" is the RPG Maker placeholder shape
    // (mmnz-game), where <slug> is the installer's own tag.
    words.len() == 2 && words[1] == "game"
}

#[cfg(test)]
mod tests {
    use super::title_is_placeholder;

    #[test]
    fn tool_placeholders_are_recognised() {
        for t in [
            "mmnz-game",
            "MMNZ GAME",
            "game",
            "Game",
            "demo",
            "sample",
            "untitled",
            "new folder",
            "sample game",
            "",
            "   ",
            "_maker",
        ] {
            assert!(title_is_placeholder(t), "{t:?} should be refused");
        }
    }

    #[test]
    fn real_titles_survive() {
        for t in [
            "Summer Witch",
            "Summer Witch [v0.12] [renpy]",
            "Pumpkin Days",
            "Kana Game 2",
            "Himehib! Hyper",
            "NEOGAMES", // one word, not the "<slug> game" shape
        ] {
            assert!(!title_is_placeholder(t), "{t:?} must stay a title");
        }
    }
}
