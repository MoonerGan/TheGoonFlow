//! #24: one HTTP client policy for every networked path (clip, enrich, update
//! checks, match search, video download). The only knob is the user-set proxy
//! (plain HTTP proxy or SOCKS5 to a local Tor), stored in the vault `meta`
//! table so it travels with the vault, not the machine.
//!
//! #50: there are **two** identities here on purpose — do not merge them back
//! into one:
//!
//! * [`client_for`] sends the honest, self-identifying UA ([`crate::ingest::UA`])
//!   for **site APIs, metadata pages and site search**. Being identifiable is what
//!   keeps those endpoints open to a personal library, and it keeps us traceable
//!   when a site wants to know who is asking. Polite + attributable is policy.
//! * [`file_client_for`] sends an ordinary browser UA and is used only for the
//!   **image/file bytes the user asked for** (clip covers, thumbnails, direct
//!   file links). Content CDNs reject anything that is not a browser: measured on
//!   `wimg.rule34.xxx/images/7783/…png?8886918`, our UA gets a 403 with a 77 KB
//!   error page while a browser UA gets 200 with the real 1 042 205 byte PNG.
//!   `Referer` makes no difference there — only the UA does.
//!
//! Cookies and referer handling are identical in both clients (hotlink checks
//! still see the real site, saved source cookies are replayed verbatim). The file
//! half of the split is also applied per-request by `ingest::get_bytes`, so
//! callers that hand their page client to `fetch_meta` still fetch art like a
//! browser without losing the vault proxy — reqwest only fills *default* headers
//! a request did not already set.

use std::time::Duration;

use crate::Core;

