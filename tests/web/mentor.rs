//! `/mcp`, the route a candidate's own model reads a live session through.
//!
//! A module of the `tests/web.rs` target for the reason the others are: cargo
//! discovers only `tests/*.rs`, and this crate statically links libwebrtc.

use super::*;
use codetrial::mentor::{MentorBoard, Snapshot};

async fn spawn_mentor_server(board: MentorBoard) -> (String, TestServer) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = spawn_test_server(
        listener,
        codetrial::web::web_service_with_mentor(web_config(), None, board),
    );
    (format!("http://{addr}"), server)
}

async fn rpc(base: &str, token: Option<&str>, body: serde_json::Value) -> reqwest::Response {
    let mut request = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .json(&body);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    request.send().await.unwrap()
}

/// Off unless an operator turned it on, and off looks like no route at all.
#[tokio::test]
async fn the_route_is_absent_without_a_token() {
    let (base, _server) = spawn_mentor_server(MentorBoard::default()).await;
    let response = rpc(
        &base,
        Some("anything"),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }),
    )
    .await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn a_wrong_or_missing_token_is_refused() {
    let board = MentorBoard::with_token(Some("right".to_string()));
    let (base, _server) = spawn_mentor_server(board).await;
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    assert_eq!(rpc(&base, None, ping.clone()).await.status(), 401);
    assert_eq!(rpc(&base, Some("wrong"), ping.clone()).await.status(), 401);
    assert_eq!(rpc(&base, Some("right"), ping).await.status(), 200);
}

/// The handshake a real client runs, then a tool call that reads the board,
/// end to end over HTTP.
#[tokio::test]
async fn a_client_can_initialize_list_tools_and_read_the_live_code() {
    let board = MentorBoard::with_token(Some("right".to_string()));
    let problem = codetrial::agent::get_problem(None);
    let mut state = codetrial::agent::RuntimeState::for_problem(problem);
    state.language = "python".to_string();
    state.code = "def two_sum(nums, target):\n    pass".to_string();
    board.publish("interview-1", Snapshot::of(problem, &state, 0));
    let (base, _server) = spawn_mentor_server(board).await;

    let initialized = rpc(
        &base,
        Some("right"),
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }
        }),
    )
    .await;
    assert_eq!(initialized.status(), 200);
    let initialized: serde_json::Value = initialized.json().await.unwrap();
    assert_eq!(initialized["result"]["capabilities"]["tools"], json!({}));

    let notified = rpc(
        &base,
        Some("right"),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    assert_eq!(notified.status(), 202);

    let tools: serde_json::Value = rpc(
        &base,
        Some("right"),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await
    .json()
    .await
    .unwrap();
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(
        names,
        [
            "list_sessions",
            "get_problem",
            "get_code",
            "get_test_results"
        ]
    );

    let code: serde_json::Value = rpc(
        &base,
        Some("right"),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "get_code", "arguments": {} }
        }),
    )
    .await
    .json()
    .await
    .unwrap();
    let text = code["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("def two_sum(nums, target):"), "{text}");
}

/// Nothing is pushed, so a client that opens the optional GET stream is told
/// there is none rather than left waiting on one.
#[tokio::test]
async fn the_get_stream_is_not_offered() {
    let board = MentorBoard::with_token(Some("right".to_string()));
    let (base, _server) = spawn_mentor_server(board).await;
    let response = reqwest::Client::new()
        .get(format!("{base}/mcp"))
        .bearer_auth("right")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 405);
}
