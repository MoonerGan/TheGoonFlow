//! #76: what the query box means — tokenizer, AST and SQL plan, with no
//! database in sight (plan §7 stage S2).
//!
//! Three parts, in the order the plan sets them out: [`parse`] turns the box's
//! text into a [`Query`] (a list of [`Term`]s, each one an AST [`Node`] plus the
//! exact text the user typed, which is what the chip row shows and what a chip
//! click deletes), and [`Query::plan`] turns that into a `WHERE` fragment plus
//! the parameters that go with it. Nothing here opens a connection or runs a
//! statement: stage S3's `db::search_items` is the only thing that hands a
//! [`Plan`] to SQLite.
//!
//! The rules that are easy to get wrong, and where each came from:
//!
//! * **A keyword is reserved always**, never when the value happens to look
//!   plausible. The closed, code-owned keyword set is the whole anti-import
//!   guarantee (plan §9 decision 4); a grammar that reads `ext:gif` one way on
//!   one vault and another way on the next is not a grammar. A tag whose real
//!   name contains a colon — `ext:something`, `type:video`, `source:x` — stays
//!   reachable through the quoted form `tag:"ext:something"`, pinned by a test.
//! * **An unmeasured fact is NULL and never matches** (DEVELOPMENT §4 invariant
//!   27). Most fragments get that for free from SQL's three-valued logic:
//!   `NULL > 100` is NULL, and a NULL in a `WHERE` is not a match. The one place
//!   it has to be spelled out is `ratio`, where the division needs a
//!   non-zero-divisor guard first.
//! * **Dates are integer comparisons.** `items.created_at` / `items.updated_at`
//!   are TEXT holding epoch *seconds*, and `CAST(… AS INTEGER)` is the whole
//!   point: `'999999999' > '1000000000'` is true lexically, false numerically.
//!   The parser converts a typed date to an epoch itself, so a date string is
//!   never bound as a string.
//! * **Negation is exact.** Every negated term is `items.id NOT IN (…)` or
//!   `NOT EXISTS (…)`, never "filter the page afterwards" — the whole query
//!   result is what filtering, counts and *Select all* act on (76-rulings C).
//! * **Nothing here is stored.** The box's text is session state: this module
//!   takes a `&str`, returns an AST and a parameterized fragment, and never
//!   writes to the vault — no connection exists in this file at all. A [`Plan`]
//!   is rebuilt per request, and locking the vault must not leave a search
//!   behind (plan §5: nothing decrypted survives).
//! * **A parse error is a value, not a panic or a toast.** [`ParseError`] names
//!   the offending token in one sentence the form can show inline (plan §5).

use rusqlite::types::ToSql;
use rusqlite::types::ToSqlOutput;

/// Which view is asking. The vocabulary differs: `status:` is the Games
/// pipeline, and a media item has no such column to read (plan §4: "D14
/// enforced in the grammar, not just the rails").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Media,
    Games,
}

/// The comparison operator a term was typed with. Bare `key:value` means
/// [`Cmp::Eq`]; for a date that equality is a *day interval*, never a
/// second-for-second match (see [`DateValue`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Eq,
    Gt,
    Ge,
    Lt,
    Le,
}

impl Cmp {
    /// The SQL operator this maps to.
    fn sql(self) -> &'static str {
        match self {
            Cmp::Eq => "=",
            Cmp::Gt => ">",
            Cmp::Ge => ">=",
            Cmp::Lt => "<",
            Cmp::Le => "<=",
        }
    }

    /// True for the keywords that compare numbers or epochs, i.e. the only ones
    /// that may be typed with an operator at all.
    fn allowed(key: &str) -> bool {
        matches!(
            key,
            "rating" | "size" | "length" | "width" | "height" | "ratio" | "added" | "updated"
        )
    }
}

/// `type:` — what GoonFlow computed from the bytes, kept deliberately separate
/// from `ext:`, which is what the file calls itself (76-rulings A: the two are
/// allowed to disagree honestly).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeKind {
    Video,
    Image,
    Gif,
}

/// `has:` — the two existence probes the plan's vocabulary freezes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HasWhat {
    Thumb,
    Tags,
}

/// `stored:` — the derived storage shape (plan §3.3): vault BLOB, on-disk file,
/// or still only a link. Derived, never a column, until #73 gives an item a
/// real location.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoredShape {
    Vault,
    Disk,
    Link,
}

/// A date the user typed, already converted to epoch **seconds** by the parser.
/// The interval is half-open: `[from, to)`.
///
/// * `added:2026-11-01` is a day — `from` is that day's 00:00 and `to` the next
///   day's 00:00, so it means "on that day". An equality against a
///   second-resolution stamp would match nothing, which is why the bare spelling
///   is a range.
/// * `added:2026-11-01T14:30` is an instant, recorded as `[t, t+1)`.
///
/// Day boundaries are **UTC**, because that is the only clock this crate can
/// read: `now_epoch()` writes UTC-based epoch seconds and goon-core has no
/// timezone database. A day window is therefore the UTC day, and the docs say so
/// rather than promising the user's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateValue {
    pub from: i64,
    pub to: i64,
}

impl DateValue {
    fn day(day_start: i64) -> Self {
        Self {
            from: day_start,
            to: day_start + 86_400,
        }
    }

    fn instant(secs: i64) -> Self {
        Self {
            from: secs,
            to: secs + 1,
        }
    }
}

/// One term's meaning. The AST is deliberately flat: space is AND, and there is
/// no grouping in this grammar (plan §4).
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// A bare token, colon included. Matches `tags.fold` (the cross-site union,
    /// so `big_breasts` finds e-hentai's `female:big_breasts`) or `tags.name`.
    Tag {
        name: String,
    },
    /// `tag:"…"` — the name exactly as the site spells it, namespace and all.
    ExactTag {
        name: String,
    },
    /// A quoted phrase on its own: the free-text search, over `title` or
    /// `circle` — exactly what the old search box read, so replacing that box
    /// costs no capability.
    Title {
        phrase: String,
    },
    Type {
        kind: TypeKind,
    },
    Ext {
        ext: String,
    },
    Source {
        site: String,
    },
    Has {
        what: HasWhat,
    },
    Stored {
        shape: StoredShape,
    },
    Status {
        status: String,
    },
    Circle {
        phrase: String,
    },
    Rating {
        cmp: Cmp,
        value: f64,
    },
    Size {
        cmp: Cmp,
        bytes: i64,
    },
    Length {
        cmp: Cmp,
        ms: i64,
    },
    Width {
        cmp: Cmp,
        px: i64,
    },
    Height {
        cmp: Cmp,
        px: i64,
    },
    Ratio {
        cmp: Cmp,
        ratio: f64,
    },
    Id {
        id: String,
    },
    Added {
        cmp: Cmp,
        at: DateValue,
    },
    Updated {
        cmp: Cmp,
        at: DateValue,
    },
}

/// A parsed term, with the text it came from so the UI can render a chip for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Term {
    /// Exactly as typed, `-anal` including the minus.
    pub text: String,
    pub negated: bool,
    pub node: Node,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub terms: Vec<Term>,
}

impl Query {
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// The SQL plan for this query (see [`Plan`]).
    pub fn plan(&self) -> Plan {
        let mut fragments: Vec<String> = Vec::with_capacity(self.terms.len());
        let mut params: Vec<Value> = Vec::new();
        for term in &self.terms {
            let one = fragment(&term.node, &mut params);
            fragments.push(if !term.negated {
                one
            } else if one.starts_with("EXISTS (") {
                // A bare existence probe reads better (and plans better) as
                // `NOT EXISTS (…)` than as `NOT (EXISTS (…))`.
                format!("NOT {one}")
            } else {
                format!("NOT ({one})")
            });
        }
        Plan {
            sql: fragments.join(" AND "),
            params,
        }
    }
}

/// Why the box cannot be understood. Shown inline under the search box as form
/// state — never a toast, because the query is still sitting there wrong a
/// minute later (plan §5, DEVELOPMENT §4 invariant 24).
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    /// One sentence, in the user's words, naming what to type instead.
    pub message: String,
    /// The token that failed, exactly as typed.
    pub token: String,
}

fn bad(token: &str, message: impl Into<String>) -> ParseError {
    ParseError {
        message: message.into(),
        token: token.to_string(),
    }
}

/// The closed keyword set (plan §9 decision 4). Meta tags are parsed and
/// evaluated, never written to `tags` (plan §3.3), and nothing here depends on
/// what a vault happens to contain.
pub const KEYWORDS: &[&str] = &[
    "type", "ext", "source", "tag", "has", "stored", "status", "circle", "rating", "size",
    "length", "width", "height", "ratio", "id", "added", "updated",
];

/// Keywords only the Games view may use.
pub const GAMES_ONLY_KEYWORDS: &[&str] = &["status"];

