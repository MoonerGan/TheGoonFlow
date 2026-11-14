//! Cookie policy for the login-gated sources (#38): decide at save time whether
//! a pasted cookie is any good, and leave a recorded verdict behind.
//!
//! Before this, a cookie paste produced no feedback at all: it went into the
//! vault and the only signal was some later fetch failing, described in a note
//! about an unrelated item. Now the paste answers immediately ("no xf_session",
//! or "f95 answered as a signed-in member"), and a fetch that later hits the
//! site's login wall marks that cookie stale, so "your cookie died" is something
//! the app knows instead of something you deduce from failed clips.
//!
//! Deliberate limits:
//! * A probe is **one page fetch**, on the same polite lane every other request
//!   uses. Never a search, never a listing crawl: it is the cheapest request
//!   that separates "this session is recognised" from "you are a guest".
//! * Every marker below was measured, on 2026-11-10/11, with the identity this
//!   app actually sends ([`crate::ingest::UA`]) - not with a browser UA, because
//!   a marker that only exists under a different UA would be a lie. Where a
//!   marker is only confirmed from one side, `probe` says so in the note instead
//!   of pretending to be sure.
//! * The cookie itself is written only to `site_settings.cookie` inside the
//!   encrypted vault. Not the pointer file, not a log line, not a response body.

use serde::Serialize;

/// One named yes/no about the paste, shown to the user verbatim.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Check {
    pub name: &'static str,
    pub ok: bool,
}

/// What the app thinks of a cookie for a site.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub site: String,
    /// Structurally usable AND accepted by the site, when a probe was possible.
    pub ok: bool,
    pub checks: Vec<Check>,
    /// None = not probed (paste was rejected first, or the site has no marker we
    /// can trust). Some = what the site answered.
    pub live: Option<bool>,
    pub note: String,
}

/// Sites that ask for a login at all. Anything else must be refused a cookie: a
/// cookie saved for a site that never asked for one is dead weight in the vault,
/// and dead weight in an encrypted vault is a question at a keyboard.
pub fn needs_cookie(site: &str) -> bool {
    matches!(site, "f95" | "ehentai" | "exhentai")
}

/// Spell the site id the way the app stores it. The extension sends the ids from
/// `ingest::KNOWN_SITES`; a human types what the browser bar says. Anything that
/// is not one of the login sites is returned as-lowercased and untouched, because
/// inventing a new id for `hitomi.la` would silently orphan its settings row.
pub fn canonical_site(site: &str) -> String {
    let flat: String = site
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | '.'))
        .collect();
    match flat.as_str() {
        "ehentai" => "ehentai".to_string(),
        "exhentai" => "exhentai".to_string(),
        "f95" | "f95zone" => "f95".to_string(),
        _ => site.trim().to_lowercase(),
    }
}

/// `(must have, nice to have)`. f95: `xf_session` is the session itself;
/// `cf_clearance` is Cloudflare's and its absence is a warning, not a rejection,
/// because a hand-built paste may legitimately lack it. e-hentai/Exhentai share
/// one login (phpBB), so both take the same pair.
fn required(site: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match site {
        "f95" => (&["xf_session"], &["cf_clearance"]),
        "ehentai" | "exhentai" => (&["ipb_member_id", "ipb_pass_hash"], &[]),
        _ => (&[], &[]),
    }
}

/// Read the paste the way the site will receive it: `k=v` pairs, `;`-separated.
/// Tolerates a trailing `;`, stray spaces, and someone pasting the whole
/// `Cookie:` line out of a devtools header.
pub fn pairs(cookie: &str) -> Vec<(&str, &str)> {
    cookie
        .trim()
        .trim_start_matches("Cookie:")
        .split(';')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            let (k, v) = s.split_once('=')?;
            let k = k.trim();
            if k.is_empty() {
                None
            } else {
                Some((k, v.trim()))
            }
        })
        .collect()
}

