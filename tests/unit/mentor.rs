//! The `tests` module of `src/mentor.rs`, which declares this file by path.

use super::*;
use crate::agent::get_problem;

fn board_with(rooms: &[(&str, &str)]) -> MentorBoard {
    let board = MentorBoard::with_token(Some("secret".to_string()));
    let problem = get_problem(None);
    for (room, code) in rooms {
        let mut state = RuntimeState::for_problem(problem);
        state.language = "python".to_string();
        state.code = (*code).to_string();
        board.publish(room, Snapshot::of(problem, &state, 100));
    }
    board
}

fn call(board: &MentorBoard, name: &str, arguments: Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    });
    handle_message(board, &request, 130).expect("a request gets a reply")["result"].clone()
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap_or_default()
}

/// The route is off unless an operator gave it a token, and a blank one is
/// not a token: an empty `CODETRIAL_MCP_TOKEN=` line must not open it to
/// anyone who sends an empty bearer.
#[test]
fn only_a_nonblank_token_turns_the_route_on() {
    assert!(!MentorBoard::with_token(None).enabled());
    assert!(!MentorBoard::with_token(Some("  ".to_string())).enabled());
    assert!(!MentorBoard::with_token(Some(String::new())).authorizes(""));

    let board = MentorBoard::with_token(Some(" secret ".to_string()));
    assert!(board.enabled());
    assert!(board.authorizes("secret"));
    assert!(!board.authorizes("secre"));
}

/// A client picks the protocol revision; a server that does not know it
/// answers in its own rather than echoing a revision it cannot speak.
#[test]
fn initialize_echoes_a_known_revision_and_falls_back_otherwise() {
    let board = MentorBoard::default();
    let reply = |version: &str| {
        let request = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": version, "capabilities": {}, "clientInfo": { "name": "t" } }
        });
        handle_message(&board, &request, 0).unwrap()["result"]["protocolVersion"].clone()
    };
    assert_eq!(reply("2025-03-26"), "2025-03-26");
    assert_eq!(reply("1999-01-01"), PROTOCOL_VERSION);
}

#[test]
fn notifications_get_no_reply_and_unknown_methods_an_error() {
    let board = MentorBoard::default();
    let notification = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    assert_eq!(handle_message(&board, &notification, 0), None);

    let unknown = json!({ "jsonrpc": "2.0", "id": 3, "method": "resources/list" });
    let reply = handle_message(&board, &unknown, 0).unwrap();
    assert_eq!(reply["id"], 3);
    assert_eq!(reply["error"]["code"], -32601);
}

/// The whole point of the module: a mentor sees what the candidate sees and
/// nothing the interviewer holds back.
#[test]
fn the_problem_is_the_posed_one_without_the_interviewers_half() {
    let board = board_with(&[("room-a", "")]);
    let problem = get_problem(None);
    let variant = problem.variant();
    let posed = text(&call(&board, "get_problem", json!({}))).to_string();

    assert!(posed.contains(variant.title));
    assert!(posed.contains(&variant.brief_text()));
    assert!(
        !posed.contains(problem.optimal),
        "the optimal approach leaked"
    );
    assert!(!posed.contains(variant.contract), "the contract leaked");
    for hint in variant.hints {
        assert!(!posed.contains(hint), "a hint rung leaked: {hint}");
    }
}

#[test]
fn code_and_tests_come_from_the_live_state() {
    let board = board_with(&[("room-a", "def two_sum(nums, target):\n    return []")]);
    let code = text(&call(&board, "get_code", json!({}))).to_string();
    assert!(code.contains("Language: python"));
    assert!(code.contains("def two_sum(nums, target):"));

    let tests = text(&call(&board, "get_test_results", json!({}))).to_string();
    assert!(tests.starts_with("No test run was recorded"), "{tests}");
}

/// Leaving `room` out is only unambiguous with one session live; with two,
/// the model is told to ask rather than handed whichever sorts first.
#[test]
fn room_may_be_left_out_only_when_one_session_is_live() {
    let none = board_with(&[]);
    let result = call(&none, "get_code", json!({}));
    assert_eq!(result["isError"], true);

    let two = board_with(&[("room-a", "a = 1"), ("room-b", "b = 2")]);
    let ambiguous = call(&two, "get_code", json!({}));
    assert_eq!(ambiguous["isError"], true);
    assert!(text(&ambiguous).contains("pass room"));

    let named = call(&two, "get_code", json!({ "room": "room-b" }));
    assert!(text(&named).contains("b = 2"));

    let listed = text(&call(&two, "list_sessions", json!({}))).to_string();
    assert!(listed.contains("room-a") && listed.contains("room-b"));
    assert!(listed.contains("updated 30s ago"), "{listed}");
}

#[test]
fn a_finished_interview_takes_its_snapshot_with_it() {
    let board = board_with(&[]);
    let problem = get_problem(None);
    {
        let _published = Published::new(&board, "room-a");
        board.publish(
            "room-a",
            Snapshot::of(problem, &RuntimeState::for_problem(problem), 0),
        );
        assert_eq!(board.sessions().len(), 1);
    }
    assert!(board.sessions().is_empty());
}
