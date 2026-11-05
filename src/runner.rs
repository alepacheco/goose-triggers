//! Runs goose for one run: a new headless recipe run, or the same session resumed after an
//! approval. Watches the store while goose runs, and stops goose once the run is held.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::config::Config;
use crate::daemon::{say, STOP};
use crate::hook::{ENV_DB, ENV_RUN};
use crate::store::{state, Run, Store};
use crate::util::{is_cli_agent_provider, short};

/// Once a run is held, goose gets this long to record the denial before it is stopped anyway.
const HOLD_GRACE: Duration = Duration::from_secs(10);
/// After SIGTERM, goose gets this long to exit before SIGKILL.
const KILL_GRACE: Duration = Duration::from_secs(5);
/// A new session must have run goose-triggers' SessionStart hook this long after goose's
/// first message, or the run is stopped: without the plugin nothing would be held.
const HOOK_CHECK: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct Ctx {
    pub cfg: Arc<Config>,
    pub exe: PathBuf,
    pub db: PathBuf,
}

pub enum Mode {
    Start,
    /// Resume the run's session with this message.
    Resume(String),
}

/// The message that resumes a run after the user approved its held call.
pub fn resume_text(tool: &str, input: &str) -> String {
    format!(
        "The user approved the call that goose-triggers held: tool `{tool}` with input {input}. Make exactly that call again now, with the same input, then carry on with the task."
    )
}

pub fn spawn(ctx: Ctx, run: Run, mode: Mode) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let id = run.id;
        if let Err(e) = run_goose(&ctx, &run, &mode) {
            let msg = format!("{e:#}");
            say(&format!("run {id} ({}): failed: {msg}", run.rule));
            if let Ok(store) = Store::open(&ctx.db) {
                let _ = store.transition(
                    id,
                    &[state::QUEUED, state::APPROVED, state::RUNNING],
                    state::FAILED,
                    Some(&msg),
                );
                let _ = store.log(Some(id), &format!("failed: {msg}"));
            }
        }
    })
}

/// The provider a run's goose would use: the rule's, the recipe's, `GOOSE_PROVIDER`, or
/// goose's config.
pub fn effective_provider(run: &Run) -> Option<String> {
    if let Some(p) = &run.provider {
        return Some(p.clone());
    }
    if let Some(p) = crate::recipe::read(Path::new(&run.recipe))
        .ok()
        .and_then(|i| i.provider)
    {
        return Some(p);
    }
    if let Some(p) = std::env::var("GOOSE_PROVIDER")
        .ok()
        .filter(|p| !p.is_empty())
    {
        return Some(p);
    }
    let text =
        std::fs::read_to_string(crate::plugin::goose_config_dir().join("config.yaml")).ok()?;
    let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).ok()?;
    ["active_provider", "GOOSE_PROVIDER"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|p| p.as_str()).map(str::to_string))
}

fn check_provider(run: &Run) -> Result<()> {
    if run.holds.is_empty() {
        return Ok(());
    }
    if let Some(p) = effective_provider(run) {
        if is_cli_agent_provider(&p) {
            bail!(
                "provider {p} hands the run to another agent, which runs its own tools: goose's hooks do not see those calls, so the rule's holds could not be enforced. Use a provider for which goose runs the tools (an API provider or a local model)"
            );
        }
    }
    Ok(())
}

fn signal_group(pid: u32, sig: libc::c_int) {
    // SAFETY: goose runs in its own process group (its pid), so this signals goose and the
    // processes it started, nothing else.
    unsafe {
        libc::kill(-(pid as libc::pid_t), sig);
    }
}

