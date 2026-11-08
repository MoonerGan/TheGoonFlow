//! #24: one HTTP client policy for every networked path (clip, enrich, update
//! checks, match search, video download). The only knob is the user-set proxy
//! (plain HTTP proxy or SOCKS5 to a local Tor), stored in the vault `meta`
//! table so it travels with the vault, not the machine.

use std::time::Duration;

use crate::AppState;

/// Build the reqwest client honoring the vault's `proxy_url` meta key.
/// Examples: `http://127.0.0.1:7890`, `socks5://127.0.0.1:9150` (Tor Browser).
pub fn client_for(state: &AppState, timeout_secs: u64) -> Result<reqwest::Client, String> {
    let proxy = {
        let inner = state.inner.lock().unwrap();
        match inner.opened.as_ref() {
            Some(o) => goon_core::db::get_meta(&o.conn, "proxy_url"),
            None => None,
        }
    };
    let mut b = reqwest::Client::builder()
        .user_agent(crate::ingest::UA)
        .timeout(Duration::from_secs(timeout_secs));
    if let Some(p) = proxy.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
        let pr = reqwest::Proxy::all(&p)
            .map_err(|e| format!("{p} is not a usable proxy URL: {e}"))?;
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

        let state = AppState {
            inner: Mutex::new(Inner {
                vault_root: Some(vault),
                opened: Some(OpenedVault { conn, _key: key }),
            }),
            exe_dir: tmp.clone(),
            companion: None,
            locked: Arc::new(AtomicBool::new(false)),
            jobs: Arc::new(Jobs::default()),
            queue: Arc::new(crate::queue::SiteQueue::default()),
        };

        let client = client_for(&state, 5).expect("client");
        // No DNS needed: an HTTP proxy receives the whole URL in its request
        // line, so reqwest must dial the stub no matter what the target is.
        let r = client.get("http://goonflow.invalid/").send().await;
        assert!(r.is_err(), "the stub proxy must not produce a successful response");

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
