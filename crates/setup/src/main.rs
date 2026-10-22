//! The setup service: the setup UI's server, on HTTPS on the LAN.
//!
//! It serves only when setup.toml has a password (the image writer puts one
//! there); without one the setup UI stays off. Plain HTTP on port 80 only
//! redirects to HTTPS.
//!
//! API, all JSON, all but login behind the session cookie:
//!
//! | | |
//! |---|---|
//! | `POST /api/login` | `{password}`; sets the cookie |
//! | `POST /api/logout` | ends the session |
//! | `POST /api/password` | `{password}`; required after a first-login password |
//! | `GET /api/state` | the machine, its disks, the install job |
//! | `POST /api/disks` | `{state, games?}` as in setup.toml, `wipe` stated; starts the install |
//! | `GET /api/log` | the end of the native build's log |

mod auth;
mod config;
mod host;
mod job;
mod static_files;
mod tls;

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::sync::Mutex;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use neslina_common::{error, info, warn};
use neslina_disks::blockdev;
use neslina_disks::setup::Disks;
use serde::Deserialize;
use serde_json::json;
use tokio::net::TcpListener;

use auth::{COOKIE, Credential, Sessions};

const BUILD_LOG: &str = "/var/neslina/log/build-a.log";
const LOG_LINES: usize = 200;

struct App {
    credential: Mutex<Credential>,
    sessions: Sessions,
    runner: job::Runner,
}

type Shared = &'static App;

fn main() {
    neslina_common::kmsg::init("setup");
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            error!("starting the runtime: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = rt.block_on(run()) {
        error!("{e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let setup = tokio::task::spawn_blocking(config::read).await??;
    let credential = match (setup.password_hash, setup.password) {
        (Some(h), _) => Credential::Hash(h),
        (None, Some(p)) if !p.is_empty() => Credential::FirstLogin(p),
        _ => {
            info!("no password in {}; the setup UI stays off", config::SETUP);
            return Ok(());
        }
    };
    let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default().trim().to_owned();
    let tls = tokio_rustls::TlsAcceptor::from(tls::server_config(&hostname)?);

    let app: Shared = Box::leak(Box::new(App {
        credential: Mutex::new(credential),
        sessions: Sessions::default(),
        runner: Default::default(),
    }));
    let router = Router::new()
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/password", post(change_password))
        .route("/api/state", get(state))
        .route("/api/disks", post(assign_disks))
        .route("/api/log", get(build_log))
        .fallback(static_files::serve)
        .with_state(app);

    let https = TcpListener::bind("0.0.0.0:443").await.context("binding port 443")?;
    let http = TcpListener::bind("0.0.0.0:80").await.context("binding port 80")?;
    info!("setup UI on https://{hostname}.local/");
    tokio::spawn(redirect_http(http));
    loop {
        let (stream, peer) = match https.accept().await {
            Ok(c) => c,
            Err(e) => {
                warn!("accepting: {e}");
                continue;
            }
        };
        let (tls, router) = (tls.clone(), router.clone());
        tokio::spawn(async move {
            // Failed handshakes are routine (the browser's first look at a
            // self-signed certificate), so they are not logged.
            let Ok(stream) = tls.accept(stream).await else { return };
            let service = TowerToHyperService::new(router);
            if let Err(e) =
                hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream), service).await
            {
                neslina_common::debug!("connection from {peer}: {e}");
            }
        });
    }
}

async fn redirect_http(listener: TcpListener) {
    let router = Router::new().fallback(|headers: HeaderMap, uri: axum::http::Uri| async move {
        let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default();
        let host = host.split(':').next().unwrap_or_default();
        Redirect::permanent(&format!("https://{host}{}", uri.path()))
    });
    if let Err(e) = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await {
        warn!("port 80: {e}");
    }
}

fn reply(status: StatusCode, body: serde_json::Value) -> Response {
    (status, Json(body)).into_response()
}

fn fail(status: StatusCode, message: &str) -> Response {
    reply(status, json!({ "error": message }))
}

// Returning the refusal as the error is the usual shape for an axum handler guard.
#[allow(clippy::result_large_err)]
/// The caller's session, or the response that refuses them. Unless `allow_first`,
/// a session that still has to change its password is refused too.
fn session(app: Shared, headers: &HeaderMap, allow_first: bool) -> Result<(String, auth::Session), Response> {
    let token = headers
        .get(header::COOKIE)
        .and_then(|c| c.to_str().ok())
        .and_then(auth::token_from)
        .ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "not logged in"))?;
    let s = app.sessions.get(token).ok_or_else(|| fail(StatusCode::UNAUTHORIZED, "not logged in"))?;
    if s.must_change_password && !allow_first {
        return Err(fail(StatusCode::FORBIDDEN, "set a new password first"));
    }
    Ok((token.to_owned(), s))
}