fn goose_command(ctx: &Ctx, run: &Run, mode: &Mode, wd: &Path) -> Result<Command> {
    let mut cmd = Command::new(&ctx.cfg.goose);
    cmd.arg("run");
    match mode {
        Mode::Start => {
            cmd.arg("--recipe").arg(&run.recipe);
            for (k, v) in &run.params {
                cmd.arg("--params").arg(format!("{k}={v}"));
            }
            cmd.arg("--name")
                .arg(format!("goose-triggers {} run {}", run.rule, run.id));
        }
        Mode::Resume(text) => {
            let sid = run
                .session_id
                .as_deref()
                .context("the run has no goose session to resume")?;
            cmd.args(["--resume", "--session-id", sid, "--text", text]);
        }
    }
    if let Some(p) = &run.provider {
        cmd.arg("--provider").arg(p);
    }
    if let Some(m) = &run.model {
        cmd.arg("--model").arg(m);
    }
    if let Some(t) = run.max_turns {
        cmd.arg("--max-turns").arg(t.to_string());
    }
    cmd.args(["--output-format", "stream-json"]);

    let runs_dir = ctx.cfg.runs_dir();
    std::fs::create_dir_all(&runs_dir)?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(runs_dir.join(format!("{}.stderr", run.id)))?;
    cmd.current_dir(wd)
        .env("GOOSE_MODE", "auto")
        .env(ENV_RUN, run.id.to_string())
        .env(ENV_DB, &ctx.db)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr));
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        #[cfg(target_os = "linux")]
        // SAFETY: prctl is async-signal-safe; it makes goose get SIGTERM if goose-triggers dies.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
    }
    Ok(cmd)
}

