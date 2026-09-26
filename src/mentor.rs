//! What a candidate's own model can read about their interview, over MCP.
//!
//! The candidate brings the model: Claude through claude.ai, or a local one
//! behind any MCP client such as Open WebUI. CodeTrial is the server, so it
//! decides what the model can see and never has to pay for or host it. What it
//! cannot decide is what that model says, which is why only what the candidate
//! already sees is exposed: the problem as posed, their own code and their own
//! test runs. The interviewer's half of the problem -- the contract, the hint
//! ladder, the clarifications, the optimal approach -- stays in this process.
//!
//! A mentor is meant for practice. Nothing here stops one from being used in a
//! scored interview, which is why the endpoint stays off until an operator
//! turns it on with a token.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::agent::{Problem, RuntimeState, format_test_run};

/// The protocol revision this server answers in when the client asks for one
/// it does not know. Tools are all it offers, and they have not changed shape
/// across the revisions a client is likely to send.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
const KNOWN_PROTOCOL_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

/// One live interview as the candidate sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub title: &'static str,
    pub brief: String,
    pub language: String,
    pub code: String,
    pub last_test_run: Option<Value>,
    pub test_runs: u32,
    pub updated_at: u64,
}

impl Snapshot {
    pub fn of(problem: &Problem, state: &RuntimeState, now_seconds: u64) -> Self {
        let variant = problem.variant();
        Self {
            title: variant.title,
            brief: variant.brief_text(),
            language: state.language.clone(),
            code: state.code.clone(),
            last_test_run: state.last_test_run.clone(),
            test_runs: state.test_runs,
            updated_at: now_seconds,
        }
    }
}

/// Live interviews by room, shared between the interviewers that write them
/// and the `/mcp` route that reads them.
///
/// Also carries the token that turns the route on, so the route and its
/// switch cannot be configured apart. `None` means the route answers 404.
#[derive(Debug, Clone, Default)]
pub struct MentorBoard {
    sessions: Arc<Mutex<BTreeMap<String, Snapshot>>>,
    token: Option<Arc<str>>,
}

impl MentorBoard {
    pub fn with_token(token: Option<String>) -> Self {
        Self {
            sessions: Arc::default(),
            token: token
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty())
                .map(Arc::from),
        }
    }

    pub fn enabled(&self) -> bool {
        self.token.is_some()
    }

    /// Whether `presented` is the configured token. Compared as SHA-256
    /// digests, so the time taken does not depend on how much of it matched.
    pub fn authorizes(&self, presented: &str) -> bool {
        use sha2::{Digest, Sha256};
        self.token.as_deref().is_some_and(|token| {
            Sha256::digest(token.as_bytes()) == Sha256::digest(presented.as_bytes())
        })
    }

    pub fn publish(&self, room: &str, snapshot: Snapshot) {
        self.lock().insert(room.to_string(), snapshot);
    }

    pub fn remove(&self, room: &str) {
        self.lock().remove(room);
    }

    pub fn sessions(&self) -> Vec<(String, Snapshot)> {
        self.lock()
            .iter()
            .map(|(room, snapshot)| (room.clone(), snapshot.clone()))
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Snapshot>> {
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

/// Removes a room's snapshot when the interview that published it ends, on
/// every path out of it including a panic. A snapshot left behind would keep
/// offering a finished interview's code as live.
pub struct Published<'a> {
    board: &'a MentorBoard,
    room: String,
}

impl<'a> Published<'a> {
    pub fn new(board: &'a MentorBoard, room: &str) -> Self {
        Self {
            board,
            room: room.to_string(),
        }
    }
}

impl Drop for Published<'_> {
    fn drop(&mut self) {
        self.board.remove(&self.room);
    }
}

