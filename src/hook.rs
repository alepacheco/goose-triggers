//! `goose-triggers hook`: the command goose runs for goose-triggers' plugin hooks.
//!
//! goose passes the hook event as JSON on stdin and the run's environment, in which the
//! runner set `GOOSE_TRIGGERS_RUN` (the run) and `GOOSE_TRIGGERS_DB` (the store).
//! - `SessionStart`: records the goose session the run started, which `approve` resumes.
//! - `UserPromptSubmit`: notes that goose runs the plugin's hooks in this turn.
//! - `PreToolUse`: a call to a tool the rule holds is denied with a reason and stored as
//!   pending, and the run becomes held; the runner then stops goose. A call that matches the
//!   one-time permission an approval gave goes through once.
//!
//! The PreToolUse hook is installed with `on_failure: block`, so when this command fails
//! (exit 1), goose denies the call: the gate fails closed.

use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::store::{PreTool, Store};
use crate::util::{canonical_json, short};

pub const ENV_RUN: &str = "GOOSE_TRIGGERS_RUN";
pub const ENV_DB: &str = "GOOSE_TRIGGERS_DB";

/// What goose is told to do with a call.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Block(String),
}

pub fn main_hook() -> u8 {
    let Some(run) = std::env::var_os(ENV_RUN) else {
        // Not a goose-triggers run (the plugin's command checks this too).
        return 0;
    };
    let result = (|| -> Result<Verdict> {
        let run: i64 = run
            .to_str()
            .and_then(|s| s.parse().ok())
            .context("GOOSE_TRIGGERS_RUN is not a run number")?;
        let db = PathBuf::from(std::env::var_os(ENV_DB).context("GOOSE_TRIGGERS_DB is not set")?);
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input)?;
        let payload: Value =
            serde_json::from_str(&input).context("the hook payload is not JSON")?;
        let store = Store::open(&db)?;
        decide(&store, run, &payload)
    })();
    match result {
        Ok(Verdict::Allow) => 0,
        Ok(Verdict::Block(reason)) => {
            println!("{}", json!({"decision": "block", "reason": reason}));
            0
        }
        Err(e) => {
            eprintln!("goose-triggers hook: {e:#}");
            1
        }
    }
}

pub fn decide(store: &Store, run: i64, payload: &Value) -> Result<Verdict> {
    let event = payload.get("event").and_then(Value::as_str).unwrap_or("");
    let session = payload.get("session_id").and_then(Value::as_str);
    match event {
        "SessionStart" => {
            if let Some(sid) = session {
                if store.set_session_if_unset(run, sid)? {
                    store.log(Some(run), &format!("goose session {sid} started"))?;
                }
            }
            Ok(Verdict::Allow)
        }
        "UserPromptSubmit" => {
            // goose runs this at the start of each turn, before any tool call: it shows the
            // runner that this goose process runs goose-triggers' hooks.
            store.note_turn(run)?;
            Ok(Verdict::Allow)
        }
        "PreToolUse" => {
            let tool = payload
                .get("tool_name")
                .and_then(Value::as_str)
                .context("PreToolUse payload without tool_name")?;
            let input = canonical_json(payload.get("tool_input").unwrap_or(&Value::Null));
            let call_id = payload.get("tool_call_id").and_then(Value::as_str);
            if let Some(sid) = session {
                // In case the session-start hook did not record it: approve resumes this session.
                if store.set_session_if_unset(run, sid)? {
                    store.log(Some(run), &format!("goose session {sid} recorded"))?;
                }
            }
            Ok(match store.pre_tool_use(run, tool, &input, call_id)? {
                PreTool::Allowed | PreTool::AllowedByApproval => Verdict::Allow,
                PreTool::Held(_) => Verdict::Block(format!(
                    "goose-triggers held this call ({tool} {}) for the user's approval. Do not retry it and do not try another way: end your turn now. If the user approves, you will be asked to make the call again.",
                    short(&input, 200)
                )),
                PreTool::RunHeld => Verdict::Block(
                    "goose-triggers holds this run for the user's approval of an earlier call: no tool call goes through until they decide. End your turn now.".into(),
                ),
                PreTool::NotRunning(st) => Verdict::Block(format!(
                    "goose-triggers: this run is {st}, so no tool call goes through. End your turn now."
                )),
            })
        }
        _ => Ok(Verdict::Allow),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::state;
    use crate::store::tests::{running_run, temp_store};

    #[test]
    fn session_start_records_the_first_session_only() {
        let (s, _) = temp_store("hook-session");
        let run = running_run(&s, &["shell"]);
        let p = json!({"event": "SessionStart", "session_id": "20261106_1"});
        assert_eq!(decide(&s, run, &p).unwrap(), Verdict::Allow);
        let p = json!({"event": "SessionStart", "session_id": "20261106_2"});
        assert_eq!(decide(&s, run, &p).unwrap(), Verdict::Allow);
        assert_eq!(
            s.run(run).unwrap().unwrap().session_id.as_deref(),
            Some("20261106_1")
        );
    }

    #[test]
    fn pre_tool_use_holds_marked_tools_whatever_the_key_order() {
        let (s, _) = temp_store("hook-pre");
        let run = running_run(&s, &["shell"]);
        let call = |input: Value| json!({"event": "PreToolUse", "session_id": "S", "tool_name": "shell", "tool_input": input, "tool_call_id": "c"});
        let p = json!({"event": "PreToolUse", "session_id": "S", "tool_name": "tree", "tool_input": {"path": "."}});
        assert_eq!(decide(&s, run, &p).unwrap(), Verdict::Allow);
        let v = decide(&s, run, &call(json!({"command": "ls", "timeout": 5}))).unwrap();
        assert!(
            matches!(&v, Verdict::Block(r) if r.contains("held this call")),
            "{v:?}"
        );
        assert_eq!(s.run(run).unwrap().unwrap().state, state::HELD);
        s.approve(run).unwrap();
        s.transition(run, &[state::APPROVED], state::RUNNING, None)
            .unwrap();
        // Same input, keys in another order: the approval matches.
        let v = decide(&s, run, &call(json!({"timeout": 5, "command": "ls"}))).unwrap();
        assert_eq!(v, Verdict::Allow);
    }

    #[test]
    fn unknown_run_is_an_error_so_goose_blocks() {
        let (s, _) = temp_store("hook-unknown");
        let p = json!({"event": "PreToolUse", "session_id": "S", "tool_name": "shell", "tool_input": {}});
        assert!(decide(&s, 999, &p).is_err());
    }
}