/// Structural checks, no network. A paste that fails here is never probed:
/// sending a cookie that cannot be a session is a wasted request and an
/// unnecessary one to a site that is allowed to log it.
pub fn structural(site: &str, cookie: &str) -> Verdict {
    let (must, warn) = required(site);
    let have: Vec<&str> = pairs(cookie).iter().map(|(k, _)| *k).collect();
    let checks: Vec<Check> = must
        .iter()
        .chain(warn.iter())
        .map(|k| Check {
            name: k,
            ok: have.contains(k),
        })
        .collect();
    let hard_ok =
        needs_cookie(site) && !cookie.trim().is_empty() && must.iter().all(|k| have.contains(k));
    let soft_missing: Vec<&str> = warn.iter().filter(|k| !have.contains(k)).copied().collect();
    let missing: Vec<&str> = must.iter().filter(|k| !have.contains(k)).copied().collect();
    let note = if !needs_cookie(site) {
        format!("{site} does not ask for a login cookie")
    } else if cookie.trim().is_empty() {
        "nothing was pasted".to_string()
    } else if !hard_ok {
        format!("not a session cookie: missing {}", missing.join(", "))
    } else if !soft_missing.is_empty() {
        format!(
            "usable; no {} - a copy from a logged-in browser normally has it",
            soft_missing.join(", ")
        )
    } else {
        "looks like a real session cookie".to_string()
    };
    Verdict {
        site: site.to_string(),
        ok: hard_ok,
        checks,
        live: None,
        note,
    }
}

/// The cheapest page that separates a recognised session from a guest, with the
/// measurement next to it so marker and URL can never drift apart.
///
/// * f95 `https://f95zone.to/` - measured with and without the user's cookie,
///   both times with this app's own UA: the guest page (157 716 B) contains the
///   header links `Log in` and `Register` exactly once each, the signed-in page
///   (272 053 B) contains neither. Class names were *not* used: the obvious
///   candidate (`p-header-accountMenu`) was absent from both.
/// * e-hentai `https://e-hentai.org/favorites.php` - measured 2026-11-14 on all
///   three sides with this app's own UA: no cookie and a right-shape-but-wrong-
///   hash cookie both answer `302` to `bounce_login.php?b=d&bt=1-6` with an empty
///   body, the real session answers `200` with 11 496 B (same size three times
///   in a row). This URL **replaced** `https://e-hentai.org/g/`, which the 2026
///   site answers with `404` for everybody: cookie, junk cookie and no cookie
///   alike, so it could only ever accuse a good cookie of being stale.
/// * exhentai `https://exhentai.org/favorites.php` - a guest is bounced to
///   `https://forums.e-hentai.org/remoteapi.php?ex=…` with an empty body, which
///   is a clean rejection. Anything else is *not* a clean acceptance: a real
///   cookie, a made-up `ipb_pass_hash` and a real member id with that bad hash
///   all answer `200` with **0 bytes**. So the empty `200` proves only that a
///   cookie was present, never that a session exists, and `judged` returns
///   `None` for it instead of a green light.
pub fn probe_url(site: &str) -> Option<&'static str> {
    match site {
        "f95" => Some("https://f95zone.to/"),
        "ehentai" => Some("https://e-hentai.org/favorites.php"),
        "exhentai" => Some("https://exhentai.org/favorites.php"),
        _ => None,
    }
}

/// Decide from an answer, with no network in sight, so every marker above has a
/// test. `target` is the `Location` header when the site answered a redirect,
/// otherwise the URL that was asked for.
pub fn judged(site: &str, status: u16, target: &str, body: &str) -> Option<bool> {
    match site {
        "f95" => {
            if status != 200 {
                return Some(false);
            }
            // A Cloudflare interstitial or a consent wall still offers to log in,
            // which is exactly when we must not claim a session.
            Some(!(body.contains(">Log in<") || body.contains(">Register<")))
        }
        "ehentai" => {
            // Measured both ways: the login bounce is the guest answer.
            if target.contains("bounce_login") || status == 302 || status == 403 {
                return Some(false);
            }
            (status == 200 && !body.trim().is_empty()).then_some(true)
        }
        "exhentai" => {
            if redirected_to_login(target) || status == 302 || status == 403 {
                return Some(false);
            }
            if status != 200 {
                return Some(false);
            }
            // The empty 200 is the site's answer to *every* cookie from this
            // client, good or junk: nothing here can tell them apart.
            (!body.trim().is_empty()).then_some(true)
        }
        _ => None,
    }
}

fn redirected_to_login(target: &str) -> bool {
    target.contains("forums.e-hentai.org")
}

