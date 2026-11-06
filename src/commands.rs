//! `status`, `log`, `approve` and `reject`: read and decide on what the store holds.

use std::io::{BufRead, BufReader};

use anyhow::{bail, Result};
use serde_json::Value;

use crate::config::Config;
use crate::store::{state, Store};
use crate::util::{call_line, clean, fmt_time, short};

/// A tool call's input in full, for the user to read before deciding: pretty JSON (which
/// escapes control characters in its strings), any other control character escaped.
fn full_call(input: &str) -> String {
    let text = serde_json::from_str::<Value>(input)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| input.to_string());
    text.split('\n').map(clean).collect::<Vec<_>>().join("\n")
}

fn indent(text: &str, by: &str) -> String {
    text.lines()
        .map(|l| format!("{by}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn open(cfg: &Config) -> Result<Store> {
    let db = cfg.db_path();
    if !db.exists() {
        bail!(
            "no goose-triggers state in {} yet: start `goose-triggers run` first",
            cfg.state_dir.display()
        );
    }
    Store::open(&db)
}

pub fn status(cfg: &Config, all: bool) -> Result<()> {
    let store = open(cfg)?;
    let runs = store.list_runs(if all { None } else { Some(20) })?;
    if runs.is_empty() {
        println!("no runs yet");
        return Ok(());
    }
    println!(
        "{:>5}  {:<20} {:<9} {:<19}  {:<14} DETAIL",
        "RUN", "RULE", "STATE", "UPDATED", "SESSION"
    );
    for r in &runs {
        let detail = match r.state.as_str() {
            state::HELD => match store.pending_call(r.id)? {
                Some(c) => format!("waits on: {} {}", c.tool_name, call_line(&c.tool_input, 80)),
                None => "held".into(),
            },
            // `log <run>` is named once, below the table.
            state::FAILED => r
                .error
                .as_deref()
                .map(|e| e.trim_end_matches(&format!("; see goose-triggers log {}", r.id)))
                .map(|e| short(e, 100))
                .unwrap_or_default(),
            state::APPROVED => format!(
                "approved: resumes when `goose-triggers run` picks it up (`reject {}` withdraws it)",
                r.id
            ),
            _ => String::new(),
        };
        println!(
            "{:>5}  {:<20} {:<9} {:<19}  {:<14} {}",
            r.id,
            short(&r.rule, 20),
            r.state,
            fmt_time(r.updated_at),
            r.session_id.as_deref().unwrap_or("-"),
            detail
        );
    }
    let held = runs.iter().filter(|r| r.state == state::HELD).count();
    if held > 0 {
        println!(
            "\n{held} run(s) held. Read a held call in full with goose-triggers log <RUN>, then: goose-triggers approve <RUN> | goose-triggers reject <RUN>"
        );
    } else {
        println!("\nA run in full: goose-triggers log <RUN>");
    }
    if !crate::daemon::is_running(&cfg.state_dir) {
        println!(
            "\n(goose-triggers run is not running for {})",
            cfg.state_dir.display()
        );
    }
    Ok(())
}

/// One line for a goose stream-json message, or None for lines not worth showing.
fn describe(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    match v.get("type").and_then(Value::as_str) {
        Some("message") => {
            let role = v
                .pointer("/message/role")
                .and_then(Value::as_str)
                .unwrap_or("?");
            let parts = v
                .pointer("/message/content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for p in parts {
                match p.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = p.get("text").and_then(Value::as_str).unwrap_or("");
                        if !text.trim().is_empty() {
                            let who = if role == "assistant" { "goose" } else { "user" };
                            out.push(format!("{who}: {}", short(text.trim(), 300)));
                        }
                    }
                    Some("toolRequest") => {
                        let call = p.pointer("/toolCall/value");
                        let name = call
                            .and_then(|c| c.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or("?");
                        let args = call
                            .and_then(|c| c.get("arguments"))
                            .map(|a| a.to_string())
                            .unwrap_or_default();
                        out.push(format!("tool call: {name} {}", short(&args, 200)));
                    }
                    Some("toolResponse") => {
                        let r = p.get("toolResult");
                        let ok = r.and_then(|r| r.get("status")).and_then(Value::as_str)
                            == Some("success");
                        let is_error =
                            r.and_then(|r| r.pointer("/value/isError")) == Some(&Value::Bool(true));
                        let text = if ok {
                            r.and_then(|r| r.pointer("/value/content/0/text"))
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string()
                        } else {
                            r.and_then(|r| r.get("error"))
                                .map(|e| {
                                    e.as_str()
                                        .map(str::to_string)
                                        .unwrap_or_else(|| e.to_string())
                                })
                                .unwrap_or_default()
                        };
                        let tag = if ok && !is_error {
                            "tool result"
                        } else {
                            "tool error"
                        };
                        out.push(format!("{tag}: {}", short(text.trim(), 200)));
                    }
                    _ => {}
                }
            }
        }
        Some("error") => out.push(format!("goose error: {}", short(&v.to_string(), 300))),
        _ => {}
    }
    out
}

pub fn log(cfg: &Config, run: Option<i64>) -> Result<()> {
    let store = open(cfg)?;
    let Some(id) = run else {
        for l in store.recent_timeline(40)? {
            let who = l.run_id.map(|r| format!("run {r}")).unwrap_or_default();
            println!("{}  {who:<8} {}", fmt_time(l.at), clean(&l.what));
        }
        return Ok(());
    };
    let Some(r) = store.run(id)? else {
        bail!("there is no run {id}")
    };
    println!(
        "run {id}: rule {}, {} (since {}, created {})",
        r.rule,
        r.state,
        fmt_time(r.updated_at),
        fmt_time(r.created_at)
    );
    if let Some(e) = &r.error {
        println!("error: {}", clean(e));
    }
    println!("recipe: {}", r.recipe);
    if let Some(ev) = store.event(r.event_id)? {
        println!(
            "event {} ({}, {}):",
            ev.id,
            ev.source,
            fmt_time(ev.received_at)
        );
        for (k, v) in &ev.fields {
            println!("  {} = {}", clean(k), short(v, 200));
        }
    }
    println!("goose session: {}", r.session_id.as_deref().unwrap_or("-"));
    println!("working folder: {}", r.working_dir);
    let calls = store.calls(id)?;
    if !calls.is_empty() {
        println!("\nheld calls:");
        for c in calls {
            println!(
                "  {}  {}  {}",
                fmt_time(c.created_at),
                c.state,
                clean(&c.tool_name)
            );
            println!("{}", indent(&full_call(&c.tool_input), "      "));
        }
    }
    println!("\ntimeline:");
    for l in store.timeline(id)? {
        println!("  {}  {}", fmt_time(l.at), clean(&l.what));
    }
    let out = cfg.runs_dir().join(format!("{id}.jsonl"));
    if let Ok(f) = std::fs::File::open(&out) {
        println!("\ngoose ({}):", out.display());
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let (text, event) = crate::runner::split_line(&line);
            if !text.is_empty() {
                // goose's own messages, such as an error before the run started.
                println!("  goose printed: {}", short(text, 300));
            }
            for d in event.iter().flat_map(describe) {
                println!("  {d}");
            }
        }
    }
    let err = cfg.runs_dir().join(format!("{id}.stderr"));
    let tail = crate::runner::tail(&err, 0, 15);
    if !tail.is_empty() {
        println!("\ngoose's stderr, last lines ({}):", err.display());
        for l in tail {
            println!("  {}", short(&l, 300));
        }
    }
    Ok(())
}