/// Whether a stream-json line is goose's message recording the result of call `call_id`.
fn is_tool_response(v: &Value, call_id: &str) -> bool {
    v.get("type").and_then(Value::as_str) == Some("message")
        && v.pointer("/message/content")
            .and_then(Value::as_array)
            .is_some_and(|c| {
                c.iter().any(|part| {
                    part.get("type").and_then(Value::as_str) == Some("toolResponse")
                        && part.get("id").and_then(Value::as_str) == Some(call_id)
                })
            })
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Kill {
    Hold,
    Stop,
    NoHook,
}

fn run_goose(ctx: &Ctx, run: &Run, mode: &Mode) -> Result<()> {
    let store = Store::open(&ctx.db)?;
    let wd = PathBuf::from(&run.working_dir);
    std::fs::create_dir_all(&wd).with_context(|| format!("creating {}", wd.display()))?;
    crate::plugin::install(&wd, &ctx.exe)?;
    crate::plugin::preflight(&wd)?;
    check_provider(run)?;

    let from = match mode {
        Mode::Start => state::QUEUED,
        Mode::Resume(_) => state::APPROVED,
    };
    let mut cmd = goose_command(ctx, run, mode, &wd)?;
    if !store.transition(run.id, &[from], state::RUNNING, None)? {
        return Ok(());
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("starting goose ({})", ctx.cfg.goose))?;
    let pid = child.id();
    store.set_pid(run.id, Some(pid as i64))?;
    let what = match mode {
        Mode::Start => format!("goose started (pid {pid}) in {}", wd.display()),
        Mode::Resume(_) => format!(
            "goose resumed session {} (pid {pid})",
            run.session_id.as_deref().unwrap_or("?")
        ),
    };
    store.log(Some(run.id), &what)?;
    say(&format!("run {} ({}): {what}", run.id, run.rule));

    let stdout = child.stdout.take().context("goose's output")?;
    let mut out_log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(ctx.cfg.runs_dir().join(format!("{}.jsonl", run.id)))?;
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let _ = writeln!(out_log, "{line}");
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut hold: Option<(Instant, Option<String>)> = None;
    let mut kill: Option<(Kill, Instant)> = None;
    let mut sigkilled = false;
    let mut first_message: Option<Instant> = None;
    let mut saw_complete = false;
    let mut last_error: Option<String> = None;
    let mut status: Option<ExitStatus> = None;

    let do_kill = |why: Kill, kill: &mut Option<(Kill, Instant)>| {
        if kill.is_none() {
            signal_group(pid, libc::SIGTERM);
            *kill = Some((why, Instant::now()));
        }
    };

    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    match v.get("type").and_then(Value::as_str) {
                        Some("complete") => saw_complete = true,
                        Some("error") => {
                            last_error = v.get("error").or_else(|| v.get("message")).map(|e| {
                                e.as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| e.to_string())
                            })
                        }
                        Some("message") => {
                            first_message.get_or_insert_with(Instant::now);
                            if let Some((_, Some(cid))) = &hold {
                                if is_tool_response(&v, cid) {
                                    do_kill(Kill::Hold, &mut kill);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }

        if status.is_none() {
            status = child.try_wait()?;
        }
        if let Some(st) = status {
            // goose exited: take what is left of its output, then stop.
            while let Ok(line) = rx.recv_timeout(Duration::from_millis(300)) {
                if line.contains("\"type\":\"complete\"") {
                    saw_complete = true;
                }
            }
            status = Some(st);
            break;
        }

        let cur = store.run(run.id)?.context("the run left the store")?;
        if hold.is_none() && cur.state == state::HELD {
            let call = store.pending_call(run.id)?;
            let call_id = call.as_ref().and_then(|c| c.tool_call_id.clone());
            if let Some(c) = &call {
                say(&format!(
                    "run {} ({}): held: {} {}",
                    run.id,
                    run.rule,
                    c.tool_name,
                    short(&c.tool_input, 120)
                ));
            }
            if call_id.is_none() {
                do_kill(Kill::Hold, &mut kill);
            }
            hold = Some((Instant::now(), call_id));
        }
        if let Some((at, _)) = &hold {
            if at.elapsed() > HOLD_GRACE {
                do_kill(Kill::Hold, &mut kill);
            }
        }
        if STOP.load(Ordering::SeqCst) {
            do_kill(Kill::Stop, &mut kill);
        }
        if matches!(mode, Mode::Start)
            && cur.session_id.is_none()
            && first_message.is_some_and(|t| t.elapsed() > HOOK_CHECK)
        {
            do_kill(Kill::NoHook, &mut kill);
        }
        if let Some((_, at)) = kill {
            if !sigkilled && at.elapsed() > KILL_GRACE {
                signal_group(pid, libc::SIGKILL);
                sigkilled = true;
            }
        }
    }

    store.set_pid(run.id, None)?;
    let status = status.expect("loop ends with an exit status");
    let cur = store.run(run.id)?.context("the run left the store")?;
    if cur.state == state::HELD {
        let how = match kill {
            Some((Kill::Hold, _)) => "goose-triggers stopped goose at the hold",
            _ => "goose ended its turn at the hold",
        };
        let what = format!(
            "{how}; the run waits for the user: goose-triggers approve {id} | reject {id}",
            id = run.id
        );
        store.log(Some(run.id), &what)?;
        say(&format!("run {} ({}): {what}", run.id, run.rule));
        return Ok(());
    }
    if cur.state != state::RUNNING {
        return Ok(());
    }
    let session_ok = cur.session_id.is_some();
    let failure = match kill.map(|k| k.0) {
        Some(Kill::Stop) => Some("goose-triggers was stopped while the run was going".to_string()),
        Some(Kill::NoHook) | None if !session_ok => Some(format!(
            "goose did not run goose-triggers' plugin hooks (no session start was recorded), so nothing could be held{}; check that goose loads plugins from {}",
            if kill.is_some() { " and the run was stopped" } else { "" },
            wd.join(".agents/plugins").display()
        )),
        _ if status.success() && saw_complete => None,
        _ => Some(format!(
            "goose exited with {status}{}; see goose-triggers log {}",
            last_error.map(|e| format!(": {}", short(&e, 300))).unwrap_or_default(),
            run.id
        )),
    };
    match failure {
        None => {
            store.transition(run.id, &[state::RUNNING], state::DONE, None)?;
            store.log(Some(run.id), "done")?;
            say(&format!("run {} ({}): done", run.id, run.rule));
        }
        Some(msg) => {
            store.transition(run.id, &[state::RUNNING], state::FAILED, Some(&msg))?;
            store.log(Some(run.id), &format!("failed: {msg}"))?;
            say(&format!("run {} ({}): failed: {msg}", run.id, run.rule));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spots_the_tool_response_for_the_held_call() {
        let v = json!({"type": "message", "message": {"role": "user", "content": [
            {"type": "toolResponse", "id": "call_7", "toolResult": {"status": "error"}}
        ]}});
        assert!(is_tool_response(&v, "call_7"));
        assert!(!is_tool_response(&v, "call_8"));
        let v = json!({"type": "message", "message": {"role": "assistant", "content": [
            {"type": "toolRequest", "id": "call_7"}
        ]}});
        assert!(!is_tool_response(&v, "call_7"));
    }

    #[test]
    fn resume_text_names_the_call() {
        let t = resume_text("shell", r#"{"command":"ls"}"#);
        assert!(t.contains("`shell`") && t.contains(r#"{"command":"ls"}"#));
    }
}