/// The file extensions that mean "video" to GoonFlow. `type:video` needs this
/// because a `file://` video is never vaulted (D17), so the only fact about it
/// is the name its own file carries. `ingest::VIDEO_EXTS` must stay this list —
/// a test in the desktop crate compares the two.
pub const VIDEO_EXTS: &[&str] = &["mp4", "webm", "mkv", "mov", "avi"];

/// The `status:` vocabulary, from `model::Status`. Closed, like every other
/// value set here.
pub const STATUSES: &[&str] = &["seen", "tracking", "wishlist", "downloaded", "played"];

/// The fixed value list for a keyword, empty for the open ones (`source:` takes
/// any site slug, `ext:` any extension, `tag:`/`circle:` any text, `id:` any id,
/// `added:`/`updated:` a date, and the numeric keys a number). Stage S5's
/// autocomplete types its candidates from this.
pub fn closed_values(key: &str) -> &'static [&'static str] {
    match key {
        "type" => &["video", "image", "gif"],
        "has" => &["thumb", "tags"],
        "stored" => &["vault", "disk", "link"],
        "status" => STATUSES,
        _ => &[],
    }
}

/// The keywords a view accepts, in the order the autocomplete should show them.
pub fn keywords(scope: Scope) -> Vec<&'static str> {
    KEYWORDS
        .iter()
        .copied()
        .filter(|k| !(GAMES_ONLY_KEYWORDS.contains(k) && scope == Scope::Media))
        .collect()
}

/// Parse the query box's contents for a view. An empty box is an empty query,
/// not an error: it means "everything", which is what the grid shows.
pub fn parse(input: &str, scope: Scope) -> Result<Query, ParseError> {
    let mut terms = Vec::new();
    for token in tokenize(input)? {
        terms.push(parse_term(&token, scope)?);
    }
    Ok(Query { terms })
}

/// Splits on whitespace while keeping a `"…"` run together, wherever the quote
/// starts — that is what lets `circle:"Red Sun"` and `"summer witch"` work, and
/// quoting is the only place a space survives (plan §4).
fn tokenize(input: &str) -> Result<Vec<String>, ParseError> {
    let mut out: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            if let Some(token) = current.take() {
                out.push(token);
            }
            continue;
        }
        if c == '"' {
            let mut run = String::from('"');
            let mut closed = false;
            for inner in chars.by_ref() {
                run.push(inner);
                if inner == '"' {
                    closed = true;
                    break;
                }
            }
            if !closed {
                return Err(bad(
                    &run,
                    "a quoted value runs to the next quote, and this one never closes",
                ));
            }
            current.get_or_insert_with(String::new).push_str(&run);
            continue;
        }
        current.get_or_insert_with(String::new).push(c);
    }
    if let Some(token) = current {
        out.push(token);
    }
    Ok(out)
}

fn parse_term(token: &str, scope: Scope) -> Result<Term, ParseError> {
    let (negated, rest) = match token.strip_prefix('-') {
        Some(rest) if !rest.is_empty() => (true, rest),
        Some(_) => {
            return Err(bad(
                token,
                "a lone '-' negates a term: -anal, -source:local, -has:tags",
            ))
        }
        None => (false, token),
    };

    let node = match unquoted(rest) {
        // A quoted token on its own is the free-text search, same as the old box.
        Some(phrase) => Node::Title {
            phrase: phrase.to_string(),
        },
        None => match parse_predicate(rest, token, scope)? {
            Some(node) => node,
            // Not a keyword, so a tag name — `female:big_breasts` and an unknown
            // `foo:bar` both stay literal, which is what keeps site vocabulary
            // importable (plan §4).
            None => Node::Tag {
                name: rest.to_string(),
            },
        },
    };
    Ok(Term {
        text: token.to_string(),
        negated,
        node,
    })
}

/// The inner text of a fully quoted value, or `None` if it is not quoted.
fn unquoted(value: &str) -> Option<&str> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    // `"a" "b"` tokenizes as two tokens, so a quote in the middle means the
    // value is not a single quoted run.
    if inner.contains('"') {
        None
    } else {
        Some(inner)
    }
}

/// Splits `keyword [cmp] ':' [cmp] value` into operator and value. Both the
/// spellings the plan freezes parse: `size>100mb` and `size:>100mb`.
fn split_operator(tail: &str) -> Option<(Cmp, &str)> {
    if let Some(after_colon) = tail.strip_prefix(':') {
        let (cmp, value) = split_cmp(after_colon);
        return Some((cmp, value));
    }
    let (cmp, value) = split_cmp(tail);
    if cmp == Cmp::Eq {
        // No colon and no operator: the keyword run just runs into something
        // else (`size-ish`, `id#3`), which is a tag name, not a predicate.
        return None;
    }
    Some((cmp, value.strip_prefix(':').unwrap_or(value)))
}

fn split_cmp(text: &str) -> (Cmp, &str) {
    if let Some(rest) = text.strip_prefix(">=") {
        (Cmp::Ge, rest)
    } else if let Some(rest) = text.strip_prefix("<=") {
        (Cmp::Le, rest)
    } else if let Some(rest) = text.strip_prefix('>') {
        (Cmp::Gt, rest)
    } else if let Some(rest) = text.strip_prefix('<') {
        (Cmp::Lt, rest)
    } else if let Some(rest) = text.strip_prefix('=') {
        (Cmp::Eq, rest)
    } else {
        (Cmp::Eq, text)
    }
}
/// Turns `key…value` into a node. `token` is the whole term, so an error message
/// can quote what the user actually typed.
fn parse_predicate(rest: &str, token: &str, scope: Scope) -> Result<Option<Node>, ParseError> {
    let key_len = rest.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
    if key_len == 0 {
        return Ok(None);
    }
    let key = rest[..key_len].to_ascii_lowercase();
    if !KEYWORDS.contains(&key.as_str()) {
        return Ok(None);
    }
    if GAMES_ONLY_KEYWORDS.contains(&key.as_str()) && scope == Scope::Media {
        return Err(bad(
            token,
            "`status:` is a Games word - media items have no pipeline status \
             (what the media view can ask is type:, stored:, has:, size:, length:)",
        ));
    }
    let tail = &rest[key_len..];
    let Some((cmp, raw)) = split_operator(tail) else {
        if tail.is_empty() {
            return Err(bad(
                token,
                format!(
                    "`{key}` is a keyword, so it needs a value: {}",
                    value_hint(&key)
                ),
            ));
        }
        return Ok(None);
    };
    if cmp != Cmp::Eq && !Cmp::allowed(&key) {
        return Err(bad(
            token,
            format!("`{key}:` matches by name, so it has no > or < - write `{key}:…`"),
        ));
    }
    let value = unquoted(raw).unwrap_or(raw);
    let node = build(&key, cmp, value, token)?;
    Ok(Some(node))
}

/// The "what can I type here" half of an error message.
fn value_hint(key: &str) -> String {
    let fixed = closed_values(key);
    if !fixed.is_empty() {
        return format!("{key}:{}", fixed.join(" | "));
    }
    match key {
        "size" => "size>100mb | size<500kb | size:>2gb".to_string(),
        "length" => "length>1:30 | length<90s | length>2m".to_string(),
        "width" | "height" => format!("{key}>1200"),
        "ratio" => "ratio:>2 | ratio:<0.5".to_string(),
        "rating" => "rating:>3".to_string(),
        "circle" => "circle:\"Red Sun\" | circle:renpy".to_string(),
        "source" => "source:ehentai | -source:local".to_string(),
        "ext" => "ext:gif | ext:mp4".to_string(),
        "tag" => "tag:\"female:big_breasts\"".to_string(),
        "id" => "id:01J…".to_string(),
        "added" | "updated" => format!("{key}:>=2026-11-01"),
        _ => format!("{key}:…"),
    }
}