/// Answers one JSON-RPC message. `None` for a notification, which gets no
/// reply; the route answers those with 202 and no body.
pub fn handle_message(board: &MentorBoard, message: &Value, now_seconds: u64) -> Option<Value> {
    let Some(message) = message.as_object() else {
        return Some(rpc_error(
            Value::Null,
            -32600,
            "Expected one JSON-RPC object.",
        ));
    };
    let id = message.get("id").cloned()?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match message.get("method").and_then(Value::as_str) {
        Some("initialize") => Ok(initialize(&params)),
        Some("ping") => Ok(json!({})),
        Some("tools/list") => Ok(json!({ "tools": tool_definitions() })),
        Some("tools/call") => Ok(call_tool(board, &params, now_seconds)),
        Some(method) => Err((-32601, format!("Method not found: {method}"))),
        None => Err((-32600, "Missing method.".to_string())),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => rpc_error(id, code, &message),
    })
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|version| KNOWN_PROTOCOL_VERSIONS.contains(version))
        .unwrap_or(PROTOCOL_VERSION);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "codetrial", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Read a CodeTrial candidate's live practice session: the problem as posed, their code and their test runs.",
    })
}

fn tool_definitions() -> Value {
    let room = json!({
        "type": "object",
        "properties": {
            "room": {
                "type": "string",
                "description": "The session to read, from list_sessions. May be left out when exactly one session is live."
            }
        }
    });
    json!([
        {
            "name": "list_sessions",
            "description": "List the CodeTrial sessions that are live right now.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "get_problem",
            "description": "The problem the candidate is solving, as it was posed to them.",
            "inputSchema": room
        },
        {
            "name": "get_code",
            "description": "The candidate's current code and the language it is in.",
            "inputSchema": room
        },
        {
            "name": "get_test_results",
            "description": "The candidate's most recent test run. Tests run when the candidate presses Run, not when this is called.",
            "inputSchema": room
        }
    ])
}

fn call_tool(board: &MentorBoard, params: &Value, now_seconds: u64) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let room = params
        .get("arguments")
        .and_then(|arguments| arguments.get("room"))
        .and_then(Value::as_str);
    let text = match name {
        "list_sessions" => Ok(list_sessions(board, now_seconds)),
        "get_problem" => {
            pick(board, room).map(|snapshot| format!("{}\n\n{}", snapshot.title, snapshot.brief))
        }
        "get_code" => pick(board, room).map(|snapshot| {
            if snapshot.code.trim().is_empty() {
                format!("The editor is empty. Language: {}.", snapshot.language)
            } else {
                format!(
                    "Language: {}\n\n```{}\n{}\n```",
                    snapshot.language, snapshot.language, snapshot.code
                )
            }
        }),
        "get_test_results" => pick(board, room)
            .map(|snapshot| format_test_run(snapshot.last_test_run.as_ref(), snapshot.test_runs)),
        _ => Err(format!("Unknown tool: {name}")),
    };
    match text {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
        Err(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
    }
}

fn list_sessions(board: &MentorBoard, now_seconds: u64) -> String {
    let sessions = board.sessions();
    if sessions.is_empty() {
        return "No CodeTrial session is live right now.".to_string();
    }
    sessions
        .iter()
        .map(|(room, snapshot)| {
            format!(
                "{room}: {} ({}, updated {}s ago)",
                snapshot.title,
                snapshot.language,
                now_seconds.saturating_sub(snapshot.updated_at)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The session a tool call means: the one it names, or the only one live.
fn pick(board: &MentorBoard, room: Option<&str>) -> Result<Snapshot, String> {
    let mut sessions = board.sessions();
    match room {
        Some(room) => sessions
            .into_iter()
            .find(|(live, _)| live == room)
            .map(|(_, snapshot)| snapshot)
            .ok_or_else(|| format!("No live session named {room}. Call list_sessions.")),
        None if sessions.len() > 1 => {
            Err("More than one session is live; pass room. Call list_sessions.".to_string())
        }
        None => sessions
            .pop()
            .map(|(_, snapshot)| snapshot)
            .ok_or_else(|| "No CodeTrial session is live right now.".to_string()),
    }
}

#[cfg(test)]
#[path = "../tests/unit/mentor.rs"]
mod tests;
