//! A scripted model server for `selftest`: it speaks the chat-completions protocol that
//! goose's `openai` provider uses (streamed or not), and answers from a fixed script instead
//! of a model, so the selftest needs no API key and gives the same result every time.
//!
//! The script reads the conversation goose sends:
//! - the recipe's prompt names the flow (`SELFTEST-FOLDER file_name=...` or
//!   `SELFTEST-WEBHOOK order=...`) and the first answer is a `shell` call built from it;
//! - after a denied call it waits `deny_delay` before answering, so goose is still in its
//!   turn when goose-triggers stops it;
//! - told that the user approved, the folder flow makes the same call again and the webhook
//!   flow makes a different one (which must be held again);
//! - after a call that ran, it says it is done.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

pub const FOLDER_MARK: &str = "SELFTEST-FOLDER";
pub const WEBHOOK_MARK: &str = "SELFTEST-WEBHOOK";
pub const DONE_TEXT: &str = "SELFTEST DONE";

#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Text(String),
    Call { tool: String, args: Value },
}

/// The command the folder flow's call runs, for a file name.
pub fn folder_command(file_name: &str) -> String {
    format!("echo 'noted {file_name}' >> selftest-notes.txt")
}

/// The command the webhook flow's first call runs, for an order.
pub fn webhook_command(order: &str) -> String {
    format!("echo 'order {order}' >> selftest-orders.txt")
}

/// The different command the webhook flow makes after the approval.
pub fn webhook_changed_command(order: &str) -> String {
    format!("echo 'order {order} and more' >> selftest-orders.txt")
}