fn build(key: &str, cmp: Cmp, value: &str, token: &str) -> Result<Node, ParseError> {
    Ok(match key {
        "type" => match value.to_ascii_lowercase().as_str() {
            "video" => Node::Type {
                kind: TypeKind::Video,
            },
            "image" => Node::Type {
                kind: TypeKind::Image,
            },
            "gif" => Node::Type {
                kind: TypeKind::Gif,
            },
            other => {
                return Err(bad(
                    token,
                    format!(
                        "`type:{other}` is not a type - it is what GoonFlow read from the \
                         bytes, so the words are type:video, type:image (a still picture) \
                         and type:gif (anything the container says animates)"
                    ),
                ))
            }
        },
        "ext" => Node::Ext {
            ext: word(value, 8, token, "an extension: ext:gif, ext:mp4")?,
        },
        "source" => Node::Source {
            site: slug(value, token, "a site name: source:ehentai, -source:local")?,
        },
        "tag" => {
            if value.is_empty() {
                return Err(bad(
                    token,
                    "tag: needs the name exactly as the site spells it, \
                     and quotes are what keep a colon or a space literal",
                ));
            }
            Node::ExactTag {
                name: value.to_string(),
            }
        }
        "has" => match value.to_ascii_lowercase().as_str() {
            "thumb" => Node::Has {
                what: HasWhat::Thumb,
            },
            "tags" => Node::Has {
                what: HasWhat::Tags,
            },
            other => {
                return Err(bad(
                    token,
                    format!(
                        "`has:{other}` is not something an item can have - the two are \
                         has:thumb and has:tags (write -has:tags for the untagged queue)"
                    ),
                ))
            }
        },
        "stored" => match value.to_ascii_lowercase().as_str() {
            "vault" => Node::Stored {
                shape: StoredShape::Vault,
            },
            "disk" => Node::Stored {
                shape: StoredShape::Disk,
            },
            "link" => Node::Stored {
                shape: StoredShape::Link,
            },
            other => {
                return Err(bad(
                    token,
                    format!(
                        "`stored:{other}` is not a storage shape - the three are stored:vault \
                         (bytes in the vault), stored:disk (a file on this machine) and \
                         stored:link (still only a url)"
                    ),
                ))
            }
        },
        "status" => {
            let lowered = value.to_ascii_lowercase();
            if !STATUSES.contains(&lowered.as_str()) {
                return Err(bad(
                    token,
                    format!(
                        "`status:{lowered}` is not a pipeline stage - the five are {}",
                        STATUSES
                            .iter()
                            .map(|s| format!("status:{s}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
            Node::Status { status: lowered }
        }
        "circle" => {
            if value.is_empty() {
                return Err(bad(token, "circle: needs a name, or a quoted phrase"));
            }
            Node::Circle {
                phrase: value.to_string(),
            }
        }
        "rating" => Node::Rating {
            cmp,
            value: decimal(value, token, "a rating number, e.g. rating:>3")?,
        },
        "size" => Node::Size {
            cmp,
            bytes: byte_size(value, token)?,
        },
        "length" => Node::Length {
            cmp,
            ms: duration_ms(value, token)?,
        },
        "width" => Node::Width {
            cmp,
            px: count(value, token, "a pixel width, e.g. width>1200")?,
        },
        "height" => Node::Height {
            cmp,
            px: count(value, token, "a pixel height, e.g. height>800")?,
        },
        "ratio" => Node::Ratio {
            cmp,
            ratio: decimal(value, token, "a width/height ratio, e.g. ratio:>2")?,
        },
        "id" => Node::Id {
            id: word(value, 64, token, "an item id")?,
        },
        "added" => Node::Added {
            cmp,
            at: epoch(value, token)?,
        },
        "updated" => Node::Updated {
            cmp,
            at: epoch(value, token)?,
        },
        // Reachable only if KEYWORDS and this match arm drift apart, which the
        // keyword-set test below is there to catch.
        other => {
            return Err(bad(
                token,
                format!("`{other}:` is not a word this app knows"),
            ))
        }
    })
}

/// A lowercase single word: `a-z0-9`, at most `max` characters.
fn word(value: &str, max: usize, token: &str, expects: &str) -> Result<String, ParseError> {
    let lowered = value.to_ascii_lowercase();
    let ok = !lowered.is_empty()
        && lowered.len() <= max
        && lowered.bytes().all(|b| b.is_ascii_alphanumeric());
    if ok {
        Ok(lowered)
    } else {
        Err(bad(token, format!("expects {expects}")))
    }
}

/// A site slug: lowercase, and allowed to contain `.`/`_`/`-` because that is
/// how site names are spelled in `sources.site`. Unknown site names are not an
/// error - they match nothing, which is honest (the vocabulary is the vault's,
/// not a compile-time list, invariant 10).
fn slug(value: &str, token: &str, expects: &str) -> Result<String, ParseError> {
    let lowered = value.to_ascii_lowercase();
    let ok = !lowered.is_empty()
        && lowered.len() <= 40
        && lowered
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(lowered)
    } else {
        Err(bad(token, format!("expects {expects}")))
    }
}

fn count(value: &str, token: &str, expects: &str) -> Result<i64, ParseError> {
    value
        .parse::<i64>()
        .map_err(|_| bad(token, format!("expects {expects}")))
}

fn decimal(value: &str, token: &str, expects: &str) -> Result<f64, ParseError> {
    let parsed = value
        .parse::<f64>()
        .ok()
        .filter(|f| f.is_finite() && *f >= 0.0);
    parsed.ok_or_else(|| bad(token, format!("expects {expects}")))
}

/// `100mb` / `2gb` / `500kb` / a bare byte count. Multiples are 1024-based,
/// because that is the number the grid and Explorer print beside a file; the
/// docs say so, so a `size>100mb` list is not a surprise.
fn byte_size(value: &str, token: &str) -> Result<i64, ParseError> {
    // Lowercased once: `SIZE>1MB` is the same query, and the unit is the only
    // case-insensitive part of a number.
    let text = value.trim().to_ascii_lowercase();
    let text = text.as_str();
    let (number, multiplier) =
        if let Some(rest) = text.strip_suffix("gb").or(text.strip_suffix("g")) {
            (rest, 1024_i64 * 1024 * 1024)
        } else if let Some(rest) = text.strip_suffix("mb").or(text.strip_suffix("m")) {
            (rest, 1024_i64 * 1024)
        } else if let Some(rest) = text.strip_suffix("kb").or(text.strip_suffix("k")) {
            (rest, 1024)
        } else {
            (text, 1)
        };
    let amount = decimal(
        number,
        token,
        "a size: size>100mb, size<500kb, size:>2gb, size>1048576",
    )?;
    let bytes = amount * multiplier as f64;
    if bytes < 0.0 || bytes > i64::MAX as f64 {
        return Err(bad(token, "that size is too large to compare against"));
    }
    Ok(bytes as i64)
}

/// Durations: `1:30` (mm:ss), `1:02:03` (h:mm:ss), `90s`, `2m`, `1h`, and a bare
/// number which means seconds. Returns milliseconds.
fn duration_ms(value: &str, token: &str) -> Result<i64, ParseError> {
    let text = value.trim().to_ascii_lowercase();
    let text = text.as_str();
    let expect = "a length: length>1:30, length<90s, length>2m, length>1h";
    if text.contains(':') {
        let parts: Vec<&str> = text.split(':').collect();
        if parts.len() > 3 || parts.iter().any(|p| p.is_empty()) {
            return Err(bad(token, format!("expects {expect}")));
        }
        // Read right to left: seconds, minutes, hours. Everything below the
        // largest unit is 0-59, which is why `1:70` is a typo and not a length.
        let mut total = 0_i64;
        for (index, part) in parts.iter().rev().enumerate() {
            let unit: i64 = part
                .parse::<i64>()
                .map_err(|_| bad(token, format!("expects {expect}")))?;
            if unit < 0 {
                return Err(bad(token, format!("expects {expect}")));
            }
            if index != parts.len() - 1 && unit > 59 {
                return Err(bad(
                    token,
                    "the minutes and seconds of a length are 0-59 (1:70 is not a length)",
                ));
            }
            let step = unit
                .checked_mul(60_i64.pow(index as u32))
                .and_then(|t| total.checked_add(t));
            total =
                step.ok_or_else(|| bad(token, "that length is too large to compare against"))?;
        }
        return Ok(total.saturating_mul(1000));
    }
    let (number, seconds_each) = if let Some(rest) = text.strip_suffix("ms") {
        (rest, 0.001)
    } else if let Some(rest) = text.strip_suffix('h') {
        (rest, 3600.0)
    } else if let Some(rest) = text.strip_suffix('m') {
        (rest, 60.0)
    } else if let Some(rest) = text.strip_suffix('s') {
        (rest, 1.0)
    } else {
        (text, 1.0)
    };
    let amount = decimal(number, token, expect)?;
    Ok((amount * seconds_each * 1000.0) as i64)
}

/// A date or datetime, converted here to epoch seconds. `2026-11-01`,
/// `2026-11-01T14:30`, `2026-11-01T14:30:15`, and a bare epoch integer. A day is
/// a half-open interval, never an exact second (review requirement: an equality
/// against a second-resolution timestamp is a query that matches nothing).
fn epoch(value: &str, token: &str) -> Result<DateValue, ParseError> {
    let text = value.trim();
    let expect = "a date: added:2026-11-01, added:>=2026-11-01, updated:<2026-11-01T14:30";
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) && text.len() >= 5 {
        return text
            .parse::<i64>()
            .map(DateValue::instant)
            .map_err(|_| bad(token, format!("expects {expect}")));
    }
    let (date_part, time_part) = match text.split_once(['T', 't']) {
        Some((d, t)) => (d, Some(t)),
        None => (text, None),
    };
    let bits: Vec<&str> = date_part.split('-').collect();
    if bits.len() != 3
        || bits
            .iter()
            .any(|b| b.is_empty() || !b.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err(bad(token, format!("expects {expect}")));
    }
    let year = bits[0].parse::<i64>().expect("all digits");
    let month = bits[1].parse::<i64>().expect("all digits");
    let day = bits[2].parse::<i64>().expect("all digits");
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return Err(bad(
            token,
            format!("`{date_part}` is not a date on the calendar (check the month and day)"),
        ));
    }
    let mut secs = days_from_civil(year, month, day) * 86_400;
    if let Some(time) = time_part {
        let parts: Vec<&str> = time.split(':').collect();
        if parts.len() < 2 || parts.len() > 3 || parts.iter().any(|p| p.len() != 2) {
            return Err(bad(
                token,
                "a time is HH:MM or HH:MM:SS, as in added:>=2026-11-01T14:30",
            ));
        }
        let mut numbers: Vec<i64> = Vec::with_capacity(parts.len());
        for part in &parts {
            let n = part
                .parse::<i64>()
                .map_err(|_| bad(token, "a time is HH:MM or HH:MM:SS"))?;
            numbers.push(n);
        }
        if numbers[0] > 23 || numbers[1] > 59 || (numbers.len() == 3 && numbers[2] > 59) {
            return Err(bad(token, "that time is not on the clock (HH:MM:SS)"));
        }
        secs += numbers[0] * 3600 + numbers[1] * 60 + numbers.get(2).copied().unwrap_or(0);
        Ok(DateValue::instant(secs))
    } else {
        Ok(DateValue::day(secs))
    }
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    const LENGTHS: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if month == 2 && is_leap(year) {
        29
    } else {
        LENGTHS[(month - 1) as usize]
    }
}

/// Days since 1970-01-01 for a proleptic-Gregorian date (Howard Hinnant's
/// `days_from_civil`). Written here rather than pulled in as a date crate:
/// goon-core has no chrono and this card does not need one.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let year_of_era = y - era * 400;
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}
/// A value bound to a `?` in a planned fragment. Dates and sizes are integers,
/// which is the whole point: a fact is compared as a number, never as text.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    Int(i64),
    Real(f64),
}

impl ToSql for Value {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(match self {
            Value::Text(s) => ToSqlOutput::from(s.as_str()),
            Value::Int(i) => ToSqlOutput::from(*i),
            Value::Real(f) => ToSqlOutput::from(*f),
        })
    }
}

