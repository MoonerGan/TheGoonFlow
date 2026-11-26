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
//!   non-zero-divisor guard first. A [`Plan`] also reports which facts it read
//!   ([`Plan::unmeasured`]) so the caller can say "not measured yet" instead of
//!   presenting a zero as an answer.
//! * **Dates are integer comparisons, and a day is a UTC day.**
//!   `items.created_at` / `items.updated_at` are TEXT holding epoch *seconds*, and
//!   `CAST(… AS INTEGER)` is the whole point: `'999999999' > '1000000000'` is true
//!   lexically, false numerically. The parser converts a typed date to an epoch
//!   itself, so a date string is never bound as a string. The day boundary is UTC
//!   on purpose: the same query text must select the same rows on every machine,
//!   which a per-machine timezone would break (and would make the tests depend on
//!   where they ran). The wording users see says "UTC day" rather than pretending
//!   to know the user's zone.
//! * **Negation is exact.** Every negated term is `items.id NOT IN (…)` or
//!   `NOT EXISTS (…)`, never "filter the page afterwards" — the whole query
//!   result is what filtering, counts and *Select all* act on (76-rulings C). The
//!   exception is a negated **fact**: `-size>10mb` plans the contrary comparison
//!   (`bytes <= ?`), because `NOT (bytes > ?)` is true for a file nobody ever
//!   measured, and "un-measured" is not the same fact as "small".
//! * **Media predicates are `EXISTS`, never a join.** An item has several `media`
//!   rows (`thumb`, `full`, `video`), so a `JOIN media` in the grid query would
//!   multiply both the rows and the paired `COUNT(*)`; and no fact predicate ever
//!   reads a `thumb` row, because a re-encoded tile is not the item's size.
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

impl Scope {
    /// The wire spelling of the view, from the `view:` argument of the query
    /// commands. An unknown word is refused rather than defaulted: a frontend
    /// that sends `kind` where it means `view` would otherwise get a silently
    /// different library, and the grid would look like it lost items.
    pub fn parse_wire(text: &str) -> Result<Scope, ParseError> {
        match text {
            "media" => Ok(Scope::Media),
            "games" => Ok(Scope::Games),
            other => Err(bad(
                other,
                "the query box speaks one of two vocabularies: `media` or `games`",
            )),
        }
    }
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