/// Browser-shaped UA for **file bytes only** (#50). Kept plain and version-stable:
/// a fake UA that changes shape per release is what gets a whole app block-listed.
pub const FILE_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                           (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// Build the reqwest client honoring the vault's `proxy_url` meta key.
/// Examples: `http://127.0.0.1:7890`, `socks5://127.0.0.1:9150` (Tor Browser).
/// Honest UA: use for APIs, metadata pages and search.
pub fn client_for(state: &Core, timeout_secs: u64) -> Result<reqwest::Client, String> {
    build_client(
        state,
        timeout_secs,
        crate::ingest::UA,
        reqwest::redirect::Policy::default(),
    )
}

/// Same proxy/timeout policy as [`client_for`], browser UA (#50): use for image
/// and video bytes pulled off content hosts (`wimg.rule34.xxx`,
/// `attachments.f95zone.to`, `img.dlsite.jp`, e-hentai thumbs). Never use it for
/// a site's API or search endpoint.
pub fn file_client_for(state: &Core, timeout_secs: u64) -> Result<reqwest::Client, String> {
    build_client(
        state,
        timeout_secs,
        FILE_UA,
        reqwest::redirect::Policy::default(),
    )
}

/// #38: the cookie probe client. Page identity (honest UA, it is a page fetch)
/// but **redirects are not followed**, because for Exhentai the redirect to the
/// forums login hop *is* the answer: following it hides the fact that the vault's
/// cookie is not being accepted.
pub fn probe_client_for(state: &Core, timeout_secs: u64) -> Result<reqwest::Client, String> {
    build_client(
        state,
        timeout_secs,
        crate::ingest::UA,
        reqwest::redirect::Policy::none(),
    )
}

fn build_client(
    state: &Core,
    timeout_secs: u64,
    ua: &str,
    redirect: reqwest::redirect::Policy,
) -> Result<reqwest::Client, String> {
    let proxy = {
        let inner = state.inner.lock().unwrap();
        match inner.opened.as_ref() {
            Some(o) => goon_core::db::get_meta(&o.conn, "proxy_url"),
            None => None,
        }
    };
    let mut b = reqwest::Client::builder()
        .user_agent(ua)
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(redirect);
    if let Some(p) = proxy
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
    {
        let pr =
            reqwest::Proxy::all(&p).map_err(|e| format!("{p} is not a usable proxy URL: {e}"))?;
        b = b.proxy(pr);
    }
    b.build().map_err(crate::err)
}

/// Validate before storing (set_proxy command uses this). reqwest accepts
/// almost any URL here, so check the scheme ourselves first — a typo like
/// "http//127.0.0.1:9050" must not be saved and silently break enrichment.
pub fn validate_proxy(url: &str) -> Result<(), String> {
    let u: reqwest::Url = url
        .trim()
        .parse()
        .map_err(|_| format!("{url} is not a usable proxy URL"))?;
    match u.scheme() {
        "http" | "https" | "socks4" | "socks5" | "socks5h" => {}
        s => {
            return Err(format!(
                "{s}:// is not a supported proxy scheme — use http://, https:// or socks5://"
            ))
        }
    }
    if u.host_str().map(str::is_empty).unwrap_or(true) {
        return Err(format!("{url} has no proxy host"));
    }
    reqwest::Proxy::all(u.as_str())
        .map(|_| ())
        .map_err(|e| format!("{url} is not a usable proxy URL: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_urls_parse_and_reject() {
        assert!(validate_proxy("http://127.0.0.1:7890").is_ok());
        assert!(validate_proxy(" socks5h://127.0.0.1:9050 ").is_ok());
        assert!(validate_proxy("http://user:pass@host.example:8080").is_ok());
        assert!(validate_proxy("not a proxy").is_err());
        assert!(validate_proxy("ftp://x").is_err());
    }
}

#[cfg(test)]
mod ua_policy_tests {
    use super::*;
    use crate::{Inner, Jobs};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::{Arc, Mutex};

    /// Local stub that records each request head, then answers 200 with no body.
    /// Proves what actually goes on the wire without touching a content host.
    fn stub_server(max: usize) -> (u16, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(max) {
                let mut s = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let mut buf = [0u8; 4096];
                let mut head = String::new();
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    head.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if head.contains("\r\n\r\n") {
                        break;
                    }
                }
                let _ = tx.send(head);
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                );
                let _ = s.flush();
            }
        });
        (port, rx)
    }

    /// Header value for `name` from a recorded request head (names are matched
    /// case-insensitively, the value keeps its case).
    fn header_of(head: &str, name: &str) -> String {
        for line in head.lines() {
            if let Some((k, v)) = line.split_once(':') {
                if k.trim().eq_ignore_ascii_case(name) {
                    return v.trim().to_string();
                }
            }
        }
        String::new()
    }

    fn locked_down_state() -> Core {
        Core {
            inner: Arc::new(Mutex::new(Inner {
                vault_root: None,
                opened: None,
                ..Default::default()
            })),
            jobs: Arc::new(Jobs::default()),
            queue: Arc::new(crate::queue::SiteQueue::default()),
        }
    }

    /// #50: page traffic stays honest and traceable, file bytes go out as a
    /// browser. If someone "simplifies" the two clients into one, this fails.
    #[tokio::test]
    async fn page_clients_are_honest_and_file_clients_look_like_browsers() {
        let state = locked_down_state();
        let (port, rx) = stub_server(3);
        let url = format!("http://127.0.0.1:{port}/images/7783/post.png?8886918");

        client_for(&state, 5)
            .unwrap()
            .get(&url)
            .header(reqwest::header::REFERER, "https://rule34.xxx")
            .send()
            .await
            .expect("page request");
        file_client_for(&state, 5)
            .unwrap()
            .get(&url)
            .send()
            .await
            .expect("file request");
        // what ingest::get_bytes does when handed a page client: browser UA
        // per-request, so the vault proxy of that client is kept
        client_for(&state, 5)
            .unwrap()
            .get(&url)
            .header(reqwest::header::USER_AGENT, FILE_UA)
            .send()
            .await
            .expect("per-request override");

        let page = rx.recv_timeout(Duration::from_secs(5)).expect("page hit");
        let file = rx.recv_timeout(Duration::from_secs(5)).expect("file hit");
        let mixed = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("override hit");
        let ua_page = header_of(&page, "user-agent");
        let ua_file = header_of(&file, "user-agent");
        let ua_mixed = header_of(&mixed, "user-agent");

        assert!(
            ua_page.starts_with("TheGoonFlow/"),
            "API/page traffic lost the honest UA: {ua_page:?}"
        );
        assert!(
            !ua_page.to_lowercase().contains("mozilla"),
            "page traffic must not pretend to be a browser: {ua_page:?}"
        );
        // hotlink protection + saved source cookies stay untouched
        assert_eq!(header_of(&page, "referer"), "https://rule34.xxx");

        assert!(
            ua_file.starts_with("Mozilla/5.0"),
            "file bytes must go out with the browser UA (#50), got {ua_file:?}"
        );
        assert!(
            !ua_file.to_lowercase().contains("goonflow"),
            "file client leaked the app UA: {ua_file:?}"
        );

        assert!(
            ua_mixed.starts_with("Mozilla/5.0") && !ua_mixed.to_lowercase().contains("goonflow"),
            "the per-request UA override did not win over the client default: {ua_mixed:?}"
        );
    }
}