/// A whole number stays a whole number, so `rating:>3` compares integers and the
/// index on the column still does its job.
fn number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9.0e18 {
        Value::Int(value as i64)
    } else {
        Value::Real(value)
    }
}

/// The SQL one query turns into: the `WHERE` body (terms AND-ed, exactly the
/// order they were typed) and the parameters for its `?` marks.
///
/// The fragments and the parameter list are produced by the same pass, in the
/// same order, and no fragment ever skips a parameter — which is what the old
/// `?1 IS NULL OR …` shape got wrong (the placeholder-skipping bug pinned at
/// `db.rs`'s list-query test). Stage S3 joins this with the view's `kind`
/// predicate and binds [`Plan::bind`] positionally.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// Empty means "no predicate at all", i.e. the whole library.
    pub sql: String,
    pub params: Vec<Value>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.sql.is_empty()
    }

    /// `" WHERE …"`, or `""` when the query is empty — so a caller can append it
    /// to any SELECT without a special case.
    pub fn where_clause(&self) -> String {
        if self.sql.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", self.sql)
        }
    }

    /// The parameters as rusqlite wants them, in the order the `?` marks appear.
    pub fn bind(&self) -> Vec<&dyn ToSql> {
        self.params.iter().map(|v| v as &dyn ToSql).collect()
    }
}

/// The `tags` rows that answer a bare token: the folded name (so `big_breasts`
/// answers for e-hentai's `female:big_breasts` and gelbooru's `big_breasts` at
/// once) or the literal spelling. Tag-first, not a per-row EXISTS: `item_tags`
/// is walked from the tag side through `item_tags_tag_idx` (plan §6).
const TAG_SEMI: &str = "items.id IN (SELECT it.item_id FROM item_tags AS it \
                        JOIN tags AS t ON t.id = it.tag_id WHERE ";

/// `tags` of the `extension` type: what the file calls itself (76-rulings A),
/// written by the writers stage S4/S7 wires up, not imported from any site.
const EXTENSION_SEMI: &str = "items.id IN (SELECT it.item_id FROM item_tags AS it \
                              JOIN tags AS t ON t.id = it.tag_id \
                              JOIN tag_types AS tt ON tt.id = t.type_id WHERE ";

/// The stored rows that hold an original's bytes. A `thumb` row is deliberately
/// excluded from every fact predicate: a 360 px re-encoded tile says nothing
/// about the item's size or pixels, and measuring it is the exact lie review
/// note 1 of the code map warns about.
const ORIGINAL_MEDIA: &str = "m.kind IN ('full','video')";

