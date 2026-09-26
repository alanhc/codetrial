//! Serves `/mcp` over one made-up live session, so an MCP client can be
//! pointed at it without LiveKit or a Gemini key.
//!
//!     cargo run --example mentor_demo -- 127.0.0.1:8795 demo-token
//!
//! The session is Two Sum with the inner loop starting at 0, and a test run
//! that caught it: what a candidate asking a mentor "why does my code fail?"
//! would have on screen.

use codetrial::agent::{RuntimeState, get_problem, sanitize_test_run};
use codetrial::mentor::{MentorBoard, Snapshot};
use serde_json::json;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let addr = args.next().unwrap_or_else(|| "127.0.0.1:8795".to_string());
    let token = args.next().unwrap_or_else(|| "demo-token".to_string());

    let problem = get_problem(Some("two-sum"));
    let mut state = RuntimeState::for_problem(problem);
    state.language = "python".to_string();
    state.code = "def two_sum(nums, target):\n    for i in range(len(nums)):\n        for j in range(len(nums)):\n            if nums[i] + nums[j] == target:\n                return [i, j]\n    return []\n".to_string();
    state.last_test_run = Some(sanitize_test_run(&json!({
        "language": "python",
        "passed": 2,
        "total": 3,
        "failures": [
            { "label": "nums=[3,2,4], target=6", "expected": "[1, 2]", "got": "[0, 0]" }
        ]
    })));
    state.test_runs = 1;

    let board = MentorBoard::with_token(Some(token));
    board.publish(
        "interview-demo",
        Snapshot::of(problem, &state, codetrial::current_epoch_seconds()),
    );

    // Nothing here mints a token or signs anyone in, so the LiveKit project is
    // a placeholder and accounts are off.
    let config = codetrial::web::WebServerConfig {
        web_dir: std::env::temp_dir(),
        github_client_id: None,
        github_client_secret: None,
        session_secret: None,
        db_path: None,
        github_oauth_base_url: None,
        github_api_base_url: None,
        room_prefix: "interview".to_string(),
        fixed_room_name: None,
        production: false,
        compiler_explorer_enabled: false,
        trusted_proxy_hops: 0,
        recording: None,
        pool: codetrial::config::ProviderPool {
            providers: vec![codetrial::config::Provider {
                id: codetrial::config::PRIMARY_PROVIDER_ID.to_string(),
                url: "wss://unused.invalid".to_string(),
                api_key: "unused".to_string(),
                api_secret: "unused".to_string(),
                google_api_keys: Vec::new(),
            }],
        },
        probe_provider_quota: false,
    };
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    eprintln!("mentor demo: POST http://{addr}/mcp with a bearer token");
    axum::serve(
        listener,
        codetrial::web::web_service_with_mentor(config, None, board),
    )
    .await
    .expect("serve");
}
