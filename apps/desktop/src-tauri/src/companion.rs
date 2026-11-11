//! Companion HTTP API (DESIGN §4, D20) — the browser extension's door into the app.
//!
//! Binds 127.0.0.1 only, random port, bearer-token auth. `handshake` is the
//! unauthenticated pairing probe (returns no secrets); everything else needs
//! the session token shown in the app (Sources panel). #19 wired `/api/clip`
//! for real: it runs the *same* ingestion path as the in-app "+ Clip URL"
//! button (classification, enrichment, vault lock discipline) through a
//! shared [`Core`], so an extension clip appears in the activity strip and
//! obeys the same per-site politeness lanes.

use crate::ingest;
use crate::Core;
use axum::extract::{rejection::JsonRejection, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;

#[derive(Clone, Debug)]
pub struct CompanionInfo {
    pub port: u16,
    pub token: String,
}

#[derive(Clone)]
struct Shared {
    info: Arc<CompanionInfo>,
    locked: Arc<AtomicBool>,
    core: Core,
}

/// Bind the listener synchronously (so the port is known immediately), then
/// convert + serve it on Tauri's tokio runtime. (`TcpListener::from_std`
/// needs a running reactor, so it must happen inside the spawned task.)
pub fn start(locked: Arc<AtomicBool>, core: Core) -> Result<CompanionInfo, String> {
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = std_listener.local_addr().map_err(|e| e.to_string())?.port();
    std_listener
        .set_nonblocking(true)
        .map_err(|e| e.to_string())?;

    let mut raw = [0u8; 16];
    OsRng.fill_bytes(&mut raw);
    let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();

    let info = Arc::new(CompanionInfo {
        port,
        token: token.clone(),
    });
    let app = router(locked, core, info.clone());

    tauri::async_runtime::spawn(async move {
        let Ok(listener) = TcpListener::from_std(std_listener) else {
            return;
        };
        let _ = axum::serve(listener, app).await;
    });

    Ok(CompanionInfo { port, token })
}

/// Exposed separately so tests can serve it on their own listener.
pub fn router(locked: Arc<AtomicBool>, core: Core, info: Arc<CompanionInfo>) -> Router {
    let shared = Shared { info, locked, core };
    let public = Router::new().route("/api/handshake", get(handshake));
    let protected = Router::new()
        .route("/api/ping", get(ping))
        .route("/api/clip", post(clip))
        .route_layer(middleware::from_fn_with_state(
            shared.clone(),
            require_token,
        ));
    public.merge(protected).with_state(shared)
}

async fn handshake(State(shared): State<Shared>) -> Json<serde_json::Value> {
    Json(json!({
        "app": "TheGoonFlow",
        "version": env!("CARGO_PKG_VERSION"),
        "companion": 1,
        "locked": shared.locked.load(Ordering::Relaxed),
    }))
}

async fn ping(State(shared): State<Shared>) -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "locked": shared.locked.load(Ordering::Relaxed),
    }))
}

#[derive(Deserialize)]
struct ClipBody {
    url: String,
}

async fn clip(
    State(shared): State<Shared>,
    body: Result<Json<ClipBody>, JsonRejection>,
) -> Response {
    let Json(ClipBody { url }) = match body {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("expected {{\"url\": \"…\"}}: {e}") })),
            )
                .into_response()
        }
    };
    let url = url.trim().to_string();
    if url.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "url is empty" })),
        )
            .into_response();
    }

    // Same observability as UI-initiated work: the strip shows browser clips.
    let job = match shared.core.jobs.start("clip", &url) {
        Some(j) => j,
        None => {
            return (
                StatusCode::CONFLICT,
                Json(json!({ "error": "an import/clip is already running — try again shortly" })),
            )
                .into_response()
        }
    };
    match ingest::clip(&shared.core, url).await {
        Ok(report) => {
            job.finish(&format!("clipped ({})", report.site));
            Json(json!({ "ok": true, "report": report })).into_response()
        }
        Err(e) => {
            job.finish(&format!("clip failed: {e}"));
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": e })),
            )
                .into_response()
        }
    }
}