#[cfg(test)]
mod wiring_tests {
    use super::*;
    use crate::{Inner, Jobs, OpenedVault};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// The whole point of #24: a proxy_url stored in the vault must make every
    /// client connect to the proxy instead of the target. A local stub that
    /// accepts, records the hit, and hangs up proves the route without network.
    #[tokio::test]
    async fn vault_proxy_is_applied_to_clients() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hit = Arc::new(AtomicBool::new(false));
        {
            let hit = hit.clone();
            std::thread::spawn(move || {
                if listener.accept().is_ok() {
                    hit.store(true, Ordering::SeqCst);
                }
            });
        }

        let tmp = std::env::temp_dir().join(format!("goonflow-proxy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let vault = tmp.join("vault");
        let key = goon_vault::create_keyfile(&vault, "hunter2").unwrap();
        let conn = goon_vault::open_db(&vault, &key).unwrap();
        goon_core::db::init_schema(&conn).unwrap();
        goon_core::db::set_meta(&conn, "proxy_url", Some(&format!("http://{addr}"))).unwrap();

        let state = Core {
            inner: Arc::new(Mutex::new(Inner {
                vault_root: Some(vault),
                opened: Some(OpenedVault { conn, _key: key }),
                ..Default::default()
            })),
            jobs: Arc::new(Jobs::default()),
            queue: Arc::new(crate::queue::SiteQueue::default()),
        };

        let client = client_for(&state, 5).expect("client");
        // No DNS needed: an HTTP proxy receives the whole URL in its request
        // line, so reqwest must dial the stub no matter what the target is.
        let r = client.get("http://goonflow.invalid/").send().await;
        assert!(
            r.is_err(),
            "the stub proxy must not produce a successful response"
        );

        for _ in 0..20 {
            if hit.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            hit.load(Ordering::SeqCst),
            "the stored proxy was never contacted — client_for ignored proxy_url"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// Live probe: what do the hosts we pull bytes from actually answer to the two
/// client identities this file defines? Nothing is asserted here — the printout
/// *is* the product, because the answer is what decides the policy above. Site
/// protection moves without telling us (rule34 used to 403 our UA on the CDN,
/// #50; e-hentai rewrote its gallery page around the time of #63), so the only
/// honest way to fix a download is to re-measure first.
///
/// ```text
/// $env:GOON_EH_COOKIE = (Get-Content "$env:USERPROFILE\Downloads\ehentaicookie.txt" -Raw).Trim()
/// powershell -ExecutionPolicy Bypass -Command "& tools\dev-env.ps1; `
///   cargo test -p goonflow-desktop --lib -- --ignored --nocapture --test-threads 1 live_site_answers"
/// ```
#[cfg(test)]
mod live_probe {
    use super::*;
    use std::time::Duration;

    /// Same knobs as `build_client`, minus the vault (no proxy to honor).
    fn client(ua: &str, follow: bool, http1: bool) -> reqwest::Client {
        let mut b = reqwest::Client::builder()
            .user_agent(ua)
            .timeout(Duration::from_secs(30))
            .redirect(if follow {
                reqwest::redirect::Policy::default()
            } else {
                reqwest::redirect::Policy::none()
            });
        if http1 {
            b = b.http1_only();
        }
        b.build().expect("client")
    }

    /// One request, printed in one line: status, content type, size, and the
    /// Cloudflare markers that say a bot check answered rather than the origin
    /// refusing us. Returns whether the host gave us bytes.
    async fn ask(
        label: &str,
        url: &str,
        ua: &str,
        referer: Option<&str>,
        cookie: Option<&str>,
        http1: bool,
    ) -> bool {
        let mut req = client(ua, true, http1).get(url);
        if let Some(r) = referer {
            req = req.header(reqwest::header::REFERER, r);
        }
        if let Some(c) = cookie {
            req = req.header(reqwest::header::COOKIE, c);
        }
        match req.send().await {
            Err(e) => {
                println!("{label:<44} TRANSPORT {e}");
                false
            }
            Ok(r) => {
                let status = r.status();
                let kind = r
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("?")
                    .to_string();
                let cf = ["cf-mitigated", "server", "cf-cache-status"]
                    .iter()
                    .filter_map(|h| {
                        r.headers()
                            .get(*h)
                            .and_then(|v| v.to_str().ok())
                            .map(|v| format!("{h}={v}"))
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let bytes = r.bytes().await.map(|b| b.len()).unwrap_or(0);
                println!(
                    "{label:<44} {:<3} {kind} {bytes} B  {cf}",
                    status.as_u16(),
                );
                status.is_success() && bytes > 0
            }
        }
    }

    /// Read a page as text with one of the two client identities.
    async fn page_text(url: &str, ua: &str, cookie: Option<&str>) -> Option<String> {
        let mut req = client(ua, true, false).get(url);
        if let Some(c) = cookie {
            req = req.header(reqwest::header::COOKIE, c);
        }
        req.send().await.ok()?.text().await.ok()
    }

    /// First substring that starts just after `pat` and runs to the next
    /// delimiter. Hand-rolled on purpose: this binary has no regex crate, and
    /// the pages we scan are small.
    fn grab(html: &str, pat: &str) -> Option<String> {
        let i = html.find(pat)? + pat.len();
        let rest = &html[i..];
        let end = rest.find(['"', '\'', ')', ' ', ';', '\n']).unwrap_or(rest.len());
        let s = rest[..end].trim();
        (!s.is_empty()).then(|| s.to_string())
    }

    /// Like [`grab`] but keeps `pat` itself — for urls that are already written
    /// out in full in the page.
    fn grab_url(html: &str, pat: &str) -> Option<String> {
        let i = html.find(pat)?;
        let rest = &html[i..];
        let end = rest.find(['"', '\'', ')', ' ', ';', '\n']).unwrap_or(rest.len());
        let s = rest[..end].trim();
        (!s.is_empty()).then(|| s.to_string())
    }

    #[ignore = "live: asks rule34, e621 and e-hentai for real bytes"]
    #[tokio::test]
    async fn live_site_answers() {
        let cookie: Option<String> = match std::env::var("GOON_EH_COOKIE") {
            Ok(c) if !c.trim().is_empty() => Some(c.trim().to_string()),
            _ => {
                println!("GOON_EH_COOKIE not set: the e-hentai cookie rows below are skipped");
                None
            }
        };
        let ck = cookie.as_deref();

        println!("--- rule34: does the site still answer, and does the CDN care? ---");
        // The dapi call clip() makes for a rule34 post link.
        ask(
            "rule34 dapi, honest UA",
            "https://rule34.xxx/public/index.php?page=dapi&s=post&q=index&json=1&id=3411909",
            crate::ingest::UA,
            Some("https://rule34.xxx"),
            None,
            false,
        )
        .await;
        ask(
            "rule34 dapi, browser UA",
            "https://rule34.xxx/public/index.php?page=dapi&s=post&q=index&json=1&id=3411909",
            FILE_UA,
            Some("https://rule34.xxx"),
            None,
            false,
        )
        .await;
        // The clipped file link itself, exactly as ingest::get_bytes asks for it.
        ask(
            "rule34 cdn, file UA + referer (h2)",
            "https://wimg.rule34.xxx//images/3043/a6cf369e5d7e473d9decc31cf7d0e1b9.jpeg?3411909",
            FILE_UA,
            Some("https://rule34.xxx"),
            None,
            false,
        )
        .await;
        ask(
            "rule34 cdn, file UA + referer (http1)",
            "https://wimg.rule34.xxx//images/3043/a6cf369e5d7e473d9decc31cf7d0e1b9.jpeg?3411909",
            FILE_UA,
            Some("https://rule34.xxx"),
            None,
            true,
        )
        .await;
        ask(
            "rule34 cdn, honest UA + referer",
            "https://wimg.rule34.xxx//images/3043/a6cf369e5d7e473d9decc31cf7d0e1b9.jpeg?3411909",
            crate::ingest::UA,
            Some("https://rule34.xxx"),
            None,
            false,
        )
        .await;

        println!("--- e621: api json, then the file that json names ---");
        if ask(
            "e621 posts.json, honest UA",
            "https://e621.net/posts.json?limit=1",
            crate::ingest::UA,
            Some("https://e621.net"),
            None,
            false,
        )
        .await
        {
            let body = page_text("https://e621.net/posts.json?limit=1", crate::ingest::UA, None).await;
            let file = body.and_then(|t| {
                serde_json::from_str::<serde_json::Value>(&t)
                    .ok()
                    .and_then(|v| v["posts"][0]["file"]["url"].as_str().map(|s| s.to_string()))
            });
            println!("e621 live file url: {file:?}");
            if let Some(url) = file {
                ask("e621 file, file UA + referer", &url, FILE_UA, Some("https://e621.net"), None, false).await;
                ask("e621 file, file UA, no referer", &url, FILE_UA, None, None, false).await;
                ask("e621 file, honest UA + referer", &url, crate::ingest::UA, Some("https://e621.net"), None, false).await;
            }
        }

        println!("--- e-hentai: gallery page, sprite sheet, slideshow, cookie verdicts ---");
        // gid+key of the gallery the user reported; the key is part of the url
        // now, /g/<gid>/ alone is a bare apache 404.
        let gallery = "https://e-hentai.org/g/4089666/b9d914bef4/";
        ask("eh gallery, honest UA, no cookie", gallery, crate::ingest::UA, None, None, false).await;
        if ck.is_some() {
            ask("eh gallery, honest UA, cookie", gallery, crate::ingest::UA, None, ck, false).await;
            ask("eh gallery, file UA, cookie", gallery, FILE_UA, None, ck, false).await;
        }
        // The sprite sheet and the slideshow keys are per-session, so pull them
        // out of the page we just got instead of hardcoding ones that rot.
        if let Some(html) = page_text(gallery, crate::ingest::UA, ck).await {
            // Two different sheets in the 2026 page: the cover tile on ehgt.org
            // (`gthumb`, offset 0 0) and the grid tiles on a random
            // `*.hath.network` host, laid out side by side with negative x
            // offsets. Both are webp; neither was there before (#46's gdtm/
            // og:image markup is gone).
            let cover = grab(&html, "background:transparent url(");
            let grid = grab(&html, "url(").filter(|g| !g.contains("ehgt.org"));
            let slide = grab_url(&html, "https://e-hentai.org/s/");
            println!(
                "eh page {} B, cover sheet {cover:?}, grid sheet {grid:?}",
                html.len()
            );
            println!("eh live slide: {slide:?}");
            for s in [cover, grid].into_iter().flatten() {
                ask("eh cover sheet, file UA + referer", &s, FILE_UA, Some("https://e-hentai.org/"), ck, false).await;
                ask("eh cover sheet, file UA, no referer", &s, FILE_UA, None, ck, false).await;
            }
            if let Some(s) = slide {
                ask("eh slideshow, honest UA", &s, crate::ingest::UA, Some(gallery), ck, false).await;
                let slide_body = page_text(&s, crate::ingest::UA, ck).await.unwrap_or_default();
                let full = grab_url(&slide_body, "https://e-hentai.org/r/");
                let orig = grab_url(&slide_body, "https://e-hentai.org/fullimg/");
                println!(
                    "eh live r/ image: {full:?}, fullimg: {orig:?} (slide page {} B)",
                    slide_body.len()
                );
                if let Some(f) = full {
                    ask("eh r/ image, file UA + referer", &f, FILE_UA, Some(&s), ck, false).await;
                    ask("eh r/ image, honest UA, no cookie", &f, crate::ingest::UA, None, None, false).await;
                }
                if let Some(f) = orig {
                    ask("eh fullimg, file UA + referer", &f, FILE_UA, Some(&s), ck, false).await;
                }
            }
        }
        if let Some(c) = ck {
            // exhentai in 2026 answers 200 with an *empty* body unless the
            // request carries `igneous=mystery`; the browser gets that value
            // from a Set-Cookie we never keep. Without it a "valid" cookie
            // looks authenticated and still yields nothing.
            println!(
                "exhentai root, cookie only        {} B",
                page_text("https://exhentai.org/", crate::ingest::UA, Some(c))
                    .await
                    .map(|t| t.len())
                    .unwrap_or(0)
            );
            let with_igneous = format!("{c}; igneous=mystery");
            println!(
                "exhentai root, cookie + igneous   {} B",
                page_text("https://exhentai.org/", crate::ingest::UA, Some(&with_igneous))
                    .await
                    .map(|t| t.len())
                    .unwrap_or(0)
            );
            println!(
                "exhentai /g/ cookie + igneous     {} B",
                page_text(gallery.replace("e-hentai", "exhentai").as_str(), crate::ingest::UA, Some(&with_igneous))
                    .await
                    .map(|t| t.len())
                    .unwrap_or(0)
            );
            // redirects NOT followed: for exhentai the login hop is the verdict.
            let r = client(crate::ingest::UA, false, false)
                .get("https://exhentai.org/")
                .header(reqwest::header::COOKIE, c)
                .send()
                .await;
            match r {
                Ok(r) => println!(
                    "exhentai root, honest UA + cookie     {} location={:?}",
                    r.status().as_u16(),
                    r.headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|v| v.to_str().ok())
                ),
                Err(e) => println!("exhentai root, honest UA + cookie     TRANSPORT {e}"),
            }
        }
    }
}