/// What the site's own answer proved about the paste. Three states because the
/// EH pair genuinely has a middle one (see [`probe_url`]): an answer that fits
/// every cookie is not evidence, and calling it a success is how a dead cookie
/// kept looking alive in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Live {
    Yes,
    No,
    /// The site answered, but the same answer comes back for a good cookie and a
    /// made-up one, so it settles nothing.
    Inconclusive,
}

/// Ask the site once, with an already-built client. Split out from [`probe`] so
/// the live test can drive the markers without a vault.
pub async fn ask(client: &reqwest::Client, site: &str, cookie: &str) -> Result<Live, String> {
    let url = probe_url(site).ok_or_else(|| format!("{site} does not take a cookie"))?;
    let mut req = client.get(url);
    if !cookie.is_empty() {
        req = req.header(reqwest::header::COOKIE, cookie);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("{site} did not answer the cookie check: {e}"))?;
    let status = resp.status().as_u16();
    // Redirects are not followed on purpose: for both EH entries the hop to a
    // login page *is* the answer.
    let target = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or(url)
        .to_string();
    let body = resp.text().await.unwrap_or_default();
    Ok(match judged(site, status, &target, &body) {
        Some(true) => Live::Yes,
        Some(false) => Live::No,
        None => Live::Inconclusive,
    })
}

/// Ask the site once, on the polite lane: same proxy policy, page identity (the
/// honest UA, because this is a page fetch), and it waits its turn behind other
/// traffic to that site like every other request does.
pub async fn probe(core: &crate::Core, site: &str, cookie: &str) -> Result<Live, String> {
    let rate = {
        let inner = core.inner.lock().unwrap();
        match inner.opened.as_ref() {
            Some(o) => {
                goon_core::db::site_rate_ms(&o.conn, site, crate::ingest::default_rate_ms(site))
            }
            None => return Err("vault is locked".into()),
        }
    };
    if !core.queue.reserve(site, rate, || false).await {
        return Err("stopped".into());
    }
    let client = crate::http::probe_client_for(core, 20)?;
    ask(&client, site, cookie).await
}

/// Fold the site's answer into the verdict. The wording matters: "usable, but
/// the site answered as a guest" is the case where a paste looked perfect and
/// still does not work, and an inconclusive answer must not read as a success.
pub fn finish(v: &mut Verdict, live: Result<Live, String>) -> &Verdict {
    match live {
        Ok(Live::Yes) => {
            v.live = Some(true);
            v.note = format!("{}; the site answered as a signed-in member", v.note);
        }
        Ok(Live::No) => {
            v.live = Some(false);
            v.ok = false;
            v.note = format!(
                "{}; but {} answered as a guest - the cookie is stale or copied from a \
                 logged-out window",
                v.note, v.site
            );
        }
        Ok(Live::Inconclusive) => {
            v.live = None;
            v.note = format!(
                "{}; {} answered in a way that cannot tell a good cookie from a bad one, so \
                 only the paste itself was checked",
                v.note, v.site
            );
        }
        Err(e) => {
            v.live = None;
            v.note = format!("{}; not checked against the site ({e})", v.note);
        }
    }
    v
}

/// The whole check, shared by the UI command and the companion route the browser
/// extension uses, so a cookie pasted into the app and a cookie handed over by
/// the extension get the identical verdict and the identical record.
pub async fn check(core: &crate::Core, site: &str, cookie: &str) -> Verdict {
    let mut v = structural(site, cookie);
    if !v.ok {
        return v;
    }
    let live = probe(core, site, cookie).await;
    finish(&mut v, live);
    let mut inner = core.inner.lock().unwrap();
    if let Some(o) = inner.opened.as_mut() {
        let _ = record(&o.conn, site, v.ok, &crate::now_epoch());
    }
    v
}

// ---- recorded verdicts ----------------------------------------------------
// Kept in the vault `meta` table next to the cookie, because the alternative
// (a column per site) would hard-code the site list into the schema. `None`
// means "never checked", which the UI must render as unknown, not as failure.

pub const OK_KEY: &str = "cookie_ok:";
pub const CHECKED_KEY: &str = "cookie_checked_at:";