/// LIKE wildcards and the escape character in a typed phrase are literal
/// characters: a title that really contains `_` (and in this library most do)
/// must not match as a one-character wildcard.
fn like_pattern(phrase: &str) -> String {
    let mut out = String::with_capacity(phrase.len() + 2);
    out.push('%');
    for c in phrase.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

fn fragment(node: &Node, params: &mut Vec<Value>) -> String {
    match node {
        Node::Tag { name } => {
            let (a, b) = (name.clone(), name.clone());
            params.push(Value::Text(a));
            params.push(Value::Text(b));
            format!("{TAG_SEMI}t.fold = ? OR t.name = ?)")
        }
        Node::ExactTag { name } => {
            params.push(Value::Text(name.clone()));
            format!("{TAG_SEMI}t.name = ?)")
        }
        Node::Title { phrase } => {
            let pattern = like_pattern(phrase);
            params.push(Value::Text(pattern.clone()));
            params.push(Value::Text(pattern));
            "(items.title LIKE ? ESCAPE '\\' OR IFNULL(items.circle, '') LIKE ? ESCAPE '\\')"
                .to_string()
        }
        Node::Type { kind } => match kind {
            TypeKind::Video => {
                // A vault video is decided by its own row; a link or on-disk
                // video has no row, so the only fact about it is the extension
                // its file carries (D17: videos stay links by default).
                let mut placeholders: Vec<&str> = Vec::with_capacity(VIDEO_EXTS.len());
                for ext in VIDEO_EXTS {
                    params.push(Value::Text((*ext).to_string()));
                    placeholders.push("?");
                }
                format!(
                    "(EXISTS (SELECT 1 FROM media AS m WHERE m.item_id = items.id \
                     AND m.kind = 'video') OR {EXTENSION_SEMI}tt.key = 'extension' \
                     AND t.name IN ({})))",
                    placeholders.join(", ")
                )
            }
            TypeKind::Image => "EXISTS (SELECT 1 FROM media AS m WHERE m.item_id = items.id \
                 AND m.kind = 'full' AND m.animated = 0)"
                .to_string(),
            TypeKind::Gif => "EXISTS (SELECT 1 FROM media AS m WHERE m.item_id = items.id \
                              AND m.animated = 1)"
                .to_string(),
        },
        Node::Ext { ext } => {
            params.push(Value::Text(ext.clone()));
            format!("{EXTENSION_SEMI}tt.key = 'extension' AND t.name = ?)")
        }
        Node::Source { site } => {
            params.push(Value::Text(site.clone()));
            "items.id IN (SELECT s.item_id FROM sources AS s WHERE s.site = ?)".to_string()
        }
        Node::Has { what } => match what {
            HasWhat::Thumb => "EXISTS (SELECT 1 FROM media AS m WHERE m.item_id = items.id \
                               AND m.kind = 'thumb')"
                .to_string(),
            HasWhat::Tags => {
                "EXISTS (SELECT 1 FROM item_tags AS it WHERE it.item_id = items.id)".to_string()
            }
        },
        Node::Stored { shape } => match shape {
            StoredShape::Vault => format!(
                "EXISTS (SELECT 1 FROM media AS m WHERE m.item_id = items.id AND {ORIGINAL_MEDIA})"
            ),
            StoredShape::Disk => "EXISTS (SELECT 1 FROM sources AS s WHERE s.item_id = items.id \
                                  AND s.site = 'local')"
                .to_string(),
            StoredShape::Link => format!(
                "(NOT EXISTS (SELECT 1 FROM media AS m WHERE m.item_id = items.id AND \
                 {ORIGINAL_MEDIA}) AND NOT EXISTS (SELECT 1 FROM sources AS s WHERE \
                 s.item_id = items.id AND s.site = 'local'))"
            ),
        },
        Node::Status { status } => {
            params.push(Value::Text(status.clone()));
            "items.status = ?".to_string()
        }
        Node::Circle { phrase } => {
            params.push(Value::Text(like_pattern(phrase)));
            "(IFNULL(items.circle, '') LIKE ? ESCAPE '\\')".to_string()
        }
        Node::Rating { cmp, value } => {
            params.push(number(*value));
            format!("items.rating {} ?", cmp.sql())
        }
        Node::Size { cmp, bytes } => {
            fact_pair("m.bytes", "s.bytes", cmp.sql(), Value::Int(*bytes), params)
        }
        Node::Length { cmp, ms } => fact_pair(
            "m.duration_ms",
            "s.duration_ms",
            cmp.sql(),
            Value::Int(*ms),
            params,
        ),
        Node::Width { cmp, px } => {
            fact_pair("m.width", "s.width", cmp.sql(), Value::Int(*px), params)
        }
        Node::Height { cmp, px } => {
            fact_pair("m.height", "s.height", cmp.sql(), Value::Int(*px), params)
        }
        Node::Ratio { cmp, ratio } => {
            params.push(number(*ratio));
            // The NULL guard is spelled out here because the arithmetic is: an
            // unmeasured picture has no ratio, and a zero height is not a
            // divide-by-zero invitation either (invariant 27).
            format!(
                "EXISTS (SELECT 1 FROM media AS m WHERE m.item_id = items.id AND \
                 {ORIGINAL_MEDIA} AND m.width IS NOT NULL AND m.height IS NOT NULL AND \
                 m.height > 0 AND CAST(m.width AS REAL) / m.height {} ?)",
                cmp.sql()
            )
        }
        Node::Id { id } => {
            params.push(Value::Text(id.clone()));
            "items.id = ?".to_string()
        }
        Node::Added { cmp, at } => date_condition("items.created_at", *cmp, *at, params),
        Node::Updated { cmp, at } => date_condition("items.updated_at", *cmp, *at, params),
    }
}

/// A fact predicate over the two places a fact can be recorded: the vault's own
/// row, and the `sources` row that caches what a disk file or a bare link
/// measured (76-rulings B). Either one matching is enough, so the pair is an
/// OR — and both sides compare numbers, so an unmeasured NULL never matches.
fn fact_pair(
    media_column: &str,
    source_column: &str,
    op: &str,
    bound: Value,
    params: &mut Vec<Value>,
) -> String {
    let for_media = bound.clone();
    let for_source = bound;
    params.push(for_media);
    params.push(for_source);
    format!(
        "(EXISTS (SELECT 1 FROM media AS m WHERE m.item_id = items.id AND \
         {ORIGINAL_MEDIA} AND {media_column} {op} ?) OR \
         EXISTS (SELECT 1 FROM sources AS s WHERE s.item_id = items.id AND \
         {source_column} {op} ?))"
    )
}

/// `added:` / `updated:`. The `CAST` is not decoration: those columns are TEXT
/// holding epoch seconds, and `'999999999' > '1000000000'` is true as text and
/// false as numbers. The value is always an integer the parser computed.
fn date_condition(column: &str, cmp: Cmp, at: DateValue, params: &mut Vec<Value>) -> String {
    let cast = format!("CAST({column} AS INTEGER)");
    match cmp {
        // Equality on a date is the day (or the second), never a coin flip
        // against a second-resolution stamp.
        Cmp::Eq => {
            params.push(Value::Int(at.from));
            params.push(Value::Int(at.to));
            format!("({cast} >= ? AND {cast} < ?)")
        }
        Cmp::Ge => {
            params.push(Value::Int(at.from));
            format!("{cast} >= ?")
        }
        // `>` past a day means the day after it, and past an instant the next
        // second: seconds are the grain, so `>=` of the exclusive end is the
        // same set and stays index-friendly.
        Cmp::Gt => {
            params.push(Value::Int(at.to));
            format!("{cast} >= ?")
        }
        Cmp::Lt => {
            params.push(Value::Int(at.from));
            format!("{cast} < ?")
        }
        Cmp::Le => {
            params.push(Value::Int(at.to));
            format!("{cast} < ?")
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Parses as the media view would, and fails the test with the message a
    /// user would actually see if it does not parse.
    fn media(query: &str) -> Query {
        match parse(query, Scope::Media) {
            Ok(q) => q,
            Err(e) => panic!("media query {query:?} should parse: {}", e.message),
        }
    }

    fn games(query: &str) -> Query {
        match parse(query, Scope::Games) {
            Ok(q) => q,
            Err(e) => panic!("games query {query:?} should parse: {}", e.message),
        }
    }

    fn error(query: &str, scope: Scope) -> ParseError {
        match parse(query, scope) {
            Ok(q) => panic!("{query:?} should not parse, got {:?}", q.terms),
            Err(e) => e,
        }
    }

    fn nodes(query: &str) -> Vec<Node> {
        media(query).terms.into_iter().map(|t| t.node).collect()
    }

    // ---- the fourteen worked examples, plan §4 --------------------------------

    #[test]
    fn example_01_bare_tag_is_a_fold_match() {
        assert_eq!(
            nodes("big_breasts"),
            vec![Node::Tag {
                name: "big_breasts".into()
            }]
        );
        // The fold-or-name pair is what finds gelbooru's row and e-hentai's
        // `female:big_breasts` with one token, and the walk starts at the tag,
        // through `item_tags_tag_idx`, not per item (plan §6).
        let sql = media("big_breasts").plan().sql;
        assert!(sql.contains("t.fold = ? OR t.name = ?"), "{sql}");
        assert!(sql.contains("FROM item_tags AS it"), "{sql}");
    }

    #[test]
    fn example_02_a_namespaced_token_stays_the_literal_name() {
        assert_eq!(
            nodes("female:big_breasts"),
            vec![Node::Tag {
                name: "female:big_breasts".into()
            }]
        );
    }

    #[test]
    fn example_03_provenance_pinned_exact_name() {
        assert_eq!(
            nodes("source:ehentai tag:\"female:big_breasts\""),
            vec![
                Node::Source {
                    site: "ehentai".into()
                },
                Node::ExactTag {
                    name: "female:big_breasts".into()
                },
            ]
        );
    }

    #[test]
    fn example_04_two_sources_is_an_and_not_an_or() {
        let terms = media("source:gelbooru source:rule34").terms;
        assert_eq!(terms.len(), 2);
        let plan = media("source:gelbooru source:rule34").plan();
        assert!(plan.sql.contains(" AND "), "{}", plan.sql);
        assert_eq!(
            plan.params,
            vec![Value::Text("gelbooru".into()), Value::Text("rule34".into())]
        );
    }

    #[test]
    fn example_05_content_tag_and_a_negated_tag() {
        let terms = media("big_breasts -anal").terms;
        assert_eq!(
            terms[0].node,
            Node::Tag {
                name: "big_breasts".into()
            }
        );
        assert_eq!(
            terms[1].node,
            Node::Tag {
                name: "anal".into()
            }
        );
        assert!(terms[1].negated);
        assert_eq!(terms[1].text, "-anal", "the chip keeps the minus");
        assert!(media("big_breasts -anal")
            .plan()
            .sql
            .contains("NOT (items.id IN"));
    }

    #[test]
    fn example_06_quoted_phrase_and_a_negated_source() {
        assert_eq!(
            nodes("\"summer witch\" -source:local"),
            vec![
                Node::Title {
                    phrase: "summer witch".into()
                },
                Node::Source {
                    site: "local".into()
                },
            ]
        );
        assert!(media("\"summer witch\" -source:local").terms[1].negated);
    }

    #[test]
    fn example_07_short_videos_by_duration() {
        assert_eq!(
            nodes("type:video length<1:30"),
            vec![
                Node::Type {
                    kind: TypeKind::Video
                },
                Node::Length {
                    cmp: Cmp::Lt,
                    ms: 90_000
                },
            ]
        );
    }

    #[test]
    fn example_08_long_clips_that_are_not_vaulted() {
        assert_eq!(
            nodes("type:video length>2m -stored:vault"),
            vec![
                Node::Type {
                    kind: TypeKind::Video
                },
                Node::Length {
                    cmp: Cmp::Gt,
                    ms: 120_000
                },
                Node::Stored {
                    shape: StoredShape::Vault
                },
            ]
        );
        assert!(media("type:video length>2m -stored:vault").terms[2].negated);
    }

    #[test]
    fn example_09_big_still_images() {
        assert_eq!(
            nodes("size>100mb type:image"),
            vec![
                Node::Size {
                    cmp: Cmp::Gt,
                    bytes: 100 * 1024 * 1024
                },
                Node::Type {
                    kind: TypeKind::Image
                },
            ]
        );
    }

    #[test]
    fn example_10_video_bytes_the_vault_actually_holds() {
        assert_eq!(
            nodes("stored:vault type:video"),
            vec![
                Node::Stored {
                    shape: StoredShape::Vault
                },
                Node::Type {
                    kind: TypeKind::Video
                },
            ]
        );
    }

    #[test]
    fn example_11_the_untagged_queue_narrowed_to_one_site() {
        let terms = media("-has:tags source:ehentai").terms;
        assert!(terms[0].negated);
        assert_eq!(
            terms[0].node,
            Node::Has {
                what: HasWhat::Tags
            }
        );
        assert!(!terms[1].negated);
        assert!(media("-has:tags source:ehentai")
            .plan()
            .sql
            .contains("NOT EXISTS (SELECT 1 FROM item_tags"));
    }

    #[test]
    fn example_12_on_disk_items_with_no_tile() {
        assert_eq!(
            nodes("-has:thumb stored:disk"),
            vec![
                Node::Has {
                    what: HasWhat::Thumb
                },
                Node::Stored {
                    shape: StoredShape::Disk
                },
            ]
        );
        assert!(media("-has:thumb stored:disk").terms[0].negated);
    }

    #[test]
    fn example_13_games_pipeline_stage_and_an_engine_tag() {
        assert_eq!(
            games("status:wishlist renpy")
                .terms
                .iter()
                .map(|t| t.node.clone())
                .collect::<Vec<_>>(),
            vec![
                Node::Status {
                    status: "wishlist".into()
                },
                Node::Tag {
                    name: "renpy".into()
                },
            ]
        );
    }

    #[test]
    fn example_14_small_originals_without_a_tile() {
        assert_eq!(
            nodes("width<400 type:image -has:thumb"),
            vec![
                Node::Width {
                    cmp: Cmp::Lt,
                    px: 400
                },
                Node::Type {
                    kind: TypeKind::Image
                },
                Node::Has {
                    what: HasWhat::Thumb
                },
            ]
        );
    }

    /// The plan's worked examples (plan §4) plus the shapes the meta vocabulary
    /// adds, reused by every "does this hold for all of them" test.
    const WORKED: &[&str] = &[
        "big_breasts",
        "female:big_breasts",
        "source:ehentai tag:\"female:big_breasts\"",
        "source:gelbooru source:rule34",
        "big_breasts -anal",
        "\"summer witch\" -source:local",
        "type:video length<1:30",
        "type:video length>2m -stored:vault",
        "size>100mb type:image",
        "stored:vault type:video",
        "-has:tags source:ehentai",
        "-has:thumb stored:disk",
        "width<400 type:image -has:thumb",
        "ext:gif ext:mp4",
        "ratio:>2 height>800",
        "circle:\"Red Sun\" rating:>3",
        "id:01JG9XYQ2M8R4K5N6P7Q8S9T0V",
        "added:2026-11-01 updated:>=2026-11-01T14:30",
        "-size>1gb type:gif",
        "stored:link -source:local type:video length:>90s",
        "type:image -has:tags size<2mb width>1200 ratio:<1",
        "added:>2026-01-01 added:<2027-01-01 has:thumb",
    ];

    /// The invariant behind the per-example tests: a fragment and its bound
    /// parameters are produced in one pass, so their counts can never drift —
    /// and every fragment has to be balanced, because a stray parenthesis is a
    /// query SQLite refuses to prepare.
    #[test]
    fn every_placeholder_has_exactly_one_parameter() {
        for query in WORKED {
            let plan = media(query).plan();
            assert_eq!(
                plan.sql.matches('?').count(),
                plan.params.len(),
                "placeholders and parameters disagree for {query:?}: {}",
                plan.sql
            );
            assert_eq!(
                scan_sql(&plan.sql).0,
                0,
                "parentheses do not balance for {query:?}: {}",
                plan.sql
            );
        }
    }

    /// Every emitted fragment has to be real SQL against the real v10 schema —
    /// the point of stage S2 is that stage S3 has nothing left to discover.
    #[test]
    fn every_example_is_sql_the_schema_can_run() {
        use rusqlite::Connection;
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        for query in WORKED {
            let plan = media(query).plan();
            let sql = format!(
                "SELECT items.id FROM items{} ORDER BY items.updated_at DESC LIMIT 5",
                plan.where_clause()
            );
            let mut stmt = conn.prepare(&sql).unwrap_or_else(|e| {
                panic!("{query:?} did not prepare: {e}\n{sql}");
            });
            let rows = stmt
                .query_map(rusqlite::params_from_iter(plan.bind()), |r| {
                    r.get::<_, String>(0)
                })
                .unwrap_or_else(|e| panic!("{query:?} would not bind: {e}"));
            let matched = rows.count();
            assert!(matched <= 5, "{query:?} ignored its LIMIT");
        }
    }
    // ---- the colon is already taken, so keywords need an escape hatch -------

    #[test]
    fn a_colon_in_a_tag_name_is_not_a_predicate() {
        for token in [
            "foo:bar",
            "other:variant_set",
            "critter:feathers",
            "artist:horosuke",
        ] {
            assert_eq!(
                nodes(token),
                vec![Node::Tag { name: token.into() }],
                "{token} is site vocabulary, so it has to survive unquoted"
            );
        }
        // A keyword run that runs into a non-operator is still a tag name.
        for token in ["size-ish", "id#3", "taggart", "sourceful"] {
            assert!(
                matches!(nodes(token)[0], Node::Tag { .. }),
                "{token:?} should be a tag, got {:?}",
                nodes(token)[0]
            );
        }
        // Plan §9 decision 2: the `<site>:<name>` shorthand is deliberately NOT
        // reserved, so a query written today still means a literal tag match if
        // a future grammar adds the sugar - nothing that exists can break.
        for token in ["ehentai:big_breasts", "local:clip", "gelbooru:big_breasts"] {
            assert_eq!(
                nodes(token),
                vec![Node::Tag { name: token.into() }],
                "{token} must stay a literal tag until the shorthand is reserved"
            );
        }
    }

    #[test]
    fn ext_is_reserved_whatever_the_value_is() {
        // Reservation cannot depend on the value looking like a real extension:
        // that would make the grammar depend on table contents, and `ext:gif`
        // would mean one thing on this vault and another on the next.
        assert_eq!(nodes("ext:gif"), vec![Node::Ext { ext: "gif".into() }]);
        assert_eq!(
            nodes("ext:banana"),
            vec![Node::Ext {
                ext: "banana".into()
            }]
        );
        assert_eq!(nodes("ext:\"gif\""), vec![Node::Ext { ext: "gif".into() }]);
        let e = error("ext:", Scope::Media);
        assert!(e.message.contains("extension"), "{}", e.message);
    }

    #[test]
    fn a_reserved_name_is_still_reachable_as_a_literal_tag() {
        // The escape hatch for the always-reserved keywords (review requirement
        // for stage S2, and what S5's suggester should hint at when a keyword
        // form matches nothing).
        for (query, name) in [
            ("tag:\"ext:something\"", "ext:something"),
            ("tag:\"type:video\"", "type:video"),
            ("tag:\"source:x\"", "source:x"),
            ("tag:size", "size"),
        ] {
            assert_eq!(
                nodes(query),
                vec![Node::ExactTag { name: name.into() }],
                "{query} must reach the literal tag name"
            );
        }
        let plan = media("tag:\"ext:something\"").plan();
        assert!(plan.sql.contains("t.name = ?"), "{}", plan.sql);
        assert!(
            !plan.sql.contains("t.fold"),
            "exact means exact: {}",
            plan.sql
        );
    }

    #[test]
    fn a_bare_keyword_says_what_to_type_instead() {
        for (query, needle) in [
            ("size", "size>100mb"),
            ("type", "type:video"),
            ("stored", "stored:vault"),
            ("tag", "tag:\"female:big_breasts\""),
        ] {
            let e = error(query, Scope::Media);
            assert_eq!(e.token, query);
            assert!(e.message.contains(needle), "{}", e.message);
        }
    }

    #[test]
    fn keywords_are_case_insensitive_and_tag_names_are_not() {
        assert_eq!(
            nodes("SIZE>1MB"),
            vec![Node::Size {
                cmp: Cmp::Gt,
                bytes: 1024 * 1024
            }]
        );
        assert_eq!(
            nodes("Big_Breasts"),
            vec![Node::Tag {
                name: "Big_Breasts".into()
            }],
            "tag matching is against the name the site spells, so the token is kept as typed"
        );
        assert_eq!(
            nodes("Source:Ehentai"),
            vec![Node::Source {
                site: "ehentai".into()
            }],
            "site slugs are lowercase in sources.site"
        );
    }

    #[test]
    fn every_keyword_reaches_a_node() {
        // Catches a keyword added to KEYWORDS with no arm in `build`.
        for key in KEYWORDS {
            let value = match *key {
                "type" => "video",
                "ext" => "gif",
                "source" => "ehentai",
                "tag" => "some:tag",
                "has" => "tags",
                "stored" => "vault",
                "status" => "wishlist",
                "circle" => "red",
                "rating" => "3",
                "size" => "1mb",
                "length" => "90s",
                "width" => "100",
                "height" => "100",
                "ratio" => "2",
                "id" => "abc123",
                "added" | "updated" => "2026-11-01",
                other => other,
            };
            let query = format!("{key}:{value}");
            let terms = games(&query).terms;
            assert!(
                !matches!(terms[0].node, Node::Tag { .. }),
                "{query} fell through to a tag: build() has no arm for {key}"
            );
        }
    }

    // ---- numbers: sizes, durations, pixels, ratings --------------------------

    #[test]
    fn size_units_are_the_1024_the_grid_prints() {
        assert_eq!(
            nodes("size>1kb"),
            vec![Node::Size {
                cmp: Cmp::Gt,
                bytes: 1024
            }]
        );
        assert_eq!(
            nodes("size<500mb"),
            vec![Node::Size {
                cmp: Cmp::Lt,
                bytes: 500 * 1024 * 1024
            }]
        );
        assert_eq!(
            nodes("size:>2gb"),
            vec![Node::Size {
                cmp: Cmp::Gt,
                bytes: 2 * 1024 * 1024 * 1024
            }]
        );
        assert_eq!(
            nodes("size>=1048576"),
            vec![Node::Size {
                cmp: Cmp::Ge,
                bytes: 1_048_576
            }],
            "a bare number is bytes"
        );
        assert_eq!(
            nodes("size>1.5mb"),
            vec![Node::Size {
                cmp: Cmp::Gt,
                bytes: 1_572_864
            }]
        );
    }

    #[test]
    fn both_spellings_of_a_range_plan_the_same_sql() {
        assert_eq!(media("size:>100mb").plan(), media("size>100mb").plan());
        assert_eq!(
            media("added:>=2026-11-01").plan(),
            media("added>=2026-11-01").plan()
        );
    }

    #[test]
    fn a_length_can_be_written_four_ways() {
        assert_eq!(
            nodes("length>1:30"),
            vec![Node::Length {
                cmp: Cmp::Gt,
                ms: 90_000
            }]
        );
        assert_eq!(
            nodes("length<90s"),
            vec![Node::Length {
                cmp: Cmp::Lt,
                ms: 90_000
            }]
        );
        assert_eq!(
            nodes("length>2m"),
            vec![Node::Length {
                cmp: Cmp::Gt,
                ms: 120_000
            }]
        );
        assert_eq!(
            nodes("length:>1h"),
            vec![Node::Length {
                cmp: Cmp::Gt,
                ms: 3_600_000
            }]
        );
        assert_eq!(
            nodes("length>1:02:03"),
            vec![Node::Length {
                cmp: Cmp::Gt,
                ms: 3_723_000
            }]
        );
        assert_eq!(
            nodes("length>45"),
            vec![Node::Length {
                cmp: Cmp::Gt,
                ms: 45_000
            }],
            "a bare number is seconds"
        );
        assert_eq!(
            nodes("length:>500ms"),
            vec![Node::Length {
                cmp: Cmp::Gt,
                ms: 500
            }]
        );
        // The colon is a separator, not part of the operator: `:>` is `>` and
        // `:>=` is `>=`, and the two are different queries.
        assert_eq!(
            nodes("length:>=1h"),
            vec![Node::Length {
                cmp: Cmp::Ge,
                ms: 3_600_000
            }]
        );
        assert_ne!(media("size:>100mb").plan(), media("size:>=100mb").plan());
    }

    #[test]
    fn a_nonsense_value_names_the_spelling_that_works() {
        for (query, needle) in [
            ("size>x", "size>100mb"),
            ("length>1:70", "0-59"),
            ("width>abc", "width>1200"),
            ("height>1.5", "height>800"),
            ("ratio:>two", "ratio:>2"),
            ("rating:>high", "rating:>3"),
            ("type:mpeg", "type:video"),
            ("stored:maybe", "stored:vault"),
            ("has:nope", "has:thumb"),
            ("source:", "source:ehentai"),
            ("ext:too.long", "extension"),
        ] {
            let e = error(query, Scope::Games);
            assert!(
                e.message.contains(needle),
                "{query:?} said {:?}, which does not contain {needle:?}",
                e.message
            );
        }
    }

    // ---- dates ---------------------------------------------------------------

    #[test]
    fn a_bare_date_is_the_utc_day_not_one_second() {
        let terms = media("added:2026-11-01").terms;
        assert_eq!(
            terms[0].node,
            Node::Added {
                cmp: Cmp::Eq,
                at: DateValue {
                    from: 1_793_491_200,
                    to: 1_793_577_600,
                }
            },
            "2026-11-01T00:00:00Z .. 2026-11-02T00:00:00Z"
        );
        let plan = media("added:2026-11-01").plan();
        assert_eq!(
            plan.params,
            vec![Value::Int(1_793_491_200), Value::Int(1_793_577_600)]
        );
        assert!(plan.sql.contains("items.created_at"), "{}", plan.sql);
    }

    #[test]
    fn a_date_and_a_time_is_an_instant() {
        assert_eq!(
            media("updated:>=2026-11-01T14:30").terms[0].node,
            Node::Updated {
                cmp: Cmp::Ge,
                at: DateValue {
                    from: 1_793_543_400,
                    to: 1_793_543_401,
                }
            }
        );
        // `>` past an instant is the next second; past a day is the next day.
        assert_eq!(
            media("added:>2026-11-01").plan().params,
            vec![Value::Int(1_793_577_600)]
        );
        assert_eq!(
            media("added:<2026-11-01").plan().params,
            vec![Value::Int(1_793_491_200)]
        );
        assert_eq!(
            media("added:<=2026-11-01").plan().params,
            vec![Value::Int(1_793_577_600)]
        );
    }

    #[test]
    fn a_date_is_compared_as_a_number_never_as_text() {
        use rusqlite::Connection;
        // The single assertion that keeps this dead: nine digits sort *after*
        // ten digits when they are text, and every timestamp in this schema is
        // TEXT holding epoch seconds.
        let conn = Connection::open_in_memory().unwrap();
        let as_text: i64 = conn
            .query_row("SELECT ('999999999' > '1000000000')", [], |r| r.get(0))
            .unwrap();
        let as_number: i64 = conn
            .query_row(
                "SELECT (CAST('999999999' AS INTEGER) > CAST('1000000000' AS INTEGER))",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(as_text, 1, "this is the trap, so it had better stay pinned");
        assert_eq!(as_number, 0);

        for query in [
            "added:2026-11-01",
            "added:>=2026-11-01",
            "updated:<2026-11-01T14:30",
            "added:>1764547200",
        ] {
            let plan = media(query).plan();
            assert!(
                plan.sql.contains("CAST(items.created_at AS INTEGER)")
                    || plan.sql.contains("CAST(items.updated_at AS INTEGER)"),
                "{query:?} compares a timestamp without CAST: {}",
                plan.sql
            );
            assert!(
                plan.params.iter().all(|v| matches!(v, Value::Int(_))),
                "{query:?} bound a non-integer for a date: {:?}",
                plan.params
            );
        }
    }

    #[test]
    fn a_day_that_does_not_exist_is_refused() {
        for query in [
            "added:2026-13-01",
            "added:2026-02-29",
            "added:2026-04-31",
            "updated:2026-11-01T25:60",
            "updated:2026-11-01T12",
            "added:nonsense",
        ] {
            let e = error(query, Scope::Media);
            assert!(
                e.message.contains("date") || e.message.contains("time"),
                "{query:?} said {:?}",
                e.message
            );
        }
        // 2024-02-29 does exist, and its neighbour day is 60_4800 seconds on.
        let leap = media("added:2024-02-29").terms[0].node.clone();
        let Node::Added { at, .. } = leap else {
            panic!("expected an added: predicate")
        };
        assert_eq!(at.to - at.from, 86_400);
        let next = media("added:2024-03-01").terms[0].node.clone();
        let Node::Added { at: next_at, .. } = next else {
            panic!("expected an added: predicate")
        };
        assert_eq!(
            next_at.from - at.from,
            86_400,
            "2024-02-29 exists, so the day after it is 2024-03-01"
        );
    }
    // ---- what the plan promises about honesty -------------------------------

    #[test]
    fn a_fact_never_measures_a_thumbnail() {
        // The review note that started this: `media.width/height` were only ever
        // written on thumb rows, so a `width>1200` query measured the 360 px
        // tile and lied. Every fact predicate reads originals only.
        for query in [
            "size>1mb",
            "width>1200",
            "height>800",
            "ratio:>2",
            "stored:vault",
        ] {
            let sql = media(query).plan().sql;
            assert!(
                sql.contains("m.kind IN ('full','video')"),
                "{query:?} can match a thumb row: {sql}"
            );
        }
        // A thumb still answers `has:thumb`, which is the one predicate that
        // wants the tile.
        let thumb = media("has:thumb").plan().sql;
        assert!(thumb.contains("m.kind = 'thumb'"), "{thumb}");
    }

    #[test]
    fn an_unprobed_fact_is_left_to_never_match() {
        // `type:gif`/`type:image` read the container's answer, and NULL (never
        // asked) compares to neither 0 nor 1 - so the SQL must stay a plain
        // comparison instead of inventing a default (invariant 27).
        let gif = media("type:gif").plan().sql;
        assert!(gif.contains("m.animated = 1"), "{gif}");
        assert!(!gif.contains("IFNULL"), "{gif}");
        let image = media("type:image").plan().sql;
        assert!(image.contains("m.animated = 0"), "{image}");
        // `ratio` divides, so it has to exclude the unknowns itself first.
        let ratio = media("ratio:>2").plan().sql;
        assert!(ratio.contains("m.width IS NOT NULL"), "{ratio}");
        assert!(ratio.contains("m.height > 0"), "{ratio}");
        // Both places a fact can live are read: the vault's row and the cached
        // measurement on a disk/link source (76-rulings B).
        let length = media("length>2m").plan().sql;
        assert!(length.contains("m.duration_ms > ?"), "{length}");
        assert!(length.contains("s.duration_ms > ?"), "{length}");
    }

    #[test]
    fn negation_is_a_query_not_a_client_side_filter() {
        // Rulings C: filtering, counts and Select all apply to the whole result,
        // so every negation has to be exact SQL over the library.
        let cases = [
            (
                "-big_breasts",
                "NOT (items.id IN (SELECT it.item_id FROM item_tags",
            ),
            (
                "-source:ehentai",
                "NOT (items.id IN (SELECT s.item_id FROM sources",
            ),
            ("-tag:\"ext:x\"", "NOT (items.id IN"),
            ("-has:tags", "NOT EXISTS (SELECT 1 FROM item_tags"),
            ("-stored:disk", "NOT EXISTS (SELECT 1 FROM sources"),
            ("-size>1gb", "NOT ((EXISTS"),
            ("-width<400", "NOT ((EXISTS"),
        ];
        for (query, needle) in cases {
            let sql = media(query).plan().sql;
            assert!(sql.contains(needle), "{query:?} planned as {sql}");
        }
    }

    #[test]
    fn stored_link_is_the_absence_of_both_other_shapes() {
        let sql = media("stored:link").plan().sql;
        assert!(sql.contains("NOT EXISTS (SELECT 1 FROM media"), "{sql}");
        assert!(
            sql.contains("NOT EXISTS (SELECT 1 FROM sources AS s WHERE s.item_id = items.id AND s.site = 'local')"),
            "{sql}"
        );
    }

    #[test]
    fn wildcards_in_a_typed_phrase_stay_literal() {
        // Titles in this library are full of `_`; a bare LIKE would treat it as
        // a one-character wildcard and match the wrong items.
        let plan = media("\"50%_off\"").plan();
        assert_eq!(
            plan.params,
            vec![
                Value::Text("%50\\%\\_off%".into()),
                Value::Text("%50\\%\\_off%".into())
            ],
            "{}",
            plan.sql
        );
        assert!(plan.sql.contains(r#"LIKE ? ESCAPE '\'"#), "{}", plan.sql);
    }

    #[test]
    fn an_empty_box_is_the_whole_library() {
        let query = media("   ");
        assert!(query.is_empty());
        let plan = query.plan();
        assert!(plan.is_empty());
        assert_eq!(plan.where_clause(), "");
        assert!(plan.params.is_empty());
    }

    #[test]
    fn the_chip_text_is_what_the_user_typed() {
        let terms = media("type:video -anal \"summer witch\"").terms;
        let chips: Vec<&str> = terms.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(chips, vec!["type:video", "-anal", "\"summer witch\""]);
    }

    #[test]
    fn type_video_carries_the_extension_vocabulary() {
        // A link or on-disk video has no media row (D17), so its only fact is
        // the extension its own file carries.
        let plan = media("type:video").plan();
        let exts: Vec<&str> = plan
            .params
            .iter()
            .filter_map(|v| match v {
                Value::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(exts, VIDEO_EXTS.to_vec());
        assert!(plan.sql.contains("tt.key = 'extension'"), "{}", plan.sql);
        assert!(plan.sql.contains("m.kind = 'video'"), "{}", plan.sql);
    }

    #[test]
    fn an_id_predicate_is_exact() {
        let plan = media("id:01jg9xyq2m8r").plan();
        assert_eq!(plan.sql, "items.id = ?");
        assert_eq!(plan.params, vec![Value::Text("01jg9xyq2m8r".into())]);
    }

    // ---- the vocabulary differs by view (plan §4: D14 in the grammar) -------

    #[test]
    fn a_media_query_rejects_the_games_pipeline() {
        let e = error("status:wishlist", Scope::Media);
        assert_eq!(e.token, "status:wishlist");
        assert!(e.message.contains("Games"), "{}", e.message);
        // GamesView is the other half of the same rule.
        assert_eq!(
            games("status:played").terms[0].node,
            Node::Status {
                status: "played".into()
            }
        );
    }

    #[test]
    fn only_the_five_pipeline_stages_exist() {
        assert_eq!(games("status:tracking").terms.len(), 1);
        let e = error("status:done", Scope::Games);
        for status in STATUSES {
            assert!(e.message.contains(status), "{}", e.message);
        }
    }

    #[test]
    fn the_keyword_set_is_closed_and_per_view() {
        let media_words = keywords(Scope::Media);
        let game_words = keywords(Scope::Games);
        assert!(!media_words.contains(&"status"), "{media_words:?}");
        assert!(game_words.contains(&"status"));
        for word in ["type", "ext", "source", "tag", "has", "stored"] {
            assert!(media_words.contains(&word), "{word} missing");
        }
        // The value lists S5's autocomplete shows, and which keys are open.
        assert_eq!(closed_values("type"), &["video", "image", "gif"]);
        assert_eq!(closed_values("stored"), &["vault", "disk", "link"]);
        assert_eq!(closed_values("has"), &["thumb", "tags"]);
        assert!(closed_values("ext").is_empty(), "ext: is an open value set");
        assert!(closed_values("source").is_empty());
    }

    // ---- two shapes that must be form state, not a crash --------------------

    #[test]
    fn an_unterminated_quote_says_so() {
        let e = error("\"summer witch", Scope::Media);
        assert!(e.message.contains("never closes"), "{}", e.message);
        assert_eq!(e.token, "\"summer witch");
    }

    #[test]
    fn a_lone_minus_explains_itself() {
        let e = error("-", Scope::Media);
        assert!(e.message.contains("negates"), "{}", e.message);
        // A minus inside a tag name is a character, not an operator.
        assert_eq!(
            nodes("f95-completed"),
            vec![Node::Tag {
                name: "f95-completed".into()
            }]
        );
    }

    #[test]
    fn a_comparison_on_a_name_key_is_refused() {
        for query in [
            "tag>big_breasts",
            "source>ehentai",
            "type>video",
            "circle>red",
        ] {
            let e = error(query, Scope::Media);
            assert!(e.message.contains("matches by name"), "{}", e.message);
        }
    }

    #[test]
    fn quotes_make_a_space_survive_and_a_colon_literal() {
        // `circle:"…"` keeps the space, and `tag:"…"` is the only way to ask for
        // a name that starts with a keyword.
        assert_eq!(
            nodes("circle:\"Red Sun\""),
            vec![Node::Circle {
                phrase: "Red Sun".into()
            }]
        );
        assert_eq!(
            nodes("circle:red"),
            vec![Node::Circle {
                phrase: "red".into()
            }]
        );
        assert_eq!(
            nodes("circle:\"Red Sun\" -source:ehentai"),
            vec![
                Node::Circle {
                    phrase: "Red Sun".into()
                },
                Node::Source {
                    site: "ehentai".into()
                },
            ]
        );
        assert!(media("circle:\"Red Sun\" -source:ehentai").terms[1].negated);
    }
    #[test]
    fn terms_are_joined_in_the_order_they_were_typed() {
        let plan = media("type:video source:ehentai -anal").plan();
        let sql = plan.sql;
        let video = sql.find("m.kind = 'video'").expect("video arm");
        let source = sql.find("s.site = ?").expect("source arm");
        let anal = sql.find("NOT (items.id IN").expect("negated tag arm");
        assert!(video < source && source < anal, "{sql}");
        // Two joins at the top level; the ANDs inside a fragment are deeper and
        // belong to that fragment.
        assert_eq!(scan_sql(&sql), (0, 2), "{sql}");
    }

    /// Walks a planned fragment: `(paren depth at the end, number of ` AND `
    /// joins at the top level)`. Depth is what catches an unbalanced subquery,
    /// which is a query SQLite refuses to prepare.
    fn scan_sql(sql: &str) -> (i32, usize) {
        let mut depth = 0_i32;
        let mut joins = 0;
        let bytes = sql.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                b'A' if depth == 0 && sql[i..].starts_with("AND ") => {
                    joins += 1;
                    i += 3;
                }
                _ => {}
            }
            i += 1;
        }
        (depth, joins)
    }
}
