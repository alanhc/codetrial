//! `/mcp`: the HTTP half of [`crate::mentor`].
//!
//! Streamable HTTP without the stream. Every reply is one JSON object, which
//! the transport allows, so there is no session id and no server-sent events:
//! the tools only read, and a client that reconnects asks again.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use super::{AppState, json_response};
use crate::current_epoch_seconds;

pub(crate) async fn mcp_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let board = &state.mentor;
    // Not 401 or 403: a server that did not turn the route on has no route,
    // and should not tell a scanner that a token would open one.
    if !board.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if !board.authorizes(presented.trim()) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
        )
            .into_response();
    }
    let Ok(message) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({
                "jsonrpc": "2.0", "id": null,
                "error": { "code": -32700, "message": "Parse error." }
            }),
        );
    };
    match crate::mentor::handle_message(board, &message, current_epoch_seconds()) {
        Some(reply) => json_response(StatusCode::OK, reply),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// A client may open a GET stream for server-initiated messages. This server
/// sends none, which the transport says to answer with 405.
pub(crate) async fn mcp_get_handler(State(state): State<AppState>) -> Response {
    if !state.mentor.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response()
}