fn text_of(m: &Value) -> String {
    match m.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The value after `key=` in a text, up to the next whitespace.
fn value_after(text: &str, key: &str) -> Option<String> {
    let at = text.find(&format!("{key}="))? + key.len() + 1;
    Some(
        text[at..]
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string(),
    )
}

/// What the script answers to one request; `delay` is true when it should wait first.
pub fn answer(req: &Value) -> (Answer, bool) {
    let has_tools = req
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|t| !t.is_empty());
    if !has_tools {
        return (Answer::Text("goose-triggers selftest".into()), false);
    }
    let msgs = req
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let all_user: String = msgs
        .iter()
        .filter(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        .map(text_of)
        .collect::<Vec<_>>()
        .join("\n");
    let shell = |cmd: String| Answer::Call {
        tool: "shell".into(),
        args: json!({ "command": cmd }),
    };
    let (initial, after_approval) = if all_user.contains(FOLDER_MARK) {
        let name = value_after(&all_user, "file_name").unwrap_or_default();
        (folder_command(&name), folder_command(&name))
    } else if all_user.contains(WEBHOOK_MARK) {
        let order = value_after(&all_user, "order").unwrap_or_default();
        (webhook_command(&order), webhook_changed_command(&order))
    } else {
        return (
            Answer::Text("This server only answers goose-triggers selftest recipes.".into()),
            false,
        );
    };
    let Some(last) = msgs.last() else {
        return (shell(initial), false);
    };
    match last.get("role").and_then(Value::as_str) {
        Some("tool") => {
            let t = text_of(last).to_lowercase();
            if t.contains("denied") || t.contains("policy") || t.contains("held") {
                (
                    Answer::Text(
                        "The call is held for the user's approval; ending my turn.".into(),
                    ),
                    true,
                )
            } else {
                (Answer::Text(DONE_TEXT.into()), false)
            }
        }
        _ => {
            if text_of(last).contains("The user approved") {
                (shell(after_approval), false)
            } else {
                (shell(initial), false)
            }
        }
    }
}

fn sse(chunks: &[Value]) -> String {
    let mut out = String::new();
    for c in chunks {
        out.push_str("data: ");
        out.push_str(&c.to_string());
        out.push_str("\n\n");
    }
    out.push_str("data: [DONE]\n\n");
    out
}

/// The HTTP response body for a request: (content type, body).
pub fn respond(req: &Value, call_seq: u64) -> (String, String) {
    let (a, _) = answer(req);
    let model = req.get("model").cloned().unwrap_or(json!("selftest"));
    let usage = json!({"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15});
    let (message, delta, finish) = match a {
        Answer::Text(t) => (
            json!({"role": "assistant", "content": t}),
            json!({"role": "assistant", "content": t}),
            "stop",
        ),
        Answer::Call { tool, args } => {
            let call = json!({"id": format!("call_{call_seq}"), "type": "function",
                "function": {"name": tool, "arguments": args.to_string()}});
            let mut streamed = call.clone();
            streamed["index"] = json!(0);
            (
                json!({"role": "assistant", "content": null, "tool_calls": [call]}),
                json!({"role": "assistant", "tool_calls": [streamed]}),
                "tool_calls",
            )
        }
    };
    if req.get("stream").and_then(Value::as_bool) == Some(true) {
        let c1 = json!({"id": "selftest", "object": "chat.completion.chunk", "created": 0, "model": model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": null}]});
        let c2 = json!({"id": "selftest", "object": "chat.completion.chunk", "created": 0, "model": model,
            "choices": [{"index": 0, "delta": {}, "finish_reason": finish}], "usage": usage});
        ("text/event-stream".into(), sse(&[c1, c2]))
    } else {
        let body = json!({"id": "selftest", "object": "chat.completion", "created": 0, "model": model,
            "choices": [{"index": 0, "message": message, "finish_reason": finish}], "usage": usage});
        ("application/json".into(), body.to_string())
    }
}

pub struct Server {
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Starts the server on a free local port; it stops when the returned value is dropped.
pub fn start(deny_delay: Duration) -> Result<Server> {
    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| anyhow!("scripted model server: {e}"))?;
    let addr = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow!("scripted model server has no IP address"))?;
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    std::thread::spawn(move || {
        let mut seq = 0u64;
        while !stop2.load(Ordering::SeqCst) {
            let Ok(Some(mut req)) = server.recv_timeout(Duration::from_millis(200)) else {
                continue;
            };
            seq += 1;
            let call_seq = seq;
            std::thread::spawn(move || {
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let url = req.url().to_string();
                let (ctype, text, status) = if url.ends_with("/models") {
                    ("application/json".to_string(), json!({"object": "list", "data": [{"id": "goose-triggers-selftest", "object": "model"}]}).to_string(), 200)
                } else {
                    match serde_json::from_str::<Value>(&body) {
                        Ok(v) => {
                            if answer(&v).1 {
                                std::thread::sleep(deny_delay);
                            }
                            let (c, t) = respond(&v, call_seq);
                            (c, t, 200)
                        }
                        Err(_) => (
                            "application/json".into(),
                            json!({"error": {"message": "bad request"}}).to_string(),
                            400,
                        ),
                    }
                };
                let header =
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes()).unwrap();
                let _ = req.respond(
                    tiny_http::Response::from_string(text)
                        .with_status_code(status)
                        .with_header(header),
                );
            });
        }
    });
    Ok(Server { addr, stop })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(msgs: Value) -> Value {
        json!({"model": "m", "stream": true, "tools": [{"type": "function", "function": {"name": "shell"}}], "messages": msgs})
    }

    #[test]
    fn folder_flow_repeats_the_call_after_approval() {
        let first = json!([{"role": "system", "content": "s"}, {"role": "user", "content": "SELFTEST-FOLDER file_name=a.txt file_path=/x/a.txt"}]);
        let (a, delay) = answer(&req(first.clone()));
        assert_eq!(
            a,
            Answer::Call {
                tool: "shell".into(),
                args: json!({"command": folder_command("a.txt")})
            }
        );
        assert!(!delay);
        let mut denied = first.as_array().unwrap().clone();
        denied.push(json!({"role": "assistant", "content": null}));
        denied.push(json!({"role": "tool", "content": "Tool call denied by policy hook"}));
        let (a, delay) = answer(&req(Value::Array(denied.clone())));
        assert!(matches!(a, Answer::Text(_)) && delay);
        denied.push(json!({"role": "user", "content": "The user approved the call ..."}));
        let (a, _) = answer(&req(Value::Array(denied.clone())));
        assert_eq!(
            a,
            Answer::Call {
                tool: "shell".into(),
                args: json!({"command": folder_command("a.txt")})
            }
        );
        denied.push(json!({"role": "tool", "content": "(no output)"}));
        assert_eq!(
            answer(&req(Value::Array(denied))).0,
            Answer::Text(DONE_TEXT.into())
        );
    }

    #[test]
    fn webhook_flow_changes_the_call_after_approval() {
        let msgs = json!([{"role": "user", "content": [{"type": "text", "text": "SELFTEST-WEBHOOK order=A-7"}]},
            {"role": "user", "content": "The user approved the call"}]);
        let (a, _) = answer(&req(msgs));
        assert_eq!(
            a,
            Answer::Call {
                tool: "shell".into(),
                args: json!({"command": webhook_changed_command("A-7")})
            }
        );
    }

    #[test]
    fn streams_a_tool_call() {
        let r = req(json!([{"role": "user", "content": "SELFTEST-FOLDER file_name=b.txt"}]));
        let (ctype, body) = respond(&r, 3);
        assert_eq!(ctype, "text/event-stream");
        assert!(
            body.contains("\"tool_calls\"")
                && body.contains("call_3")
                && body.ends_with("data: [DONE]\n\n")
        );
        let mut r = r;
        r["stream"] = json!(false);
        let (ctype, body) = respond(&r, 4);
        assert_eq!(ctype, "application/json");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "shell"
        );
    }
}
