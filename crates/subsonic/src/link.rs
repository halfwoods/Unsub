//! Serving audio at links that carry a secret, for things other than Subsonic clients, such as
//! speakers told to fetch a song. The secret is the only credential, so the host decides what a
//! secret opens and when it stops opening it.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};

use crate::{Stream, api};

/// What a link's secret opens.
pub trait LinkSource: Send + Sync + 'static {
    /// The audio `secret` was made for, or `None` if it opens nothing (it never did, or no longer
    /// does). Called on a blocking thread, so it may read from disk or a database.
    fn open(&self, secret: &str) -> Option<Stream>;
}

/// Handles `/link/<secret>` and `/link/<secret>.<extension>`: the extension is for players that
/// decide how to decode by it, and is ignored.
pub async fn handle(
    State(links): State<Arc<dyn LinkSource>>,
    Path(name): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if !matches!(method, Method::GET | Method::HEAD) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let secret = name.split('.').next().unwrap_or_default().to_owned();
    let opened = tokio::task::spawn_blocking(move || links.open(&secret))
        .await
        .ok()
        .flatten();
    let Some(stream) = opened else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // A file that has gone since the link was made is as good as a link that opens nothing.
    api::serve(stream, &headers, false)
        .await
        .unwrap_or_else(|_| StatusCode::NOT_FOUND.into_response())
}