    /// The comparison that means "measured, and not this". Used for a **negated
    /// fact** term (`-size>10mb`), where wrapping the fragment in `NOT (…)` would
    /// be a lie: `NOT (bytes > ?)` is true for a NULL column, so every un-probed
    /// item would answer "not big" — the exact claim invariant 27 forbids. A
    /// negated fact must name the contrary measurement instead, and SQL's
    /// three-valued logic then does the honest part: `NULL <= ?` is NULL, so an
    /// un-measured item matches neither side.
    fn opposite_sql(self) -> &'static str {
        match self {
            Cmp::Eq => "<>",
            Cmp::Gt => "<=",
            Cmp::Ge => "<",
            Cmp::Lt => ">=",
            Cmp::Le => ">",
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
/// on-disk file *and nothing else*, or still only a link. Derived, never a
/// column, until #73 gives an item a real location.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoredShape {
    Vault,
    Disk,
    /// The old #40 `local_only` badge, one-to-one: a local source **and** no
    /// source from any other site. `stored:disk` alone is wider (it also matches
    /// an item that has a local file *and* a site page), and a preset that
    /// silently widened the set it selected is what rulings C forbids.
    LocalOnly,
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
    /// `id:…` — one item, matched exactly. The app mints ids as
    /// `new_id(prefix)`: `c-`/`g-`/`i-`/`v-`/`b-` then 20 lowercase hex digits.
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
        let mut unmeasured: Vec<FactKey> = Vec::new();
        for term in &self.terms {
            fact_keys(&term.node, &mut unmeasured);
            let one = fragment(&term.node, term.negated, &mut params);
            fragments.push(if term.negated && !self_negates(&term.node) {
                if one.starts_with("EXISTS (") {
                    // A bare existence probe reads better (and plans better) as
                    // `NOT EXISTS (…)` than as `NOT (EXISTS (…))`.
                    format!("NOT {one}")
                } else {
                    format!("NOT ({one})")
                }
            } else {
                // A negated fact already planned its contrary comparison;
                // wrapping it in NOT would let an un-measured item answer.
                one
            });
        }
        Plan {
            sql: fragments.join(" AND "),
            params,
            unmeasured,
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
        "stored" => &["vault", "disk", "local-only", "link"],
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

/// How much the box may hold. Two ceilings, both chosen so the failure is an
/// inline form state rather than a SQLite prepare error (plan §5: a parse error
/// is a value):
///
/// * 50 terms — the widest fragment is `type:video` at 5 parameters, so 50 terms
///   stay far below `SQLITE_MAX_VARIABLE_NUMBER` (999) and well inside
///   `SQLITE_MAX_SQL_LENGTH`;
/// * 2 048 characters, which is more query than this grammar can express
///   usefully and keeps the suggester's per-keystroke re-plan trivial.
pub const MAX_TERMS: usize = 50;
pub const MAX_INPUT_CHARS: usize = 2_048;

/// The vocabulary one view accepts, spelled out for an error message. Generated
/// from [`keywords`], so a hint cannot go stale when the set grows: a
/// hand-written "type:, stored:, has:, size:" list inside an error message is a
/// promise the parser is not obliged to keep.
fn keyword_hint(scope: Scope) -> String {
    keywords(scope)
        .iter()
        .map(|key| format!("{key}:"))
        .collect::<Vec<String>>()
        .join(" ")
}

/// Parse an explicit list of tokens. [`parse`] is exactly this over
/// [`tokenize`], and the autocomplete path needs the token-list form: the token
/// being typed has to come off the end before the rest can be understood.
fn parse_terms(tokens: &[String], scope: Scope) -> Result<Query, ParseError> {
    if tokens.len() > MAX_TERMS {
        let extra = &tokens[MAX_TERMS];
        return Err(bad(
            extra,
            format!(
                "{MAX_TERMS} terms is the most one query can be; this is term {} - drop \
                 something",
                tokens.len()
            ),
        ));
    }
    let mut terms = Vec::with_capacity(tokens.len());
    for token in tokens {
        terms.push(parse_term(token, scope)?);
    }
    Ok(Query { terms })
}

/// Parse the query box's contents for a view. An empty box is an empty query,
/// not an error: it means "everything", which is what the grid shows.
pub fn parse(input: &str, scope: Scope) -> Result<Query, ParseError> {
    if input.chars().count() > MAX_INPUT_CHARS {
        // An empty `token` means "the whole box" - see ParseError.
        return Err(bad(
            "",
            format!(
                "that is more query than the box holds: {MAX_INPUT_CHARS} characters, so one \
                 fewer term"
            ),
        ));
    }
    parse_terms(&tokenize(input)?, scope)
}

/// The characters that make a query look right and parse wrong: a control,
/// format or bidi mark (invisible, so `big_breasts<U+200B>anal` is one tag name
/// that matches nothing) or a lookalike of the `-`, `:` or `=` the grammar runs
/// on (`–anal` with an en dash is not a negation at all, and `type：video` with a
/// fullwidth colon is not a predicate). Rejected in the grammar, never silently
/// normalized: normalizing would mean the text shown in the chip row is not what
/// the user typed.
fn invisible_or_lookalike(c: char) -> Option<&'static str> {
    if c.is_control() && !c.is_whitespace() {
        return Some("a control character");
    }
    if matches!(
        c,
        '\u{ad}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{206a}'..='\u{206f}'
            | '\u{feff}'
    ) {
        return Some("an invisible formatting or right-to-left mark");
    }
    if matches!(
        c,
        '\u{2010}'..='\u{2015}' | '\u{2212}' | '\u{fe58}' | '\u{fe63}' | '\u{ff0d}'
    ) {
        return Some("a dash that is not the '-' this grammar uses");
    }
    if c == '\u{ff1a}' {
        return Some("a colon that is not the ':' this grammar uses");
    }
    if matches!(c, '\u{ff1d}' | '\u{2212}' | '\u{2264}' | '\u{2265}') {
        return Some("a comparison sign this grammar cannot read");
    }
    None
}

/// What the box means **while the user is still typing in it** — the input to
/// the autocomplete, and the reason the suggester is not a second parser.
///
/// Only the **last** token can be unfinished, because the box is read left to
/// right and the caret is at the end: everything before it is already a query.
/// So this splits the box into "the query that would run if the token were
/// finished" plus "what is being typed", and the suggester counts candidates
/// against exactly that query. Plan §5 asks for counts *scoped to the query minus
/// the token being typed*, and this is the whole mechanism for it.
#[derive(Clone, Debug, PartialEq)]
pub struct SuggestCtx {
    /// The box without the unfinished token — the query the candidate counts are
    /// measured against.
    pub base: Query,
    /// The unfinished token, exactly as typed, `-type:vi` including the minus.
    pub token: String,
    pub negated: bool,
    /// The keyword the token has started, if its left side is one
    /// (`type:vi` → `Some("type")`; `female:big` → `None`, because `female` is
    /// not a keyword and that token is a tag name).
    pub keyword: Option<&'static str>,
    /// What is being completed: the value after the keyword, or the whole token
    /// when there is no keyword.
    pub value: String,
}

/// Keywords whose **value** autocomplete can enumerate: either a closed set the
/// grammar owns ([`closed_values`]) or a vocabulary the vault can count
/// (`source:`, `ext:`, `tag:`). Deliberately *not* the numeric keywords, and not
/// `circle:` or `id:`: completing those would invent a number or an id nobody
/// typed, which is the same dishonesty invariant 27 refuses about facts.
pub fn value_completion(key: &str) -> bool {
    matches!(key, "source" | "tag" | "ext") || !closed_values(key).is_empty()
}

/// Read the box as "query so far" + "token being typed". `None` means "there is
/// nothing to suggest", which is the honest answer more often than it sounds:
///
/// * the box ends in whitespace, so the last term is finished and a new token has
///   not started (offering the whole vocabulary there is noise);
/// * the last token is a closed quoted phrase (`"summer witch"`), which is a
///   complete free-text term;
/// * a quote is open (`title "summer`), where what is wanted is a phrase, not a
///   token;
/// * the token spells a keyword the **other** view owns (`status:wishlist` in the
///   media box), where the grammar has already written the error worth showing;
/// * or the text before the caret does not parse — the box is still wrong, and
///   completing a broken query would hide the inline error the form is showing.
pub fn parse_suggestion(input: &str, scope: Scope) -> Option<SuggestCtx> {
    if input.chars().count() > MAX_INPUT_CHARS || input.is_empty() {
        return None;
    }
    if input.ends_with(char::is_whitespace) {
        // The last term is finished and the next has not started: the honest
        // answer is no candidates, not the whole vocabulary.
        return None;
    }
    let mut tokens = tokenize(input).ok()?;
    let token = tokens.pop()?;
    // A quoted token is either a complete free-text phrase or part of an open
    // one (`tokenize` refuses the latter); either way there is no token to
    // complete, and this pass has no title-candidate source to offer instead.
    if token.starts_with('"') {
        return None;
    }
    // A keyword the **other** view owns is refused by the parser in this view
    // (`status:` in the media box is a `ParseError`, not a tag name), so there is
    // nothing to complete: the honest answer is no candidates, and the inline
    // error the grammar already wrote is what the form shows (plan §5).
    if keyword_of(&token, scope).is_none() && wrong_view_keyword(&token, scope) {
        return None;
    }
    let base = parse_terms(&tokens, scope).ok()?;
    let (negated, rest) = match token.strip_prefix('-') {
        Some(rest) if !rest.is_empty() => (true, rest),
        _ => (false, token.as_str()),
    };
    // The same left-of-operator rule the parser uses, so a token cannot be a
    // keyword here and a tag name there.
    let (keyword, value) = match keyword_of(rest, scope) {
        Some((key, value)) => (Some(key), value.to_string()),
        None => (None, rest.to_string()),
    };
    Some(SuggestCtx {
        base,
        token,
        negated,
        keyword,
        value,
    })
}

/// The keyword a token has started, with the value text after it. `type:vi` →
/// `("type", "vi")`, `size>100` → `("size", "100")`, `female:big` → `None`,
/// because `female` is not a keyword and that token is one tag name.
fn keyword_of(rest: &str, scope: Scope) -> Option<(&'static str, &str)> {
    let stop = rest.find([':', '=', '>', '<'])?;
    let key = keywords(scope).into_iter().find(|k| *k == &rest[..stop])?;
    // The operator run after a keyword is `:` and/or one comparison character,
    // in either order (`size:>100mb`, `size>100mb`, `rating=3`). Consuming the
    // whole run is what keeps the completion text aligned with where the caret
    // is, instead of offering `:>100` for a value the grammar reads as `100`.
    let mut value = &rest[stop..];
    while let Some(rest) = value.strip_prefix([':', '>', '<', '=']) {
        value = rest;
    }
    Some((key, value))
}

/// Is this token spelled with a keyword the **other** view owns? Today that is
/// exactly `status:`, which only the Games pipeline has a column to read, and it
/// only counts when an operator follows it.
fn wrong_view_keyword(token: &str, scope: Scope) -> bool {
    let token = token.strip_prefix('-').unwrap_or(token);
    let Some(stop) = token.find([':', '=', '>', '<']) else {
        return false;
    };
    let left = &token[..stop];
    keywords(scope).iter().all(|k| *k != left) && GAMES_ONLY_KEYWORDS.contains(&left)
}

/// Splits on whitespace while keeping a `"…"` run together, wherever the quote
/// starts — that is what lets `circle:"Red Sun"` and `"summer witch"` work, and
/// quoting is the only place a space survives (plan §4).
fn tokenize(input: &str) -> Result<Vec<String>, ParseError> {
    let mut out: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        // `char::is_whitespace` covers NBSP and the other Unicode spaces, so a
        // stray NBSP separates terms instead of hiding inside a tag name. Pinned
        // on purpose: it is what makes a copy-paste from a web page behave.
        if c.is_whitespace() {
            if let Some(token) = current.take() {
                out.push(token);
            }
            continue;
        }
        if c == '"' {
            // Inside quotes the text is data, not grammar: a scraped title really
            // can hold a soft hyphen or a bidi mark, and the one place the user
            // can ask for it verbatim has to stay open.
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
        if let Some(what) = invisible_or_lookalike(c) {
            let so_far: String = current.take().unwrap_or_default();
            return Err(bad(
                &format!("{so_far}{c}"),
                format!(
                    "that term contains {what} (U+{:04X}), which no tag in this library can \
                     match - type '-' or ':' in plain ASCII",
                    c as u32
                ),
            ));
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
        Some(phrase) => {
            if phrase.trim().is_empty() {
                // `""` would plan as `LIKE '%%'` and answer "every item in the
                // library", which is the opposite of what an empty pair of quotes
                // looks like it asks for.
                return Err(bad(
                    token,
                    "empty quotes search for nothing: type a word, as in \"summer witch\"",
                ));
            }
            Node::Title {
                phrase: phrase.to_string(),
            }
        }
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
/// spellings the plan freezes parse: `size>100mb` and `size:>100mb`, and plan §4's
/// EBNF also allows the colon-free `rating=3`.
fn split_operator(tail: &str) -> Option<(Cmp, &str)> {
    if let Some(after_colon) = tail.strip_prefix(':') {
        let (cmp, value) = split_cmp(after_colon);
        return Some((cmp, value));
    }
    // `key=value` with no colon. It has to be an explicit `=`: treating "no
    // operator" as Eq too would turn `size-ish`, `id#3` and `taggart` into
    // predicates, and those have to stay tag names (plan §9 decision 2).
    if let Some(after_eq) = tail.strip_prefix('=') {
        return Some((Cmp::Eq, after_eq.strip_prefix(':').unwrap_or(after_eq)));
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
            format!(
                "`status:` is a Games word - a media item has no pipeline status. The words \
                 this view does know: {}",
                keyword_hint(scope)
            ),
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
        "id" => "id:c-1f9a2b7c11d4e5a6b7c8".to_string(),
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
            if value.trim().is_empty() {
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
            "local-only" => Node::Stored {
                shape: StoredShape::LocalOnly,
            },
            other => {
                return Err(bad(
                    token,
                    format!(
                        "`stored:{other}` is not a storage shape - the four are stored:vault \
                         (bytes in the vault), stored:disk (a file on this machine), \
                         stored:local-only (a file here and no source from any site, which \
                         is the old 'local only' queue) and stored:link (still only a url)"
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
            if value.trim().is_empty() {
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
            id: id_token(value, token)?,
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

/// The ids this app mints are `new_id(prefix)`: a letter, a hyphen, 20 lowercase
/// hex digits (`c-` clip, `g-` game, `i-` images, `v-` video, `b-` bookmark).
/// The plan's sketch wrote `id:<ulid>`; against the code the shape is a hyphen,
/// so the hyphen is legal here and an unknown id is simply a query that matches
/// nothing.
fn id_token(value: &str, token: &str) -> Result<String, ParseError> {
    let lowered = value.to_ascii_lowercase();
    let ok = !lowered.is_empty()
        && lowered.len() <= 64
        && lowered
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if ok {
        Ok(lowered)
    } else {
        Err(bad(
            token,
            "id: expects an item id as the app minted it, e.g. id:c-1f9a2b7c11d4e5a6b7c8",
        ))
    }
}

/// A whole count that cannot be negative. `width>-100` would otherwise parse,
/// match every measured image, and look like a filter that works.
fn count(value: &str, token: &str, expects: &str) -> Result<i64, ParseError> {
    let parsed = value.trim().parse::<i64>().ok().filter(|n| *n >= 0);
    parsed.ok_or_else(|| bad(token, format!("expects {expects}")))
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
        // Checked, not saturating: `saturating_mul(1000)` would silently accept a
        // length the guard above just refused, and a comparison against
        // `i64::MAX` milliseconds matches nothing while looking like a filter.
        return total
            .checked_mul(1_000)
            .ok_or_else(|| bad(token, "that length is too large to compare against"));
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
    let ms = amount * seconds_each * 1000.0;
    // Same rule as the colon form and as `byte_size`: a number this big is a
    // typo, and `as i64` on a float that does not fit saturates to `i64::MAX`,
    // which is a query that answers "nothing" forever without saying so.
    if ms > i64::MAX as f64 {
        return Err(bad(token, "that length is too large to compare against"));
    }
    Ok(ms as i64)
}

/// A date or datetime, converted here to epoch seconds. `2026-11-01`,
/// `2026-11-01T14:30`, `2026-11-01T14:30:15`, and a bare epoch integer. A day is
/// a half-open interval, never an exact second (review requirement: an equality
/// against a second-resolution timestamp is a query that matches nothing).
///
/// Two bounds keep this a parser and not a panic:
///
/// * the year is **four digits**, because `added:11-01-02` is a typo for a date
///   and not the year 11 CE, and because the arithmetic below is only safe for a
///   year in `1..=9999`;
/// * every multiply and add is **checked**, because a 12-digit year used to reach
///   `days_from_civil(300000000000, 1, 1) * 86_400`, which overflows in a test
///   build as a panic and in a release build as a wrapped epoch that can match
///   every item in the library.
fn epoch(value: &str, token: &str) -> Result<DateValue, ParseError> {
    let text = value.trim();
    let expect = "a date: added:2026-11-01, added:>=2026-11-01, updated:<2026-11-01T14:30 \
                  (a day is a UTC day)";
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) && text.len() >= 5 {
        // A bare number is an epoch, and only a plausible one: everything this
        // grammar can name as a date ends at the year 9999.
        if text.len() > 12 {
            return Err(bad(token, format!("expects {expect}")));
        }
        return text
            .parse::<i64>()
            .ok()
            .filter(|secs| *secs <= LAST_DATE_EPOCH)
            .map(DateValue::instant)
            .ok_or_else(|| bad(token, format!("expects {expect}")));
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
    if bits[0].len() != 4 {
        return Err(bad(
            token,
            "a year is written in full, as in added:2026-11-01 (a day is a UTC day)",
        ));
    }
    let year = bits[0].parse::<i64>().expect("four digits");
    let month = bits[1].parse::<i64>().expect("digits");
    let day = bits[2].parse::<i64>().expect("digits");
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return Err(bad(
            token,
            format!("`{date_part}` is not a date on the calendar (check the month and day)"),
        ));
    }
    let mut secs = match days_from_civil(year, month, day).checked_mul(86_400) {
        Some(secs) => secs,
        None => return Err(bad(token, "that date is too far away to compare against")),
    };
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
        let clock = numbers[0] * 3600 + numbers[1] * 60 + numbers.get(2).copied().unwrap_or(0);
        secs = match secs.checked_add(clock) {
            Some(secs) => secs,
            None => return Err(bad(token, "that date is too far away to compare against")),
        };
        Ok(DateValue::instant(secs))
    } else {
        Ok(DateValue::day(secs))
    }
}

/// The last epoch this grammar will name: `9999-12-31T23:59:59`. A bare epoch
/// above it is a typo, and bounding it is what keeps the day-window arithmetic
/// (`from + 86_400`) inside `i64` for every date that parses.
const LAST_DATE_EPOCH: i64 = 253_402_300_799;

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
///
/// Every media predicate is `EXISTS`-shaped, never a `JOIN media`: one item has
/// several media rows (`thumb`, `full`, `video`), so a join would multiply both
/// the grid's rows and the paired `COUNT(*)` the count line prints.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// Empty means "no predicate at all", i.e. the whole library.
    pub sql: String,
    pub params: Vec<Value>,
    /// The facts this query reads that a vault may never have measured — see
    /// [`FactKey`]. Empty means every predicate is answerable from data the vault
    /// already holds.
    pub unmeasured: Vec<FactKey>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.sql.is_empty()
    }

    /// True when some predicate in this query reads a column nobody has measured
    /// yet, so a `0 matches` would be an answer the vault cannot honestly give.
    /// The caller decides what to say — stage S5 renders "not measured yet" plus
    /// a *measure my files* affordance, never a bare zero.
    pub fn needs_facts(&self) -> bool {
        !self.unmeasured.is_empty()
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

/// A fact a query reads that a vault may not hold yet. This is the planner
/// reporting on **its own fragments**, not the UI guessing which words happen to
/// be probe-dependent: the same honesty as #38's three-state verdict and #65's
/// `? px` instead of a size nobody read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactKey {
    /// `media.width/height` and `sources.width/height` — read by `width:`,
    /// `height:` and `ratio:`.
    Pixels,
    /// `media.animated` — read by `type:image` and `type:gif`.
    Motion,
    /// `media.duration_ms` and `sources.duration_ms` — read by `length:`.
    Duration,
    /// `media.bytes` is measured for every vaulted original (the v10 migration
    /// backfilled it as `length(data)`), but the disk/link side of `size>` is
    /// `sources.bytes`, which the facts job's stat pass writes.
    CachedBytes,
    /// The `extension` tag that `ext:` and half of `type:video` read. Not a probe:
    /// the writers record it (stages S4/S7, #77), so a vault from before then
    /// simply has none.
    Extension,
}

/// The facts job's own progress question, in one read that never counts rows:
/// is any stored original still un-measured? It is `refine_stored_facts`'s own
/// `WHERE`, so "no row" means the job has nothing left to do — and "a row" means
/// there is a *Measure my files* job to run. Note the half this job cannot do
/// yet: the duration of a vaulted `video` row comes from the media probe stage S7
/// adds, which is why `FactKey::Duration` also answers to the source probe below.
pub const FACTS_PENDING_SQL: &str =
    "SELECT 1 FROM media WHERE kind = 'full' AND probed_at IS NULL LIMIT 1";

/// Has any source ever been measured at all? `sources.bytes` / `duration_ms` /
/// `width` / `height` are the cache for disk files and bare links, and nothing
/// writes them before stage S7, so `size>` on an on-disk-only item is unknown.
pub const SOURCE_FACTS_SEEN_SQL: &str = "SELECT 1 FROM sources WHERE probed_at IS NOT NULL LIMIT 1";

/// Does this vault record file extensions anywhere? `ext:` and the extension arm
/// of `type:video` read `tags` of the `extension` type, which no writer produced
/// before stages S4/S7 (#77), so an old vault answers `ext:gif` with "nothing"
/// truthfully and uselessly — the UI has to say which.
pub const EXTENSION_TAGS_SEEN_SQL: &str = "SELECT 1 FROM tags AS t \
     JOIN tag_types AS tt ON tt.id = t.type_id WHERE tt.key = 'extension' LIMIT 1";

/// Which terms plan their own negation instead of being wrapped in `NOT (…)`:
/// the fact predicates, where `NOT (bytes > ?)` is true for a fact nobody ever
/// measured. Everything else — tags, sources, `has:`, `stored:`, status, circle,
/// title, dates (both stamp columns are `NOT NULL`) — is honest as a complement.
fn self_negates(node: &Node) -> bool {
    matches!(
        node,
        Node::Type { .. }
            | Node::Size { .. }
            | Node::Length { .. }
            | Node::Width { .. }
            | Node::Height { .. }
            | Node::Ratio { .. }
    )
}

/// Which un-measured facts one term reads, added to the plan's list without
/// duplicates.
fn fact_keys(node: &Node, out: &mut Vec<FactKey>) {
    let keys: &[FactKey] = match node {
        Node::Width { .. } | Node::Height { .. } | Node::Ratio { .. } => &[FactKey::Pixels],
        Node::Length { .. } => &[FactKey::Duration],
        Node::Type { kind } => match kind {
            TypeKind::Video => &[FactKey::Extension],
            TypeKind::Image | TypeKind::Gif => &[FactKey::Motion],
        },
        Node::Ext { .. } => &[FactKey::Extension],
        Node::Size { .. } => &[FactKey::CachedBytes],
        _ => &[],
    };
    for key in keys {
        if !out.contains(key) {
            out.push(*key);
        }
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

/// The vault row that says "a still picture": an original whose container said it
/// does not move. `animated = 0` is a measurement, not an absence, so it never
/// matches a NULL (invariant 27).
const STILL_IMAGE: &str = "EXISTS (SELECT 1 FROM media AS m WHERE m.item_id = items.id \
                           AND m.kind = 'full' AND m.animated = 0)";

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

/// The SQL for one term.
///
/// `negated` is decided by the caller, and the two kinds of term answer it
/// differently:
///
/// * A **tag / source / existence / date / status** term is honest as `NOT (…)`
///   or `NOT EXISTS (…)`: the row is either there or it is not, and `created_at`
///   / `updated_at` are `NOT NULL` columns, so the complement is a real set.
/// * A **fact** term (`type:`, `size`, `length`, `width`, `height`, `ratio`)
///   negates by planning the *contrary measurement* instead ([`Cmp::opposite_sql`]),
///   because `NOT (bytes > ?)` is true for a fact nobody ever measured.
///   [`self_negates`] is the single place that rule is stated, and it is why
///   `-type:gif` on an un-probed vault answers "nothing" rather than "everything".
fn fragment(node: &Node, negated: bool, params: &mut Vec<Value>) -> String {
    // A negated *fact* keeps the columns and flips the comparison; a negated
    // anything else keeps the operator and is wrapped in NOT by the caller.
    let fact_op = |cmp: Cmp| -> &'static str {
        if negated {
            cmp.opposite_sql()
        } else {
            cmp.sql()
        }
    };
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
                if negated {
                    // `-type:video` is "the vault row says still picture", not
                    // "everything except a video": an on-disk file whose extension
                    // nobody recorded has no answer, and an unknown may not be
                    // reported as a fact (invariant 27).
                    STILL_IMAGE.to_string()
                } else {
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
            }
            TypeKind::Image => {
                if negated {
                    // The contrary of a measured still is a measured moving picture
                    // or a vault video row, never "a row we never asked".
                    "EXISTS (SELECT 1 FROM media AS m WHERE m.item_id = items.id \
                     AND m.kind IN ('full','video') \
                     AND (m.kind = 'video' OR m.animated = 1))"
                        .to_string()
                } else {
                    STILL_IMAGE.to_string()
                }
            }
            // `animated` is the container's own answer, so both directions are a
            // comparison against 0 or 1 and NULL (never asked) answers neither.
            TypeKind::Gif => {
                let answer = if negated { "= 0" } else { "= 1" };
                format!(
                    "EXISTS (SELECT 1 FROM media AS m WHERE m.item_id = items.id \
                     AND {ORIGINAL_MEDIA} AND m.animated {answer})"
                )
            }
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
            StoredShape::LocalOnly => {
                // Word-for-word the old #40 `local_only` badge (`db.rs`'s list
                // query and `source_stats`): a local source **and** no source from
                // any other site. The aliases are the rail's own, so a reviewer
                // diffing the two sees the identity; `presets_are_the_old_rails_predicate`
                // is what keeps it true after the rails are deleted in S5.
                "(EXISTS (SELECT 1 FROM sources AS sl WHERE sl.item_id = items.id \
                  AND sl.site = 'local') \
                  AND NOT EXISTS (SELECT 1 FROM sources AS so WHERE so.item_id = items.id \
                  AND so.site <> 'local'))"
                    .to_string()
            }
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
        Node::Size { cmp, bytes } => fact_pair(
            "m.bytes",
            "s.bytes",
            fact_op(*cmp),
            Value::Int(*bytes),
            params,
        ),
        Node::Length { cmp, ms } => fact_pair(
            "m.duration_ms",
            "s.duration_ms",
            fact_op(*cmp),
            Value::Int(*ms),
            params,
        ),
        Node::Width { cmp, px } => {
            fact_pair("m.width", "s.width", fact_op(*cmp), Value::Int(*px), params)
        }
        Node::Height { cmp, px } => fact_pair(
            "m.height",
            "s.height",
            fact_op(*cmp),
            Value::Int(*px),
            params,
        ),
        Node::Ratio { cmp, ratio } => {
            // Symmetric with `width`/`height`: the same two places a pixel size
            // can be recorded, so an item measured only on its disk/link source
            // answers `ratio:>2` as honestly as it answers `width>1200`.
            let bound = number(*ratio);
            params.push(bound.clone());
            params.push(bound);
            let mguard = ratio_guard("m");
            let sguard = ratio_guard("s");
            format!(
                "(EXISTS (SELECT 1 FROM media AS m WHERE m.item_id = items.id AND \
                 {ORIGINAL_MEDIA} AND {mguard} AND CAST(m.width AS REAL) / m.height {} ?) OR \
                 EXISTS (SELECT 1 FROM sources AS s WHERE s.item_id = items.id AND {sguard} \
                 AND CAST(s.width AS REAL) / s.height {} ?))",
                fact_op(*cmp),
                fact_op(*cmp)
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

/// The guards a ratio needs before it divides: both sides measured, and a height
/// that is not zero. An unmeasured picture has no ratio and must not match, and a
/// zero height is not a divide-by-zero invitation (invariant 27).
fn ratio_guard(alias: &str) -> String {
    format!("{alias}.width IS NOT NULL AND {alias}.height IS NOT NULL AND {alias}.height > 0")
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
        "id:c-1f9a2b7c11d4e5a6b7c8",
        "added:2026-11-01 updated:>=2026-11-01T14:30",
        "-size>1gb type:gif",
        "stored:link -source:local type:video length:>90s",
        "type:image -has:tags size<2mb width>1200 ratio:<1",
        "added:>2026-01-01 added:<2027-01-01 has:thumb",
        "stored:local-only",
        "stored:disk -stored:local-only",
        "stored:link -stored:disk",
        "-type:video length<30s",
        "rating=3",
        "circle:renpy -rating:>3",
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

    // ---- the review pass: values never travel as SQL text -------------------

    /// Every fragment of planned SQL that legitimately contains a digit. If a new
    /// arm of the planner wants a number in its text, it has to appear here - which
    /// is the point: a value reaching the SQL text instead of a parameter is the one
    /// mistake this module must never make.
    const CODE_OWNED_DIGITS: &[&str] = &[
        "SELECT 1 FROM",
        "animated = 0",
        "animated = 1",
        "height > 0",
    ];

    /// The one string the planner is allowed to write into SQL text: `local` is a
    /// code-owned site slug (DEVELOPMENT §4 invariant 10), and the `stored:` shapes
    /// read it exactly as the #40 rails did.
    const CODE_OWNED_LITERALS: &[&str] = &["local"];
    fn strip_code_owned_digits(sql: &str) -> String {
        let mut out = sql.to_string();
        for frag in CODE_OWNED_DIGITS {
            out = out.replace(frag, "<code>");
        }
        out
    }

    /// The placeholder-count test cannot catch a fragment that interpolated the
    /// value instead of binding it (0 `?` and 0 parameters would pass it), so this
    /// pins the other half: the only digits in planned SQL are the code-owned ones
    /// named by `CODE_OWNED_DIGITS`, and no bound value's own text may show up.
    #[test]
    fn a_value_is_bound_and_never_written_into_the_sql() {
        for query in WORKED {
            let plan = media(query).plan();
            // Only three kinds of digit may appear in planned SQL at all, and
            // every one of them is code-owned text: the `1` of an `EXISTS
            // (SELECT 1 …)`, the `0`/`1` an `animated` comparison is written
            // against, and `ratio`'s zero-divisor guard. Any other digit is a
            // typed value that skipped its parameter.
            let clean = strip_code_owned_digits(&plan.sql);
            assert!(
                !clean.bytes().any(|b| b.is_ascii_digit()),
                "{query:?} put a number in the SQL text instead of a parameter: {}",
                plan.sql
            );
            for value in &plan.params {
                let text = match value {
                    Value::Text(s) => s.clone(),
                    Value::Int(i) => i.to_string(),
                    Value::Real(f) => f.to_string(),
                };
                if text.is_empty() || CODE_OWNED_LITERALS.contains(&text.as_str()) {
                    // The `local` site slug appears as SQL text in the storage
                    // shapes, word-for-word as the #40 rails wrote it: it is part
                    // of the derivation, not a value the box supplied.
                    continue;
                }
                // A number is looked for in the SQL with the code-owned digits
                // removed, so a bound `1` is not confused with the `1` of a
                // `SELECT 1`; a string has no such collision to allow for.
                let haystack = if matches!(value, Value::Text(_)) {
                    plan.sql.as_str()
                } else {
                    clean.as_str()
                };
                assert!(
                    !haystack.contains(&text),
                    "{query:?} interpolated {text:?} into: {}",
                    plan.sql
                );
            }
        }
    }

    /// SQL text a user types is data. It goes to SQLite as one bound parameter,
    /// and the library is still there afterwards.
    #[test]
    fn a_quoted_phrase_cannot_reach_the_sql_text() {
        let query = media("\"'); DROP TABLE items;--\"");
        let plan = query.plan();
        assert!(
            !plan.sql.to_ascii_uppercase().contains("DROP"),
            "{}",
            plan.sql
        );
        // The phrase travels as a bound LIKE pattern - twice, for title and
        // circle - and never as SQL text.
        assert_eq!(
            plan.params,
            vec![
                Value::Text("%'); DROP TABLE items;--%".into()),
                Value::Text("%'); DROP TABLE items;--%".into()),
            ]
        );
        let conn = seeded_library();
        let before = item_count(&conn);
        let read = run_plan(&conn, "SELECT items.id FROM items", "", &plan);
        assert!(
            read.is_empty(),
            "{read:?} matched a phrase that is not a title"
        );
        // And the library is still there: the text never became a statement.
        assert_eq!(item_count(&conn), before, "the library survived it");
    }

    // ---- the review pass: a parse error is a value, never a panic -----------

    /// The plan §4 EBNF allows `key=value` with no colon, and a user types it.
    /// What must not change is that a keyword run that merely *looks* like a
    /// keyword is still a tag name: `size-ish` and `id#3` are literal.
    #[test]
    fn an_equals_sign_makes_a_predicate_and_a_dash_does_not() {
        assert_eq!(
            nodes("rating=3"),
            vec![Node::Rating {
                cmp: Cmp::Eq,
                value: 3.0
            }],
            "`rating=3` is a predicate, not a tag named `rating=3` that matches nothing"
        );
        assert_eq!(
            nodes("type=video"),
            vec![Node::Type {
                kind: TypeKind::Video
            }]
        );
        assert_eq!(
            nodes("added=2026-11-01"),
            vec![Node::Added {
                cmp: Cmp::Eq,
                at: DateValue {
                    from: 1_793_491_200,
                    to: 1_793_577_600
                }
            }]
        );
        for token in ["size-ish", "id#3", "taggart", "added-daily"] {
            assert!(
                matches!(nodes(token)[0], Node::Tag { .. }),
                "{token:?} has no '=' and no operator, so it stays a tag: {:?}",
                nodes(token)[0]
            );
        }
    }

    /// A 12-digit year used to reach `days_from_civil(...) * 86_400` and overflow:
    /// a panic under `cargo test`, and in release a wrapped epoch that matches
    /// every item in the library. A two-digit year is the same class of trap:
    /// `added:11-01-02` is a typo, not the year 11 CE.
    #[test]
    fn no_date_can_overflow_the_epoch_arithmetic() {
        for value in [
            "added:300000000000-01-01",
            "added:999999999999-12-31",
            "added:11-01-02",
            "added:1-2-3",
            "updated:17934912009999999999",
            "added:2026-11-01T99:99",
        ] {
            let e = error(value, Scope::Media);
            assert!(
                e.message.contains("date")
                    || e.message.contains("year")
                    || e.message.contains("time")
                    || e.message.contains("far away"),
                "{value:?} said: {}",
                e.message
            );
        }
        // The top of the legal range still works, and so does a real epoch.
        assert!(
            matches!(nodes("added:9999-12-31")[0], Node::Added { .. }),
            "year 9999 is the last one the grammar names"
        );
        assert!(matches!(nodes("added:253402282800")[0], Node::Added { .. }));
    }

    #[test]
    fn a_count_or_a_length_out_of_range_is_refused_not_guessed() {
        // `width>-100` parses today and matches every measured image.
        for value in ["width>-100", "height>-1", "size>-5mb", "rating:>-2"] {
            let e = error(value, Scope::Media);
            assert!(
                e.message.contains("expects") || e.message.contains("e.g."),
                "{value:?} said: {}",
                e.message
            );
        }
        // A length this large is a typo; saturating it to `i64::MAX` would make a
        // predicate that answers "nothing" for the rest of the vault's life.
        for value in [
            "length>9999999999999999h",
            "length>999999999999999999999s",
            "length>999999999999999:00",
        ] {
            let e = error(value, Scope::Media);
            assert!(
                e.message.contains("too large") || e.message.contains("a length:"),
                "{value:?} said: {}",
                e.message
            );
        }
        assert!(matches!(
            nodes("width=0")[0],
            Node::Width {
                cmp: Cmp::Eq,
                px: 0
            }
        ));
    }

    #[test]
    fn invisible_and_lookalike_characters_are_refused() {
        // Invisible: the term looks like two tags and is one that matches nothing.
        for (query, what) in [
            ("big_breasts\u{200b}anal", "U+200B"),
            ("anal\u{200f}", "U+200F"),
            ("type:vi\u{202a}deo", "U+202A"),
            ("stored:vau\u{feff}lt", "U+FEFF"),
        ] {
            let e = error(query, Scope::Media);
            assert!(
                e.message.contains(what) && e.message.contains("invisible"),
                "{query:?} said: {}",
                e.message
            );
        }
        // A dash that is not `-`: `-anal` typed with an en dash or a minus sign is
        // not a negation, and would become a tag name that matches nothing.
        for query in ["\u{2013}anal", "-anal\u{2013}x", "type\u{ff1a}video"] {
            let e = error(query, Scope::Media);
            assert!(
                e.message.contains("that is not the") || e.message.contains("invisible"),
                "{query:?} said: {}",
                e.message
            );
        }
        // NBSP splits terms, because `char::is_whitespace` says so and a paste
        // from a web page should behave like a space, not like a hidden byte
        // inside a tag name.
        assert_eq!(
            nodes("big_breasts\u{a0}anal"),
            vec![
                Node::Tag {
                    name: "big_breasts".into()
                },
                Node::Tag {
                    name: "anal".into()
                }
            ]
        );
        // Inside quotes the text is data: a scraped title really can hold a soft
        // hyphen, and the one place to ask for it verbatim has to stay open.
        assert!(matches!(nodes("\"\u{2013}anal\"")[0], Node::Title { .. }));
        // Non-ASCII that is not invisible is a legitimate tag name (this library
        // has Japanese tags), so it stays legal.
        assert_eq!(
            nodes("東方"),
            vec![Node::Tag {
                name: "東方".into()
            }]
        );
    }

    #[test]
    fn empty_quotes_search_for_nothing_and_are_refused() {
        // `""` plans as `LIKE '%%'`, which is the whole library - the opposite of
        // what an empty pair of quotes looks like it asks for.
        for query in ["\"\"", "\" \"", "circle:\"\t\""] {
            let e = error(query, Scope::Media);
            assert!(!e.message.is_empty(), "{query:?} gave no message");
        }
        assert!(!media("\"a\"").plan().sql.is_empty());
    }

    #[test]
    fn a_quote_that_opens_mid_query_names_itself() {
        let e = error("type:video \"open", Scope::Media);
        assert_eq!(e.token, "\"open");
        assert!(e.message.contains("never closes"), "{}", e.message);
    }

    #[test]
    fn one_box_holds_a_query_not_an_argument_list() {
        // Past this, SQLite would answer with a prepare error; plan §5 promises the
        // form state instead, so the grammar has to stop it first.
        let too_many_terms = vec!["big_breasts"; MAX_TERMS + 1].join(" ");
        let e = error(&too_many_terms, Scope::Media);
        assert!(e.message.contains("terms"), "{}", e.message);
        let long_input = format!("\"{}\"", "z".repeat(MAX_INPUT_CHARS + 10));
        let e = error(&long_input, Scope::Media);
        assert!(e.message.contains("characters"), "{}", e.message);
        // Exactly at the limit still parses.
        let at_limit = vec!["big_breasts"; MAX_TERMS].join(" ");
        assert_eq!(
            parse(&at_limit, Scope::Media).unwrap().terms.len(),
            MAX_TERMS
        );
    }

    // ---- the review pass: two documents, one vocabulary ---------------------

    /// Plan §1 says Media never gets `circle:`/`rating:`; plan §4's vocabulary
    /// table says both views do. §4 wins, against the schema: `items.circle` and
    /// `items.rating` are columns every item row has, and an e-hentai gallery
    /// genuinely has a circle. Rejecting the key would hide a fact that exists.
    #[test]
    fn circle_and_rating_are_valid_in_both_views_because_items_carry_them() {
        for query in ["circle:renpy", "rating:>3", "circle:\"Red Sun\" rating=4"] {
            assert!(
                parse(query, Scope::Media).is_ok(),
                "media view must accept {query:?}"
            );
            assert!(
                parse(query, Scope::Games).is_ok(),
                "games view must accept {query:?}"
            );
        }
        // And the columns really are on the shared table, which is the reason.
        assert_eq!(
            nodes("circle:renpy"),
            vec![Node::Circle {
                phrase: "renpy".into()
            }]
        );
    }

    #[test]
    fn status_is_games_only_and_says_which_words_this_view_has() {
        let e = error("status:wishlist", Scope::Media);
        assert!(e.message.contains("Games"), "{}", e.message);
        // The hint is generated from `keywords(Scope::Media)`, so it cannot go
        // stale when the set changes: everything this view knows is in the message.
        for key in keywords(Scope::Media) {
            assert!(
                e.message.contains(&format!("{key}:")),
                "the hint lost {key:?}: {}",
                e.message
            );
        }
        assert!(!keyword_hint(Scope::Media).contains("status:"));
        assert!(parse("status:wishlist", Scope::Games).is_ok());
    }

    // ---- the review pass: negation, NULL, and the old badges ----------------

    /// B3: a negated **fact** has to require the measurement to exist. `NOT
    /// (animated = 1)` is true for every item the facts job has not touched, so
    /// "the non-gifs" would claim to be a set of contents nobody has read. A
    /// negated derived/existence predicate (`-has:tags`, `-stored:disk`, `-tag`)
    /// has no unknown, so it stays a plain complement.
    #[test]
    fn a_negated_fact_never_counts_an_unmeasured_item() {
        let gif = media("-type:gif").plan().sql;
        assert!(gif.contains("m.animated = 0"), "{gif}");
        assert!(!gif.contains("NOT ("), "{gif}");
        let size = media("-size>1gb").plan().sql;
        assert!(size.contains("m.bytes <= ?"), "{size}");
        assert!(size.contains("s.bytes <= ?"), "{size}");
        assert!(!size.contains("NOT ("), "{size}");
        // A negated fact and its positive read the same two places.
        for (positive, negative) in [
            ("width>1200", "-width>1200"),
            ("length<1:30", "-length<1:30"),
            ("ratio:>2", "-ratio:<2"),
        ] {
            let up = media(positive).plan().sql;
            let down = media(negative).plan().sql;
            for needle in ["FROM media AS m", "FROM sources AS s"] {
                assert!(
                    up.contains(needle) && down.contains(needle),
                    "{positive} / {negative} read different tables: {up} vs {down}"
                );
            }
        }
        // The existence predicates keep their complement, because there is no
        // unknown behind them.
        assert!(
            media("-has:tags").plan().sql.starts_with("NOT EXISTS"),
            "{}",
            media("-has:tags").plan().sql
        );
        assert!(
            media("-stored:disk").plan().sql.starts_with("NOT EXISTS"),
            "{}",
            media("-stored:disk").plan().sql
        );

        // And settled against rows, which is the part an AST assertion cannot do:
        // an item with no measurement anywhere answers neither `size>1gb` nor
        // `-size>1gb`. The two that do answer are the measured ones.
        let conn = seeded_library();
        assert_eq!(matched(&conn, "size>1gb"), ["i_huge"]);
        assert_eq!(
            matched(&conn, "-size>1gb"),
            ["i_diskonly", "i_small"],
            "i_site_only, i_local_only and i_both_sites have no size at all"
        );
        assert_eq!(matched(&conn, "type:gif"), ["i_huge"]);
        assert_eq!(matched(&conn, "-type:gif"), ["i_small"]);
        assert_eq!(
            matched(&conn, "ratio:>2"),
            ["i_diskonly", "i_small"],
            "ratio reads the sources cache too (review S6): i_diskonly has pixels only there"
        );
        assert_eq!(
            matched(&conn, "-ratio:>2"),
            ["i_huge"],
            "and its negation only claims what was measured"
        );
    }

    /// The planner reports which facts its own fragments read, so the empty state
    /// can say "not measured yet" instead of presenting a zero as an answer.
    #[test]
    fn a_query_that_reads_unmeasured_columns_reports_itself() {
        assert!(!media("big_breasts").plan().needs_facts());
        assert_eq!(media("type:gif").plan().unmeasured, vec![FactKey::Motion]);
        assert_eq!(media("width>1200").plan().unmeasured, vec![FactKey::Pixels]);
        assert_eq!(
            media("length>1m").plan().unmeasured,
            vec![FactKey::Duration]
        );
        assert_eq!(media("ext:gif").plan().unmeasured, vec![FactKey::Extension]);
        assert_eq!(
            media("size>1mb").plan().unmeasured,
            vec![FactKey::CachedBytes]
        );
        // A negated fact needs the measurement just as much.
        assert_eq!(media("-type:gif").plan().unmeasured, vec![FactKey::Motion]);
        // One entry per key, whatever the order it was typed in.
        let all = media("ratio:>2 width>1 length>1s ext:gif size>1kb type:image").plan();
        for key in [
            FactKey::Pixels,
            FactKey::Motion,
            FactKey::Duration,
            FactKey::CachedBytes,
            FactKey::Extension,
        ] {
            assert!(
                all.unmeasured.contains(&key),
                "lost {key:?}: {:?}",
                all.unmeasured
            );
        }
        assert_eq!(all.unmeasured.len(), 5, "{:?}", all.unmeasured);
    }

    /// The three #40 queues have to map to grammar text exactly, or a preset chip
    /// silently widens the set it selects - which is what rulings C forbids. Pinned
    /// against the rails' own SQL, so the badge count and the query count agree by
    /// construction and keep agreeing after S5 deletes the rails.
    #[test]
    fn the_old_source_queues_are_the_same_predicate_in_grammar_text() {
        let conn = seeded_library();
        let rail = |source: &str, kind: &str| -> Vec<String> {
            crate::db::list_items(
                &conn,
                &crate::db::ListQuery {
                    limit: 300,
                    kind: Some(kind),
                    source: Some(source),
                    ..Default::default()
                },
            )
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect()
        };
        let mut local_only = rail("local_only", "media");
        local_only.sort();
        assert_eq!(matched(&conn, "stored:local-only"), local_only);
        let mut no_local = rail("no_local", "media");
        no_local.sort();
        assert_eq!(matched(&conn, "-stored:disk"), no_local);
        let mut no_tags = rail("no_tags", "media");
        no_tags.sort();
        assert_eq!(matched(&conn, "-has:tags"), no_tags);
        // `stored:disk` is strictly wider than the old badge: one item has a local
        // source *and* an ehentai page, and the badge excluded it.
        assert_eq!(
            matched(&conn, "stored:disk"),
            ["i_both_sites", "i_diskonly", "i_local_only"],
            "stored:disk must not quietly become the old badge"
        );
        assert_eq!(
            matched(&conn, "stored:local-only"),
            ["i_diskonly", "i_local_only"],
            "a folder import with no site page is the old local-only queue"
        );
        // And the badge count agrees with the query count.
        // And the badge count agrees with the query count, for all three queues:
        // the same predicate counted two ways, not two predicates that happen to
        // agree today.
        let stats = crate::db::source_stats(&conn, Some("media")).unwrap();
        assert_eq!(
            stats.local_only as usize,
            matched(&conn, "stored:local-only").len(),
            "the local-only badge and stored:local-only disagree"
        );
        assert_eq!(
            stats.no_local as usize,
            matched(&conn, "-stored:disk").len(),
            "the no-local badge and -stored:disk disagree"
        );
        assert_eq!(
            stats.no_tags as usize,
            matched(&conn, "-has:tags").len(),
            "the no-tags badge and -has:tags disagree"
        );
    }

    #[test]
    fn stored_local_only_is_the_badges_own_sql() {
        let sql = media("stored:local-only").plan().sql;
        assert!(
            sql.contains("EXISTS (SELECT 1 FROM sources AS sl WHERE sl.item_id = items.id AND sl.site = 'local')"),
            "{sql}"
        );
        assert!(
            sql.contains("NOT EXISTS (SELECT 1 FROM sources AS so WHERE so.item_id = items.id AND so.site <> 'local')"),
            "{sql}"
        );
    }

    // ---- helpers the fixture tests share -----------------------------------

    /// Runs one planned query against a seeded vault, ordered by id so the
    /// assertions read as a set.
    fn matched(conn: &rusqlite::Connection, query: &str) -> Vec<String> {
        run_plan(
            conn,
            "SELECT items.id FROM items",
            " ORDER BY items.id",
            &media(query).plan(),
        )
    }

    /// Runs a plan the way stage S3 will: one SELECT, the planned `WHERE`, the
    /// planned parameters, nothing interpolated.
    fn run_plan(
        conn: &rusqlite::Connection,
        select: &str,
        order: &str,
        plan: &Plan,
    ) -> Vec<String> {
        let sql = format!("{select}{}{order}", plan.where_clause());
        let mut stmt = conn.prepare(&sql).unwrap();
        stmt.query_map(rusqlite::params_from_iter(plan.bind()), |r| {
            r.get::<_, String>(0)
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }

    fn item_count(conn: &rusqlite::Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            .unwrap()
    }

    /// A seeded, obviously synthetic vault: six items covering the storage shapes
    /// and the three states of a fact (measured big, measured small, measured only
    /// on the source cache, never measured at all). No id or title here belongs to
    /// a real library.
    fn seeded_library() -> rusqlite::Connection {
        use crate::db;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();
        for id in [
            "i_local_only",
            "i_both_sites",
            "i_site_only",
            "i_huge",
            "i_small",
            "i_diskonly",
        ] {
            db::insert_item(&conn, &fixture_item(id, &format!("Fixture {id}"))).unwrap();
        }
        db::insert_source(
            &conn,
            "i_local_only",
            "local",
            "file:///D:/fixture/a.png",
            None,
            None,
        )
        .unwrap();
        db::insert_source(
            &conn,
            "i_both_sites",
            "local",
            "file:///D:/fixture/b.png",
            None,
            None,
        )
        .unwrap();
        db::insert_source(
            &conn,
            "i_both_sites",
            "ehentai",
            "https://eh.example/fixture/1",
            None,
            None,
        )
        .unwrap();
        db::insert_source(
            &conn,
            "i_diskonly",
            "local",
            "file:///D:/fixture/d.png",
            None,
            None,
        )
        .unwrap();
        for (id, url) in [
            ("i_site_only", "https://eh.example/fixture/2"),
            ("i_huge", "https://eh.example/fixture/3"),
            ("i_small", "https://eh.example/fixture/4"),
        ] {
            db::insert_source(&conn, id, "ehentai", url, None, None).unwrap();
        }
        // The fact columns say what was measured and when. A row with NULL facts is
        // a row nobody ever asked, and it must answer neither a fact predicate nor
        // its negation (invariant 27). Scoped, because a prepared statement borrows
        // the connection and the function hands the connection back.
        {
            let mut media = conn
                .prepare(
                    "INSERT INTO media (item_id, kind, hash, mime, width, height, data,
                                        bytes, duration_ms, animated, probed_at)
                     VALUES (?1, 'full', ?2, 'image/png', ?3, ?4, x'89504e470d0a1a0a',
                            ?5, ?6, ?7, ?8)",
                )
                .unwrap();
            // measured big, and the container says it moves
            media
                .execute(rusqlite::params![
                    "i_huge",
                    "fixture-hash-huge",
                    400,
                    300,
                    5_000_000_000_i64,
                    1,
                    1,
                    "1760000000"
                ])
                .unwrap();
            // measured small, and still
            media
                .execute(rusqlite::params![
                    "i_small",
                    "fixture-hash-small",
                    1_200,
                    400,
                    1_024_i64,
                    None::<i64>,
                    0,
                    "1760000000"
                ])
                .unwrap();
        }
        // a source measured on disk (the cache the #65 probe and the facts job
        // write), with no vault row behind it: `width`/`ratio` must both read it.
        conn.execute(
            "UPDATE sources SET bytes = 4096, width = 3000, height = 1000,
                     probed_at = '1760000000' WHERE item_id = 'i_diskonly'",
            [],
        )
        .unwrap();
        db::add_item_tag(&conn, "i_huge", "fixture_content", db::TYPE_CONTENT).unwrap();
        conn
    }

    fn fixture_item(id: &str, title: &str) -> crate::model::Item {
        crate::model::Item {
            id: id.to_string(),
            kind: crate::model::ItemKind::Gallery,
            title: title.to_string(),
            original_title: None,
            circle: Some("Fixture Circle".to_string()),
            status: crate::model::Status::Seen,
            dnf: false,
            rating: None,
            notes: String::new(),
            version: None,
            created_at: "1760000000".to_string(),
            updated_at: "1760000000".to_string(),
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
                "id" => "c-1f9a2b7c11d4e5a6b7c8",
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
        // The decimal written out, not a product, because that is the number the
        // ruling asked to pin: `size>100mb` means the 104 857 600 the grid and
        // Explorer print beside the same file, not the 100 000 000 a hard-drive
        // advertisement would call 100 MB.
        assert_eq!(
            nodes("size>100mb"),
            vec![Node::Size {
                cmp: Cmp::Gt,
                bytes: 104_857_600
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
        // tile and lied. Every fact predicate - and every negation of one - reads
        // originals only.
        for query in [
            "size>1mb",
            "-size>1gb",
            "width>1200",
            "height>800",
            "ratio:>2",
            "length>1m",
            "stored:vault",
            "type:gif",
            "-type:gif",
            "type:image",
            "-type:image",
        ] {
            let sql = media(query).plan().sql;
            assert!(
                sql.contains("m.kind IN ('full','video')")
                    || sql.contains("m.kind = 'full'")
                    || sql.contains("m.kind = 'video'"),
                "{query:?} can measure a thumb row: {sql}"
            );
            assert!(!sql.contains("'thumb'"), "{query:?} mentions a tile: {sql}");
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
            (
                "-stored:local-only",
                "NOT ((EXISTS (SELECT 1 FROM sources AS sl",
            ),
            // A negated *fact* is not a NOT: it plans the contrary measurement,
            // so an un-measured item cannot answer it (review B3).
            ("-size>1gb", "m.bytes <= ?"),
            ("-width<400", "m.width >= ?"),
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
    fn an_id_predicate_is_exact_and_knows_the_real_id_shape() {
        // The plan sketched `id:<ulid>`; the code mints `new_id(prefix)` =
        // `c-`/`g-`/`i-`/`v-`/`b-` plus 20 hex digits, so the hyphen is part of
        // every id in a real vault and a validator that forbade it would make
        // `id:` useless.
        let plan = media("id:c-1f9a2b7c11d4e5a6b7c8").plan();
        assert_eq!(plan.sql, "items.id = ?");
        assert_eq!(
            plan.params,
            vec![Value::Text("c-1f9a2b7c11d4e5a6b7c8".into())]
        );
        // An id that does not exist is a query that matches nothing, not an
        // error; one that cannot be an id at all says so.
        assert!(
            matches!(nodes("id:zz-00")[0], Node::Id { .. }),
            "an unknown id still parses"
        );
        let e = error("id:cl/ip", Scope::Media);
        assert!(
            e.message.contains("c-1f9a2b7c11d4e5a6b7c8"),
            "{}",
            e.message
        );
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
        assert_eq!(
            closed_values("stored"),
            &["vault", "disk", "local-only", "link"]
        );
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

    // ---------- #76 stage S4: the box read **while it is being typed** ---------

    #[test]
    fn the_view_vocabulary_on_the_wire_is_two_words_and_no_others() {
        assert_eq!(Scope::parse_wire("media"), Ok(Scope::Media));
        assert_eq!(Scope::parse_wire("games"), Ok(Scope::Games));
        let e = Scope::parse_wire("gallery").unwrap_err();
        assert_eq!(e.token, "gallery");
        assert!(
            e.message.contains("media") && e.message.contains("games"),
            "a refused word has to be told the two that work: {}",
            e.message
        );
    }

    #[test]
    fn a_token_mid_flight_splits_into_the_query_and_what_is_being_typed() {
        let s = parse_suggestion("type:vi", Scope::Media).unwrap();
        assert_eq!(s.keyword, Some("type"));
        assert_eq!(s.value, "vi");
        assert_eq!(s.token, "type:vi");
        assert!(!s.negated);
        assert!(s.base.is_empty(), "there is nothing before the token");

        // The base is a real query, so the candidate counts can be measured
        // against it: `source:ehentai bi` suggests tags that exist alongside
        // the ehentai rows, not tags from the whole library.
        let s = parse_suggestion("source:ehentai bi", Scope::Media).unwrap();
        assert_eq!(s.keyword, None, "`bi` has no keyword in front of it");
        assert_eq!(s.value, "bi");
        assert_eq!(s.base.terms.len(), 1);
        assert_eq!(s.base.terms[0].text, "source:ehentai");

        // The minus belongs to the term, and it has to survive into the
        // completion: picking a candidate must not silently drop the negation.
        let s = parse_suggestion("-stor", Scope::Media).unwrap();
        assert!(s.negated);
        assert_eq!(
            s.keyword, None,
            "no operator yet, so nothing names the keyword"
        );
        assert_eq!(s.value, "stor");
        let s = parse_suggestion("-has:t", Scope::Media).unwrap();
        assert!(s.negated);
        assert_eq!(s.keyword, Some("has"));
        assert_eq!(s.value, "t");

        // Both operator spellings are the same keyword, so a token cannot be a
        // predicate in the parser and a tag name in the suggester.
        let s = parse_suggestion("size>10", Scope::Media).unwrap();
        assert_eq!(s.keyword, Some("size"));
        assert_eq!(s.value, "10");
        let s = parse_suggestion("size:>10", Scope::Media).unwrap();
        assert_eq!(s.keyword, Some("size"));
        assert_eq!(s.value, "10");
    }

    #[test]
    fn a_colon_inside_a_tag_name_is_not_a_keyword() {
        // `female` is not a keyword, so this token is one tag name and gets
        // tag completions — the same rule that keeps site vocabulary importable
        // (plan §4).
        let s = parse_suggestion("female:big", Scope::Media).unwrap();
        assert_eq!(s.keyword, None);
        assert_eq!(s.value, "female:big");
    }

    #[test]
    fn there_is_nothing_to_suggest_when_there_is_no_partial_token() {
        assert_eq!(parse_suggestion("", Scope::Media), None, "an empty box");
        assert_eq!(
            parse_suggestion("big_breasts ", Scope::Media),
            None,
            "the last term is finished and the next has not started"
        );
        assert_eq!(
            parse_suggestion("\"summer witch\"", Scope::Media),
            None,
            "a closed quoted phrase is a complete free-text term"
        );
        assert_eq!(
            parse_suggestion("circle:\"Red", Scope::Media),
            None,
            "an open quote wants a phrase, and this pass has no phrase candidates"
        );
        assert_eq!(
            parse_suggestion("status:wishlist", Scope::Media),
            None,
            "a Games-only word in the media box is an error, not a completion"
        );
        assert_eq!(
            parse_suggestion("type:banana bi", Scope::Media),
            None,
            "the base has to parse: completing past an error would hide it"
        );
        let s = parse_suggestion("size>banana", Scope::Media).unwrap();
        assert_eq!(s.keyword, Some("size"));
        assert_eq!(s.value, "banana", "and no value set can complete that");
        let bare = parse_suggestion("status", Scope::Media);
        assert!(
            bare.is_some(),
            "a bare `status` is a tag name in this view, so it is completeable"
        );
        assert_eq!(bare.unwrap().value, "status");
    }

    #[test]
    fn the_suggester_only_offers_values_it_can_be_honest_about() {
        assert!(value_completion("type"), "the grammar owns that list");
        assert!(value_completion("stored"));
        assert!(value_completion("source"), "the vault can count those");
        assert!(value_completion("ext"));
        for key in [
            "size", "length", "width", "height", "ratio", "rating", "added", "updated", "circle",
            "id",
        ] {
            assert!(
                !value_completion(key),
                "{key}: would have to invent the number, and an un-measured fact is not a value"
            );
        }
    }
}
