//! The setup UI's files, embedded in the binary: the root image stays the only
//! thing a host runs, with nothing to serve from disk.
//!
//! The UI is a single-page app, so any path that is not a file gets
//! `index.html`.

#[cfg(ui_built)]
use axum::http::StatusCode;
use axum::http::{Uri, header};
use axum::response::{IntoResponse, Response};

#[cfg(ui_built)]
static UI: include_dir::Dir = include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../ui/build");

#[cfg(ui_built)]
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let file = UI.get_file(path).or_else(|| UI.get_file("index.html"));
    let Some(file) = file else { return StatusCode::NOT_FOUND.into_response() };
    let name = file.path().to_string_lossy();
    // Vite puts a content hash in every name under _app/immutable.
    let cache = if name.starts_with("_app/immutable/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    ([(header::CONTENT_TYPE, content_type(&name)), (header::CACHE_CONTROL, cache)], file.contents()).into_response()
}

#[cfg(not(ui_built))]
pub async fn serve(_: Uri) -> Response {
    let page = "<!doctype html><title>neslina setup</title><p>This build has no setup UI: \
                run <code>deno task build</code> in <code>ui/</code> before building neslina-setup.";
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], page).into_response()
}

#[cfg_attr(not(ui_built), allow(dead_code))]
fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