#[derive(Deserialize)]
struct PasswordBody {
    password: String,
}

async fn login(State(app): State<Shared>, Json(body): Json<PasswordBody>) -> Response {
    let credential = app.credential.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let password = body.password;
    let ok = tokio::task::spawn_blocking(move || credential.verify(&password)).await.unwrap_or(false);
    if !ok {
        tokio::time::sleep(auth::FAILURE_DELAY).await;
        return fail(StatusCode::UNAUTHORIZED, "wrong password");
    }
    let first = matches!(*app.credential.lock().unwrap_or_else(|e| e.into_inner()), Credential::FirstLogin(_));
    let token = app.sessions.create(first);
    let cookie = format!("{COOKIE}={token}; Path=/; Secure; HttpOnly; SameSite=Strict");
    ([(header::SET_COOKIE, cookie)], Json(json!({ "must_change_password": first }))).into_response()
}

async fn logout(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if let Ok((token, _)) = session(app, &headers, true) {
        app.sessions.remove(&token);
    }
    let cookie = format!("{COOKIE}=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0");
    ([(header::SET_COOKIE, cookie)], Json(json!({}))).into_response()
}

async fn change_password(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<PasswordBody>) -> Response {
    let (token, _) = match session(app, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if body.password.chars().count() < auth::MIN_PASSWORD {
        return fail(StatusCode::BAD_REQUEST, &format!("use at least {} characters", auth::MIN_PASSWORD));
    }
    let saved = tokio::task::spawn_blocking(move || -> Result<String> {
        let hash = auth::hash(&body.password)?;
        config::set_password_hash(&hash)?;
        Ok(hash)
    })
    .await;
    match saved {
        Ok(Ok(hash)) => {
            *app.credential.lock().unwrap_or_else(|e| e.into_inner()) = Credential::Hash(hash);
            app.sessions.keep_only(&token);
            info!("the setup password was changed");
            reply(StatusCode::OK, json!({}))
        }
        Ok(Err(e)) => {
            error!("saving the new password: {e:#}");
            fail(StatusCode::INTERNAL_SERVER_ERROR, "could not save the new password to the boot medium")
        }
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn state(State(app): State<Shared>, headers: HeaderMap) -> Response {
    let (_, s) = match session(app, &headers, true) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let host = tokio::task::spawn_blocking(host::read).await;
    match host {
        Ok(host) => reply(
            StatusCode::OK,
            json!({ "host": host, "job": app.runner.current(), "must_change_password": s.must_change_password }),
        ),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn assign_disks(State(app): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Err(r) = session(app, &headers, false) {
        return r;
    }
    // Parsed by hand so a missing `wipe` reads as the same plain error setup.toml gives.
    let disks: Disks = match serde_json::from_slice(&body) {
        Ok(d) => d,
        Err(e) => return fail(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    let checked = tokio::task::spawn_blocking(move || -> Result<Disks, String> {
        let list = blockdev::list().map_err(|e| e.to_string())?;
        let state = disks.state.resolve("state", &list)?;
        if let Some(g) = &disks.games {
            let games = g.resolve("games", &list)?;
            if games.name == state.name {
                return Err("state and games are the same disk".into());
            }
        }
        config::set_disks(&disks).map_err(|e| format!("{e:#}"))?;
        Ok(disks)
    })
    .await;
    match checked {
        Ok(Ok(_)) => {
            if !app.runner.start() {
                return fail(StatusCode::CONFLICT, "an install is already running");
            }
            info!("disks assigned; installing");
            reply(StatusCode::ACCEPTED, json!({}))
        }
        Ok(Err(e)) => fail(StatusCode::BAD_REQUEST, &e),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn build_log(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if let Err(r) = session(app, &headers, false) {
        return r;
    }
    let lines = tokio::task::spawn_blocking(|| {
        let Ok(f) = File::open(BUILD_LOG) else { return Vec::new() };
        let all: Vec<String> = BufReader::new(f).lines().map_while(Result::ok).collect();
        all[all.len().saturating_sub(LOG_LINES)..].to_vec()
    })
    .await
    .unwrap_or_default();
    reply(StatusCode::OK, json!({ "lines": lines }))
}
