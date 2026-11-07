//! Companion HTTP API skeleton (PLAN.md §4).
//!
//! Binds 127.0.0.1 only, random port, bearer-token auth. `handshake` is the
//! unauthenticated pairing probe (no secrets); everything else needs the
//! session token the user pastes into the extension in M4. `/api/clip` is a
//! deliberate stub until M1 — the contract exists so M4 is wiring, not design.

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rand::rngs::OsRng;
use rand::RngCore;
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
}

/// Bind the listener synchronously (so the port is known immediately), then
/// convert + serve it on Tauri's tokio runtime. (`TcpListener::from_std`
/// needs a running reactor, so it must happen inside the spawned task.)
pub fn start(locked: Arc<AtomicBool>) -> Result<CompanionInfo, String> {
    let std_listener =
        std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = std_listener.local_addr().map_err(|e| e.to_string())?.port();
    std_listener.set_nonblocking(true).map_err(|e| e.to_string())?;

    let mut raw = [0u8; 16];
    OsRng.fill_bytes(&mut raw);
    let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();

    let shared = Shared {
        info: Arc::new(CompanionInfo { port, token: token.clone() }),
        locked,
    };

    let public = Router::new().route("/api/handshake", get(handshake));
    let protected = Router::new()
        .route("/api/ping", get(ping))
        .route("/api/clip", post(clip))
        .route_layer(middleware::from_fn_with_state(shared.clone(), require_token));
    let app = public.merge(protected).with_state(shared);

    tauri::async_runtime::spawn(async move {
        let Ok(listener) = TcpListener::from_std(std_listener) else {
            return;
        };
        let _ = axum::serve(listener, app).await;
    });

    Ok(CompanionInfo { port, token })
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

async fn clip(State(_shared): State<Shared>, body: String) -> Response {
    let _ = body;
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({ "error": "clip ingestion lands in M1" })),
    )
        .into_response()
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