fn refuse_inside_a_run() -> Result<()> {
    if std::env::var_os(crate::hook::ENV_RUN).is_some() {
        bail!("refusing to decide on a hold from inside a goose-triggers run");
    }
    Ok(())
}

pub fn approve(cfg: &Config, run: i64, yes: bool) -> Result<()> {
    refuse_inside_a_run()?;
    let store = open(cfg)?;
    let Some(shown) = store.pending_call(run)? else {
        match store.run(run)? {
            None => bail!("there is no run {run}"),
            Some(r) => bail!("run {run} is {}, not held", r.state),
        }
    };
    println!("run {run} waits on {} with input", clean(&shown.tool_name));
    println!("{}", indent(&full_call(&shown.tool_input), "  "));
    // SAFETY: isatty only inspects a file descriptor.
    let interactive = unsafe { libc::isatty(0) } == 1;
    if !interactive && !yes {
        bail!("not approved: there is no terminal to ask on; pass --yes to approve run {run} without being asked");
    }
    if !yes {
        print!("Approve this call? [y/N] ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            println!("not approved; the run stays held");
            return Ok(());
        }
    }
    store.approve_call(run, shown.id)?;
    println!("run {run}: approved");
    if crate::daemon::is_running(&cfg.state_dir) {
        println!("goose-triggers resumes its goose session now; that exact call may run once.");
    } else {
        println!("goose-triggers run is not running: the run resumes once it starts.");
    }
    Ok(())
}

pub fn reject(cfg: &Config, run: i64) -> Result<()> {
    refuse_inside_a_run()?;
    let store = open(cfg)?;
    let call = store.reject(run)?;
    let what = if call.state == "approved" {
        "withdrew the approval of"
    } else {
        "rejected"
    };
    println!(
        "run {run}: {what} {} {}; the run has ended",
        clean(&call.tool_name),
        call_line(&call.tool_input, 200)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn full_call_is_whole_and_terminal_safe() {
        let input = crate::util::canonical_json(
            &json!({"path": "/a", "content": "line1\nline2\u{1b}[2J\u{9b}x"}),
        );
        let shown = full_call(&input);
        assert!(shown.contains("\"path\": \"/a\""), "{shown}");
        assert!(
            !shown.chars().any(|c| c.is_control() && c != '\n'),
            "{shown:?}"
        );
        assert!(shown.contains("line1\\nline2"), "{shown}");
    }

    #[test]
    fn describes_goose_messages() {
        let v = json!({"type": "message", "message": {"role": "assistant", "content": [
            {"type": "text", "text": "Saving it."},
            {"type": "toolRequest", "id": "c1", "toolCall": {"status": "success", "value": {"name": "shell", "arguments": {"command": "ls"}}}}
        ]}});
        assert_eq!(
            describe(&v),
            ["goose: Saving it.", r#"tool call: shell {"command":"ls"}"#]
        );
        let v = json!({"type": "message", "message": {"role": "user", "content": [
            {"type": "toolResponse", "id": "c1", "toolResult": {"status": "error", "error": "denied"}}
        ]}});
        assert_eq!(describe(&v), ["tool error: denied"]);
        assert!(describe(&json!({"type": "complete"})).is_empty());
    }
}
