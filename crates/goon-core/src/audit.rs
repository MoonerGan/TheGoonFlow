// The repo's own fixture audit: **no tracked file may name a real item on a
// content site.** This repository is public, and a committed url that addresses
// one specific post, gallery, thread or file is a fact about somebody's library
// — unrecoverable once pushed, even when the item itself is publicly reachable,
// because what it discloses is which items someone keeps.
//
// So the rule the audit enforces is:
//
//   * a committed fixture url must be **self-evidently fake**: placeholder ids
//     (`9000001`), placeholder hashes (`1111…aaaa`), `example.org`, `x.to`. A
//     fixture that is obviously made up is the only kind an audit can check
//     without guessing, and this audit is not allowed to guess.
//   * a **live probe** cannot use a fake url — it would "succeed" by being
//     refused, and the printout would then describe a policy no host holds. So a
//     live probe takes its target from the environment (`probe_url` in the
//     desktop crate) and says out loud when it was given none.
//
// It runs inside the suite because a rule nobody runs is a rule nobody keeps:
// the sweep that found this repo's 33 such urls found them in files that had
// already been reviewed and merged.
#![cfg(test)]
use std::collections::BTreeSet;

/// Tracked text files are the surface. `git ls-files` is the list, which is
/// also what keeps the audit honest: an untracked file (the operator's own
/// probe case list, say) may hold real urls precisely because it never
/// leaves the machine.
fn tracked_text_files(root: &std::path::Path) -> Result<Vec<String>, String> {
    let out = std::process::Command::new("git")
        .args(["ls-files"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    if !out.status.success() {
        return Err("git ls-files exited non-zero".to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|f| {
            [
                ".rs", ".md", ".ts", ".svelte", ".js", ".mjs", ".json", ".toml", ".ps1", ".css",
                ".html", ".yml", ".yaml", ".txt", ".example", ".conf", ".slim",
            ]
            .iter()
            .any(|e| f.ends_with(e))
        })
        .map(str::to_string)
        .collect())
}

/// Where a url stops: the delimiters a url appears between in Rust,
/// Markdown and JSON.
const URL_END: [char; 12] = [
    '"', '\'', '`', ')', ']', '<', '>', ' ', '\t', ',', ';', '\\',
];

/// Path+query only: ids live there, and a host or a port is not an item.
fn address_of(url: &str) -> String {
    let after = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    match after.find('/') {
        Some(i) => after[i..].to_ascii_lowercase(),
        None => String::new(),
    }
}

/// Digit-run length of the tail of a `key=value` pair.
fn value_digits(v: &str) -> usize {
    v.chars().take_while(|c| c.is_ascii_digit()).count()
}

/// Hosts that exist for examples: `example.*` is reserved by IANA for
/// documentation, and `x.to` is this repo's standing stand-in. A url on one
/// of these cannot name a real item, whatever its path looks like.
fn example_host(url: &str) -> bool {
    let after = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = after.split('/').next().unwrap_or("").to_ascii_lowercase();
    host == "x.to"
        || host.ends_with(".example.org")
        || host.ends_with(".example.com")
        || host.ends_with(".example.net")
        || host.ends_with(".example")
        || host == "example.org"
        || host == "example.com"
        || host == "example.net"
}

/// A url is *content-shaped* when it addresses one item rather than a site:
/// a file on a CDN, a numbered post, a gallery, a thread. Site roots and
/// documentation links are not content.
fn content_shaped(url: &str) -> bool {
    if example_host(url) {
        return false;
    }
    let a = address_of(url);
    let seg: Vec<&str> = a.split('/').filter(|s| !s.is_empty()).collect();
    // a booru file link: the file, with the post id as its query
    let file_link = [
        ".gif?", ".jpg?", ".jpeg?", ".png?", ".webp?", ".mp4?", ".webm?",
    ]
    .iter()
    .any(|e| a.contains(e));
    // ?id=NNNN or &id=NNNN (a post page or a dapi call)
    let named_post = a
        .split(['?', '&'])
        .any(|kv| kv.strip_prefix("id=").is_some_and(|v| value_digits(v) >= 4))
        || a.contains("/post/show/");
    // a forum thread addressed as <slug>.<numeric id> (f95)
    let thread = seg.iter().any(|s| {
        s.contains('.')
            && s.rsplit('.')
                .next()
                .is_some_and(|t| value_digits(t.trim_end_matches('/')) >= 3)
    });
    // a file store, or the e-hentai gallery/show shapes: /g/<gid>/…, /s/<key>/…
    let store = a.contains("/images/") || a.contains("/upload/");
    let eh = seg.windows(2).any(|w| {
        (w[0] == "g" || w[0] == "s")
            && w[1]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit() || c.is_ascii_hexdigit())
            && w[1].len() >= 4
    });
    // a content hash sitting in the path
    let hash = seg.iter().any(|s| {
        s.len() >= 16
            && s.chars().all(|c| c.is_ascii_hexdigit())
            && s.chars().any(|c| c.is_ascii_digit())
    });
    // a file named by its number alone (a CDN whose file name is a timestamp)
    let numeric_file = seg.iter().any(|s| {
        let stem = s.split('.').next().unwrap_or(s);
        stem.len() >= 6 && stem.chars().all(|c| c.is_ascii_digit())
    });
    file_link || named_post || thread || store || eh || hash || numeric_file
}

/// An id is obviously a placeholder when it is one repeated digit, a `1`
/// followed by nothing but zeros, or starts with `9` (the range the sweep
/// chose). Real content ids are handed out by the site and are not shaped
/// like that: an assigned post id or gallery id is seven or eight digits that
/// start with 1-8, and none of them are made of one repeated character.
fn id_is_placeholder(id: &str) -> bool {
    if id.len() < 4 || !id.chars().all(|c| c.is_ascii_digit()) {
        return true; // too short to name a specific item
    }
    if id.chars().all(|c| c == id.chars().next().unwrap()) || id.starts_with('9') {
        return true;
    }
    if id.starts_with('1') && id[1..].chars().all(|c| c == '0') {
        return true;
    }
    // a four-digit year is a date, not an item number: attachment urls carry
    // one (`/2026-11-<attach>.jpg`) and policing it would bury the finding.
    // A real assigned id is longer than this and never a year.
    if id.len() == 4 {
        let year: i32 = id.parse().unwrap_or(0);
        if (1900..=2199).contains(&year) {
            return true;
        }
    }
    // counting up or down: `0123456789`, `123456`. An assigned id can be
    // lucky enough to look like this, and that is the accepted cost of the
    // rule - the alternative is an audit that has to guess.
    let d: Vec<i32> = id.chars().map(|c| c as i32 - 48).collect();
    let up = d.windows(2).all(|w| w[1] - w[0] == 1);
    let down = d.windows(2).all(|w| w[0] - w[1] == 1);
    up || down
}

/// A content hash is a placeholder when it barely varies: at most four
/// distinct characters (`1111…aaaa`), or it counts up from zero.
fn hash_is_placeholder(h: &str) -> bool {
    let distinct: BTreeSet<char> = h.chars().collect();
    distinct.len() <= 4 || h.starts_with("0123456789abcdef") || h.starts_with("0123456789")
}

/// Every id-like token in the address must read as a placeholder, or the url
/// names a real item.
fn placeholders_only(url: &str) -> bool {
    let flat: String = address_of(url)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect();
    for tok in flat.split(' ').filter(|t| !t.is_empty()) {
        let all_digits = tok.chars().all(|c| c.is_ascii_digit());
        if all_digits && tok.len() >= 4 && !id_is_placeholder(tok) {
            return false;
        }
        let all_hex = tok.chars().all(|c| c.is_ascii_hexdigit());
        let has_digit = tok.chars().any(|c| c.is_ascii_digit());
        if all_hex && has_digit && tok.len() >= 16 && !hash_is_placeholder(tok) {
            return false;
        }
    }
    true
}

#[test]
fn fixture_urls_in_the_repo_are_never_real() {
    // crates/goon-core -> the repository root
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/goon-core sits two levels below the repo root")
        .to_path_buf();
    if !root.join(".git").exists() {
        eprintln!(
            "SKIP fixture audit: {} is not a git checkout",
            root.display()
        );
        return;
    }
    let files = match tracked_text_files(&root) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("SKIP fixture audit: {e}");
            return;
        }
    };
    let mut bad: Vec<String> = Vec::new();
    let mut seen = 0usize;
    for f in &files {
        let Ok(text) = std::fs::read_to_string(root.join(f)) else {
            continue; // binary, or a path this checkout cannot open
        };
        for (i, line) in text.lines().enumerate() {
            let mut rest = line;
            while let Some(i0) = rest.find("http") {
                let from = &rest[i0..];
                let url = &from[..from.find(URL_END).unwrap_or(from.len())];
                rest = &from[url.len().max(1)..];
                if !content_shaped(url) {
                    continue;
                }
                seen += 1;
                if !placeholders_only(url) {
                    bad.push(format!("{f}:{}  {url}", i + 1));
                }
            }
        }
    }
    println!(
        "fixture audit: {} tracked text files, {seen} content-shaped urls, {} not obviously fake",
        files.len(),
        bad.len()
    );
    assert!(
        bad.is_empty(),
        "these committed urls name a real item, which discloses part of someone's library:\n{}\n\
         make them self-evidently fake (placeholder id, example.org, x.to). If this is a live \
         probe, take the url from the environment instead - probe_url in the desktop crate.",
        bad.join("\n")
    );
}