async fn require_token(State(shared): State<Shared>, req: Request, next: Next) -> Response {
    let wanted = format!("Bearer {}", shared.info.token);
    let authorized = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == wanted);
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Inner, Jobs};

    fn temp_core(tag: &str, unlocked: bool) -> (Core, std::path::PathBuf) {
        let tmp = std::env::temp_dir().join(format!("goonflow-cp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let vault = tmp.join("vault");
        let (conn, key) = if unlocked {
            let key = goon_vault::create_keyfile(&vault, "hunter2").unwrap();
            let conn = goon_vault::open_db(&vault, &key).unwrap();
            goon_core::db::init_schema(&conn).unwrap();
            (Some(conn), Some(key))
        } else {
            (None, None)
        };
        let core = Core {
            inner: Arc::new(std::sync::Mutex::new(Inner {
                vault_root: Some(vault),
                opened: conn.map(|c| crate::OpenedVault {
                    conn: c,
                    _key: key.unwrap(),
                }),
                ..Default::default()
            })),
            jobs: Arc::new(Jobs::default()),
            queue: Arc::new(crate::queue::SiteQueue::default()),
        };
        (core, tmp)
    }

    async fn serve(locked: bool, unlocked: bool) -> (reqwest::Client, CompanionInfo) {
        let (core, _tmp) = temp_core(if unlocked { "open" } else { "shut" }, unlocked);
        let info = Arc::new(CompanionInfo {
            port: 0, // tests don't dial by port; the caller holds the client
            token: "testtoken".into(),
        });
        let app = router(Arc::new(AtomicBool::new(locked)), core, info.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let mut info = (*info).clone();
        info.port = addr.port();
        (client, info)
    }

    #[tokio::test]
    async fn auth_ladder_handshake_ping_clip() {
        let (client, info) = serve(true, false).await;
        let base = format!("http://127.0.0.1:{}", info.port);

        // handshake: public, no secrets
        let h: serde_json::Value = client
            .get(format!("{base}/api/handshake"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(h["app"], "TheGoonFlow");
        assert_eq!(h["locked"], true);

        // protected routes need the bearer token
        let r = client.get(format!("{base}/api/ping")).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let r = client
            .get(format!("{base}/api/ping"))
            .header("Authorization", "Bearer testtoken")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);

        // clip without token → 401
        let r = client
            .post(format!("{base}/api/clip"))
            .json(&json!({ "url": "https://rule34.xxx/index.php?page=post&s=view&id=1" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

        // clip with token but a locked vault → 422 + friendly error, no crash
        let r = client
            .post(format!("{base}/api/clip"))
            .header("Authorization", "Bearer testtoken")
            .json(&json!({ "url": "https://rule34.xxx/index.php?page=post&s=view&id=1" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let v: serde_json::Value = r.json().await.unwrap();
        assert!(
            v["error"].as_str().unwrap().contains("vault is locked"),
            "got: {v}"
        );

        // malformed body → 400
        let r = client
            .post(format!("{base}/api/clip"))
            .header("Authorization", "Bearer testtoken")
            .json(&json!({ "noturl": "x" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unreachable_clip_stores_bare_and_second_is_duplicate() {
        let (client, info) = serve(false, true).await;
        let base = format!("http://127.0.0.1:{}", info.port);
        let auth = format!("Bearer {}", info.token);
        // D10 spirit: a dead link still lands (bare entry), the fetch failure
        // is a note, not a lost clip.
        let r = client
            .post(format!("{base}/api/clip"))
            .header("Authorization", auth.clone())
            .json(&json!({ "url": "http://127.0.0.1:9/nope.jpg" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["report"]["created"], true);
        // same url again → recognized as already tracked, no double entry
        let r = client
            .post(format!("{base}/api/clip"))
            .header("Authorization", auth)
            .json(&json!({ "url": "http://127.0.0.1:9/nope.jpg" }))
            .send()
            .await
            .unwrap();
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["report"]["created"], false);
        assert_eq!(v["report"]["note"], "already in the flow");
    }
}
