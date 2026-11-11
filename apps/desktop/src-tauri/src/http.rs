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
    build_client(state, timeout_secs, crate::ingest::UA)
}

/// Same proxy/timeout policy as [`client_for`], browser UA (#50): use for image
/// and video bytes pulled off content hosts (`wimg.rule34.xxx`,
/// `attachments.f95zone.to`, `img.dlsite.jp`, e-hentai thumbs). Never use it for
/// a site's API or search endpoint.
pub fn file_client_for(state: &Core, timeout_secs: u64) -> Result<reqwest::Client, String> {
    build_client(state, timeout_secs, FILE_UA)
}

fn build_client(state: &Core, timeout_secs: u64, ua: &str) -> Result<reqwest::Client, String> {
    let proxy = {
        let inner = state.inner.lock().unwrap();
        match inner.opened.as_ref() {
            Some(o) => goon_core::db::get_meta(&o.conn, "proxy_url"),
            None => None,
        }
    };
    let mut b = reqwest::Client::builder()
        .user_agent(ua)
        .timeout(Duration::from_secs(timeout_secs));
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