/// The half that matters: the audit has to catch what it exists for. These
/// urls are assembled at run time so that this file does not itself contain
/// a content-shaped url that the audit above would - correctly - flag.
#[test]
fn the_audit_refuses_a_real_shaped_url() {
    let (booru, eh, cdn) = ("rule34", "e-hentai", "wimg.rule34");
    let (post, gid, key, thread) = (1234987u64, 2468135u64, "deadbeef01", 321420u64);
    for url in [
        format!("https://{booru}.xxx/index.php?page=post&s=view&id={post}"),
        format!("https://{booru}.xxx/post/show/{post}"),
        format!(
            "https://{cdn}.xxx//images/9000/{}abcd.jpeg?{post}",
            "a".repeat(28)
        ),
        format!(
            "https://{cdn}.xxx/images/9001/{}.jpeg",
            "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6"
        ),
        format!("https://{eh}.org/g/{gid}/{key}/"),
        format!("https://{eh}.org/s/{key}/{gid}-7"),
        format!("https://f95zone.to/threads/some-game-name.{thread}/"),
        format!("https://f95zone.to/data/avatars/m/9005/{}.jpg", 1121990u64),
        format!("https://i.4cdn.org/h/{}.jpg", 1743761874698456u64),
    ] {
        assert!(content_shaped(&url), "{url} should count as content");
        assert!(
            !placeholders_only(&url),
            "{url} should NOT read as a placeholder"
        );
    }
    // and the fakes this repo actually uses must pass, or the audit is noise
    for url in [
        "https://rule34.xxx/index.php?page=post&s=view&id=9000004",
        "https://wimg.rule34.xxx//images/9001/1111111111111111111111111111aaaa.gif?9000001",
        "https://e-hentai.org/g/9000009/0123456789/",
        "https://e-hentai.org/s/aaaaaaaaaa/9000009-7",
        "https://f95zone.to/threads/some-game.9000013/",
        "https://attachments.f95zone.to/2026-11-9000015_third.jpg?sig=1&t=24",
        "https://i.4cdn.org/h/1000000000000.jpg",
        "https://rule34.xxx/index.php?page=post&s=view&id=123456",
    ] {
        assert!(content_shaped(url), "{url} should count as content");
        assert!(placeholders_only(url), "{url} should read as a placeholder");
    }
    // the reserved example hosts are exempt whatever they carry
    assert!(!content_shaped(
        "https://example.org/images/55/66/556677889900aabbccddeeff1122334455.mp4?7654321"
    ));
}

/// And the shapes that are *not* one item must not be policed, or the audit
/// gets switched off by its own noise. This is also where the audit's limits
/// are written down: it looks for numbered items, so a tile sheet or a site's
/// own static asset is outside what it can see.
#[test]
fn site_links_are_not_content() {
    for url in [
        "https://rule34.xxx",
        "https://e-hentai.org/",
        "https://exhentai.org/g/",
        "https://rule34.xxx/index.php?page=post&s=view&id=1",
        "https://www.dlsite.com/maniax/work/=/product_id/RJ1.html",
        "https://i.4cdn.org/",
        "https://ehgt.org/w/02/546/900009-aaaaaaaaaa.webp",
        "http://127.0.0.1:9004/a/b.webm",
        "https://docs.rs/rusqlite",
    ] {
        assert!(!content_shaped(url), "{url} is a site/link, not an item");
    }
}