pub fn ok_key(site: &str) -> String {
    format!("{OK_KEY}{site}")
}

pub fn checked_key(site: &str) -> String {
    format!("{CHECKED_KEY}{site}")
}

pub fn record(
    conn: &rusqlite::Connection,
    site: &str,
    ok: bool,
    checked_at: &str,
) -> rusqlite::Result<()> {
    goon_core::db::set_meta(conn, &ok_key(site), Some(if ok { "1" } else { "0" }))?;
    goon_core::db::set_meta(conn, &checked_key(site), Some(checked_at))
}

/// Forget a verdict (cookie replaced or removed): the next check has to happen
/// again rather than the UI repeating last month's answer.
pub fn forget(conn: &rusqlite::Connection, site: &str) -> rusqlite::Result<()> {
    goon_core::db::set_meta(conn, &ok_key(site), None)?;
    goon_core::db::set_meta(conn, &checked_key(site), None)
}

pub fn recorded(conn: &rusqlite::Connection, site: &str) -> Option<(bool, String)> {
    let ok = goon_core::db::get_meta(conn, &ok_key(site))?;
    let at = goon_core::db::get_meta(conn, &checked_key(site)).unwrap_or_default();
    Some((ok == "1", at))
}

/// A fetch hit the site's login wall while the vault held a cookie for that
/// site: that is the staleness signal. Returns None when no cookie is saved,
/// because then the wall says nothing about a paste we never had.
pub fn note_gate(conn: &rusqlite::Connection, site: &str, checked_at: &str) -> Option<bool> {
    if goon_core::db::site_cookie(conn, site)?.is_empty() {
        return None;
    }
    record(conn, site, false, checked_at).ok()?;
    Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_survives_the_way_people_paste_cookies() {
        let p = pairs("Cookie: xf_user=7; xf_session=abc; cf_clearance=x ;");
        assert_eq!(p.len(), 3);
        assert_eq!(p[1], ("xf_session", "abc"));
    }

    #[test]
    fn an_f95_paste_without_a_session_is_refused_before_any_request() {
        let v = structural("f95", "cf_clearance=x; some_other=1");
        assert!(!v.ok);
        assert_eq!(v.live, None, "nothing should have been probed");
        assert!(
            v.note.contains("xf_session"),
            "the note names what is missing"
        );
        assert!(v.checks.iter().any(|c| c.name == "cf_clearance" && c.ok));
    }

    #[test]
    fn a_real_f95_paste_passes_structurally() {
        let v = structural("f95", "xf_csrf=a; cf_clearance=b; xf_session=c; xf_user=d");
        assert!(v.ok);
        assert_eq!(v.live, None);
    }

    #[test]
    fn a_missing_cloudflare_key_is_a_warning_not_a_refusal() {
        let v = structural("f95", "xf_session=c; xf_user=d");
        assert!(v.ok, "cf_clearance is nice to have");
        assert!(v.note.contains("cf_clearance"));
    }

    #[test]
    fn exhentai_wants_the_shared_login_pair_not_an_f95_cookie() {
        assert!(!structural("exhentai", "xf_session=c; xf_user=d").ok);
        assert!(structural("ehentai", "ipb_member_id=1; ipb_pass_hash=beef").ok);
    }

    #[test]
    fn a_site_that_never_asks_is_not_given_a_cookie() {
        assert!(!needs_cookie("dlsite"));
        let v = structural("dlsite", "ipb_member_id=1");
        assert!(!v.ok);
        assert!(v.note.contains("does not ask"));
    }

    #[test]
    fn spellings_people_type_resolve_to_the_ids_the_app_stores() {
        assert_eq!(canonical_site("E-Hentai"), "ehentai");
        assert_eq!(canonical_site("ex-hentai"), "exhentai");
        assert_eq!(canonical_site("f95zone"), "f95");
        // an id we do not know must survive untouched, or a rewritten key would
        // silently orphan that site's settings row
        assert_eq!(canonical_site("hitomi.la"), "hitomi.la");
    }

    #[test]
    fn f95_markers_are_the_ones_that_measured_under_our_own_ua() {
        // shapes taken from the measured guest/member home pages: the guest one
        // carries both header links once each, the member one carries neither
        let guest = r#"<a class="p-navgroup-link"><span class="p-navgroup-linkText">Log in</span></a>
                       <a class="p-navgroup-link"><span class="p-navgroup-linkText">Register</span></a>"#;
        let member = r#"<div class="p-header"><a href="/account/">My account</a></div>"#;
        assert_eq!(
            judged("f95", 200, "https://f95zone.to/", guest),
            Some(false)
        );
        assert_eq!(
            judged("f95", 200, "https://f95zone.to/", member),
            Some(true)
        );
        assert_eq!(
            judged("f95", 403, "https://f95zone.to/", member),
            Some(false)
        );
    }

    #[test]
    fn the_exhentai_login_hop_reads_as_not_signed_in() {
        assert_eq!(
            judged(
                "exhentai",
                302,
                "https://forums.e-hentai.org/remoteapi.php?ex=MTc5NDM3Mzc4NS1iYjcwNzM5YzYw",
                ""
            ),
            Some(false)
        );
        // Measured 2026-11-14: the real cookie, a made-up `ipb_pass_hash` and a
        // real member id with that bad hash all get `200` with **no body**. So an
        // empty 200 settles nothing and must not become a green light; only a
        // page actually in the body counts.
        assert_eq!(judged("exhentai", 200, "https://exhentai.org/favorites.php", ""), None);
        assert_eq!(
            judged("exhentai", 200, "https://exhentai.org/favorites.php", "<div/>"),
            Some(true)
        );
    }

    #[test]
    fn e_hentai_bounces_a_guest_and_answers_a_member() {
        // Both shapes measured with this app's own UA on 2026-11-14, the member
        // side three times over (always 200, always 11 496 B).
        assert_eq!(
            judged(
                "ehentai",
                302,
                "https://e-hentai.org/bounce_login.php?b=d&bt=1-6",
                ""
            ),
            Some(false)
        );
        assert_eq!(
            judged(
                "ehentai",
                200,
                "https://e-hentai.org/favorites.php",
                "<title>E-Hentai Galleries</title><div>Watched</div>"
            ),
            Some(true)
        );
        // The route this app used to probe (`/g/`) answers 404 to everybody now,
        // cookie or not, which is why it is no longer the marker: an answer that
        // cannot tell the two apart is `None`, not an accusation.
        assert_eq!(
            judged("ehentai", 404, "https://e-hentai.org/g/", "<title>404 Not Found</title>"),
            None
        );
    }

    #[test]
    fn a_stale_cookie_is_reported_as_stale_not_as_usable() {
        let mut v = structural("f95", "xf_session=c; cf_clearance=b");
        assert!(v.ok);
        finish(&mut v, Ok(Live::No));
        assert!(!v.ok);
        assert!(v.note.contains("stale"), "{v:?}");
    }

    #[test]
    fn an_inconclusive_answer_is_not_a_pass() {
        let mut v = structural("exhentai", "ipb_member_id=1; ipb_pass_hash=beef");
        assert!(v.ok);
        finish(&mut v, Ok(Live::Inconclusive));
        assert_eq!(v.live, None, "the site proved nothing, so nothing is claimed");
        assert!(v.note.contains("cannot tell a good cookie from a bad one"), "{v:?}");
    }

    #[test]
    fn an_unprobed_verdict_says_it_was_not_checked() {
        let mut v = structural("f95", "xf_session=c; cf_clearance=b");
        finish(&mut v, Err("connection reset".into()));
        assert!(v.ok, "structure passed and nothing contradicted it");
        assert!(v.note.contains("not checked against the site"), "{v:?}");
        assert!(v.note.contains("connection reset"), "the reason survives");
    }

    #[test]
    fn verdicts_are_recorded_read_back_and_forgotten() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        goon_core::db::init_schema(&conn).unwrap();
        assert_eq!(recorded(&conn, "f95"), None, "unknown is not a failure");
        record(&conn, "f95", true, "2026-11-10T22:00:00Z").unwrap();
        assert_eq!(
            recorded(&conn, "f95"),
            Some((true, "2026-11-10T22:00:00Z".into()))
        );
        record(&conn, "f95", false, "2026-11-11T08:00:00Z").unwrap();
        assert!(!recorded(&conn, "f95").unwrap().0, "the later verdict wins");
        forget(&conn, "f95").unwrap();
        assert_eq!(recorded(&conn, "f95"), None);
    }

    #[test]
    fn a_gate_is_only_stale_when_a_cookie_was_saved() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        goon_core::db::init_schema(&conn).unwrap();
        assert_eq!(note_gate(&conn, "f95", "2026-11-10T22:00:00Z"), None);
        goon_core::db::set_site_setting(&conn, "f95", None, Some("xf_session=c"), None, 1200)
            .unwrap();
        assert_eq!(note_gate(&conn, "f95", "2026-11-10T22:00:00Z"), Some(false));
        assert!(
            !recorded(&conn, "f95").unwrap().0,
            "the wall was recorded as a stale cookie"
        );
    }

    /// #55: the *fetch* path's mark-stale helper has to work for every login site.
    /// f95 called it; e-hentai and Exhentai hit their gate, returned a note, and left
    /// the Sources line green on a cookie the site had just refused. Pins the helper's
    /// contract at the Core level: a saved cookie that hits the wall is recorded as
    /// *refused* with a time; a site with no cookie saved is still not "refused"; and
    /// one site's verdict never bleeds into another's row.
    #[test]
    fn a_login_wall_marks_any_login_site_refused() {
        let (core, tmp) = crate::scratch_core("gate", true);
        {
            let inner = core.inner.lock().unwrap();
            let conn = &inner.opened.as_ref().expect("unlocked").conn;
            for site in ["f95", "ehentai", "exhentai"] {
                assert!(recorded(conn, site).is_none(), "{site} was never checked");
            }
            // A paste-shaped cookie for the site whose gate we are about to hit.
            goon_core::db::set_site_setting(
                conn,
                "exhentai",
                Some(true),
                Some("ipb_member_id=0; ipb_pass_hash=0"),
                None,
                10_500,
            )
            .unwrap();
        }
        crate::ingest::mark_cookie_gate(Some(&core), "exhentai");
        {
            let inner = core.inner.lock().unwrap();
            let conn = &inner.opened.as_ref().expect("unlocked").conn;
            let (ok, at) = recorded(conn, "exhentai").expect("the wall must be recorded");
            assert!(!ok, "a login wall is a refusal, never 'accepted'");
            assert!(!at.is_empty(), "and it carries the time it happened");
            assert!(
                recorded(conn, "ehentai").is_none(),
                "exhentai's verdict must not leak into e-hentai's row"
            );
        }
        // Nothing saved for f95, so a wall there is not a dead-cookie verdict.
        // This call sits *outside* the block above on purpose: mark_cookie_gate takes
        // the inner lock itself, and calling it with the guard held deadlocks the
        // thread. That is how this test first presented itself - the whole suite hung
        // on it, with no failure and no output.
        crate::ingest::mark_cookie_gate(Some(&core), "f95");
        {
            let inner = core.inner.lock().unwrap();
            let conn = &inner.opened.as_ref().expect("unlocked").conn;
            assert!(
                recorded(conn, "f95").is_none(),
                "no cookie saved, so no cookie to be stale"
            );
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Live proof that the f95 markers hold under the identity this app actually
    /// sends, both directions, in one run:
    ///
    /// ```text
    /// $env:GOON_F95_COOKIE = (Get-Content ~\Downloads\f95cookie.txt -Raw).Trim()
    /// cargo test -p goon-desktop --lib -- --ignored --nocapture cookies::
    /// ```
    ///
    /// The cookie is never printed and never committed. This test fails the
    /// moment f95 changes its header markup, which is exactly the drift it exists
    /// to catch.
    #[tokio::test]
    #[ignore]
    async fn live_f95_markers_separate_member_from_guest_under_our_ua() {
        let cookie = std::env::var("GOON_F95_COOKIE").expect("GOON_F95_COOKIE must be set");
        let client = reqwest::Client::builder()
            .user_agent(crate::ingest::UA)
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let guest = ask(&client, "f95", "").await.unwrap();
        let member = ask(&client, "f95", &cookie).await.unwrap();
        println!("no cookie says {guest:?}, the saved cookie says {member:?}");
        assert_eq!(guest, Live::No, "a cookie-less request must read as a guest");
        assert_eq!(member, Live::Yes, "the saved cookie must read as a member");
    }
}
